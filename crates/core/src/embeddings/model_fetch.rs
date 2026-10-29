//! Self-hosted provisioning + loading of the embedding model.
//!
//! Devscriptor uses exactly one model: the **INT8-quantized** ONNX export of
//! `jinaai/jina-embeddings-v2-base-code` (`onnx/model_quantized.onnx`, 768-dim,
//! ~150 MB on disk, < 1 GB resident). It is loaded by the pure-Rust `tract`
//! backend ([`TractEmbedder::load`](super::tract_backend::TractEmbedder::load)),
//! which reads the ONNX + `tokenizer.json` files straight from a local cache, so
//! once provisioned the model loads fully offline. The quantized model is what
//! allows semantic search to run within the 4 GB-minimum / 8 GB-recommended
//! memory targets.
//!
//! # Provisioning order (first run)
//!
//! Missing files are fetched from **Hugging Face first**, then from the
//! **Devscriptor mirror** as a fallback if Hugging Face is unreachable:
//!
//! 1. `https://huggingface.co/jinaai/jina-embeddings-v2-base-code/resolve/<SHA>/`
//!    (pinned to `PINNED_HF_REVISION`; override with `DEVSCRIPTOR_HF_BASE_URL`)
//! 2. `https://github.com/vladimirpesic/devscriptor/releases/download/<release>/`,
//!    the GitHub release `MODEL_MIRROR_RELEASE` the model mirror workflow
//!    (`.github/workflows/model-mirror.yml`) publishes, which holds the same
//!    files side by side (`model_quantized.onnx`, not `onnx/…`); override with
//!    `DEVSCRIPTOR_MODEL_MIRROR_URL`, a place that holds them the same way
//!
//! Set `DEVSCRIPTOR_MODEL_PREFER_MIRROR=1` to try the mirror first. Every file
//! is checked against the SHA-256 this build pins, whichever source it comes
//! from. Downloading requires the `model-download` feature; loading from the
//! local cache is always available so airgapped builds can ship the files
//! out-of-band.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Mirror folder name and local cache sub-directory for the model.
const MODEL_DIR_NAME: &str = "jina-embeddings-v2-base-code";

/// Files provisioned for the model, relative to the model directory. The tract
/// backend loads `onnx/model_quantized.onnx` and `tokenizer.json`; `config.json`
/// and `tokenizer_config.json` are fetched alongside them so the cache holds the
/// complete upstream model directory.
const REQUIRED_FILES: &[&str] = &[
    "onnx/model_quantized.onnx",
    "tokenizer.json",
    "config.json",
    "tokenizer_config.json",
];

/// Files that improve fidelity but are not strictly required. `special_tokens_map.json`
/// only re-marks tokens that this BERT-based tokenizer already defines in
/// `tokenizer.json`, so the backend loads correctly whether or not it is present.
#[cfg(feature = "model-download")]
const OPTIONAL_FILES: &[&str] = &["special_tokens_map.json"];

/// SHA-256 checksums (hex, 64 chars) for required files in
/// `jinaai/jina-embeddings-v2-base-code`.
///
/// After each file is downloaded its SHA-256 is computed and compared against
/// the pinned value **before** the `.part` temp file is renamed into place.
/// This is fail-closed: a mismatch aborts the download and removes the temp
/// file, preventing a compromised mirror or MITM from installing an untrusted
/// ONNX model into the inference runtime.
///
/// The hashes are those of the files at [`PINNED_HF_REVISION`]:
///   sha256sum onnx/model_quantized.onnx tokenizer.json config.json tokenizer_config.json
///
/// Setting `DEVSCRIPTOR_SKIP_MODEL_VERIFY=1` bypasses all checksum checks (e.g.
/// for airgapped installs where files were provisioned out-of-band).
///
/// Hugging Face downloads name that immutable commit, never `resolve/main`, so
/// a later push to the model repository cannot change what is downloaded.
#[cfg(feature = "model-download")]
const REQUIRED_FILE_SHA256: &[(&str, &str)] = &[
    (
        "onnx/model_quantized.onnx",
        "ed45870251c9f0cf656e78aab0d37a23489066df8a222bb1c8caf8a45f2cb16d",
    ),
    (
        "tokenizer.json",
        "b01c78a902aa4facb2f47f95449f48e2f7bbfea5d2472ee2f6ce92323c6f86e5",
    ),
    (
        "config.json",
        "e426aa684c7f9a95c5f020aa855faf93a24f065f5fad0c9e17b124670cabdea6",
    ),
    (
        "tokenizer_config.json",
        "f477aeb15ff9f78d3c1ddf2361d2b0b8b20cf55220f839f29a37f3a18efddd89",
    ),
];

