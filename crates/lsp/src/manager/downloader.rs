//! Download logic for LSP servers

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use futures::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use super::mirror::{self, ManifestEntry};
use super::registry::{ServerInfo, SourceType};

/// Whether the mirror's `entry` may stand in for the registry's source of
/// `server_info`. The mirror holds one build per server, packaged for the
/// registry of its day, so a pinned build must be that build: an older one
/// may not start with the settings of this one (jdtls 1.44.0 cannot run on
/// Java 25, which the pinned 1.61.0 can). A package manager's version only
/// says what its fallback installs; the mirror's builds of those servers
/// are taken as they are.
fn mirror_entry_fits(server_info: &ServerInfo, entry: &ManifestEntry) -> Result<()> {
    match server_info.pinned_build() {
        Some(pin) if entry.version != pin => Err(anyhow!(
            "the mirror holds {} {}, not the pinned {pin}",
            server_info.id,
            if entry.version.is_empty() {
                "of no stated version"
            } else {
                entry.version.as_str()
            },
        )),
        _ => Ok(()),
    }
}

/// The URL of `server_info`'s archive on the mirror and the SHA-256 the
/// archive must match, both as this build pins them, or why the mirror cannot
/// serve it.
fn resolve_mirror_url(server_info: &ServerInfo) -> Result<(String, &'static str)> {
    let platform = mirror::platform();
    let manifest = mirror::manifest(platform)
        .ok_or_else(|| anyhow!("the mirror has no packages for {platform}"))?;
    let entry = manifest
        .servers
        .get(&server_info.id)
        .ok_or_else(|| anyhow!("the mirror has no {} for {platform}", server_info.id))?;
    mirror_entry_fits(server_info, entry)?;
    let base = mirror::base_url().ok_or_else(|| anyhow!("no mirror release is pinned"))?;
    Ok((format!("{base}/{}", entry.filename), entry.sha256.as_str()))
}

/// Shared HTTP client for all LSP download operations.
///
/// Reuses connections and TLS sessions across downloads instead of
/// creating a fresh `reqwest::Client` on every call.
static LSP_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    http_client_builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("reqwest client must build")
});

/// A client builder with the downloader's TLS configuration.
///
/// reqwest is built without a crypto provider of its own (see the workspace
/// manifest) and panics building a client without this configuration: ring
/// for the cryptography, and the Mozilla roots `webpki-roots` bundles, so a
/// host without a CA bundle still verifies. HTTP/1.1 is the only protocol
/// offered, as reqwest itself would offer without its `http2` feature. The
/// model download configures its client the same way.
fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().tls_backend_preconfigured(tls_config())
}

fn tls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports TLS 1.2 and 1.3")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

/// Maximum download size (1 GB)
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// GitHub release information with strict schema validation
///
/// This struct validates the structure of GitHub API responses
/// to prevent issues from malformed or malicious responses.
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    /// Release tag name (e.g., "v1.0.0")
    #[serde(rename = "tag_name")]
    tag_name: String,
    /// Release assets
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

/// GitHub release asset with strict schema validation
#[derive(Debug, Deserialize)]
struct GitHubAsset {
    /// Asset filename
    name: String,
    /// Download URL
    #[serde(rename = "browser_download_url")]
    download_url: String,
    /// Asset size in bytes
    #[serde(default)]
    size: u64,
    /// `sha256:<hex>`, which GitHub computes for every release asset uploaded
    /// since mid-2025. Absent for older assets.
    #[serde(default)]
    digest: Option<String>,
}

/// Maximum acceptable response size (10 MB)
const MAX_GITHUB_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Maximum number of assets to process
const MAX_ASSETS: usize = 100;

/// Maximum asset name length
const MAX_ASSET_NAME_LENGTH: usize = 256;

/// Maximum URL length
const MAX_URL_LENGTH: usize = 2048;

/// Validate GitHub release data
///
/// # Arguments
/// * `release` - The parsed GitHub release
///
/// # Returns
/// `Ok(())` if valid, `Err` otherwise
fn validate_github_release(release: &GitHubRelease) -> Result<()> {
    // Validate tag name
    if release.tag_name.is_empty() {
        return Err(anyhow!("Invalid release: empty tag name"));
    }
    if release.tag_name.len() > 100 {
        return Err(anyhow!("Invalid release: tag name too long"));
    }

    // Validate tag name format (should be like v1.0.0 or 1.0.0)
    if !is_plain_tag(&release.tag_name) {
        return Err(anyhow!(
            "Invalid release: tag name contains invalid characters"
        ));
    }

    // Validate assets count
    if release.assets.len() > MAX_ASSETS {
        return Err(anyhow!(
            "Invalid release: too many assets ({})",
            release.assets.len()
        ));
    }

    // Validate each asset
    for asset in &release.assets {
        // Validate asset name
        if asset.name.is_empty() {
            return Err(anyhow!("Invalid asset: empty name"));
        }
        if asset.name.len() > MAX_ASSET_NAME_LENGTH {
            return Err(anyhow!("Invalid asset: name too long"));
        }
        // Check for path traversal in asset name
        if asset.name.contains("..") || asset.name.contains('/') || asset.name.contains('\\') {
            return Err(anyhow!("Invalid asset: name contains path traversal"));
        }

        // Validate download URL
        if asset.download_url.is_empty() {
            return Err(anyhow!("Invalid asset: empty download URL"));
        }
        if asset.download_url.len() > MAX_URL_LENGTH {
            return Err(anyhow!("Invalid asset: download URL too long"));
        }
        // Validate URL starts with expected prefix
        if !asset.download_url.starts_with("https://") {
            return Err(anyhow!("Invalid asset: download URL must use HTTPS"));
        }

        // Validate size (must be reasonable)
        if asset.size > 1024 * 1024 * 1024 {
            // 1 GB
            return Err(anyhow!("Invalid asset: size exceeds 1GB"));
        }
    }

    Ok(())
}

/// Whether `tag` is a plain release tag (`v1.0.0`, `1.3.13`), safe in a URL path.
fn is_plain_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Progress callback for downloads
pub trait DownloadProgress: Send + Sync {
    fn on_progress(&self, downloaded: u64, total: u64);
    fn on_complete(&self);
    fn on_error(&self, error: &str);
}

impl DownloadProgress for () {
    fn on_progress(&self, _downloaded: u64, _total: u64) {}
    fn on_complete(&self) {}
    fn on_error(&self, _error: &str) {}
}

/// No-op progress reference for internal use.
const NO_OP_PROGRESS: &dyn DownloadProgress = &();

/// Install `server_info` into `install_dir` from Devscriptor's mirror.
///
/// The archive and its SHA-256 are those this build pins for its platform
/// (see [`mirror`]): the archive is checked before it is unpacked, and an
/// archive without the server's `bin_path` is refused, its files removed, so
/// that the caller can install the server from its original source instead.
pub async fn download_from_mirror(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let server_id = &server_info.id;

    // Resolve the exact filename + sha256 from the manifest.
    let (mirror_url, expected_sha256) = resolve_mirror_url(server_info)?;

    debug!("Mirror URL for {server_id}: {mirror_url}");

    // Derive the local archive filename from the resolved mirror URL
    // (e.g. `rust-analyzer-2026-04-21-linux-x86_64.tar.gz`).
    let package_name = mirror_url
        .rsplit('/')
        .next()
        .unwrap_or("lsp-server.tar.gz")
        .to_string();

    // Create a client with a longer timeout for the actual download.
    let client = http_client_builder()
        .timeout(std::time::Duration::from_secs(300)) // 5-min timeout for large archives
        .build()
        .context("Failed to create HTTP client")?;

    // Check if the package exists on the mirror
    let head_response = match client.head(&mirror_url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            debug!("Mirror HEAD request failed for {}: {}", server_id, e);
            return Err(anyhow!("Mirror unavailable: {e}"));
        }
    };

    if !head_response.status().is_success() {
        debug!(
            "Mirror returned {} for {}",
            head_response.status(),
            server_id
        );
        return Err(anyhow!("Package not found on mirror"));
    }

    // Download the package
    let download_path = install_dir.join(&package_name);

    match download_file_with_client(&client, &mirror_url, &download_path, progress).await {
        Ok(()) => {
            info!("Successfully downloaded {} from mirror", server_id);

            // SECURITY: Verify checksum before extraction
            if let Err(e) = verify_checksum(&download_path, expected_sha256) {
                let _ = tokio::fs::remove_file(&download_path).await;
                return Err(anyhow!("Checksum verification failed: {e}"));
            }

            // Extract the archive. The mirror serves `.zip` on Windows and
            // `.tar.gz` on the Unix platforms, so dispatch on the resolved
            // filename to unpack each correctly.
            if package_name.ends_with(".zip") {
                extract_zip(&download_path, install_dir).await?;
            } else {
                extract_tar_gz(&download_path, install_dir).await?;
            }

            // Clean up the archive
            tokio::fs::remove_file(&download_path).await.ok();

            lift_into_place(install_dir, &server_info.bin_path).await?;
            if !binary_present(&install_dir.join(&server_info.bin_path)).await {
                clear_dir(install_dir).await?;
                return Err(anyhow!(
                    "the mirror's {package_name} holds no {}",
                    server_info.bin_path
                ));
            }

            Ok(install_dir.to_path_buf())
        }
        Err(e) => {
            debug!("Mirror download failed for {}: {}", server_id, e);
            // Clean up partial download
            tokio::fs::remove_file(&download_path).await.ok();
            Err(e)
        }
    }
}

