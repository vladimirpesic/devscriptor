//! Prerequisites Detection and Management
//!
//! Handles detection of build tools, SDKs, and package managers
//! required for LSP server installation, and of the runtimes a server
//! cannot start without ([`start_prerequisites`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A prerequisite for installing an LSP server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prerequisite {
    /// Build tool (node, go, rustup, etc.)
    BuildTool {
        name: &'static str,
        command: &'static str,
        min_version: Option<&'static str>,
    },
    /// SDK (java, dotnet, swift, etc.)
    Sdk {
        name: &'static str,
        command: &'static str,
        env_var: Option<&'static str>,
        /// The oldest version that will do, as `major.minor.patch`.
        min_version: Option<&'static str>,
    },
    /// Package manager (npm, pip, gem, etc.)
    PackageManager {
        name: &'static str,
        command: &'static str,
    },
    /// An extension of PHP, checked by asking `php` whether it is loaded.
    PhpExtension {
        name: &'static str,
        extension: &'static str,
        install_instructions: &'static str,
    },
    /// Custom prerequisite with install instructions
    Custom {
        name: &'static str,
        check_command: &'static str,
        install_instructions: &'static str,
        install_url: Option<&'static str>,
    },
}

impl Prerequisite {
    /// Get the display name
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Prerequisite::BuildTool { name, .. } => name,
            Prerequisite::Sdk { name, .. } => name,
            Prerequisite::PackageManager { name, .. } => name,
            Prerequisite::PhpExtension { name, .. } => name,
            Prerequisite::Custom { name, .. } => name,
        }
    }

    /// The name, with the oldest version that will do if there is one
    /// (`Java 21.0.0 or newer`).
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Prerequisite::BuildTool {
                name,
                min_version: Some(min),
                ..
            }
            | Prerequisite::Sdk {
                name,
                min_version: Some(min),
                ..
            } => format!("{name} {min} or newer"),
            _ => self.name().to_string(),
        }
    }

    /// Whether this machine meets the prerequisite, for a process run with
    /// this one's environment ([`Self::shortfall`]).
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.shortfall(&|var| std::env::var_os(var)).is_none()
    }

    /// How this machine falls short of the prerequisite for a process whose
    /// environment `env` gives — its `PATH`, and the variable an SDK is found
    /// by — as said after "it needs Java 21.0.0 or newer, and": "`/usr/bin/java`
    /// is Java 17.0.9", "there is no `php` on its PATH". `None` when it is met.
    ///
    /// A command is looked for on that `PATH`; an [`Prerequisite::Sdk`]'s in
    /// `$env_var/bin` first, as jdtls's launcher takes `JAVA_HOME`'s Java over
    /// `PATH`'s, and one whose variable names an installation without it is
    /// left to find its own. A `min_version` is compared with the version the
    /// command reports ([`version_of`]), major, minor and patch in turn; one
    /// that reports none counts as met, so that a tool is not refused on a
    /// guess.
    #[must_use]
    pub fn shortfall(&self, env: &dyn Fn(&str) -> Option<OsString>) -> Option<String> {
        match self {
            Prerequisite::BuildTool {
                command,
                min_version,
                ..
            } => match find(command, env) {
                Some(program) => self.too_old(&program, *min_version),
                None => Some(not_on_path(command)),
            },
            Prerequisite::Sdk {
                command,
                env_var,
                min_version,
                ..
            } => {
                let home = env_var.and_then(env);
                let from_home = home.as_ref().and_then(|home| {
                    which::which_in(command, Some(Path::new(home).join("bin")), ".").ok()
                });
                match from_home.or_else(|| find(command, env)) {
                    Some(program) => self.too_old(&program, *min_version),
                    None if home.is_some() => None,
                    None => Some(match env_var {
                        Some(var) => format!("{} and no {var}", not_on_path(command)),
                        None => not_on_path(command),
                    }),
                }
            }
            Prerequisite::PackageManager { command, .. }
            | Prerequisite::Custom {
                check_command: command,
                ..
            } => find(command, env).is_none().then(|| not_on_path(command)),
            Prerequisite::PhpExtension { extension, .. } => {
                let Some(php) = find("php", env) else {
                    return Some(not_on_path("php"));
                };
                let loaded = std::process::Command::new(&php)
                    .args([
                        "-r",
                        &format!("exit(extension_loaded('{extension}') ? 0 : 1);"),
                    ])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                (!loaded).then(|| format!("`{}` has no {extension} extension", php.display()))
            }
        }
    }

    /// "`program` is Java 17.0.9" when the version `program` reports is older
    /// than `min_version`; `None` when it is not, or reports none.
    fn too_old(&self, program: &Path, min_version: Option<&str>) -> Option<String> {
        let min = parse_semver(min_version?)?;
        let (major, minor, patch) = version_of(program)?;
        ((major, minor, patch) < min).then(|| {
            format!(
                "`{}` is {} {major}.{minor}.{patch}",
                program.display(),
                self.name()
            )
        })
    }

    /// Async version of [`Prerequisite::is_satisfied`] that offloads PATH lookups to a
    /// blocking thread pool to avoid stalling the async executor.
    pub async fn is_satisfied_async(&self) -> bool {
        let prereq = self.clone();
        tokio::task::spawn_blocking(move || prereq.is_satisfied())
            .await
            .unwrap_or(false)
    }

    /// Get installation instructions
    #[must_use]
    pub fn install_instructions(&self) -> String {
        match self {
            Prerequisite::BuildTool { name, .. } => {
                format!(
                    "Install {}: {}",
                    self.label(),
                    get_build_tool_install_cmd(name)
                )
            }
            Prerequisite::Sdk { name, .. } => {
                format!("Install {}: {}", self.label(), get_sdk_install_cmd(name))
            }
            Prerequisite::PackageManager { name, .. } => {
                format!("Install {}: {}", name, get_pkg_manager_install_cmd(name))
            }
            Prerequisite::PhpExtension {
                name,
                install_instructions,
                ..
            } => format!("Install {name}: {install_instructions}"),
            Prerequisite::Custom {
                name,
                install_instructions,
                install_url,
                ..
            } => {
                let url_part = install_url.map(|u| format!(" ({u})")).unwrap_or_default();
                format!("{name}: {install_instructions}{url_part}")
            }
        }
    }
}

