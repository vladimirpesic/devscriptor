//! The Unix downloaders, run where nothing they fetch servers with works.
//!
//! A downloader handles each server in turn and ends with a summary of what it
//! fetched and what it could not. Under `set -e` a lookup that comes back
//! empty would end the whole run instead — no coursier, a GitHub API that
//! answers 403, a checksum that cannot be fetched — and every server after it
//! would go unreported, and the packaging step after it ship without them.
//!
//! Each script runs from a copy in a temporary directory, so what it downloads
//! lands there, with stand-ins for the tools it fetches with: unless a test
//! says otherwise, `curl` saves a placeholder for an archive and fails for
//! everything else (release lookups, published checksums), and `cs` and
//! `unzip` fail. Every other tool a server comes from is absent.
#![cfg(all(unix, feature = "lsp"))]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use devscriptor_lsp::manager::registry::{ServerInfo, ServerRegistry};

/// Each Unix downloader with the platform it packages for.
const UNIX_DOWNLOADERS: &[(&str, &str)] = &[
    (
        "scripts/linux-x86_64/download-lsp-servers.sh",
        "linux-x86_64",
    ),
    (
        "scripts/linux-aarch64/download-lsp-servers.sh",
        "linux-aarch64",
    ),
    (
        "scripts/macos-x86_64/download-lsp-servers.sh",
        "macos-x86_64",
    ),
    (
        "scripts/macos-aarch64/download-lsp-servers.sh",
        "macos-aarch64",
    ),
];

/// Whether `script` runs on this machine. Every downloader needs bash 4:
/// started by macOS's own bash, 3.2, the macOS one runs again under
/// Homebrew's (`brew install bash`, which
/// `scripts/macos-*/install-dependencies.sh` does), and the Linux ones,
/// written for Linux, do not, so they run on Linux only.
fn runs_here(script: &str) -> bool {
    cfg!(target_os = "linux") || !script.starts_with("scripts/linux-")
}

/// The downloaders this machine runs, with the platform each packages for.
fn downloaders_here() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    UNIX_DOWNLOADERS
        .iter()
        .filter(|(script, _)| runs_here(script))
}

/// What the scripts' own plumbing runs, linked from this machine.
const PLUMBING: &[&str] = &[
    "awk", "basename", "cat", "chmod", "cp", "cut", "dirname", "du", "find", "grep", "head", "ls",
    "mkdir", "mktemp", "mv", "rm", "sed", "tail", "tr",
];

/// `curl` as a host offline from everything but the archives themselves.
const CURL: &str = r#"#!/bin/sh
dest=""
for arg in "$@"; do
    case "$arg" in
        *api.github.com*|*.sha256|*.sha256sum) exit 22 ;;
    esac
done
while [ $# -gt 0 ]; do
    if [ "$1" = "-o" ]; then dest=$2; shift; fi
    shift
done
[ -n "$dest" ] || exit 22
echo placeholder > "$dest"
"#;

/// `curl` as GitHub would answer: each URL whose file `CURL_FILES` holds (the
/// URL with every character but letters and digits made `_`) is that file, to
/// `-o` or to stdout; any other fails.
const MAPPED_CURL: &str = r#"#!/bin/sh
dest=""
url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) dest=$2; shift ;;
        http*) url=$1 ;;
    esac
    shift
done
key=$(printf '%s' "$url" | tr -c 'A-Za-z0-9' '_')
[ -f "$CURL_FILES/$key" ] || exit 22
if [ -n "$dest" ]; then cp "$CURL_FILES/$key" "$dest"; else cat "$CURL_FILES/$key"; fi
"#;

const FAILING: &str = "#!/bin/sh\nexit 1\n";

/// `curl` that records, to `$RECORD`, the configuration a request of GitHub's
/// API reads from its standard input (`-K -`), and fails every request.
const TOKEN_CURL: &str = r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        *api.github.com*)
            for option in "$@"; do
                [ "$option" = -K ] && printf 'api: %s\n' "$(cat)" >> "$RECORD" && exit 22
            done
            echo 'api: no token' >> "$RECORD"
            exit 22 ;;
    esac
done
exit 22
"#;