/// Lift an unpacked server out of the one folder its archive wrapped it in.
///
/// Mirror packages archive each server's directory itself (`tar -C lsp-servers
/// <id>`, `Compress-Archive -Path <dir>`), so they start with `<id>/`; GitHub's
/// clangd zip starts with `clangd_<version>/`. Unpacked into the server's own
/// `lsp-servers/<id>/`, the binary would sit a level below the registry's
/// `bin_path` (`rust-analyzer/rust-analyzer/bin/rust-analyzer`), the resolver
/// would take the nested folder for the binary, and spawning it would fail with
/// "Permission denied". A folder that belongs to the layout (a `bin_path` of
/// `server/bin/…`) is left alone, because `bin_path` already resolves through
/// it.
async fn lift_into_place(install_dir: &Path, bin_path: &str) -> Result<()> {
    if binary_present(&install_dir.join(bin_path)).await {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(install_dir)
        .await
        .with_context(|| format!("Failed to read {}", install_dir.display()))?;
    let mut nested = None;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() && binary_present(&entry.path().join(bin_path)).await {
            nested = Some(entry.path());
            break;
        }
    }
    let Some(nested) = nested else {
        return Ok(());
    };
    // Move the folder aside first: a package such as `gopls/gopls` holds an
    // entry with the folder's own name, which could not move onto it.
    let name = nested
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unpacked = install_dir.join(format!(".{name}.unpacked"));
    tokio::fs::rename(&nested, &unpacked)
        .await
        .with_context(|| format!("Failed to move {} aside", nested.display()))?;
    let mut entries = tokio::fs::read_dir(&unpacked)
        .await
        .with_context(|| format!("Failed to read {}", unpacked.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let target = install_dir.join(entry.file_name());
        tokio::fs::rename(entry.path(), &target)
            .await
            .with_context(|| format!("Failed to move {} into place", target.display()))?;
    }
    tokio::fs::remove_dir(&unpacked).await.ok();
    Ok(())
}

/// Whether the binary `path` names exists, with the Windows executable
/// suffixes the registry's `bin_path` leaves off.
async fn binary_present(path: &Path) -> bool {
    if tokio::fs::metadata(path).await.is_ok_and(|m| m.is_file()) {
        return true;
    }
    if !cfg!(windows) {
        return false;
    }
    for ext in ["exe", "cmd", "bat", "ps1"] {
        if tokio::fs::metadata(path.with_extension(ext))
            .await
            .is_ok_and(|m| m.is_file())
        {
            return true;
        }
    }
    false
}

/// Download a file with a specific client (for timeout control)
async fn download_file_with_client(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<()> {
    let response = client
        .get(url)
        .header("User-Agent", "devscriptor-lsp-manager")
        .send()
        .await
        .with_context(|| format!("Failed to download from {url}"))?;

    let total_size = response.content_length().unwrap_or(0);
    if total_size > MAX_DOWNLOAD_BYTES {
        return Err(anyhow!(
            "download too large: {total_size} bytes (max {MAX_DOWNLOAD_BYTES})"
        ));
    }

    let mut file = tokio::fs::File::create(path).await?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded += chunk.len() as u64;
        if downloaded > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(path).await;
            return Err(anyhow!(
                "download exceeded {MAX_DOWNLOAD_BYTES} bytes mid-stream"
            ));
        }
        file.write_all(&chunk).await?;

        if let Some(p) = progress {
            p.on_progress(downloaded, total_size);
        }
    }

    file.flush().await?;

    Ok(())
}

/// Verify a SHA-256 checksum of a file against an expected value.
fn verify_checksum(path: &Path, expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{BufReader, Read};

    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {} for checksum", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let actual = hex::encode(hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(anyhow!(
            "checksum mismatch: expected {expected}, got {actual}"
        ))
    }
}

/// Safely join an archive member path with a destination directory,
/// rejecting any traversal outside the destination.
fn safe_join(dest: &Path, member: &Path) -> Result<PathBuf> {
    let mut out = dest.to_path_buf();
    for comp in member.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            // Reject any of: ParentDir (..), RootDir (/), Prefix (C:\)
            _ => {
                return Err(anyhow!("unsafe path in archive: {}", member.display()));
            }
        }
    }
    if !out.starts_with(dest) {
        return Err(anyhow!(
            "archive path escapes destination: {}",
            member.display()
        ));
    }
    Ok(out)
}

/// Install a server into `install_dir`: from Devscriptor's mirror when it
/// has the server for this platform, else from the server's original source
/// (its GitHub release, npm, …).
pub async fn download_server(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    // Tier 1: the mirror. A server that comes with its
    // toolchain is not taken from it: sourcekit-lsp runs only inside the
    // Swift toolchain it belongs to, and a copy taken from a Linux PATH can be
    // swiftly's proxy, which runs only where swiftly is set up.
    if server_info.source.source_type != SourceType::System {
        debug!(
            "Attempting tier 1 (mirror) download for {}",
            server_info.name
        );
        match download_from_mirror(
            server_info,
            install_dir,
            // Mirror attempt uses a no-op so caller progress only fires for tier 2.
            Some(NO_OP_PROGRESS),
        )
        .await
        {
            // A gem bundle's native extensions are built for one Ruby: the
            // mirror's, built for another than this machine's, cannot load them,
            // and the gems are installed here instead, for this Ruby.
            Ok(path)
                if server_info.source.source_type == SourceType::Gem
                    && let Some(built_for) = super::ruby::built_for_another_ruby(
                        install_dir,
                        super::ruby::local_extension_api(),
                    ) =>
            {
                info!(
                    "The mirror's {} was built for Ruby {built_for}, not this Ruby's: \
                     installing it for this one",
                    server_info.name
                );
                clear_dir(&path).await?;
            }
            Ok(path) => {
                info!("Downloaded {} from mirror", server_info.name);
                if let Some(p) = progress {
                    p.on_complete();
                }
                return Ok(path);
            }
            Err(e) => {
                info!(
                    "Mirror download failed for {}: {}. Falling back to original source.",
                    server_info.name, e
                );
            }
        }
    }

    // Tier 2: Fall back to original source
    debug!(
        "Attempting tier 2 (original source) download for {}",
        server_info.name
    );
    match server_info.source.source_type {
        SourceType::GithubRelease => {
            download_github_release(server_info, install_dir, progress).await
        }
        SourceType::Npm => install_npm_package(server_info, install_dir, progress).await,
        SourceType::Pip => install_pip_package(server_info, install_dir, progress).await,
        SourceType::GoInstall => install_go_package(server_info, install_dir, progress).await,
        SourceType::DotnetTool => install_dotnet_tool(server_info, install_dir, progress).await,
        SourceType::Gem => install_gem(server_info, install_dir, progress).await,
        SourceType::Download => download_direct(server_info, install_dir, progress).await,
        SourceType::System => {
            // System servers are expected to already be installed
            Ok(install_dir.to_path_buf())
        }
        SourceType::Coursier => install_coursier_package(server_info, install_dir, progress).await,
    }
}