/// Local cache directory for the embedding model:
/// `~/.cache/devscriptor/models/jina-embeddings-v2-base-code/`.
pub fn model_cache_dir() -> Result<PathBuf> {
    let base = crate::user_dirs::cache_dir()
        .ok_or_else(|| Error::Embedding("Could not determine cache directory".to_string()))?;
    Ok(base.join("devscriptor").join("models").join(MODEL_DIR_NAME))
}

/// `true` when `path` exists and is non-empty.
fn is_non_empty(path: &Path) -> bool {
    matches!(std::fs::metadata(path), Ok(m) if m.len() > 0)
}

/// Required files that are missing (or empty) under `cache_dir`.
fn missing_required(cache_dir: &Path) -> Vec<&'static str> {
    REQUIRED_FILES
        .iter()
        .copied()
        .filter(|rel| !is_non_empty(&cache_dir.join(rel)))
        .collect()
}

/// Returns `true` when every strictly-required model file is present.
#[must_use]
pub fn model_files_present(cache_dir: &Path) -> bool {
    missing_required(cache_dir).is_empty()
}

/// Ensure all required model files exist locally, downloading any missing ones
/// (Hugging Face first, Devscriptor mirror as fallback).
///
/// # Errors
/// - With `model-download` enabled: every configured source failed.
/// - Without it: an actionable error listing the files that must be provided.
pub fn ensure_model_present(cache_dir: &Path, show_progress: bool) -> Result<()> {
    let missing = missing_required(cache_dir);
    if missing.is_empty() {
        return Ok(());
    }
    provision_missing(cache_dir, &missing, show_progress)
}

// The model + tokenizer are loaded by
// [`TractEmbedder::load`](super::tract_backend::TractEmbedder::load), which reads
// `onnx/model_quantized.onnx` and `tokenizer.json` from this cache directory.
// This module is only responsible for locating and provisioning those files.

#[cfg(feature = "model-download")]
fn provision_missing(cache_dir: &Path, _missing: &[&str], show_progress: bool) -> Result<()> {
    // `reqwest::blocking` builds its own current-thread runtime, which panics if
    // started from within an existing Tokio runtime. Run it on a dedicated
    // scoped thread so provisioning is safe regardless of the caller's context.
    std::thread::scope(|scope| {
        scope
            .spawn(|| provision_from_sources(cache_dir, show_progress))
            .join()
            .map_err(|_| Error::Embedding("Model download thread panicked".to_string()))?
    })
}

#[cfg(not(feature = "model-download"))]
fn provision_missing(cache_dir: &Path, missing: &[&str], show_progress: bool) -> Result<()> {
    let _ = show_progress;
    Err(Error::Embedding(format!(
        "Embedding model files missing from {}: [{}].\n\
         This build was compiled without the `model-download` feature, so it \
         cannot fetch them automatically. Run scripts/download-embedding-model.sh \
         (or copy the files manually) to provision jinaai/jina-embeddings-v2-base-code.",
        cache_dir.display(),
        missing.join(", ")
    )))
}