/// `npm` that records, to `$RECORD`, the token its environment holds.
const RECORDING_NPM: &str = r#"#!/bin/sh
printf 'npm: %s\n' "${GITHUB_TOKEN:-no token}" >> "$RECORD"
exit 1
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

/// `name` on this process's `PATH`.
fn host_tool(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("`{name}` is not on PATH"))
}

fn write_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The `✗ <server>` lines of a summary's failed list.
fn failed_servers(stdout: &str) -> Vec<String> {
    stdout
        .split("Failed/Skipped:")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().strip_prefix("✗ "))
        .filter_map(|rest| rest.split(" - ").next())
        .map(str::to_string)
        .collect()
}

/// The tools of a run offline: `curl` saves archives and fails the rest, and
/// `cs` and `unzip` fail.
fn offline(tools: &Path) {
    write_tool(tools, "curl", CURL);
    for name in ["cs", "unzip"] {
        write_tool(tools, name, FAILING);
    }
}

/// The server ids the downloader for `platform` packages: all but those that
/// come with their toolchain or whose builds expire, and those whose release
/// has no build for it.
fn packaged_for(platform: &str) -> Vec<String> {
    let registry = ServerRegistry::load().unwrap();
    let mut ids: Vec<String> = registry
        .servers
        .values()
        .filter(|s: &&ServerInfo| s.is_mirrored())
        .filter(|s| {
            s.source.published_for.is_empty()
                || s.source.published_for.iter().any(|p| p == platform)
        })
        .map(|s| s.id.clone())
        .collect();
    ids.sort();
    ids
}

/// What `script` printed, run from a copy in `root` with the tools `tools`
/// puts in its tools directory (beside the plumbing) and `env`, after
/// `prepare` has had its `lsp-servers/` directory, and that directory.
fn run_downloader(
    root: &Path,
    script: &str,
    tools_for: impl Fn(&Path),
    env: &[(&str, &Path)],
    prepare: impl Fn(&Path),
) -> (String, PathBuf) {
    let tools = root.join("tools");
    std::fs::create_dir(&tools).unwrap();
    for name in PLUMBING {
        std::os::unix::fs::symlink(host_tool(name), tools.join(name)).unwrap();
    }
    // Homebrew's prefix, which the macOS downloader asks `brew` for: an empty
    // one. Without `brew` it takes /opt/homebrew, where an Apple Silicon Mac
    // keeps Homebrew, and puts its tools on the script's PATH, beside the
    // ones a test gives it.
    write_tool(
        &tools,
        "brew",
        &format!("#!/bin/sh\necho '{}'\n", root.join("homebrew").display()),
    );
    tools_for(&tools);
    let copy = root.join(script);
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(repo_root().join(script), &copy).unwrap();
    let lsp_dir = copy.parent().unwrap().join("lsp-servers");
    std::fs::create_dir(&lsp_dir).unwrap();
    prepare(&lsp_dir);
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();

    let output = Command::new(host_tool("bash"))
        .arg(&copy)
        .env_clear()
        .env("PATH", &tools)
        .env("HOME", &home)
        .env("TMPDIR", root)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Download Summary"),
        "{script} stopped before its summary ({})\n--- stdout tail\n{}\n--- stderr\n{stderr}",
        output.status,
        stdout
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    );
    (stdout, lsp_dir)
}

#[test]
fn a_downloader_reports_every_server_it_could_not_fetch() {
    for (script, platform) in downloaders_here() {
        let root = tempfile::tempdir().unwrap();
        let (stdout, _) = run_downloader(root.path(), script, offline, &[], |_| {});
        let failed = failed_servers(&stdout);
        let unreported: Vec<String> = packaged_for(platform)
            .into_iter()
            .filter(|id| !failed.contains(id))
            .collect();
        assert!(
            unreported.is_empty(),
            "{script} fetched nothing, yet does not report as failed: {unreported:?}\n{stdout}"
        );
    }
}

