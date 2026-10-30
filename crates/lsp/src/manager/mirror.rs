//! Devscriptor's mirror of the language servers.
//!
//! The LSP mirror workflow (`.github/workflows/lsp-mirror.yml`) packages every
//! server on each platform and publishes the archives side by side in one
//! GitHub release, `lsp-servers-<date>-<commit>`. `mirror.json` pins that release: its
//! tag, and each platform's manifest, which names every server's archive and
//! its SHA-256. The pin is compiled in, so Devscriptor installs from the mirror
//! only the bytes it was built with, wherever it fetches them from.
//!
//! `DEVSCRIPTOR_LSP_MIRROR_URL` fetches the archives from another place that
//! holds them side by side, such as a copy of the release. With no release
//! pinned, no platform has packages, and servers are installed from their
//! original sources.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// Where the mirror's releases are published.
const RELEASES: &str = "https://github.com/vladimirpesic/devscriptor/releases/download";

/// Overrides the place the archives are fetched from.
pub const MIRROR_URL_ENV_VAR: &str = "DEVSCRIPTOR_LSP_MIRROR_URL";

/// The platforms the mirror packages servers for, by the names
/// [`platform`] gives them.
pub const PLATFORMS: &[&str] = &[
    "linux-x86_64",
    "linux-aarch64",
    "macos-x86_64",
    "macos-aarch64",
    "windows-x86_64",
];

/// The release `mirror.json` pins.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mirror {
    /// The release's tag, or none when no release is pinned.
    release: Option<String>,
    /// Each platform's manifest, by platform name.
    platforms: HashMap<String, PlatformManifest>,
}

/// One platform's packages: the `manifest-<platform>.json` its packaging
/// script writes.
#[derive(Debug, Clone, Deserialize)]
pub struct PlatformManifest {
    pub platform: String,
    /// When the packages were made.
    #[serde(default)]
    pub generated: String,
    /// Each server's archive, by registry id.
    pub servers: HashMap<String, ManifestEntry>,
}

/// One server's archive in a platform manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    /// The archive's file name, which names the server, its version and the
    /// platform (`rust-analyzer-2026-04-21-linux-x86_64.tar.gz`,
    /// `clangd-17.0.0-windows-x86_64.zip`).
    pub filename: String,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// The version of the server in the archive (`1.61.0`), or the date it
    /// was packaged when the build tells none.
    #[serde(default)]
    pub version: String,
}

static MIRROR: LazyLock<Mirror> = LazyLock::new(|| {
    serde_json::from_str(include_str!("mirror.json")).expect("mirror.json parses (tested)")
});

/// The mirror's name for the platform this build runs on, which names its
/// manifest and ends its archives' names. A platform the mirror has no
/// packages for gets a name no manifest has (`unknown`).
#[must_use]
pub const fn platform() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "linux-x86_64";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "linux-aarch64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "macos-x86_64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "macos-aarch64";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "windows-x86_64";
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    return "unknown";
}

/// The pinned manifest of `platform`, if the mirror has packages for it.
#[must_use]
pub fn manifest(platform: &str) -> Option<&'static PlatformManifest> {
    MIRROR.platforms.get(platform)
}

/// Where the archives are fetched from, without a trailing slash: the
/// override, else the pinned release, else nowhere.
#[must_use]
pub fn base_url() -> Option<String> {
    base_url_of(
        std::env::var(MIRROR_URL_ENV_VAR).ok(),
        MIRROR.release.as_deref(),
    )
}