/// Download from GitHub release
///
/// # Security
/// This function validates the GitHub API response using strict schema
/// validation to prevent issues from malformed or malicious responses.
async fn download_github_release(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let repo = server_info
        .source
        .repo
        .as_ref()
        .ok_or_else(|| anyhow!("GitHub release source missing repo"))?;

    published_for(server_info, &platform_key())?;

    // SECURITY: Validate repo format to prevent injection
    if repo.contains('\0') || repo.contains('\n') || repo.contains('\r') {
        return Err(anyhow!("Invalid repository format"));
    }
    if !repo.contains('/') || repo.starts_with('/') || repo.ends_with('/') {
        return Err(anyhow!("Invalid repository format: expected 'owner/repo'"));
    }
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid repository format: expected 'owner/repo'"));
    }
    for part in &parts {
        if part.is_empty() || part.len() > 39 {
            return Err(anyhow!("Invalid repository component length"));
        }
        if !part
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(anyhow!("Invalid repository component characters"));
        }
    }

    // A pinned version is fetched by its tag, which its pinned checksum is
    // for; otherwise the latest release.
    let release_url = match server_info.source.version.as_deref() {
        Some(tag) if tag != "latest" => {
            if !is_plain_tag(tag) {
                return Err(anyhow!("Invalid release tag: {tag}"));
            }
            format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
        }
        _ => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };

    let response = LSP_HTTP_CLIENT
        .get(&release_url)
        .header("User-Agent", "devscriptor-lsp-manager")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Failed to fetch release info")?;

    // SECURITY: Limit response size to prevent memory exhaustion
    let content_length = response.content_length().unwrap_or(0);
    if content_length > MAX_GITHUB_RESPONSE_SIZE as u64 {
        return Err(anyhow!(
            "GitHub API response too large: {content_length} bytes"
        ));
    }

    let release_text = response
        .text()
        .await
        .context("Failed to get release text")?;

    // SECURITY: Check response size after reading
    if release_text.len() > MAX_GITHUB_RESPONSE_SIZE {
        return Err(anyhow!("GitHub API response exceeded maximum size"));
    }

    // SECURITY: Parse into strongly-typed struct with validation
    let release: GitHubRelease =
        serde_json::from_str(&release_text).context("Failed to parse release info")?;

    // SECURITY: Validate the parsed release data
    validate_github_release(&release)?;

    // Find matching asset
    let asset_pattern = server_info
        .source
        .asset_pattern
        .as_ref()
        .ok_or_else(|| anyhow!("GitHub release source missing asset pattern"))?;

    // Replace placeholders in pattern. `{version}` is the release's own tag
    // (clangd names its assets `clangd-linux-23.1.0.zip`), and `{platform}`
    // is tried in each spelling projects use (clangd's macOS build is `mac`).
    let patterns: Vec<String> = platform_names()
        .iter()
        .map(|platform| {
            asset_pattern
                .replace("{arch}", get_arch())
                .replace("{os}", get_os())
                .replace("{platform}", platform)
                .replace("{version}", release.tag_name.trim_start_matches('v'))
        })
        .collect();

    let asset = patterns
        .iter()
        .find_map(|pattern| release.assets.iter().find(|a| a.name.contains(pattern)))
        .ok_or_else(|| anyhow!("No matching asset found for pattern: {}", patterns[0]))?;

    let download_url = &asset.download_url;
    let asset_name = &asset.name;

    // Nothing downloaded here runs unverified. The registry pins the SHA-256 of
    // a pinned release's asset; for the latest release, GitHub publishes it in
    // the same API response that named the asset.
    let expected = expected_sha256(asset, server_info.source.expected_sha256.as_deref())
        .ok_or_else(|| {
            anyhow!(
                "{} was not installed: GitHub publishes no SHA-256 for {asset_name} and the \
                 registry pins none, so it cannot be verified. Install {} yourself; \
                 Devscriptor finds it on PATH.",
                server_info.name,
                server_info.name
            )
        })?;

    // Download the asset
    let download_path = install_dir.join(asset_name);
    download_file(download_url, &download_path, progress).await?;
    if let Err(e) = verify_checksum(&download_path, expected) {
        tokio::fs::remove_file(&download_path).await.ok();
        return Err(e);
    }

    unpack(&download_path, install_dir, &server_info.bin_path).await?;
    lift_into_place(install_dir, &server_info.bin_path).await?;
    move_clang_resource_dir(install_dir).await?;

    Ok(install_dir.to_path_buf())
}

/// The SHA-256 a release asset must match: the one the registry pins, else
/// the one GitHub publishes for it, else none.
///
/// The pin comes first: GitHub lets a release's asset be replaced, and a
/// replacement comes with a digest of its own.
fn expected_sha256<'a>(asset: &'a GitHubAsset, pinned: Option<&'a str>) -> Option<&'a str> {
    pinned.or_else(|| {
        asset
            .digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
    })
}

/// Unpack a verified download into `install_dir`, and remove it: an archive
/// is extracted, a gzipped binary (rust-analyzer's) is decompressed to
/// `bin_path`, and anything else is the binary itself (phpactor's `.phar`),
/// moved to `bin_path`. JetBrains names its macOS zips `.sit`.
async fn unpack(download_path: &Path, install_dir: &Path, bin_path: &str) -> Result<()> {
    let name = download_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bin_path = install_dir.join(bin_path);
    // `.tar.gz` before `.gz`, which it also ends with.
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(download_path, install_dir).await?;
    } else if name.ends_with(".zip") || name.ends_with(".sit") {
        extract_zip(download_path, install_dir).await?;
    } else if name.ends_with(".gz") {
        extract_gz(download_path, &bin_path).await?;
    } else {
        return place_executable(download_path, &bin_path).await;
    }
    tokio::fs::remove_file(download_path).await.ok();
    Ok(())
}

/// Move a downloaded binary to `bin_path`, executable.
async fn place_executable(from: &Path, bin_path: &Path) -> Result<()> {
    if let Some(parent) = bin_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(from, bin_path)
        .await
        .with_context(|| format!("Failed to move {} into place", bin_path.display()))?;
    make_executable(bin_path).await
}

/// Mark `path` executable for its owner, group and others (Unix only).
async fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(path).await?.permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(path, perms).await?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Give clangd the builtin headers its registry entry points it at.
///
/// clangd is launched with `--resource-dir={server_dir}/resource-dir`, since it
/// otherwise looks for `stddef.h` and the intrinsics at the path it was built
/// with. The mirror package ships that directory; GitHub's clangd release has
/// it at `lib/clang/<major>/` instead, which this moves into place.
async fn move_clang_resource_dir(install_dir: &Path) -> Result<()> {
    let target = install_dir.join("resource-dir");
    if tokio::fs::metadata(&target).await.is_ok() {
        return Ok(());
    }
    let Ok(mut versions) = tokio::fs::read_dir(install_dir.join("lib").join("clang")).await else {
        return Ok(());
    };
    while let Some(entry) = versions.next_entry().await? {
        if tokio::fs::metadata(entry.path().join("include"))
            .await
            .is_ok_and(|m| m.is_dir())
        {
            tokio::fs::rename(entry.path(), &target)
                .await
                .with_context(|| format!("Failed to move {} into place", target.display()))?;
            break;
        }
    }
    Ok(())
}

/// Install an npm package
/// The `package.json` an npm server is installed from: the server at its
/// version, and its companions (`name@version`, a scoped name keeping its
/// leading `@`). Without them typescript-language-server and volar refused
/// to start, and sql-language-server died on `ERR_PACKAGE_PATH_NOT_EXPORTED`.
fn npm_package_json(server_info: &ServerInfo) -> Result<String> {
    let source = &server_info.source;
    let package = source
        .package
        .as_ref()
        .ok_or_else(|| anyhow!("NPM source missing package name"))?;
    let mut dependencies = serde_json::Map::new();
    dependencies.insert(
        package.clone(),
        source.version.as_deref().unwrap_or("latest").into(),
    );
    for companion in &source.companions {
        let (name, version) = companion
            .rsplit_once('@')
            .filter(|(name, _)| !name.is_empty())
            .ok_or_else(|| anyhow!("{}: companion {companion} names no version", server_info.id))?;
        dependencies.insert(name.to_string(), version.into());
    }
    Ok(serde_json::json!({
        "name": "lsp-install",
        "version": "1.0.0",
        "dependencies": dependencies,
    })
    .to_string())
}

async fn install_npm_package(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    tokio::fs::write(
        install_dir.join("package.json"),
        npm_package_json(server_info)?,
    )
    .await?;

    // Run npm install
    let output = tokio::process::Command::new("npm")
        .args(["install", "--omit=dev", "--no-package-lock"])
        .current_dir(install_dir)
        .output()
        .await
        .context("Failed to run npm install")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("npm install failed: {stderr}"));
    }

    if let Some(p) = progress {
        p.on_complete();
    }

    Ok(install_dir.to_path_buf())
}