/// Node.js 18 or newer, which the npm servers run on.
const NODE_18: Prerequisite = Prerequisite::BuildTool {
    name: "Node.js",
    command: "node",
    min_version: Some("18.0.0"),
};

/// npm, which installs the npm servers where no mirror build is taken.
const NPM: Prerequisite = Prerequisite::PackageManager {
    name: "npm",
    command: "npm",
};

/// A Java runtime, on `PATH` or through `JAVA_HOME`.
const JAVA: Prerequisite = Prerequisite::Sdk {
    name: "Java",
    command: "java",
    env_var: Some("JAVA_HOME"),
    min_version: None,
};

/// Java 21 or newer, which jdtls's launcher requires.
const JAVA_21: Prerequisite = Prerequisite::Sdk {
    name: "Java",
    command: "java",
    env_var: Some("JAVA_HOME"),
    min_version: Some("21.0.0"),
};

/// Python 3, under the name jdtls's launchers run it by: `python3` in the
/// shebang of `bin/jdtls`, `python` in `bin/jdtls.bat`.
const PYTHON: Prerequisite = Prerequisite::BuildTool {
    name: "Python",
    command: if cfg!(windows) { "python" } else { "python3" },
    min_version: None,
};

/// The .NET runtime csharp-ls and fsautocomplete run on, which comes with the
/// SDK that installs them.
const DOTNET: Prerequisite = Prerequisite::Sdk {
    name: ".NET SDK",
    command: "dotnet",
    env_var: Some("DOTNET_ROOT"),
    min_version: None,
};

/// Ruby 3.0 or newer, which ruby-lsp's gem requires and is run by.
const RUBY_3: Prerequisite = Prerequisite::Sdk {
    name: "Ruby",
    command: "ruby",
    env_var: None,
    min_version: Some("3.0.0"),
};

/// gem, which installs ruby-lsp where no mirror build is taken.
const GEM: Prerequisite = Prerequisite::PackageManager {
    name: "gem",
    command: "gem",
};