/// sourcekit-lsp comes with the Swift toolchain and runs only inside it, so
/// no downloader packages one, and an earlier download's copy is removed:
/// swiftly's `bin/sourcekit-lsp` is swiftly itself, which runs nowhere swiftly
/// is not set up.
#[test]
fn no_downloader_packages_a_server_of_the_toolchain() {
    for (script, _) in downloaders_here() {
        let root = tempfile::tempdir().unwrap();
        let (stdout, lsp_dir) = run_downloader(root.path(), script, offline, &[], |lsp_dir| {
            let bin = lsp_dir.join("sourcekit-lsp").join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("sourcekit-lsp"), "swiftly").unwrap();
        });
        assert!(
            !lsp_dir.join("sourcekit-lsp").exists(),
            "{script} kept an earlier download's sourcekit-lsp"
        );
        assert!(
            stdout.contains("[sourcekit-lsp] Not packaged: it comes with the Swift toolchain"),
            "{script}:\n{stdout}"
        );
    }
}

/// A clangd copied from PATH: swiftly's proxy on Linux, which runs nowhere
/// swiftly is not set up.
fn swiftly_copy(lsp_dir: &Path) {
    let bin = lsp_dir.join("clangd").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("clangd"), "swiftly").unwrap();
    std::fs::create_dir_all(lsp_dir.join("clangd/resource-dir/include")).unwrap();
}

/// A clangd release zip as clangd/clangd publishes it: `clangd_<version>/`
/// holding `bin/clangd` and `lib/clang/<major>/include`.
fn clangd_release(version: &str) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let root = format!("clangd_{version}");
    let exe = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
    zip.start_file(format!("{root}/bin/clangd"), exe).unwrap();
    zip.write_all(b"#!/bin/sh\necho clangd version 23.1.0\n")
        .unwrap();
    let file = zip::write::SimpleFileOptions::default();
    zip.start_file(format!("{root}/lib/clang/23/include/stddef.h"), file)
        .unwrap();
    zip.write_all(b"typedef unsigned long size_t;\n").unwrap();
    zip.finish().unwrap().into_inner()
}

/// The `CURL_FILES` key of `url`.
fn curl_key(url: &str) -> String {
    url.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// clangd comes from its release, checked against the SHA-256 GitHub
/// publishes for the asset, with the builtin headers it needs as
/// `resource-dir/include`, and replaces a copy an earlier download took from
/// PATH. One whose checksum does not match is not installed.
#[test]
fn clangd_comes_from_its_release_with_its_headers() {
    use sha2::Digest;
    for (script, asset) in [
        (
            "scripts/linux-x86_64/download-lsp-servers.sh",
            "clangd-linux-23.1.0.zip",
        ),
        (
            "scripts/macos-x86_64/download-lsp-servers.sh",
            "clangd-mac-23.1.0.zip",
        ),
        (
            "scripts/macos-aarch64/download-lsp-servers.sh",
            "clangd-mac-23.1.0.zip",
        ),
    ]
    .into_iter()
    .filter(|(script, _)| runs_here(script))
    {
        for published in ["right", "wrong"] {
            let root = tempfile::tempdir().unwrap();
            let files = root.path().join("curl-files");
            std::fs::create_dir(&files).unwrap();
            let zip = clangd_release("23.1.0");
            let url = format!("https://github.com/clangd/clangd/releases/download/23.1.0/{asset}");
            std::fs::write(files.join(curl_key(&url)), &zip).unwrap();
            let digest = if published == "right" {
                hex::encode(sha2::Sha256::digest(&zip))
            } else {
                "0".repeat(64)
            };
            let release = serde_json::json!({"assets": [
                {"name": format!("clangd_indexing_tools-{}", &asset[7..]), "browser_download_url": "https://example.invalid/tools.zip", "digest": "sha256:00"},
                {"name": asset, "browser_download_url": url, "digest": format!("sha256:{digest}")},
            ]});
            std::fs::write(
                files.join(curl_key(
                    "https://api.github.com/repos/clangd/clangd/releases/latest",
                )),
                release.to_string(),
            )
            .unwrap();

            let (stdout, lsp_dir) = run_downloader(
                root.path(),
                script,
                |tools| {
                    write_tool(tools, "curl", MAPPED_CURL);
                    write_tool(tools, "cs", FAILING);
                    for name in ["jq", "unzip", "sha256sum"] {
                        std::os::unix::fs::symlink(host_tool(name), tools.join(name)).unwrap();
                    }
                },
                &[("CURL_FILES", &files)],
                swiftly_copy,
            );
            let clangd = lsp_dir.join("clangd");
            if published == "right" {
                assert_eq!(
                    std::fs::read_to_string(clangd.join("bin/clangd")).unwrap(),
                    "#!/bin/sh\necho clangd version 23.1.0\n",
                    "{script}:\n{stdout}"
                );
                assert!(
                    clangd.join("resource-dir/include/stddef.h").is_file(),
                    "{script}"
                );
                assert_eq!(
                    std::fs::read_to_string(clangd.join(".version")).unwrap(),
                    "23.1.0\n"
                );
            } else {
                assert!(
                    failed_servers(&stdout).contains(&"clangd".to_string()),
                    "{script}:\n{stdout}"
                );
                assert_eq!(
                    std::fs::read_to_string(clangd.join("bin/clangd")).unwrap(),
                    "swiftly",
                    "{script}: a clangd that does not match its checksum was installed"
                );
            }
        }
    }
}

/// clangd's release has no Linux ARM build, so the Linux ARM downloader
/// packages none, and removes an earlier download's copy from PATH.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "the Linux downloaders run on Linux: they need bash 4, and do not look for Homebrew's"
)]
fn a_downloader_packages_no_build_its_platform_lacks() {
    let script = "scripts/linux-aarch64/download-lsp-servers.sh";
    assert!(!packaged_for("linux-aarch64").contains(&"clangd".to_string()));
    let root = tempfile::tempdir().unwrap();
    let (stdout, lsp_dir) = run_downloader(root.path(), script, offline, &[], swiftly_copy);
    assert!(!lsp_dir.join("clangd").exists(), "{script} kept clangd");
    assert!(
        stdout.contains("[clangd] Not packaged: clangd publishes no Linux ARM build"),
        "{stdout}"
    );
}