#[cfg(feature = "model-download")]
fn provision_from_sources(cache_dir: &Path, show_progress: bool) -> Result<()> {
    let prefer_mirror = std::env::var("DEVSCRIPTOR_MODEL_PREFER_MIRROR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let hf = ("Hugging Face", hf_base(), Layout::Repository);
    let mirror = (
        "Devscriptor mirror",
        model_mirror_base(),
        Layout::SideBySide,
    );
    let sources = if prefer_mirror {
        [mirror, hf]
    } else {
        [hf, mirror]
    };

    let mut errors = Vec::new();
    for (name, base, layout) in sources {
        tracing::info!("Fetching embedding model from {name} ({base})");
        match fetch_all_from(cache_dir, &base, layout, show_progress) {
            Ok(()) if model_files_present(cache_dir) => {
                tracing::info!(
                    "Embedding model provisioned from {name} → {}",
                    cache_dir.display()
                );
                return Ok(());
            }
            Ok(()) => errors.push(format!("{name}: still incomplete after fetch")),
            Err(e) => {
                tracing::warn!("{name} provisioning failed: {e}");
                errors.push(format!("{name}: {e}"));
            }
        }
    }

    Err(Error::Embedding(format!(
        "Could not provision the embedding model from any source. Tried: {}.",
        errors.join("; ")
    )))
}

#[cfg(feature = "model-download")]
/// Pinned immutable commit SHA for `jinaai/jina-embeddings-v2-base-code`.
///
/// The commit Hugging Face reports for the model:
///   curl -s "https://huggingface.co/api/models/jinaai/jina-embeddings-v2-base-code" | jq -r .sha
///
/// `DEVSCRIPTOR_HF_BASE_URL` overrides the download location.
const PINNED_HF_REVISION: &str = "516f4baf13dec4ddddda8631e019b5737c8bc250";

/// Base URL for the Hugging Face model repo (no trailing slash).
///
/// Uses the pinned commit SHA so byte content is stable regardless of future
/// pushes to `main`.  Override with `DEVSCRIPTOR_HF_BASE_URL` to track a
/// different revision.
#[cfg(feature = "model-download")]
fn hf_base() -> String {
    std::env::var("DEVSCRIPTOR_HF_BASE_URL").unwrap_or_else(|_| {
        format!(
            "https://huggingface.co/jinaai/jina-embeddings-v2-base-code/resolve/{}",
            PINNED_HF_REVISION
        )
    })
}

/// The GitHub release that mirrors the model's files at
/// [`PINNED_HF_REVISION`], which its tag ends with.
#[cfg(feature = "model-download")]
const MODEL_MIRROR_RELEASE: &str = "jina-embeddings-v2-base-code-516f4ba";

/// Base URL for the Devscriptor mirror (no trailing slash).
#[cfg(feature = "model-download")]
fn model_mirror_base() -> String {
    std::env::var("DEVSCRIPTOR_MODEL_MIRROR_URL").unwrap_or_else(|_| {
        format!(
            "https://github.com/vladimirpesic/devscriptor/releases/download/{MODEL_MIRROR_RELEASE}"
        )
    })
}

/// How a source lays out the model's files.
#[cfg(feature = "model-download")]
#[derive(Debug, Clone, Copy)]
enum Layout {
    /// As the model's repository does: `onnx/model_quantized.onnx`.
    Repository,
    /// Side by side, as a release's assets are: `model_quantized.onnx`.
    SideBySide,
}

#[cfg(feature = "model-download")]
impl Layout {
    /// The URL of `rel`, a file's path in the model's repository, at `base`.
    fn url(self, base: &str, rel: &str) -> String {
        match self {
            Self::Repository => format!("{base}/{rel}"),
            Self::SideBySide => format!("{base}/{}", rel.rsplit('/').next().unwrap_or(rel)),
        }
    }
}

/// The TLS configuration the download client is given: ring for the
/// cryptography and the Mozilla roots `webpki-roots` bundles, so a host without
/// a CA bundle still verifies. reqwest is built without a crypto provider of its
/// own (see the workspace manifest) and panics building a client without one.
/// HTTP/1.1 is the only protocol offered, as reqwest itself would offer without
/// its `http2` feature. The LSP downloader configures its clients the same way.
#[cfg(feature = "model-download")]
fn tls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let mut config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports TLS 1.2 and 1.3")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

/// Download all still-missing files from a single base URL. Required files fail
/// the source on error; optional files are best-effort.
#[cfg(feature = "model-download")]
fn fetch_all_from(cache_dir: &Path, base: &str, layout: Layout, show_progress: bool) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .tls_backend_preconfigured(tls_config())
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .map_err(|e| Error::Embedding(format!("Failed to build HTTP client: {e}")))?;

    for rel in REQUIRED_FILES {
        let dest = cache_dir.join(rel);
        if is_non_empty(&dest) {
            continue;
        }
        download_one(&client, &layout.url(base, rel), &dest, rel, show_progress)?;
    }

    for rel in OPTIONAL_FILES {
        let dest = cache_dir.join(rel);
        if is_non_empty(&dest) {
            continue;
        }
        if let Err(e) = download_one(&client, &layout.url(base, rel), &dest, rel, show_progress) {
            tracing::warn!("Optional model file {rel} unavailable from {base}: {e}");
        }
    }

    Ok(())
}