/// PHP 8.2 or newer, and the three extensions phpactor's launcher refuses to
/// start without. PHP on Windows has no posix, so phpactor is not installed
/// there (`published_for`).
const PHP_FOR_PHPACTOR: [Prerequisite; 4] = [
    Prerequisite::Sdk {
        name: "PHP",
        command: "php",
        env_var: None,
        min_version: Some("8.2.0"),
    },
    Prerequisite::PhpExtension {
        name: "PHP's mbstring extension",
        extension: "mbstring",
        install_instructions: "sudo apt install php-mbstring  # Fedora: sudo dnf install php-mbstring",
    },
    Prerequisite::PhpExtension {
        name: "PHP's posix extension",
        extension: "posix",
        install_instructions: "it comes with PHP's command line on Linux and macOS  # Fedora: sudo dnf install php-process",
    },
    Prerequisite::PhpExtension {
        name: "PHP's tokenizer extension",
        extension: "tokenizer",
        install_instructions: "it comes with PHP's command line  # Fedora: sudo dnf install php-common",
    },
];

/// Go 1.20 or newer, which `go install` installs gopls with, and which gopls
/// loads a project through.
const GO: Prerequisite = Prerequisite::BuildTool {
    name: "Go",
    command: "go",
    min_version: Some("1.20.0"),
};

/// rustup, which rust-analyzer finds a project's toolchain through.
const RUSTUP: Prerequisite = Prerequisite::BuildTool {
    name: "rustup",
    command: "rustup",
    min_version: None,
};

/// The Swift toolchain, which sourcekit-lsp comes with.
const SWIFT_TOOLCHAIN: Prerequisite = Prerequisite::Custom {
    name: "Swift toolchain",
    check_command: "swift",
    install_instructions: "Install Swift toolchain",
    install_url: Some("https://swift.org/download/"),
};

/// The servers that run on Node.js, installed from npm.
const NPM_SERVERS: [&str; 9] = [
    "typescript-language-server",
    "volar",
    "svelte-language-server",
    "html-languageserver",
    "sql-language-server",
    "graphql-language-service",
    "bash-language-server",
    "nomicfoundation-solc",
    "pyright",
];

/// What an LSP server cannot start without: the runtime its launcher runs
/// on, at the version the launcher accepts. A server Devscriptor installed
/// carries these ([`LspServerConfig::prerequisites`]) and is checked against
/// them before it starts, so that one this machine cannot run is refused with
/// what it lacks, where it would otherwise exit as it starts, with only its
/// launcher's complaint. gopls and rust-analyzer start without their
/// toolchains, and say once they run what they cannot load; clangd, dart-ls
/// and the Kotlin LSP need nothing of the host's.
///
/// [`LspServerConfig::prerequisites`]: crate::config::LspServerConfig::prerequisites
#[must_use]
pub fn start_prerequisites(server_id: &str) -> Vec<Prerequisite> {
    match server_id {
        id if NPM_SERVERS.contains(&id) => vec![NODE_18],
        "metals" => vec![JAVA],
        // jdtls starts through `bin/jdtls`, a Python 3 script that runs Java.
        "jdtls" => vec![JAVA_21, PYTHON],
        "csharp-ls" | "fsautocomplete" => vec![DOTNET],
        "ruby-lsp" => vec![RUBY_3],
        "phpactor" => PHP_FOR_PHPACTOR.to_vec(),
        _ => vec![],
    }
}

/// The prerequisites of an LSP server: what it needs on the host to start
/// ([`start_prerequisites`]), and to be installed, or to load a project
/// once it runs — npm and gem, which install the npm servers and ruby-lsp;
/// Go for gopls; rustup for rust-analyzer; the Swift toolchain sourcekit-lsp
/// comes with.
///
/// Both `lsp setup` (and the `lsp_arch_setup` tool) and [`LspManager::install`]
/// check this list, so a server is installed only where it can start.
///
/// [`LspManager::install`]: crate::manager::LspManager::install
#[must_use]
pub fn get_prerequisites(server_id: &str) -> Vec<Prerequisite> {
    let mut prerequisites = start_prerequisites(server_id);
    prerequisites.extend(match server_id {
        id if NPM_SERVERS.contains(&id) => vec![NPM],
        "ruby-lsp" => vec![GEM],
        "gopls" => vec![GO],
        "rust-analyzer" => vec![RUSTUP],
        "sourcekit-lsp" => vec![SWIFT_TOOLCHAIN],
        _ => vec![],
    });
    prerequisites
}

