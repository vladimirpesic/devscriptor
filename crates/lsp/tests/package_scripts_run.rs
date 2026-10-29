//! The Linux packagers, run on a set of servers as the downloaders leave them.
//!
//! Each script runs from a copy in a temporary directory, on an `lsp-servers/`
//! of placeholder servers, and is pointed at the registry with
//! `DEVSCRIPTOR_REGISTRY_JSON`, at a copy of the registry whose license files
//! are the placeholders' own. What it writes is read back: the manifest the
//! mirror pins, the archives, and the check it makes against the registry. Run on
//! Linux only: the Linux packagers use GNU tools (`stat -c`, `sha256sum`),
//! which macOS does not have.
#![cfg(all(target_os = "linux", feature = "lsp"))]

use std::path::{Path, PathBuf};
use std::process::Command;

use devscriptor_lsp::manager::registry::{ServerRegistry, SourceType};

const LINUX_PACKAGERS: &[(&str, &str)] = &[
    (
        "scripts/linux-x86_64/package-lsp-servers.sh",
        "linux-x86_64",
    ),
    (
        "scripts/linux-aarch64/package-lsp-servers.sh",
        "linux-aarch64",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

/// The registry's servers that are packaged: all but those that come with
/// their toolchain.
fn packaged_ids() -> Vec<String> {
    let registry = ServerRegistry::load().unwrap();
    let mut ids: Vec<String> = registry
        .servers
        .values()
        .filter(|s| s.source.source_type != SourceType::System)
        .map(|s| s.id.clone())
        .collect();
    ids.sort();
    ids
}

/// What `script` printed and the manifest it wrote, run from a copy in
/// `root` on one placeholder server per id in `ids`, each with a `LICENSE`
/// the registry names.
fn run_packager(
    root: &Path,
    script: &str,
    platform: &str,
    ids: &[String],
) -> (String, serde_json::Value) {
    run_packager_with(root, script, platform, ids, &|_| local_license(), None)
}

/// A license file named as one the server ships: its `LICENSE`.
fn local_license() -> serde_json::Value {
    serde_json::json!([{ "path": "LICENSE" }])
}

/// [`run_packager`], with the registry naming `license_files(id)` as each
/// server's license files, and `tools` ahead of `PATH`.
fn run_packager_with(
    root: &Path,
    script: &str,
    platform: &str,
    ids: &[String],
    license_files: &dyn Fn(&str) -> serde_json::Value,
    tools: Option<&Path>,
) -> (String, serde_json::Value) {
    let copy = root.join(script);
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(repo_root().join(script), &copy).unwrap();
    let lsp_dir = copy.parent().unwrap().join("lsp-servers");
    for id in ids {
        let dir = lsp_dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".version"), "1.0.0\n").unwrap();
    }
    for entry in std::fs::read_dir(&lsp_dir).unwrap() {
        let dir = entry.unwrap().path();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        if dir.is_dir() && !id.ends_with(".new") {
            std::fs::write(dir.join("LICENSE"), format!("license of {id}\n")).unwrap();
        }
    }

    let mut registry: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("crates/lsp/src/manager/registry.json")).unwrap(),
    )
    .unwrap();
    for (id, server) in registry["servers"].as_object_mut().unwrap() {
        server["license_files"] = license_files(id);
    }
    let registry_json = root.join("registry.json");
    std::fs::write(&registry_json, registry.to_string()).unwrap();

    let mut command = Command::new("bash");
    command
        .arg(&copy)
        .env("DEVSCRIPTOR_REGISTRY_JSON", &registry_json)
        .env("TMPDIR", root);
    if let Some(tools) = tools {
        let path = format!(
            "{}:{}",
            tools.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        command.env("PATH", path);
    }
    let output = command.output().unwrap();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{script}: {}\n{printed}",
        output.status
    );
    let manifest = copy
        .parent()
        .unwrap()
        .join("lsp-packages")
        .join(format!("manifest-{platform}.json"));
    let manifest = serde_json::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
    (printed, manifest)
}