/// Install a pip package
async fn install_pip_package(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let package = server_info
        .source
        .package
        .as_ref()
        .ok_or_else(|| anyhow!("Pip source missing package name"))?;

    let version = server_info.source.version.as_deref().unwrap_or("latest");

    // Determine pip command
    let pip_cmd = if which::which("pip3").is_ok() {
        "pip3"
    } else if which::which("pip").is_ok() {
        "pip"
    } else {
        return Err(anyhow!("pip not found"));
    };

    // Install to the specific directory
    let output = tokio::process::Command::new(pip_cmd)
        .args([
            "install",
            "--target",
            &install_dir.to_string_lossy(),
            &format!("{package}=={version}"),
        ])
        .output()
        .await
        .context("Failed to run pip install")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("pip install failed: {stderr}"));
    }

    if let Some(p) = progress {
        p.on_complete();
    }

    Ok(install_dir.to_path_buf())
}

/// Install a Go package
async fn install_go_package(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let package = server_info
        .source
        .package
        .as_ref()
        .ok_or_else(|| anyhow!("Go source missing package name"))?;

    // Set up Go environment
    let go_path = install_dir.join("go");

    // Run go install with environment variables passed directly to the command
    // instead of using deprecated std::env::set_var()
    let output = tokio::process::Command::new("go")
        .args(["install", package])
        .env("GOPATH", &go_path)
        .env("GOBIN", install_dir.join("bin"))
        .output()
        .await
        .context("Failed to run go install")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("go install failed: {stderr}"));
    }

    if let Some(p) = progress {
        p.on_complete();
    }

    Ok(install_dir.to_path_buf())
}

/// Install a .NET tool
async fn install_dotnet_tool(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let package = server_info
        .source
        .package
        .as_ref()
        .ok_or_else(|| anyhow!(".NET source missing package name"))?;

    let version = server_info
        .source
        .version
        .as_deref()
        .ok_or_else(|| anyhow!("{}: a .NET tool is installed at a version", server_info.id))?;

    tokio::fs::create_dir_all(install_dir.join("bin")).await?;
    let output = tokio::process::Command::new("dotnet")
        .args(dotnet_tool_args(package, version, install_dir))
        .output()
        .await
        .context("Failed to run dotnet tool install")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("dotnet tool install failed: {stderr}"));
    }

    if let Some(p) = progress {
        p.on_complete();
    }

    Ok(install_dir.to_path_buf())
}

/// The `dotnet` arguments that install `package` at `version` into
/// `install_dir`'s `bin/`, where `bin_path` points (`bin/csharp-ls`), not
/// `tools/`, where the resolver does not look.
fn dotnet_tool_args(package: &str, version: &str, install_dir: &Path) -> Vec<String> {
    vec![
        "tool".to_string(),
        "install".to_string(),
        "--tool-path".to_string(),
        install_dir.join("bin").to_string_lossy().into_owned(),
        "--version".to_string(),
        version.to_string(),
        package.to_string(),
    ]
}

/// Remove everything in `dir`, keeping `dir` itself.
async fn clear_dir(dir: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("Failed to read {}", dir.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            tokio::fs::remove_dir_all(entry.path()).await?;
        } else {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

/// The `gem install` arguments that install `package` at `version` the way
/// the registry runs it and the mirror packages it: its gems in
/// `install_dir`, the `GEM_HOME` the registry sets, and its executables in
/// `install_dir/bin`, where `bin_path` points.
fn gem_install_args(package: &str, version: &str, install_dir: &Path) -> Vec<String> {
    vec![
        "install".to_string(),
        "--install-dir".to_string(),
        install_dir.to_string_lossy().into_owned(),
        "--bindir".to_string(),
        install_dir.join("bin").to_string_lossy().into_owned(),
        "--version".to_string(),
        version.to_string(),
        "--no-document".to_string(),
        package.to_string(),
    ]
}

/// Install a Ruby gem with this machine's Ruby, which builds its native
/// extensions for itself.
async fn install_gem(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let package = server_info
        .source
        .package
        .as_ref()
        .ok_or_else(|| anyhow!("Gem source missing package name"))?;

    let version = server_info.source.version.as_deref().unwrap_or(">= 0");

    let output = tokio::process::Command::new("gem")
        .args(gem_install_args(package, version, install_dir))
        .env("GEM_HOME", install_dir)
        .output()
        .await
        .context("Failed to run gem install")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("gem install failed: {stderr}"));
    }

    if let Some(p) = progress {
        p.on_complete();
    }

    Ok(install_dir.to_path_buf())
}

/// Download from a direct URL
async fn download_direct(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    // Nothing downloaded here runs unverified: what to fetch, and the checksum
    // it must match, are settled before anything is fetched.
    let (url, expected) = direct_artifact(server_info, &platform_key())?;
    let filename = url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("Invalid URL: {url}"))?;

    let download_path = install_dir.join(filename);
    download_file(&url, &download_path, progress).await?;
    if let Err(e) = verify_checksum(&download_path, expected) {
        tokio::fs::remove_file(&download_path).await.ok();
        return Err(e);
    }

    unpack(&download_path, install_dir, &server_info.bin_path).await?;
    // The Dart SDK's zip holds everything in `dart-sdk/`, the Kotlin LSP's
    // archives in `kotlin-server-<version>/`.
    lift_into_place(install_dir, &server_info.bin_path).await?;

    Ok(install_dir.to_path_buf())
}

/// Whether `server_info`'s release has a build for `platform` (a key such as
/// `linux-aarch64`), as its `published_for` says. One without refuses the
/// install, rather than take another platform's build: clangd's `clangd-linux`
/// zip, the only Linux one, is x86_64.
fn published_for(server_info: &ServerInfo, platform: &str) -> Result<()> {
    let published = &server_info.source.published_for;
    if published.is_empty() || published.iter().any(|p| p == platform) {
        return Ok(());
    }
    let name = &server_info.name;
    Err(anyhow!(
        "{name} was not installed: it is not published for {platform}, only for {}. \
         A {name} installed by other means is found on PATH.",
        published.join(", ")
    ))
}

/// The URL a download source fetches on `platform` (a key such as
/// `linux-x86_64`), and the SHA-256 the file must match.
///
/// `{version}` becomes the pinned version. A source that lists `platforms`
/// has a build for each: `{platform}` becomes the build's name, and the build
/// carries its checksum. Any other source is one artifact for every platform,
/// pinned by `expected_sha256`. A platform without a build, a missing
/// checksum, or a placeholder left unfilled refuses the install.
fn direct_artifact<'a>(server_info: &'a ServerInfo, platform: &str) -> Result<(String, &'a str)> {
    let name = &server_info.name;
    let source = &server_info.source;
    let url = source
        .url
        .as_deref()
        .ok_or_else(|| anyhow!("{name}: the download source names no URL"))?;
    let (url, sha256) = if source.platforms.is_empty() {
        let sha256 = source.expected_sha256.as_deref().ok_or_else(|| {
            anyhow!(
                "{name} was not installed: the registry pins no SHA-256 for it, so it \
                 cannot be verified. Install {name} yourself; Devscriptor finds it on PATH."
            )
        })?;
        (url.to_string(), sha256)
    } else {
        let build = source.platforms.get(platform).ok_or_else(|| {
            anyhow!(
                "{name} was not installed: the registry lists no build of it for \
                 {platform}. Install {name} yourself; Devscriptor finds it on PATH."
            )
        })?;
        (
            url.replace("{platform}", &build.name),
            build.sha256.as_str(),
        )
    };
    let url = match source.version.as_deref() {
        Some(version) => url.replace("{version}", version),
        None => url,
    };
    if let Some(placeholder) = unfilled_placeholder(&url) {
        return Err(anyhow!(
            "{name}: the download URL leaves {placeholder} unfilled: {url}"
        ));
    }
    Ok((url, sha256))
}

/// The first `{…}` placeholder left in `url`, if any.
fn unfilled_placeholder(url: &str) -> Option<&str> {
    let start = url.find('{')?;
    let end = url[start..]
        .find('}')
        .map_or(url.len(), |end| start + end + 1);
    Some(&url[start..end])
}