/// What a server cannot start without, of `prerequisites`, that this machine
/// lacks for a process whose environment `env` gives
/// ([`Prerequisite::shortfall`]): "it needs Java 21.0.0 or newer, and
/// `/usr/bin/java` is Java 17.0.9", for each — those that fall short alike
/// said together — then how to install them. `None` when all are met.
#[must_use]
pub fn unmet(
    prerequisites: &[Prerequisite],
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<String> {
    let mut short: Vec<(String, Vec<&Prerequisite>)> = Vec::new();
    for prerequisite in prerequisites {
        if let Some(found) = prerequisite.shortfall(env) {
            match short.iter_mut().find(|(seen, _)| *seen == found) {
                Some((_, alike)) => alike.push(prerequisite),
                None => short.push((found, vec![prerequisite])),
            }
        }
    }
    if short.is_empty() {
        return None;
    }
    let needs: Vec<String> = short
        .iter()
        .map(|(found, alike)| {
            let labels: Vec<String> = alike.iter().map(|p| p.label()).collect();
            format!("it needs {}, and {found}", and_list(&labels))
        })
        .collect();
    let install: Vec<String> = short
        .iter()
        .flat_map(|(_, alike)| alike.iter().map(|p| p.install_instructions()))
        .collect();
    Some(format!("{}. {}", needs.join("; "), install.join("; ")))
}

/// `items` as a sentence lists them: "a", "a and b", "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `command` where a process whose environment `env` gives finds it: on its
/// `PATH`.
fn find(command: &str, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    which::which_in(command, env("PATH"), ".").ok()
}

/// What is said of a command a process does not find.
fn not_on_path(command: &str) -> String {
    format!("there is no `{command}` on its PATH")
}

/// Check all prerequisites for a server
#[must_use]
pub fn check_prerequisites(server_id: &str) -> PrerequisitesResult {
    let prerequisites = get_prerequisites(server_id);

    let mut satisfied = Vec::new();
    let mut missing = Vec::new();

    for prereq in prerequisites {
        if prereq.is_satisfied() {
            satisfied.push(prereq);
        } else {
            missing.push(prereq);
        }
    }

    PrerequisitesResult {
        server_id: server_id.to_string(),
        satisfied,
        missing,
    }
}

/// Async version of [`check_prerequisites`] that avoids blocking the async
/// executor during PATH lookups.
pub async fn check_prerequisites_async(server_id: &str) -> PrerequisitesResult {
    let prerequisites = get_prerequisites(server_id);

    let mut satisfied = Vec::new();
    let mut missing = Vec::new();

    for prereq in prerequisites {
        if prereq.is_satisfied_async().await {
            satisfied.push(prereq);
        } else {
            missing.push(prereq);
        }
    }

    PrerequisitesResult {
        server_id: server_id.to_string(),
        satisfied,
        missing,
    }
}

/// Result of prerequisite checking
#[derive(Debug, Clone)]
pub struct PrerequisitesResult {
    pub server_id: String,
    pub satisfied: Vec<Prerequisite>,
    pub missing: Vec<Prerequisite>,
}

impl PrerequisitesResult {
    /// Check if all prerequisites are satisfied
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.missing.is_empty()
    }

    /// Get installation instructions for missing prerequisites
    #[must_use]
    pub fn install_instructions(&self) -> Vec<String> {
        self.missing
            .iter()
            .map(Prerequisite::install_instructions)
            .collect()
    }
}

/// Get installation command for a build tool
fn get_build_tool_install_cmd(name: &str) -> &'static str {
    match name {
        "Node.js" => "sudo apt install nodejs npm  # or: nvm install node",
        "Go" => "sudo apt install golang-go  # or: https://go.dev/dl/",
        "rustup" => "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        "Python" => "sudo apt install python3 python3-pip",
        _ => "See official documentation",
    }
}