/// [`base_url`] of an override and a pinned release. An empty override is
/// none.
fn base_url_of(overridden: Option<String>, release: Option<&str>) -> Option<String> {
    match overridden.filter(|url| !url.trim().is_empty()) {
        Some(url) => Some(url.trim().trim_end_matches('/').to_string()),
        None => release.map(|tag| format!("{RELEASES}/{tag}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::ServerRegistry;

    /// What the packaging scripts write parses: `size_bytes` is not read,
    /// and `version` may be left out.
    #[test]
    fn a_packaging_scripts_manifest_parses() {
        let json = r#"{
          "platform": "linux-x86_64",
          "generated": "2026-06-03T00:00:00Z",
          "servers": {
            "dart-ls": {
              "filename": "dart-ls-3.12.2-linux-x86_64.tar.gz",
              "sha256": "deadbeef",
              "size_bytes": 123,
              "version": "3.12.2"
            },
            "clangd": {
              "filename": "clangd-17.0.0-linux-x86_64.tar.gz",
              "sha256": "feedface"
            }
          }
        }"#;
        let manifest: PlatformManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.platform, "linux-x86_64");
        let dart = &manifest.servers["dart-ls"];
        assert_eq!(dart.filename, "dart-ls-3.12.2-linux-x86_64.tar.gz");
        assert_eq!(dart.sha256, "deadbeef");
        assert_eq!(dart.version, "3.12.2");
        assert!(manifest.servers["clangd"].version.is_empty());
    }

    /// A pinned release has every platform's packages, and no release none:
    /// a platform without them would install every server from its original
    /// source, with nothing to tell the release apart from a complete one.
    #[test]
    fn the_pin_is_one_release_with_every_platform() {
        match MIRROR.release.as_deref() {
            Some(tag) => {
                assert!(
                    tag.starts_with("lsp-servers-") && !tag.contains('/'),
                    "{tag}"
                );
                let mut pinned: Vec<&str> = MIRROR.platforms.keys().map(String::as_str).collect();
                pinned.sort_unstable();
                let mut expected = PLATFORMS.to_vec();
                expected.sort_unstable();
                assert_eq!(pinned, expected);
            }
            None => assert!(MIRROR.platforms.is_empty(), "packages without a release"),
        }
    }

    /// Each pinned manifest holds every server the registry mirrors and
    /// installs on its platform, at the build the registry pins, and nothing
    /// else; its archives are named for the platform and checksummed.
    #[test]
    fn the_pinned_manifests_match_the_registry() {
        let registry = ServerRegistry::load().unwrap();
        for (platform, manifest) in &MIRROR.platforms {
            assert_eq!(&manifest.platform, platform);
            let extension = if platform.starts_with("windows") {
                "zip"
            } else {
                "tar.gz"
            };
            for server in registry.list_servers() {
                let published = &server.source.published_for;
                let packaged =
                    server.is_mirrored() && (published.is_empty() || published.contains(platform));
                let entry = manifest.servers.get(&server.id);
                assert_eq!(
                    entry.is_some(),
                    packaged,
                    "{platform}: {} {}",
                    server.id,
                    if packaged {
                        "is not packaged"
                    } else {
                        "is packaged, but not installed there"
                    }
                );
                let Some(entry) = entry else { continue };
                if let Some(pin) = server.pinned_build() {
                    assert_eq!(entry.version, pin, "{platform}: {}", server.id);
                }
                assert!(
                    entry.filename.starts_with(&format!("{}-", server.id))
                        && entry
                            .filename
                            .ends_with(&format!("-{platform}.{extension}"))
                        && !entry.filename.contains('/'),
                    "{platform}: {}",
                    entry.filename
                );
                assert!(
                    entry.sha256.len() == 64
                        && entry
                            .sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "{platform}: {} {}",
                    server.id,
                    entry.sha256
                );
            }
            for id in manifest.servers.keys() {
                assert!(
                    registry.get_server(id).is_some(),
                    "{platform}: {id} is not in the registry"
                );
            }
        }
    }

    /// The archives come from the release, or from the override, which is
    /// taken without a trailing slash; an empty override is none.
    #[test]
    fn the_archives_come_from_the_release_or_the_override() {
        let tag = Some("lsp-servers-2026-10-30");
        assert_eq!(
            base_url_of(None, tag).as_deref(),
            Some(
                "https://github.com/vladimirpesic/devscriptor/releases/download/lsp-servers-2026-10-30"
            )
        );
        let local = Some("http://127.0.0.1:8000/".to_string());
        assert_eq!(
            base_url_of(local.clone(), tag).as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert_eq!(
            base_url_of(local, None).as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert_eq!(base_url_of(Some(" ".to_string()), None), None);
        assert_eq!(base_url_of(None, None), None);
    }

    /// `scripts/pin-lsp-mirror.py` pins the manifests it is given as a
    /// `mirror.json` this module reads, and refuses one of a platform the
    /// mirror does not package for.
    #[test]
    #[cfg(unix)]
    fn the_pin_script_writes_a_pin_this_module_reads() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest-macos-x86_64.json");
        std::fs::write(
            &manifest,
            r#"{"platform": "macos-x86_64", "generated": "2026-10-30T00:00:00Z",
                "servers": {"gopls": {"filename": "gopls-0.23.0-macos-x86_64.tar.gz",
                "sha256": "ab", "size_bytes": 1, "version": "0.23.0"}}}"#,
        )
        .unwrap();
        let output = dir.path().join("mirror.json");
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/pin-lsp-mirror.py"
        );
        let pin = |manifest: &std::path::Path| {
            std::process::Command::new("python3")
                .arg(script)
                .arg("lsp-servers-2026-10-30")
                .arg(manifest)
                .arg("--output")
                .arg(&output)
                .output()
                .unwrap()
        };
        assert!(pin(&manifest).status.success());
        let mirror: Mirror =
            serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(mirror.release.as_deref(), Some("lsp-servers-2026-10-30"));
        let pinned = &mirror.platforms["macos-x86_64"].servers["gopls"];
        assert_eq!(pinned.filename, "gopls-0.23.0-macos-x86_64.tar.gz");
        assert_eq!(pinned.version, "0.23.0");

        let other = dir.path().join("manifest-freebsd.json");
        std::fs::write(&other, r#"{"platform": "freebsd-x86_64", "servers": {}}"#).unwrap();
        assert!(!pin(&other).status.success());
    }

    /// The platform this build runs on is one the mirror packages for, or
    /// one that no manifest names.
    #[test]
    fn this_platform_is_one_the_mirror_names() {
        let here = platform();
        assert!(here == "unknown" || PLATFORMS.contains(&here), "{here}");
    }
}