/// Install a Coursier package (for JVM languages)
async fn install_coursier_package(
    server_info: &ServerInfo,
    install_dir: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<PathBuf> {
    let package = server_info
        .source
        .package
        .as_ref()
        .ok_or_else(|| anyhow!("Coursier source missing package name"))?;

    let version = server_info.source.version.as_deref().ok_or_else(|| {
        anyhow!(
            "{}: a Coursier package is installed at a version",
            server_info.id
        )
    })?;

    // First, ensure Coursier is available
    if which::which("cs").is_err() {
        return Err(anyhow!(
            "Coursier (cs) not found. Please install it from https://get-coursier.io/"
        ));
    }

    // A launcher for the artifact, where `bin_path` points (`bin/metals`), as
    // the download scripts make it, holding the artifact and the libraries
    // it runs on. `cs install` takes an application's name, not Maven
    // coordinates: given `org.scalameta:metals_2.13:1.2.2` it would install
    // nothing.
    let launcher = install_dir.join(&server_info.bin_path);
    if let Some(dir) = launcher.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let output = tokio::process::Command::new("cs")
        .args(coursier_bootstrap_args(package, version, &launcher))
        .output()
        .await
        .context("Failed to run cs bootstrap")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Coursier install failed: {stderr}"));
    }

    if let Some(p) = progress {
        p.on_complete();
    }

    Ok(install_dir.to_path_buf())
}

/// Where a standalone Coursier launcher keeps the libraries it holds: a thin
/// one lists their URLs instead.
const STANDALONE_LIBRARIES: &[u8] = b"coursier/bootstrap/launcher/jars/";

/// Whether the Coursier launcher at `launcher` holds the libraries it runs
/// on (`cs bootstrap --standalone`), rather than fetching them at its first
/// start. It is a zip archive behind a shell prelude, which names each
/// library it holds.
pub(crate) fn holds_its_libraries(launcher: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(launcher) else {
        return false;
    };
    let mut block = vec![0u8; 1 << 20];
    // The end of the last block, which a name may straddle.
    let mut carried = 0;
    loop {
        let Ok(read) = file.read(&mut block[carried..]) else {
            return false;
        };
        if read == 0 {
            return false;
        }
        let filled = carried + read;
        if block[..filled]
            .windows(STANDALONE_LIBRARIES.len())
            .any(|window| window == STANDALONE_LIBRARIES)
        {
            return true;
        }
        carried = (STANDALONE_LIBRARIES.len() - 1).min(filled);
        block.copy_within(filled - carried..filled, 0);
    }
}

/// The `cs` arguments that bootstrap a launcher for `package` (Maven
/// coordinates without a version) at `version` into `launcher`, holding the
/// package and the libraries it runs on (`--standalone`). A thin launcher
/// fetched them into Coursier's cache at its first start: ~130 MB for
/// Metals, which needed the network and kept the first Scala request waiting.
fn coursier_bootstrap_args(package: &str, version: &str, launcher: &Path) -> Vec<String> {
    vec![
        "bootstrap".to_string(),
        "--standalone".to_string(),
        format!("{package}:{version}"),
        "-o".to_string(),
        launcher.to_string_lossy().into_owned(),
    ]
}

/// Download a file with progress
async fn download_file(
    url: &str,
    path: &Path,
    progress: Option<&dyn DownloadProgress>,
) -> Result<()> {
    let response = LSP_HTTP_CLIENT
        .get(url)
        .header("User-Agent", "devscriptor-lsp-manager")
        .send()
        .await
        .with_context(|| format!("Failed to download from {url}"))?;

    let total_size = response.content_length().unwrap_or(0);
    if total_size > MAX_DOWNLOAD_BYTES {
        return Err(anyhow!(
            "download too large: {total_size} bytes (max {MAX_DOWNLOAD_BYTES})"
        ));
    }

    let mut file = tokio::fs::File::create(path).await?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded += chunk.len() as u64;
        if downloaded > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(path).await;
            return Err(anyhow!(
                "download exceeded {MAX_DOWNLOAD_BYTES} bytes mid-stream"
            ));
        }
        file.write_all(&chunk).await?;

        if let Some(p) = progress {
            p.on_progress(downloaded, total_size);
        }
    }

    file.flush().await?;

    Ok(())
}

/// Extract a .gz file
/// Decompress the gzipped single file `path` to `output_path`, executable.
async fn extract_gz(path: &Path, output_path: &Path) -> Result<()> {
    use async_compression::tokio::bufread::GzipDecoder;
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut decoder = GzipDecoder::new(reader);

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut output = tokio::fs::File::create(output_path).await?;
    let mut buffer = Vec::new();
    decoder.read_to_end(&mut buffer).await?;
    output.write_all(&buffer).await?;

    make_executable(output_path).await
}

/// Extract a .tar.gz file safely (guards against Zip-Slip / Tar-Slip)
async fn extract_tar_gz(path: &Path, dest: &Path) -> Result<()> {
    let path = path.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        let dec = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(dec);
        for entry in ar.entries()? {
            let mut e = entry?;
            let p = e.path()?.into_owned();
            let target = safe_join(&dest, &p)?;
            // Ensure parent directory exists
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            e.unpack(&target)?;
        }
        Ok(())
    })
    .await
    .context("tar.gz extraction panicked")?
}

/// Extract a .zip file safely (guards against Zip-Slip)
async fn extract_zip(path: &Path, dest: &Path) -> Result<()> {
    let path = path.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        let mut ar = zip::ZipArchive::new(file)
            .with_context(|| format!("Failed to read zip archive {}", path.display()))?;
        for i in 0..ar.len() {
            let mut e = ar.by_index(i)?;
            let name = e.name()?.into_owned();
            let target = safe_join(&dest, Path::new(&name))?;
            if e.is_dir() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut e, &mut out)?;
                // The permission bits a Unix-made archive records, the
                // executable ones above all (the Dart SDK's `bin/dart`, the
                // Kotlin LSP's `bin/intellij-server`); never the set-id ones.
                #[cfg(unix)]
                if let Some(mode) = e.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    out.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
                }
            }
        }
        Ok(())
    })
    .await
    .context("zip extraction panicked")?
}

/// Get the current architecture as a static string slice.
///
/// A `&'static str`, so a call allocates nothing.
const fn get_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    return "x86_64";

    #[cfg(target_arch = "aarch64")]
    return "aarch64";

    #[cfg(target_arch = "arm")]
    return "arm";

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "arm")))]
    return "unknown";
}

/// Get the current OS
fn get_os() -> &'static str {
    #[cfg(target_os = "linux")]
    return "unknown-linux-gnu";
    #[cfg(target_os = "macos")]
    return "apple-darwin";
    #[cfg(target_os = "windows")]
    return "pc-windows-msvc";
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return "unknown";
}

/// The platform's key in a download's `platforms`: `<os>-<arch>`, the way the
/// mirror names its platforms (`linux-x86_64`, `macos-aarch64`, …).
fn platform_key() -> String {
    format!("{}-{}", get_platform(), get_arch())
}

/// The platform's names in release asset names, the canonical one first.
fn platform_names() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    return &["macos", "mac"];
    #[cfg(not(target_os = "macos"))]
    return std::slice::from_ref(&PLATFORM);
}

/// [`get_platform`] as a static, for [`platform_names`].
#[cfg(not(target_os = "macos"))]
static PLATFORM: &str = get_platform();