/// `npm` that logs what it is asked to install to `$NPM_LOG` and installs a
/// placeholder of it.
const NPM: &str = r#"#!/bin/sh
echo "$*" >> "$NPM_LOG"
mkdir -p node_modules/.bin
echo placeholder > node_modules/.bin/installed
"#;

/// A server is installed at the build the registry pins, with its
/// companions; an install of another version is replaced, and one of the
/// pinned version is kept as it is.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "the Linux downloaders run on Linux: they need bash 4, and do not look for Homebrew's"
)]
fn an_install_of_another_version_is_replaced() {
    let script = "scripts/linux-x86_64/download-lsp-servers.sh";
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("npm.log");
    let (stdout, lsp_dir) = run_downloader(
        root.path(),
        script,
        |tools| {
            offline(tools);
            write_tool(tools, "npm", NPM);
        },
        &[("NPM_LOG", &log)],
        |lsp_dir| {
            for (id, version) in [("volar", "1.6.5"), ("pyright", "1.1.411")] {
                let dir = lsp_dir.join(id);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(".version"), format!("{version}\n")).unwrap();
                std::fs::write(dir.join("earlier"), "").unwrap();
            }
        },
    );
    let log = std::fs::read_to_string(&log).unwrap();
    assert!(
        log.lines()
            .any(|l| l == "install --omit=dev @vue/language-server@2.2.12 typescript@5.9.3"),
        "{log}"
    );
    assert!(
        !lsp_dir.join("volar/earlier").exists(),
        "volar 1.6.5 was kept"
    );
    assert_eq!(
        std::fs::read_to_string(lsp_dir.join("volar/.version")).unwrap(),
        "2.2.12\n"
    );
    assert!(lsp_dir.join("pyright/earlier").exists(), "{stdout}");
    assert!(!log.contains("pyright@"), "{log}");
}

/// `ruby` whose extension API version is 3.3.0.
const RUBY: &str = "#!/bin/sh\necho 3.3.0 | tr -d '\\n'\n";