/// Get installation command for an SDK
fn get_sdk_install_cmd(name: &str) -> &'static str {
    match name {
        "Java" => "sudo apt install openjdk-21-jdk  # or: https://adoptium.net/",
        ".NET SDK" => {
            "sudo apt install dotnet-sdk-8.0  # or: https://dotnet.microsoft.com/download"
        }
        "Ruby" => "sudo apt install ruby ruby-dev",
        "PHP" => "sudo apt install php-cli php-mbstring  # macOS: brew install php",
        "Swift" => "See: https://swift.org/download/",
        _ => "See official documentation",
    }
}

/// Get installation command for a package manager
fn get_pkg_manager_install_cmd(name: &str) -> &'static str {
    match name {
        "npm" => "Comes with Node.js",
        "gem" => "Comes with Ruby",
        _ => "See official documentation",
    }
}

/// Get auto-install capability for a prerequisite
#[must_use]
pub fn can_auto_install(prereq: &Prerequisite) -> bool {
    match prereq {
        Prerequisite::BuildTool { name, .. } => matches!(name, &"Node.js" | &"npm"),
        Prerequisite::Sdk { .. } => false, // SDKs usually require sudo
        Prerequisite::PackageManager { name, .. } => matches!(name, &"npm"),
        Prerequisite::PhpExtension { .. } | Prerequisite::Custom { .. } => false,
    }
}

/// Categorize prerequisites by installation difficulty
#[derive(Debug, Clone)]
pub enum InstallDifficulty {
    /// Can be auto-installed
    Auto,
    /// Requires package manager (apt, brew, etc.)
    PackageManager,
    /// Requires manual download/install
    Manual,
    /// Large download (> 500MB)
    LargeDownload,
}

/// Get installation difficulty for a prerequisite
#[must_use]
pub fn get_install_difficulty(prereq: &Prerequisite) -> InstallDifficulty {
    match prereq {
        Prerequisite::BuildTool { name, .. } => match *name {
            "Node.js" => InstallDifficulty::PackageManager,
            "Go" => InstallDifficulty::PackageManager,
            "rustup" => InstallDifficulty::Auto,
            _ => InstallDifficulty::Manual,
        },
        Prerequisite::Sdk { name, .. } => match *name {
            "Java" => InstallDifficulty::PackageManager,
            ".NET SDK" => InstallDifficulty::LargeDownload,
            "Swift" => InstallDifficulty::LargeDownload,
            _ => InstallDifficulty::PackageManager,
        },
        Prerequisite::PackageManager { name, .. } => match *name {
            "npm" => InstallDifficulty::Auto,
            _ => InstallDifficulty::PackageManager,
        },
        Prerequisite::PhpExtension { .. } => InstallDifficulty::PackageManager,
        Prerequisite::Custom { name, .. } => match *name {
            "Swift toolchain" => InstallDifficulty::LargeDownload,
            _ => InstallDifficulty::Manual,
        },
    }
}

/// Get tiered prerequisite categories for a server
#[must_use]
pub fn get_prerequisite_tiers(server_id: &str) -> PrerequisiteTiers {
    let result = check_prerequisites(server_id);

    let mut auto_installable = Vec::new();
    let mut package_manager = Vec::new();
    let mut manual = Vec::new();
    let mut large_download = Vec::new();

    for prereq in result.missing {
        let difficulty = get_install_difficulty(&prereq);
        match difficulty {
            InstallDifficulty::Auto => auto_installable.push(prereq),
            InstallDifficulty::PackageManager => package_manager.push(prereq),
            InstallDifficulty::Manual => manual.push(prereq),
            InstallDifficulty::LargeDownload => large_download.push(prereq),
        }
    }

    PrerequisiteTiers {
        auto_installable,
        package_manager,
        manual,
        large_download,
    }
}

/// Tiered prerequisite categories
#[derive(Debug, Clone)]
pub struct PrerequisiteTiers {
    pub auto_installable: Vec<Prerequisite>,
    pub package_manager: Vec<Prerequisite>,
    pub manual: Vec<Prerequisite>,
    pub large_download: Vec<Prerequisite>,
}