/// The file `member` of the archive the manifest names for `id`.
fn archived(
    root: &Path,
    script: &str,
    manifest: &serde_json::Value,
    id: &str,
    member: &str,
) -> Option<String> {
    let archive = root
        .join(script)
        .parent()
        .unwrap()
        .join("lsp-packages")
        .join(manifest["servers"][id]["filename"].as_str()?);
    let output = Command::new("tar")
        .arg("-xOzf")
        .arg(archive)
        .arg(format!("{id}/{member}"))
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Every server the downloaders package is in the manifest under its registry
/// id, and the check against the registry finds none missing: sourcekit-lsp,
/// which comes with the Swift toolchain, is not expected there.
#[test]
fn a_packaged_set_matches_the_registry() {
    let ids = packaged_ids();
    for (script, platform) in LINUX_PACKAGERS {
        let root = tempfile::tempdir().unwrap();
        let (stdout, manifest) = run_packager(root.path(), script, platform, &ids);

        let mut listed: Vec<String> = manifest["servers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        listed.sort();
        assert_eq!(listed, ids, "{script}");
        assert_eq!(manifest["platform"], *platform);
        assert!(
            stdout.contains(&format!(
                "Manifest IDs match registry exactly ({} servers)",
                ids.len()
            )),
            "{script}:\n{stdout}"
        );
    }
}

/// A run packages what `lsp-servers/` holds now: the archives an earlier run
/// left in `lsp-packages/` are gone, rather than checksummed and uploaded
/// with this run's, and a server whose download recorded no version is named
/// by the day it is packaged.
#[test]
fn a_run_leaves_nothing_of_an_earlier_one() {
    let ids = packaged_ids();
    for (script, platform) in LINUX_PACKAGERS {
        let root = tempfile::tempdir().unwrap();
        let packages = root
            .path()
            .join(script)
            .parent()
            .unwrap()
            .join("lsp-packages");
        std::fs::create_dir_all(&packages).unwrap();
        let stale = packages.join(format!("volar-2026-07-10-{platform}.tar.gz"));
        std::fs::write(&stale, "old").unwrap();
        let unversioned = root
            .path()
            .join(script)
            .parent()
            .unwrap()
            .join("lsp-servers/phpactor");
        std::fs::create_dir_all(&unversioned).unwrap();
        let others: Vec<String> = ids.iter().filter(|id| *id != "phpactor").cloned().collect();

        let (_, manifest) = run_packager(root.path(), script, platform, &others);
        assert!(!stale.exists(), "{script} kept an earlier run's archive");
        let sums =
            std::fs::read_to_string(packages.join(format!("sha256sums-{platform}.txt"))).unwrap();
        assert_eq!(sums.lines().count(), ids.len(), "{script}:\n{sums}");
        assert!(!sums.contains("volar-2026-07-10"), "{script}:\n{sums}");
        let phpactor = manifest["servers"]["phpactor"]["version"].as_str().unwrap();
        assert_eq!(phpactor.len(), "2026-10-26".len(), "{phpactor}");
        assert_eq!(manifest["servers"]["volar"]["version"], "1.0.0");
    }
}

/// macOS's tar adds a `._<name>` AppleDouble file per file with extended
/// attributes, and the attributes as headers other tars warn about; a package
/// holds neither. The Windows packager, like the Unix ones, starts from a
/// directory without an earlier run's archives.
#[test]
fn packages_hold_only_the_servers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for platform in ["macos-x86_64", "macos-aarch64"] {
        let macos = std::fs::read_to_string(
            root.join(format!("scripts/{platform}/package-lsp-servers.sh")),
        )
        .unwrap();
        assert!(macos.contains("COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf"));
    }
    let windows =
        std::fs::read_to_string(root.join("scripts/windows-x86_64/package-lsp-servers.ps1"))
            .unwrap();
    assert!(windows.contains("Remove-Item -Path (Join-Path $OutputDir \"*-$Platform.zip\")"));
    assert!(windows.contains("foreach ($id in $script:Packaged.Keys) {"));
}

/// An install a download left unfinished (`<id>.new`) is not packaged.
#[test]
fn an_unfinished_install_is_not_packaged() {
    let ids = packaged_ids();
    for (script, platform) in LINUX_PACKAGERS {
        let root = tempfile::tempdir().unwrap();
        let unfinished = root
            .path()
            .join(script)
            .parent()
            .unwrap()
            .join("lsp-servers/volar.new");
        std::fs::create_dir_all(&unfinished).unwrap();
        let (_, manifest) = run_packager(root.path(), script, platform, &ids);
        assert!(manifest["servers"].get("volar.new").is_none(), "{script}");
    }
}

/// A server whose download records no version is named by the version it
/// reports: rust-analyzer's `--version` is `rust-analyzer 0.3.2963-standalone`,
/// with no date in it.
#[test]
fn rust_analyzer_is_named_by_the_version_it_reports() {
    use std::os::unix::fs::PermissionsExt;
    let ids = packaged_ids();
    for (script, platform) in LINUX_PACKAGERS {
        let root = tempfile::tempdir().unwrap();
        let bin = root
            .path()
            .join(script)
            .parent()
            .unwrap()
            .join("lsp-servers/rust-analyzer/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let binary = bin.join("rust-analyzer");
        std::fs::write(
            &binary,
            "#!/bin/sh\necho 'rust-analyzer 0.3.2963-standalone (8d5e2af 2026-10-19)'\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let others: Vec<String> = ids
            .iter()
            .filter(|id| *id != "rust-analyzer")
            .cloned()
            .collect();
        let (_, manifest) = run_packager(root.path(), script, platform, &others);
        assert_eq!(
            manifest["servers"]["rust-analyzer"]["version"], "0.3.2963",
            "{script}"
        );
    }
}

/// Each archive holds its server's license files under `LICENSES/`, copied
/// from the server's own files where the registry names one of them.
#[test]
fn a_package_holds_its_license_files() {
    let ids = packaged_ids();
    for (script, platform) in LINUX_PACKAGERS {
        let root = tempfile::tempdir().unwrap();
        let (_, manifest) = run_packager(root.path(), script, platform, &ids);
        for id in &ids {
            assert_eq!(
                archived(root.path(), script, &manifest, id, "LICENSES/LICENSE").as_deref(),
                Some(format!("license of {id}\n").as_str()),
                "{script}: {id}"
            );
        }
    }
}

/// A license file the server does not ship is fetched from the URL the
/// registry names, and packaged only when it is the file the registry pins:
/// one of other bytes, or one the server should ship and does not, keeps its
/// server out of the packages, and the others go on.
#[test]
fn a_license_file_is_packaged_only_as_the_registry_names_it() {
    use std::os::unix::fs::PermissionsExt;
    let (script, platform) = LINUX_PACKAGERS[0];
    let ids = packaged_ids();
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    let curl = tools.join("curl");
    std::fs::write(
        &curl,
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=\"$2\"; shift; done\n\
         printf 'fetched license\\n' > \"$out\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fetched = |sha256: &'static str| {
        move |id: &str| match id {
            "volar" => serde_json::json!([{
                "url": "https://raw.githubusercontent.com/vuejs/language-tools/0000000000000000000000000000000000000000/LICENSE-VOLAR",
                "sha256": sha256,
            }]),
            "pyright" => serde_json::json!([{ "path": "node_modules/pyright/LICENSE.txt" }]),
            "gopls" => serde_json::json!([]),
            _ => local_license(),
        }
    };

    // sha256 of "fetched license\n".
    let pinned = "b2dafa7e73687fa81a35649d2200c847e857fb6046831b35cf3d439fed30e0b0";
    let (printed, manifest) = run_packager_with(
        root.path(),
        script,
        platform,
        &ids,
        &fetched(pinned),
        Some(&tools),
    );
    assert_eq!(
        archived(
            root.path(),
            script,
            &manifest,
            "volar",
            "LICENSES/LICENSE-VOLAR"
        )
        .as_deref(),
        Some("fetched license\n"),
        "{printed}"
    );
    for (id, why) in [
        ("pyright", "no node_modules/pyright/LICENSE.txt"),
        ("gopls", "registry.json names no license file"),
    ] {
        assert!(manifest["servers"].get(id).is_none(), "{id}:\n{printed}");
        assert!(printed.contains(&format!("{id}: {why}")), "{printed}");
    }
    assert!(manifest["servers"].get("rust-analyzer").is_some());

    let root = tempfile::tempdir().unwrap();
    let other = "0000000000000000000000000000000000000000000000000000000000000000";
    let (printed, manifest) = run_packager_with(
        root.path(),
        script,
        platform,
        &ids,
        &fetched(other),
        Some(&tools),
    );
    assert!(manifest["servers"].get("volar").is_none(), "{printed}");
    assert!(
        printed.contains("is not the file the registry pins"),
        "{printed}"
    );
    let licenses = root
        .path()
        .join(script)
        .parent()
        .unwrap()
        .join("lsp-servers/volar/LICENSES");
    assert!(!licenses.join("LICENSE-VOLAR").exists());
}