/// `gem` that logs that it ran to `$GEM_LOG` and installs placeholder
/// binstubs into its `--bindir`.
const GEM: &str = r#"#!/bin/sh
echo "$*" >> "$GEM_LOG"
for arg in "$@"; do
    case "$arg" in
        --bindir=*) bindir=${arg#--bindir=} ;;
    esac
done
mkdir -p "$bindir"
for stub in ruby-lsp ruby-lsp-launcher; do
    printf '#!/bin/sh\n' > "$bindir/$stub"
    chmod +x "$bindir/$stub"
done
"#;

/// A ruby-lsp bundle of the pinned version is kept only when its native
/// extensions are this Ruby's: one built with Ruby 3.2, which Ruby 3.3 cannot
/// load, is installed again with this one.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "the Linux downloaders run on Linux: they need bash 4, and do not look for Homebrew's"
)]
fn a_bundle_built_for_another_ruby_is_replaced() {
    let script = "scripts/linux-x86_64/download-lsp-servers.sh";
    for (built_for, replaced) in [("3.2.0", true), ("3.3.0", false)] {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("gem.log");
        std::fs::write(&log, "").unwrap();
        let (stdout, lsp_dir) = run_downloader(
            root.path(),
            script,
            |tools| {
                offline(tools);
                write_tool(tools, "ruby", RUBY);
                write_tool(tools, "gem", GEM);
            },
            &[("GEM_LOG", &log)],
            |lsp_dir| {
                let bundle = lsp_dir.join("ruby-lsp");
                std::fs::create_dir_all(bundle.join("bin")).unwrap();
                std::fs::write(bundle.join("bin/ruby-lsp-launcher"), "").unwrap();
                std::fs::write(bundle.join(".version"), "0.26.11\n").unwrap();
                std::fs::create_dir_all(
                    bundle.join(format!("extensions/x86_64-linux/{built_for}/prism-1.9.0")),
                )
                .unwrap();
            },
        );
        let installed = !std::fs::read_to_string(&log).unwrap().is_empty();
        assert_eq!(installed, replaced, "built for {built_for}:\n{stdout}");
        assert_eq!(
            lsp_dir
                .join(format!("ruby-lsp/extensions/x86_64-linux/{built_for}"))
                .exists(),
            !replaced,
            "built for {built_for}"
        );
    }
}

/// An install that fails leaves the one it was to replace as it is: the
/// install is made beside it and only then put in its place, so on a machine
/// without `dotnet` the .NET servers' installs stay.
#[test]
fn a_failed_install_leaves_the_earlier_one() {
    for (script, _) in downloaders_here() {
        let root = tempfile::tempdir().unwrap();
        let (stdout, lsp_dir) = run_downloader(root.path(), script, offline, &[], |lsp_dir| {
            for id in ["csharp-ls", "volar", "metals", "ruby-lsp"] {
                let bin = lsp_dir.join(id).join("bin");
                std::fs::create_dir_all(&bin).unwrap();
                std::fs::write(bin.join(id), "earlier").unwrap();
            }
        });
        for id in ["csharp-ls", "volar", "metals", "ruby-lsp"] {
            assert!(
                lsp_dir.join(id).join("bin").join(id).exists(),
                "{script}: the failed install of {id} deleted the earlier one\n{stdout}"
            );
            assert!(
                !lsp_dir.join(format!("{id}.new")).exists(),
                "{script}: {id}.new left"
            );
        }
    }
}

/// GitHub's API is asked with the token the environment gives, which reaches
/// `curl` on its standard input, not its command line, and no installer the
/// script runs sees it: npm runs the packages' install scripts.
#[test]
fn the_api_is_asked_with_a_token_no_installer_sees() {
    for (script, _) in downloaders_here() {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("record");
        run_downloader(
            root.path(),
            script,
            |tools| {
                write_tool(tools, "curl", TOKEN_CURL);
                write_tool(tools, "npm", RECORDING_NPM);
            },
            &[
                ("RECORD", &record),
                ("GITHUB_TOKEN", Path::new("the-workflows-token")),
            ],
            |_| {},
        );
        let record = std::fs::read_to_string(&record).unwrap();
        assert!(
            record.contains("api: header = \"Authorization: Bearer the-workflows-token\""),
            "{script}:\n{record}"
        );
        assert!(!record.contains("api: no token"), "{script}:\n{record}");
        assert!(record.contains("npm: no token"), "{script}:\n{record}");
        assert!(
            !record.contains("npm: the-workflows-token"),
            "{script}:\n{record}"
        );
    }
}