#[cfg(feature = "model-download")]
fn download_one(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    label: &str,
    show_progress: bool,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Embedding(format!("Failed to create {}: {e}", parent.display())))?;
    }

    tracing::info!("Downloading {label} from {url}");
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| Error::Embedding(format!("GET {url} failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Embedding(format!(
            "GET {url} returned HTTP {}",
            resp.status()
        )));
    }

    let total = resp.content_length().unwrap_or(0);
    let progress = if show_progress && total > 0 {
        let pb = indicatif::ProgressBar::new(total);
        pb.set_style(
            indicatif::ProgressStyle::with_template(
                "{msg} [{bar:40}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        pb.set_message(label.to_string());
        Some(pb)
    } else {
        None
    };

    // Download to a temp file, then verify SHA-256 before atomically renaming
    // into place — fail-closed against a compromised mirror or MITM.
    let tmp = dest.with_extension("part");
    let mut file = std::fs::File::create(&tmp)
        .map_err(|e| Error::Embedding(format!("Failed to create {}: {e}", tmp.display())))?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| Error::Embedding(format!("Read from {url} failed: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| Error::Embedding(format!("Write to {}: {e}", tmp.display())))?;
        hasher.update(&buf[..n]);
        if let Some(pb) = &progress {
            pb.inc(n as u64);
        }
    }
    file.flush()
        .map_err(|e| Error::Embedding(format!("Flush {}: {e}", tmp.display())))?;
    drop(file);

    // N1 fix: verify SHA-256 before accepting the file.
    let skip_verify = std::env::var("DEVSCRIPTOR_SKIP_MODEL_VERIFY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !skip_verify {
        let actual_hex = hex::encode(hasher.finalize());

        // Look up the pinned hash for this relative path.
        let pinned = REQUIRED_FILE_SHA256
            .iter()
            .find(|(rel, _)| *rel == label)
            .map(|(_, h)| *h);

        match pinned {
            Some(expected) if !expected.is_empty() => {
                if actual_hex != expected {
                    // Remove the bad temp file before returning an error.
                    let _ = std::fs::remove_file(&tmp);
                    return Err(Error::Embedding(format!(
                        "SHA-256 mismatch for {label}:\n  expected: {expected}\n  got:      {actual_hex}\n\
                         Refusing to use a file whose integrity cannot be confirmed. \
                         If you provisioned the model out-of-band, set \
                         DEVSCRIPTOR_SKIP_MODEL_VERIFY=1."
                    )));
                }
                tracing::debug!("{label}: SHA-256 OK ({actual_hex})");
            }
            Some(_) => {
                // Hash entry exists but is empty — not yet pinned by the maintainer.
                tracing::warn!(
                    "Security: SHA-256 for {label} is not pinned in REQUIRED_FILE_SHA256. \
                     The downloaded file has not been integrity-verified. \
                     Please add the expected hash to crates/core/src/embeddings/model_fetch.rs."
                );
            }
            None => {
                // Optional file (not in REQUIRED_FILE_SHA256) — best-effort.
                tracing::debug!(
                    "{label}: no pinned SHA-256 (optional file), skipping verification"
                );
            }
        }
    }

    std::fs::rename(&tmp, dest)
        .map_err(|e| Error::Embedding(format!("Failed to finalize {}: {e}", dest.display())))?;
    if let Some(pb) = progress {
        pb.finish_and_clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// reqwest is built without a crypto provider of its own, so a client that
    /// did not get the preconfigured TLS would panic here.
    #[test]
    #[cfg(feature = "model-download")]
    fn the_download_client_builds_without_a_process_wide_crypto_provider() {
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(tls_config())
            .build()
            .expect("client with the bundled TLS configuration");
    }

    /// The script that stages the model, for the local cache or the mirror,
    /// fetches the commit Devscriptor pins and checks each file against the
    /// checksum it pins: from `main`, a later push would stage a mirror copy
    /// Devscriptor refuses.
    #[test]
    #[cfg(feature = "model-download")]
    fn the_staging_script_fetches_the_pinned_files() {
        let script = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/download-embedding-model.sh"
        ))
        .unwrap();
        assert!(script.contains(&format!("MODEL_REVISION=\"{PINNED_HF_REVISION}\"")));
        for (file, hash) in REQUIRED_FILE_SHA256 {
            let call = format!("download_file \"$BASE_URL/{file}\"");
            let line = script
                .lines()
                .position(|l| l.starts_with(&call))
                .unwrap_or_else(|| panic!("the script does not download {file}"));
            let pinned = script.lines().nth(line + 1).unwrap_or_default().trim();
            assert_eq!(pinned, *hash, "{file}");
        }
    }

    /// A file that does not match its pinned checksum, or is cut short, is
    /// not left in place for Devscriptor to take as present: the staging
    /// script wrote straight into the cache.
    #[test]
    #[cfg(unix)]
    fn the_staging_script_leaves_no_file_it_could_not_verify() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let tools = root.path().join("tools");
        std::fs::create_dir(&tools).unwrap();
        let curl = tools.join("curl");
        std::fs::write(
            &curl,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && echo other > \"$2\"; shift; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            tools.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let target = root.path().join("model");
        let status = std::process::Command::new("bash")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scripts/download-embedding-model.sh"
            ))
            .env("PATH", path)
            .env("DEVSCRIPTOR_MODEL_DIR", &target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        let onnx = target.join("onnx");
        assert!(!onnx.join("model_quantized.onnx").exists());
        assert!(!onnx.join("model_quantized.onnx.part").exists());
    }

    /// Every REQUIRED_FILE_SHA256 entry must be a 64-char lowercase hex
    /// string. This test fails if any entry is empty or malformed, catching the
    /// case where a maintainer adds a new required file but forgets the hash.
    #[test]
    #[cfg(feature = "model-download")]
    fn all_required_hashes_are_pinned() {
        for (file, hash) in REQUIRED_FILE_SHA256 {
            assert!(
                hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()),
                "SHA-256 for {file:?} is not a 64-char hex string (got {:?}). \
                 Run `sha256sum` on the file and fill in REQUIRED_FILE_SHA256.",
                hash
            );
        }
        // Every REQUIRED_FILE must have an entry.
        for rel in REQUIRED_FILES {
            assert!(
                REQUIRED_FILE_SHA256.iter().any(|(f, _)| f == rel),
                "Required file {rel:?} has no SHA-256 entry in REQUIRED_FILE_SHA256"
            );
        }
    }

    /// The default HF base URL must contain the pinned commit SHA,
    /// not the mutable `main` branch ref.  This test fails if `hf_base()` is
    /// reverted to `resolve/main`.
    #[test]
    #[cfg(feature = "model-download")]
    fn hf_base_uses_pinned_sha_not_main() {
        // Clear override so we test the compiled-in default.
        let was_set = std::env::var("DEVSCRIPTOR_HF_BASE_URL").is_ok();
        if was_set {
            // Env override is set — test the constant directly instead.
            assert_eq!(
                PINNED_HF_REVISION.len(),
                40,
                "PINNED_HF_REVISION must be a 40-char git SHA, got {:?}",
                PINNED_HF_REVISION
            );
            assert!(
                PINNED_HF_REVISION.chars().all(|c| c.is_ascii_hexdigit()),
                "PINNED_HF_REVISION must be all hex digits"
            );
        } else {
            let base = hf_base();
            assert!(
                !base.contains("/resolve/main"),
                "hf_base() must not use the mutable `main` ref; got: {base}"
            );
            assert!(
                base.contains(PINNED_HF_REVISION),
                "hf_base() must contain PINNED_HF_REVISION; got: {base}"
            );
        }
    }

    /// The mirror is the release of the pinned revision, and holds the files
    /// side by side, which their names keep apart.
    #[test]
    #[cfg(feature = "model-download")]
    fn the_mirror_holds_the_pinned_files_side_by_side() {
        assert!(MODEL_MIRROR_RELEASE.ends_with(&format!("-{}", &PINNED_HF_REVISION[..7])));
        assert_eq!(
            Layout::SideBySide.url("https://example.com/r", "onnx/model_quantized.onnx"),
            "https://example.com/r/model_quantized.onnx"
        );
        assert_eq!(
            Layout::Repository.url("https://example.com/r", "onnx/model_quantized.onnx"),
            "https://example.com/r/onnx/model_quantized.onnx"
        );
        let mut names: Vec<&str> = REQUIRED_FILES
            .iter()
            .chain(OPTIONAL_FILES)
            .map(|rel| rel.rsplit('/').next().unwrap())
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), REQUIRED_FILES.len() + OPTIONAL_FILES.len());
    }

    #[test]
    fn cache_dir_ends_with_model_dir() {
        let dir = model_cache_dir().expect("cache dir resolvable");
        assert!(dir.ends_with(MODEL_DIR_NAME), "got {}", dir.display());
        assert!(dir.to_string_lossy().contains("devscriptor"));
    }

    #[test]
    fn missing_required_reports_all_when_empty() {
        let tmp = std::env::temp_dir().join(format!("ds-model-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!model_files_present(&tmp));
        assert_eq!(missing_required(&tmp).len(), REQUIRED_FILES.len());
    }

    #[test]
    fn present_when_required_files_exist_optional_not_needed() {
        let tmp = std::env::temp_dir().join(format!("ds-model-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for rel in REQUIRED_FILES {
            let path = tmp.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
        }
        // Optional file absent — model is still considered ready.
        assert!(model_files_present(&tmp));
        assert!(missing_required(&tmp).is_empty());

        // A zero-byte required file counts as missing.
        std::fs::write(tmp.join("tokenizer.json"), b"").unwrap();
        assert!(!model_files_present(&tmp));
        assert_eq!(missing_required(&tmp), vec!["tokenizer.json"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
