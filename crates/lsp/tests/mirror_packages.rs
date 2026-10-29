//! A platform's mirror packages install the way a user's machine installs
//! them.
//!
//! The LSP mirror workflow (`.github/workflows/lsp-mirror.yml`) pins the
//! packages it has just made in `mirror.json`, serves them from a local web
//! server, points `DEVSCRIPTOR_LSP_MIRROR_URL` at it and runs this test before
//! it publishes anything. Every server in this platform's manifest is
//! installed from the mirror alone, through the downloader's own code: a
//! checksum that does not match, an archive the manifest misnames, or one
//! without the server's `bin_path` or its license files fails the test.
#![cfg(feature = "lsp")]

use devscriptor_lsp::manager::mirror;
use devscriptor_lsp::manager::{ServerRegistry, download_from_mirror};

#[tokio::test]
#[ignore = "needs this platform's packages pinned and served at DEVSCRIPTOR_LSP_MIRROR_URL"]
async fn every_package_installs_from_the_mirror() {
    assert!(
        std::env::var(mirror::MIRROR_URL_ENV_VAR).is_ok_and(|url| !url.trim().is_empty()),
        "{} names no place the packages are served from",
        mirror::MIRROR_URL_ENV_VAR
    );
    let platform = mirror::platform();
    let manifest = mirror::manifest(platform)
        .unwrap_or_else(|| panic!("mirror.json pins no packages for {platform}"));
    let registry = ServerRegistry::load().unwrap();
    let root = tempfile::tempdir().unwrap();

    let mut ids: Vec<&String> = manifest.servers.keys().collect();
    ids.sort();
    let mut failed = Vec::new();
    for id in ids {
        let server = registry
            .get_server(id)
            .unwrap_or_else(|| panic!("{id} is not in the registry"));
        let dir = root.path().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        if let Err(e) = download_from_mirror(server, &dir, None).await {
            failed.push(format!("{id}: {e:#}"));
            continue;
        }
        let unlicensed: Vec<&str> = server
            .license_files
            .iter()
            .filter_map(|file| file.path.as_deref().or(file.url.as_deref()))
            .map(|from| from.rsplit('/').next().unwrap_or(from))
            .filter(|name| !dir.join("LICENSES").join(name).is_file())
            .collect();
        if unlicensed.is_empty() {
            println!("{id}: installed {}", manifest.servers[id].filename);
        } else {
            failed.push(format!(
                "{id}: no LICENSES/{}",
                unlicensed.join(", LICENSES/")
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} packages did not install:\n{}",
        failed.len(),
        manifest.servers.len(),
        failed.join("\n")
    );
}