/// Get the platform name
const fn get_platform() -> &'static str {
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return "unknown";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gem is installed the way the registry runs it: its gems in the
    /// server's directory, the `GEM_HOME` the registry sets, and its
    /// executables in `bin/`, where `bin_path` points.
    #[test]
    fn a_gem_installs_where_the_registry_runs_it() {
        let dir = Path::new("/srv/lsp/ruby-lsp");
        assert_eq!(
            gem_install_args("ruby-lsp", "0.26.11", dir),
            [
                "install",
                "--install-dir",
                "/srv/lsp/ruby-lsp",
                "--bindir",
                &*dir.join("bin").to_string_lossy(),
                "--version",
                "0.26.11",
                "--no-document",
                "ruby-lsp",
            ]
        );
        let registry = super::super::registry::ServerRegistry::load().unwrap();
        let ruby = registry.get_server("ruby-lsp").unwrap();
        assert_eq!(ruby.bin_path, "bin/ruby-lsp-launcher");
        assert_eq!(
            ruby.env.get("GEM_HOME").map(String::as_str),
            Some("{server_dir}")
        );
    }

    #[test]
    fn a_pinned_checksum_is_preferred_and_a_github_digest_is_the_fallback() {
        let asset = |digest: Option<&str>| GitHubAsset {
            name: "a.gz".into(),
            download_url: "https://example.invalid/a.gz".into(),
            size: 1,
            digest: digest.map(str::to_string),
        };
        // A replaced asset comes with a digest of its own: the pin decides.
        assert_eq!(
            expected_sha256(&asset(Some("sha256:ab12")), Some("ff")),
            Some("ff")
        );
        assert_eq!(expected_sha256(&asset(None), Some("ff")), Some("ff"));
        assert_eq!(
            expected_sha256(&asset(Some("sha256:ab12")), None),
            Some("ab12")
        );
        // Another algorithm is no SHA-256.
        assert_eq!(expected_sha256(&asset(Some("sha512:ab12")), None), None);
        assert_eq!(expected_sha256(&asset(None), None), None);
    }

    fn server(id: &str) -> ServerInfo {
        super::super::ServerRegistry::load()
            .unwrap()
            .get_server(id)
            .unwrap()
            .clone()
    }

    /// An npm server is installed at its pinned version with its
    /// companions, which it needs but does not depend on.
    #[test]
    fn an_npm_server_is_installed_with_its_companions() {
        let deps = |id: &str| -> serde_json::Value {
            serde_json::from_str::<serde_json::Value>(&npm_package_json(&server(id)).unwrap())
                .unwrap()["dependencies"]
                .clone()
        };
        assert_eq!(
            deps("typescript-language-server"),
            serde_json::json!({"typescript-language-server": "5.3.0", "typescript": "5.9.3"})
        );
        assert_eq!(
            deps("volar"),
            serde_json::json!({"@vue/language-server": "2.2.12", "typescript": "5.9.3"})
        );
        assert_eq!(
            deps("sql-language-server"),
            serde_json::json!({"sql-language-server": "1.7.1", "vscode-languageserver-protocol": "3.16.0"})
        );
        assert_eq!(
            deps("nomicfoundation-solc"),
            serde_json::json!({"@nomicfoundation/solidity-language-server": "0.8.25"})
        );

        // A scoped companion keeps its leading `@`; one without a version is refused.
        let mut scoped = server("pyright");
        scoped.source.companions = vec!["@types/node@22.0.0".to_string()];
        let json: serde_json::Value =
            serde_json::from_str(&npm_package_json(&scoped).unwrap()).unwrap();
        assert_eq!(json["dependencies"]["@types/node"], "22.0.0");
        scoped.source.companions = vec!["@types/node".to_string()];
        assert!(npm_package_json(&scoped).is_err());
    }

    /// A .NET tool and a Coursier launcher are installed where `bin_path`
    /// points: the tool in `bin/`, not `tools/`, and the launcher by
    /// `cs bootstrap` from its Maven coordinates, which `cs install` would take
    /// for an application's name.
    #[test]
    fn tools_and_launchers_install_where_the_registry_runs_them() {
        let dir = Path::new("/srv/lsp/csharp-ls");
        let csharp = server("csharp-ls");
        let args = dotnet_tool_args("csharp-ls", "0.16.0", dir);
        assert_eq!(
            args,
            [
                "tool",
                "install",
                "--tool-path",
                &*dir.join("bin").to_string_lossy(),
                "--version",
                "0.16.0",
                "csharp-ls"
            ]
        );
        assert_eq!(
            Path::new(&args[3]),
            dir.join(&csharp.bin_path).parent().unwrap()
        );

        let metals = server("metals");
        let launcher = Path::new("/srv/lsp/metals").join(&metals.bin_path);
        assert_eq!(
            coursier_bootstrap_args(
                metals.source.package.as_deref().unwrap(),
                metals.source.version.as_deref().unwrap(),
                &launcher
            ),
            [
                "bootstrap",
                "--standalone",
                "org.scalameta:metals_2.13:1.6.7",
                "-o",
                &*launcher.to_string_lossy(),
            ]
        );
    }

    /// A standalone launcher is told by the libraries it names, wherever in
    /// it they are: across the blocks it is read in too.
    #[test]
    fn a_launcher_holding_its_libraries_is_told_from_a_thin_one() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = dir.path().join("metals");
        let mut body = vec![b'x'; (1 << 20) - 10];
        body.extend_from_slice(b"coursier/bootstrap/launcher/jars/metals_2.13-1.6.7.jar");
        std::fs::write(&launcher, &body).unwrap();
        assert!(holds_its_libraries(&launcher));

        let mut thin = vec![b'x'; 3 << 20];
        thin.extend_from_slice(b"coursier/bootstrap/launcher/bootstrap-jar-urls");
        std::fs::write(&launcher, &thin).unwrap();
        assert!(!holds_its_libraries(&launcher));
        assert!(!holds_its_libraries(&dir.path().join("missing")));
    }

    /// The `initialize` result of the server `config` describes, started in
    /// `root`, or why there is none.
    async fn initialize(
        config: &crate::config::LspServerConfig,
        root: &Path,
    ) -> std::result::Result<serde_json::Value, String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
        let mut child = tokio::process::Command::new(&config.command)
            .args(&config.args)
            .envs(&config.env)
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", config.command))?;
        let uri = crate::bridge::translator::path_to_uri(root)
            .map_err(|e| e.to_string())?
            .as_str()
            .to_owned();
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": uri,
                "workspaceFolders": [{"uri": uri, "name": "probe"}],
                "initializationOptions": config.initialization_options,
                "capabilities": {},
            },
        })
        .to_string();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let answer = async {
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).await.ok()? == 0 {
                        return None;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse::<usize>().ok();
                    }
                }
                let mut message = vec![0; length?];
                stdout.read_exact(&mut message).await.ok()?;
                let message: serde_json::Value = serde_json::from_slice(&message).ok()?;
                if message["id"] == 1 && message.get("method").is_none() {
                    return Some(message);
                }
            }
        };
        match tokio::time::timeout(std::time::Duration::from_secs(180), answer).await {
            Ok(Some(message)) if message["result"]["capabilities"].is_object() => {
                Ok(message["result"].clone())
            }
            Ok(Some(message)) => Err(format!("answered {message}")),
            Ok(None) | Err(_) => {
                let _ = child.start_kill();
                let mut stderr = String::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        err.read_to_string(&mut stderr),
                    )
                    .await;
                }
                Err(format!(
                    "no answer to initialize; stderr: {}",
                    stderr.chars().take(600).collect::<String>()
                ))
            }
        }
    }

    /// Each server a package manager installs, installed as `lsp setup` does
    /// where the mirror has no build (all a Mac with an Intel processor
    /// gets), resolved as Devscriptor resolves it, answers `initialize`.
    /// The .NET tools need a .NET SDK and are checked by
    /// `tools_and_launchers_install_where_the_registry_runs_them`.
    #[tokio::test]
    #[ignore = "installs the npm, Coursier, gem and Go servers from their publishers"]
    async fn package_manager_installs_start() {
        let root = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let registry = super::super::ServerRegistry::load().unwrap();
        let mut failures = Vec::new();
        let mut ids: Vec<&String> = registry
            .servers
            .iter()
            .filter(|(_, s)| {
                matches!(
                    s.source.source_type,
                    SourceType::Npm
                        | SourceType::Coursier
                        | SourceType::Gem
                        | SourceType::GoInstall
                )
            })
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        for id in ids {
            let info = server(id);
            let dir = root.path().join(id);
            std::fs::create_dir_all(&dir).unwrap();
            let installed = match info.source.source_type {
                SourceType::Npm => install_npm_package(&info, &dir, None).await,
                SourceType::Coursier => install_coursier_package(&info, &dir, None).await,
                SourceType::Gem => install_gem(&info, &dir, None).await,
                _ => install_go_package(&info, &dir, None).await,
            };
            if let Err(e) = installed {
                failures.push(format!("{id}: install: {e:#}"));
                continue;
            }
            let resolver = crate::lsp_architecture::resolver::LspResolver::with_dirs(
                root.path().to_path_buf(),
                PathBuf::from("/nonexistent/bundled"),
            );
            let Some(resolved) = resolver.resolve(id) else {
                failures.push(format!("{id}: installed, but not resolved"));
                continue;
            };
            if !matches!(
                resolved.source,
                crate::lsp_architecture::LspSource::Installed(_)
            ) {
                failures.push(format!(
                    "{id}: resolved to {:?}, not the install",
                    resolved.source
                ));
                continue;
            }
            if let Err(e) = initialize(&resolved.config, project.path()).await {
                failures.push(format!("{id}: {e}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// clangd's release has builds for Linux x86_64, macOS and Windows only,
    /// and its `clangd-linux` zip is x86_64: on Linux ARM it is not taken.
    #[test]
    fn a_release_is_taken_only_for_the_platforms_it_is_published_for() {
        let clangd = server("clangd");
        for platform in [
            "linux-x86_64",
            "macos-x86_64",
            "macos-aarch64",
            "windows-x86_64",
        ] {
            assert!(published_for(&clangd, platform).is_ok(), "{platform}");
        }
        let refused = published_for(&clangd, "linux-aarch64")
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("it is not published for linux-aarch64"),
            "{refused}"
        );
        // A release with a build for each platform names none.
        assert!(published_for(&server("rust-analyzer"), "linux-aarch64").is_ok());
    }

    /// sourcekit-lsp comes with the Swift toolchain and is not installed, from
    /// the mirror or anywhere else, so nothing is put in front of the
    /// toolchain's own.
    #[tokio::test]
    async fn a_server_of_the_toolchain_is_not_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        download_server(&server("sourcekit-lsp"), dir.path(), None)
            .await
            .unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// The Dart SDK is a build per platform, each with the checksum Google
    /// publishes beside it, named the way Google names the platform.
    #[test]
    fn the_dart_sdk_resolves_to_each_platforms_build() {
        let dart = server("dart-ls");
        let base = "https://storage.googleapis.com/dart-archive/channels/stable/release/3.12.2/sdk";
        for (platform, name, sha256) in [
            (
                "linux-x86_64",
                "linux-x64",
                "28e47b44cf075f36771046c068bb0d174201cf9c7608744aed1cc23204299c2d",
            ),
            (
                "linux-aarch64",
                "linux-arm64",
                "f82c83ece7d168047550dfd4a664e4071ac7c488bddb72dc43102c22d7e0b518",
            ),
            (
                "macos-x86_64",
                "macos-x64",
                "38199f56fe22f2235e76799191d5b9516e360369c61b6ba4411398d5d5920bab",
            ),
            (
                "macos-aarch64",
                "macos-arm64",
                "cd8753928e77b6b665bd70dce0e64b4ec6d2e2fde141d6409bb716c8ac1f1c0a",
            ),
            (
                "windows-x86_64",
                "windows-x64",
                "77fd96c823ed09a85e58209a2c5f16b0fc02e5ed4f3e3d46fddf4be763d498d6",
            ),
            (
                "windows-aarch64",
                "windows-arm64",
                "ef0168b2bbceccda659e3d9a0f5ebebd5326ef84800ca6af7ccceeb1cfd3daf0",
            ),
        ] {
            let (url, expected) = direct_artifact(&dart, platform).unwrap();
            assert_eq!(url, format!("{base}/dartsdk-{name}-release.zip"));
            assert_eq!(expected, sha256, "{platform}");
        }

        let error = direct_artifact(&dart, "linux-riscv64").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no build of it for linux-riscv64")
        );
    }

    /// jdtls is one tarball for every platform, its build timestamp pinned
    /// with its version.
    #[test]
    fn jdtls_resolves_to_its_pinned_milestone() {
        let jdtls = server("jdtls");
        for platform in ["linux-x86_64", "macos-aarch64", "windows-x86_64"] {
            let (url, expected) = direct_artifact(&jdtls, platform).unwrap();
            assert_eq!(
                url,
                "https://download.eclipse.org/jdtls/milestones/1.61.0/\
                 jdt-language-server-1.61.0-202609031315.tar.gz"
            );
            assert_eq!(
                expected,
                "338e7e73d61836651ba2453919a0d34fa763eb4e7c03342092309bffb8934c64"
            );
        }
    }

    /// The Kotlin LSP is a build per platform, each with the checksum
    /// JetBrains publishes beside it; macOS's are zips named `.sit`.
    #[test]
    fn the_kotlin_lsp_resolves_to_each_platforms_build() {
        let kotlin = server("kotlin-lsp");
        let base = "https://download.jetbrains.com/language-server/kotlin-server/263.6379.0";
        for (platform, file, sha256) in [
            (
                "linux-x86_64",
                "kotlin-server-263.6379.0.tar.gz",
                "ab8ca4455dc2fc5fe1a24db2bccc46c104254d2c465155c4251ee65df8f3f7cc",
            ),
            (
                "linux-aarch64",
                "kotlin-server-263.6379.0-aarch64.tar.gz",
                "50999901ef8bcfa1e58561b6a8d782a72dea5620fcf92a64130807f8924a56fc",
            ),
            (
                "macos-x86_64",
                "kotlin-server-263.6379.0.sit",
                "e69e0c9d27b915b2db9ee692ec08d097df1a394d9f21190457ef199f2905f77e",
            ),
            (
                "macos-aarch64",
                "kotlin-server-263.6379.0-aarch64.sit",
                "ebef2e13cd4adc4ec9e04084b848000a3ec7a9d2917c64f269574ce2efe9ecad",
            ),
            (
                "windows-x86_64",
                "kotlin-server-263.6379.0.win.zip",
                "72148fa0832c2a45a4b26c55f56197d8d8886bcdb732b910607efc5e2693b9df",
            ),
            (
                "windows-aarch64",
                "kotlin-server-263.6379.0-aarch64.win.zip",
                "290f48cb564295239bb678bb66b1ebbd06755b14fd4890f3cdf822f41af011ef",
            ),
        ] {
            let (url, expected) = direct_artifact(&kotlin, platform).unwrap();
            assert_eq!(url, format!("{base}/{file}"));
            assert_eq!(expected, sha256, "{platform}");
        }
    }

    /// Nothing is fetched that could not be verified or names no file.
    #[test]
    fn a_download_without_checksum_or_with_a_placeholder_left_is_refused() {
        let mut jdtls = server("jdtls");
        jdtls.source.expected_sha256 = None;
        let error = direct_artifact(&jdtls, "linux-x86_64").unwrap_err();
        assert!(error.to_string().contains("pins no SHA-256"), "{error}");

        let mut jdtls = server("jdtls");
        jdtls.source.url =
            Some("https://download.eclipse.org/jdtls/{version}-{timestamp}.tar.gz".into());
        let error = direct_artifact(&jdtls, "linux-x86_64").unwrap_err();
        assert!(
            error.to_string().contains("leaves {timestamp} unfilled"),
            "{error}"
        );

        let mut jdtls = server("jdtls");
        jdtls.source.version = None;
        let error = direct_artifact(&jdtls, "linux-x86_64").unwrap_err();
        assert!(
            error.to_string().contains("leaves {version} unfilled"),
            "{error}"
        );
    }

    /// Every pin in the registry is complete: a download resolves on every
    /// platform the mirror serves, a release with a pinned checksum pins its
    /// version, and a download's builds fill its URL.
    #[test]
    fn every_pinned_source_in_the_registry_resolves() {
        let registry = super::super::ServerRegistry::load().unwrap();
        for server in registry.list_servers() {
            let source = &server.source;
            match source.source_type {
                SourceType::Download => {
                    assert!(source.version.is_some(), "{}: no version", server.id);
                    if source.platforms.is_empty() {
                        assert!(source.expected_sha256.is_some(), "{}", server.id);
                    } else {
                        assert!(source.expected_sha256.is_none(), "{}", server.id);
                        assert!(source.url.as_deref().unwrap().contains("{platform}"));
                    }
                    for platform in [
                        "linux-x86_64",
                        "linux-aarch64",
                        "macos-aarch64",
                        "windows-x86_64",
                    ] {
                        let (url, sha256) = direct_artifact(server, platform)
                            .unwrap_or_else(|e| panic!("{} on {platform}: {e}", server.id));
                        assert!(url.starts_with("https://"), "{url}");
                        assert_eq!(sha256.len(), 64, "{} on {platform}", server.id);
                        assert!(sha256.bytes().all(|b| b.is_ascii_hexdigit()));
                    }
                }
                SourceType::GithubRelease => {
                    if let Some(sha256) = &source.expected_sha256 {
                        assert!(
                            source.version.as_deref().is_some_and(|v| v != "latest"),
                            "{}: a pinned checksum needs a pinned release",
                            server.id
                        );
                        assert_eq!(sha256.len(), 64, "{}", server.id);
                    }
                    assert!(source.platforms.is_empty(), "{}", server.id);
                }
                _ => assert!(source.platforms.is_empty(), "{}", server.id),
            }
        }
    }

    /// The registry's pins are the checksums the publishers put beside their
    /// files: `.sha256` for jdtls and the Kotlin LSP, `.sha256sum` for the
    /// Dart SDK.
    #[tokio::test]
    #[ignore = "fetches the publishers' checksum files"]
    async fn pinned_checksums_match_the_publishers() {
        for (id, sidecar) in [
            ("jdtls", ".sha256"),
            ("kotlin-lsp", ".sha256"),
            ("dart-ls", ".sha256sum"),
        ] {
            let server = server(id);
            let mut platforms: Vec<&str> =
                server.source.platforms.keys().map(String::as_str).collect();
            if platforms.is_empty() {
                platforms.push("linux-x86_64");
            }
            for platform in platforms {
                let (url, pinned) = direct_artifact(&server, platform).unwrap();
                let published = LSP_HTTP_CLIENT
                    .get(format!("{url}{sidecar}"))
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .unwrap_or_else(|e| panic!("{id} on {platform}: {e}"))
                    .text()
                    .await
                    .unwrap();
                assert_eq!(
                    published.split_whitespace().next(),
                    Some(pinned),
                    "{id} on {platform}"
                );
            }
        }
    }

    /// The license files the packaging scripts fetch from the publishers'
    /// repositories are the ones the registry pins.
    #[tokio::test]
    #[ignore = "fetches the license files from the publishers' repositories"]
    async fn pinned_license_files_match_the_publishers() {
        use sha2::{Digest, Sha256};
        let registry = super::super::ServerRegistry::load().unwrap();
        for server in registry.list_servers() {
            for file in &server.license_files {
                let (Some(url), Some(pinned)) = (&file.url, &file.sha256) else {
                    continue;
                };
                let body = LSP_HTTP_CLIENT
                    .get(url)
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .unwrap_or_else(|e| panic!("{}: {e}", server.id))
                    .bytes()
                    .await
                    .unwrap();
                assert_eq!(
                    &hex::encode(Sha256::digest(&body)),
                    pinned,
                    "{}: {url}",
                    server.id
                );
            }
        }
    }

    /// Each pinned download installs from its publisher, as `lsp setup` does
    /// where the mirror has no build: verified, unpacked, lifted into place,
    /// its binary executable.
    #[tokio::test]
    #[ignore = "downloads about 650 MB from the publishers"]
    async fn pinned_downloads_install_from_their_publishers() {
        for id in ["jdtls", "kotlin-lsp", "dart-ls"] {
            let server = server(id);
            let dir = tempfile::tempdir().unwrap();
            download_direct(&server, dir.path(), None)
                .await
                .unwrap_or_else(|e| panic!("{id}: {e:#}"));
            let bin = dir.path().join(&server.bin_path);
            assert!(binary_present(&bin).await, "{id}: no {}", bin.display());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&bin).unwrap().permissions().mode();
                assert_eq!(
                    mode & 0o111,
                    0o111,
                    "{id}: {} is not executable",
                    bin.display()
                );
            }
            // The archive itself is gone.
            let left: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| name.ends_with(".zip") || name.ends_with(".tar.gz"))
                .collect();
            assert!(left.is_empty(), "{id}: {left:?} left behind");
        }
    }

    #[test]
    fn the_platform_key_names_os_and_arch() {
        let key = platform_key();
        assert_eq!(key, format!("{}-{}", get_platform(), get_arch()));
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(key, "linux-x86_64");
    }

    /// The Dart SDK's zip and the Kotlin LSP's macOS `.sit` record their
    /// binaries as executable; unpacked without the bit, neither could start.
    #[cfg(unix)]
    #[tokio::test]
    async fn zip_entries_keep_their_permission_bits() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("server.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        for (name, mode) in [
            ("server/bin/intellij-server", 0o755),
            ("server/lib/server.jar", 0o644),
            ("server/bin/setuid", 0o4755),
        ] {
            zip.start_file(name, SimpleFileOptions::default().unix_permissions(mode))
                .unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();

        let dest = tmp.path().join("out");
        extract_zip(&archive, &dest).await.unwrap();

        let mode = |name: &str| {
            std::fs::metadata(dest.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(mode("server/bin/intellij-server"), 0o755);
        assert_eq!(mode("server/lib/server.jar"), 0o644);
        assert_eq!(mode("server/bin/setuid"), 0o755);
    }

    fn write(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    /// A mirror package unpacks as `<id>/…`; one holding an entry named like
    /// its folder (`gopls/gopls`) still lifts.
    #[tokio::test]
    async fn a_mirror_package_is_lifted_out_of_its_top_level_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let install_dir = tmp.path().join("gopls");
        write(&install_dir.join("gopls/bin/gopls"));
        write(&install_dir.join("gopls/gopls"));

        lift_into_place(&install_dir, "bin/gopls").await.unwrap();

        assert!(install_dir.join("bin/gopls").is_file());
        assert!(install_dir.join("gopls").is_file());
        assert!(!install_dir.join(".gopls.unpacked").exists());
    }

    /// GitHub's clangd zip: `clangd_<version>/bin/clangd` and the builtin
    /// headers under `lib/clang/<major>/include`, which the registry's
    /// `--resource-dir={server_dir}/resource-dir` must find.
    #[tokio::test]
    async fn the_github_clangd_layout_is_lifted_and_given_its_resource_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("clangd_23.1.0/bin/clangd"));
        write(
            &tmp.path()
                .join("clangd_23.1.0/lib/clang/23/include/stddef.h"),
        );

        lift_into_place(tmp.path(), "bin/clangd").await.unwrap();
        move_clang_resource_dir(tmp.path()).await.unwrap();

        assert!(tmp.path().join("bin/clangd").is_file());
        assert!(tmp.path().join("resource-dir/include/stddef.h").is_file());
    }

    /// A `bin_path` of `server/bin/…` runs through the `server/` folder: that
    /// folder is part of the layout.
    #[tokio::test]
    async fn a_folder_the_bin_path_runs_through_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("server/bin/language-server"));

        lift_into_place(tmp.path(), "server/bin/language-server")
            .await
            .unwrap();

        assert!(tmp.path().join("server/bin/language-server").is_file());
    }

    #[tokio::test]
    async fn a_gzipped_binary_lands_at_its_bin_path() {
        use std::io::Write as _;
        let tmp = tempfile::tempdir().unwrap();
        let gz = tmp.path().join("rust-analyzer-x86_64-unknown-linux-gnu.gz");
        let mut encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(&gz).unwrap(),
            flate2::Compression::fast(),
        );
        encoder.write_all(b"#!/bin/sh\n").unwrap();
        encoder.finish().unwrap();

        let bin = tmp.path().join("bin/rust-analyzer");
        extract_gz(&gz, &bin).await.unwrap();

        assert_eq!(std::fs::read(&bin).unwrap(), b"#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&bin).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
    }

    /// reqwest is built without a crypto provider of its own, so a client that
    /// did not get the preconfigured TLS would panic here.
    #[test]
    fn the_shared_client_builds_without_a_process_wide_crypto_provider() {
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        let _client = &*LSP_HTTP_CLIENT;
        assert_eq!(tls_config().alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

    /// Mirrors the manifest schema emitted by
    /// `scripts/<platform>/package-lsp-servers.sh`. Guards the script↔parser
    /// contract: `servers` is a map keyed by registry ID, each entry carries
    /// `filename`/`sha256`, `version` is optional, and the `size_bytes` the
    /// scripts write too is ignored.
    /// A pinned build is taken from the mirror only at its version: a mirror
    /// build of jdtls 1.44.0 is not, against the pinned 1.61.0. A package
    /// manager's version does not bind the mirror's build.
    #[test]
    fn the_mirror_serves_a_pinned_build_only_at_its_version() {
        let entry = |version: &str| ManifestEntry {
            filename: "x.tar.gz".to_string(),
            sha256: "00".to_string(),
            version: version.to_string(),
        };
        let jdtls = server("jdtls");
        let pin = jdtls.source.version.clone().expect("jdtls is pinned");
        assert!(mirror_entry_fits(&jdtls, &entry(&pin)).is_ok());
        let stale = mirror_entry_fits(&jdtls, &entry("2026-07-10")).unwrap_err();
        assert!(stale.to_string().contains("2026-07-10"), "{stale}");
        assert!(mirror_entry_fits(&jdtls, &entry("")).is_err());

        // npm's version is what the fallback installs: the mirror's newer
        // build is taken.
        let pyright = server("pyright");
        assert!(pyright.source.version.is_some());
        assert!(mirror_entry_fits(&pyright, &entry("2026-07-10")).is_ok());
    }
}