impl PrerequisiteTiers {
    /// Check if any prerequisites require manual intervention
    #[must_use]
    pub fn requires_manual(&self) -> bool {
        !self.manual.is_empty() || !self.large_download.is_empty()
    }

    /// Get total count of missing prerequisites
    #[must_use]
    pub fn total_missing(&self) -> usize {
        self.auto_installable.len()
            + self.package_manager.len()
            + self.manual.len()
            + self.large_download.len()
    }
}

/// The version `program` reports, as `(major, minor, patch)`; `None` when it
/// cannot be run or reports none.
///
/// Tools report their version differently: `--version` (Node.js, Python,
/// Java 9+), `-version` (Java 8, which a newer minimum must refuse) or the
/// `version` subcommand (Go); the first answer that holds a version is used.
fn version_of(program: &Path) -> Option<(u64, u64, u64)> {
    ["--version", "-version", "version"].iter().find_map(|arg| {
        let output = std::process::Command::new(program)
            .arg(arg)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        parse_semver(&text)
    })
}

/// Extract the first version from an arbitrary version string, as
/// `(major, minor, patch)`: `v18.17.1`, `go1.20.3`, `25.0.4.1` (a fourth
/// part is ignored), `"1.8.0_392"`, or a bare major such as Java's `21`.
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:^|[^0-9.])(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:\.\d+)*(?:$|[^0-9.])")
            .expect("version regex is valid")
    });
    RE.captures(s).and_then(|cap| {
        let part = |i: usize| {
            cap.get(i)
                .map_or(Some(0), |m| m.as_str().parse::<u64>().ok())
        };
        Some((part(1)?, part(2)?, part(3)?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_prerequisites_rust() {
        let prereqs = get_prerequisites("rust-analyzer");
        assert!(!prereqs.is_empty());
        assert!(prereqs.iter().any(|p| p.name() == "rustup"));
    }

    #[test]
    fn test_get_prerequisites_typescript() {
        let prereqs = get_prerequisites("typescript-language-server");
        assert!(prereqs.iter().any(|p| p.name() == "Node.js"));
        assert!(prereqs.iter().any(|p| p.name() == "npm"));
    }

    #[test]
    fn test_prerequisite_is_satisfied() {
        // Node.js is likely to be installed
        let node = Prerequisite::BuildTool {
            name: "Node.js",
            command: "node",
            min_version: None,
        };
        // Just check it doesn't panic
        let _ = node.is_satisfied();
    }

    /// phpactor's launcher refuses to start without PHP 8.2 and the
    /// mbstring, posix and tokenizer extensions, so those are its
    /// prerequisites; an extension is checked by asking `php`.
    #[test]
    fn phpactor_needs_php_with_its_extensions() {
        let prerequisites = get_prerequisites("phpactor");
        assert!(
            prerequisites
                .iter()
                .any(|p| p.label() == "PHP 8.2.0 or newer")
        );
        let extensions: Vec<&str> = prerequisites
            .iter()
            .filter_map(|p| match p {
                Prerequisite::PhpExtension { extension, .. } => Some(*extension),
                _ => None,
            })
            .collect();
        assert_eq!(extensions, ["mbstring", "posix", "tokenizer"]);
        let missing = Prerequisite::PhpExtension {
            name: "PHP's absent extension",
            extension: "devscriptor_absent",
            install_instructions: "",
        };
        assert!(!missing.is_satisfied(), "with php or without it");
        if which::which("php").is_ok() {
            let core = Prerequisite::PhpExtension {
                name: "PHP's core",
                extension: "core",
                install_instructions: "",
            };
            assert!(core.is_satisfied());
        }
    }

    #[test]
    fn test_check_prerequisites() {
        let result = check_prerequisites("typescript-language-server");
        // Should have Node.js and npm as prerequisites
        assert!(result.satisfied.len() + result.missing.len() >= 2);
    }

    /// `min_version`: parse_semver extracts version tuples, which compare as
    /// the versions do.
    #[test]
    fn test_min_version_semver_parsing() {
        assert_eq!(parse_semver("18.17.1"), Some((18, 17, 1)));
        assert_eq!(parse_semver("Node.js 20.0.0"), Some((20, 0, 0)));
        assert_eq!(
            parse_semver("go version 1.20.3 linux/amd64"),
            Some((1, 20, 3))
        );
        assert_eq!(parse_semver("no version here"), None);
    }

    /// What the tools with a minimum version print. `node --version` and
    /// `go version` put no word boundary before their digits, and are read all
    /// the same. Java's GA releases have a bare major.
    #[test]
    fn versions_are_read_as_the_tools_print_them() {
        assert_eq!(parse_semver("v18.17.1\n"), Some((18, 17, 1)));
        assert_eq!(
            parse_semver("go version go1.20.3 linux/amd64"),
            Some((1, 20, 3))
        );
        assert_eq!(
            parse_semver("openjdk 25.0.4.1 2026-08-18\nOpenJDK Runtime Environment"),
            Some((25, 0, 4))
        );
        assert_eq!(
            parse_semver("openjdk 21 2023-09-19\nOpenJDK Runtime Environment (build 21+35)"),
            Some((21, 0, 0))
        );
        assert_eq!(
            parse_semver("openjdk version \"1.8.0_392\"\nOpenJDK Runtime Environment"),
            Some((1, 8, 0))
        );
        assert_eq!(parse_semver("Python 3.12.3"), Some((3, 12, 3)));
    }

    /// jdtls's launcher refuses Java older than 21; the Kotlin LSP and the
    /// Dart SDK bring their own runtimes.
    #[test]
    fn jvm_and_sdk_servers_declare_what_they_run_on() {
        let jdtls = get_prerequisites("jdtls");
        assert!(jdtls.iter().any(|p| p.label() == "Java 21.0.0 or newer"));
        assert!(jdtls.iter().any(|p| p.name() == "Python"));
        assert!(get_prerequisites("kotlin-lsp").is_empty());
        assert!(get_prerequisites("dart-ls").is_empty());
    }

    /// `min_version`: a version meeting the minimum passes; one below fails.
    #[test]
    fn test_min_version_comparison() {
        let at_min: Option<(u64, u64, u64)> = Some((18, 0, 0));
        let above_min: Option<(u64, u64, u64)> = Some((20, 0, 0));
        let below_min: Option<(u64, u64, u64)> = Some((16, 0, 0));
        let min: Option<(u64, u64, u64)> = Some((18, 0, 0));

        assert!(at_min >= min);
        assert!(above_min >= min);
        assert!(below_min < min);
    }

    /// What a server cannot start without is the runtime its launcher runs
    /// on; what installs it, or what it loads a project with once it runs,
    /// is a prerequisite of its install only. Every server's install needs
    /// what it starts with.
    #[test]
    fn a_server_starts_with_its_runtime_and_installs_with_more() {
        let labels = |list: Vec<Prerequisite>| -> Vec<String> {
            list.iter().map(Prerequisite::label).collect()
        };
        assert_eq!(
            labels(start_prerequisites("jdtls")),
            ["Java 21.0.0 or newer", "Python"]
        );
        for npm_server in ["typescript-language-server", "pyright"] {
            assert_eq!(
                labels(start_prerequisites(npm_server)),
                ["Node.js 18.0.0 or newer"]
            );
            assert_eq!(
                labels(get_prerequisites(npm_server)),
                ["Node.js 18.0.0 or newer", "npm"]
            );
        }
        assert_eq!(
            labels(get_prerequisites("ruby-lsp")),
            ["Ruby 3.0.0 or newer", "gem"]
        );
        assert_eq!(labels(start_prerequisites("phpactor")).len(), 4);
        for starts_alone in ["gopls", "rust-analyzer", "clangd", "kotlin-lsp", "dart-ls"] {
            assert!(
                start_prerequisites(starts_alone).is_empty(),
                "{starts_alone}"
            );
        }
        for id in crate::generated_registry::SERVER_IDS {
            let start = start_prerequisites(id);
            assert_eq!(get_prerequisites(id)[..start.len()], start[..], "{id}");
        }
    }

    /// A directory with nothing in it, as a process's `PATH`.
    fn path_of(dir: &Path) -> impl Fn(&str) -> Option<OsString> + '_ {
        move |var| (var == "PATH").then(|| dir.as_os_str().to_os_string())
    }

    /// A command missing from the process's `PATH` is said to be, and an SDK
    /// whose variable is not set either is said to have none.
    #[test]
    fn a_missing_command_is_said_to_be_missing() {
        let empty = tempfile::TempDir::new().unwrap();
        let env = path_of(empty.path());
        let tool = Prerequisite::BuildTool {
            name: "Absent",
            command: "devscriptor-absent-tool",
            min_version: None,
        };
        assert_eq!(
            tool.shortfall(&env).as_deref(),
            Some("there is no `devscriptor-absent-tool` on its PATH")
        );
        assert_eq!(
            JAVA_21.shortfall(&env).as_deref(),
            Some("there is no `java` on its PATH and no JAVA_HOME")
        );
        let home = |var: &str| match var {
            "PATH" => Some(empty.path().as_os_str().to_os_string()),
            "JAVA_HOME" => Some(OsString::from("/opt/java-without-bin")),
            _ => None,
        };
        assert_eq!(
            JAVA_21.shortfall(&home),
            None,
            "a JAVA_HOME the launcher finds its own way"
        );
    }

    /// The unmet prerequisites are said each with what was found, those
    /// found short alike together, then how to install each.
    #[test]
    fn unmet_prerequisites_are_said_with_how_to_install_them() {
        let empty = tempfile::TempDir::new().unwrap();
        let env = path_of(empty.path());
        let first = Prerequisite::BuildTool {
            name: "First",
            command: "devscriptor-first",
            min_version: None,
        };
        let second = Prerequisite::PackageManager {
            name: "second",
            command: "devscriptor-second",
        };
        assert_eq!(
            unmet(&[first.clone(), second], &env).as_deref(),
            Some(
                "it needs First, and there is no `devscriptor-first` on its PATH; it needs \
                 second, and there is no `devscriptor-second` on its PATH. Install First: See \
                 official documentation; Install second: See official documentation"
            )
        );
        let php = unmet(&PHP_FOR_PHPACTOR, &env).unwrap();
        assert!(
            php.starts_with(
                "it needs PHP 8.2.0 or newer, PHP's mbstring extension, PHP's posix extension \
                 and PHP's tokenizer extension, and there is no `php` on its PATH. Install PHP \
                 8.2.0 or newer: "
            ),
            "{php}"
        );
        assert_eq!(unmet(&[], &env), None);
        assert_eq!(and_list(&["a".to_string(), "b".to_string()]), "a and b");
    }

    /// A Java that says it is too old is named, with its version; one new
    /// enough, or one that says no version, passes. `JAVA_HOME`'s Java comes
    /// before `PATH`'s, as jdtls's launcher takes it.
    #[cfg(unix)]
    #[test]
    fn a_runtime_too_old_is_named_with_its_version() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::TempDir::new().unwrap();
        let java_saying = |dir: &str, says: &str| -> PathBuf {
            let bin = root.path().join(dir).join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let java = bin.join("java");
            std::fs::write(&java, format!("#!/bin/sh\necho '{says}' >&2\n")).unwrap();
            std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
            bin
        };
        let old = java_saying("old", "openjdk version \"17.0.9\" 2023-10-17");
        let new = java_saying("new", "openjdk version \"21.0.4\" 2024-07-16");
        let silent = java_saying("silent", "Unable to locate a Java Runtime.");

        assert_eq!(
            JAVA_21.shortfall(&path_of(&old)),
            Some(format!("`{}` is Java 17.0.9", old.join("java").display()))
        );
        assert_eq!(JAVA_21.shortfall(&path_of(&new)), None);
        assert_eq!(JAVA_21.shortfall(&path_of(&silent)), None);
        assert_eq!(JAVA.shortfall(&path_of(&old)), None, "any Java will do");

        let home_first = |var: &str| match var {
            "PATH" => Some(old.as_os_str().to_os_string()),
            "JAVA_HOME" => Some(root.path().join("new").into_os_string()),
            _ => None,
        };
        assert_eq!(JAVA_21.shortfall(&home_first), None);
    }
}
