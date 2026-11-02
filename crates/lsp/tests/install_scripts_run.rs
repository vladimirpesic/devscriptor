//! The dependency installers, run against stand-in package managers.
//!
//! The Linux installers need root and install system packages, so they are not
//! run whole: the function that decides what to install is taken from each
//! script and run on its own, against a fake `dpkg`. The macOS installers run
//! whole, with a fake `brew` that records what it is asked to install.
#![cfg(all(unix, feature = "lsp"))]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const LINUX_INSTALLERS: &[&str] = &[
    "scripts/linux-x86_64/install-dependencies.sh",
    "scripts/linux-aarch64/install-dependencies.sh",
];

const MACOS_INSTALLERS: &[&str] = &[
    "scripts/macos-x86_64/install-dependencies.sh",
    "scripts/macos-aarch64/install-dependencies.sh",
];

/// `dpkg -l` on a machine where only `curl` is installed.
const DPKG: &str = r#"#!/bin/sh
[ "$1" = "-l" ] || exit 2
if [ "$2" = "curl" ]; then
    echo "ii  curl  8.5.0  amd64  command line tool for transferring data with URL syntax"
    exit 0
fi
echo "dpkg-query: no packages found matching $2" >&2
exit 1
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

/// The definition of the shell function `name` in `script`.
fn function(script: &str, name: &str) -> String {
    let body = std::fs::read_to_string(repo_root().join(script)).unwrap();
    let start = body
        .find(&format!("\n{name}() {{"))
        .unwrap_or_else(|| panic!("{script} defines no {name}"));
    let end = start + body[start..].find("\n}\n").expect("the function's end") + 3;
    body[start..end].to_string()
}

/// A package that is not installed is installed: the check must not take every
/// package for installed, or the basic tools (`unzip`, `jq`, `build-essential`,
/// …) would never reach a machine without them.
#[test]
fn the_installers_install_what_is_missing() {
    let fake = tempfile::tempdir().unwrap();
    let dpkg = fake.path().join("dpkg");
    std::fs::write(&dpkg, DPKG).unwrap();
    std::fs::set_permissions(&dpkg, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        fake.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for script in LINUX_INSTALLERS {
        let check = function(script, "apt_package_installed");
        let installed = |package: &str| {
            Command::new("bash")
                .arg("-c")
                .arg(format!(
                    "set -e\nset -o pipefail\n{check}\napt_package_installed {package}"
                ))
                .env("PATH", &path)
                .status()
                .unwrap()
                .success()
        };
        assert!(installed("curl"), "{script}: curl is installed");
        assert!(!installed("jq"), "{script}: jq is not installed");
    }
}

/// The packagers read their manifests with `jq`, so the installers install it.
#[test]
fn the_installers_install_jq() {
    for script in LINUX_INSTALLERS {
        let body = std::fs::read_to_string(repo_root().join(script)).unwrap();
        let tools = body
            .lines()
            .find_map(|line| line.strip_prefix("BASIC_TOOLS=\""))
            .unwrap_or_else(|| panic!("{script} lists no BASIC_TOOLS"));
        assert!(
            tools
                .trim_end_matches('"')
                .split_whitespace()
                .any(|t| t == "jq"),
            "{script}: BASIC_TOOLS={tools}"
        );
    }
}

/// What `script` asks Homebrew to install on a Mac where every tool it checks
/// for is in place: with macOS's own bash (3.2) and Ruby (2.6) on PATH, or
/// newer ones if `modern_on_path`, and Homebrew's bash and Ruby in its
/// prefix if `homebrews`.
fn macos_installs(script: &str, modern_on_path: bool, homebrews: bool) -> String {
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("tools");
    let prefix = root.path().join("homebrew");
    std::fs::create_dir_all(&tools).unwrap();
    let log = root.path().join("brew.log");
    std::fs::write(&log, "").unwrap();
    let tool = |path: &Path, body: String| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    tool(
        &tools.join("brew"),
        format!(
            "case \"$1\" in\n--prefix) echo '{}' ;;\n--version) echo 'Homebrew 7.0.9' ;;\n*) echo \"$*\" >> '{}' ;;\nesac\n",
            prefix.display(),
            log.display()
        ),
    );
    for name in [
        "wget",
        "jq",
        "xz",
        "unzip",
        "java",
        "python3",
        "go",
        "php",
        "node",
        "cs",
        "dotnet",
        "xcode-select",
    ] {
        tool(&tools.join(name), "echo stand-in\n".to_string());
    }
    // `bash -c` and `ruby -e` are the installers' version checks.
    let version_check = if modern_on_path { 0 } else { 1 };
    tool(
        &tools.join("bash"),
        format!("[ \"$1\" = -c ] && exit {version_check}\necho stand-in\n"),
    );
    tool(
        &tools.join("ruby"),
        format!("[ \"$1\" = -e ] && exit {version_check}\necho stand-in\n"),
    );
    if homebrews {
        tool(&prefix.join("bin/bash"), String::new());
        tool(&prefix.join("opt/ruby/bin/ruby"), String::new());
    }

    // The real bash, by its path: the child's PATH, which finds the program,
    // has the stand-in.
    let output = Command::new("/bin/bash")
        .arg(repo_root().join(script))
        .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{script}: {}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(&log).unwrap()
}

/// The macOS downloader needs bash 4 or newer and Ruby 2.7 or newer, on PATH
/// or Homebrew's, and macOS's own are older: the installers install
/// Homebrew's where neither is there, and nothing where every tool is.
#[test]
fn the_macos_installers_install_a_bash_and_ruby_the_downloader_takes() {
    for script in MACOS_INSTALLERS {
        assert_eq!(
            macos_installs(script, false, false),
            "install bash\ninstall ruby\n",
            "{script}"
        );
        assert_eq!(macos_installs(script, true, false), "", "{script}");
        assert_eq!(macos_installs(script, false, true), "", "{script}");
    }
}

/// dart-ls is the Dart SDK it is installed as, which needs no Dart on the
/// machine, so no installer installs one.
#[test]
fn no_installer_installs_a_dart_sdk() {
    for script in LINUX_INSTALLERS
        .iter()
        .chain(MACOS_INSTALLERS)
        .chain(&["scripts/windows-x86_64/install-dependencies.ps1"])
    {
        let body = std::fs::read_to_string(repo_root().join(script)).unwrap();
        assert!(!body.to_lowercase().contains("dart"), "{script}");
    }
}
