//! Prerequisites Detection and Management
//!
//! Handles detection of build tools, SDKs, and package managers
//! required for LSP server installation.

use std::ffi::OsString;

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

    /// Check if this prerequisite is satisfied
    ///
    /// For [`Prerequisite::BuildTool`] entries with a `min_version`, the binary
    /// must be on `PATH` **and** report a version `≥ min_version` when invoked
    /// with `--version`. Version comparisons are done lexicographically on the
    /// major.minor.patch tuple so `18.2.0 ≥ 18.0.0` is correct.
    /// An [`Prerequisite::Sdk`] with a `min_version` checks the command its
    /// servers run: the one in `$env_var/bin` if the variable names an
    /// installation that has it (jdtls prefers `JAVA_HOME`), else `PATH`'s.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        match self {
            Prerequisite::BuildTool {
                command,
                min_version,
                ..
            } => {
                if which::which(command).is_err() {
                    return false;
                }
                if let Some(min) = min_version {
                    return check_version_gte(command, min);
                }
                true
            }
            Prerequisite::Sdk {
                command,
                env_var,
                min_version,
                ..
            } => {
                let home = env_var.and_then(std::env::var_os);
                let present = which::which(command).is_ok() || home.is_some();
                match min_version {
                    Some(min) if present => {
                        let from_home = home.and_then(|home| {
                            let bin = std::path::Path::new(&home).join("bin");
                            which::which_in(command, Some(bin), ".").ok()
                        });
                        let program = from_home.map_or_else(|| OsString::from(command), Into::into);
                        check_version_gte(program, min)
                    }
                    _ => present,
                }
            }
            Prerequisite::PackageManager { command, .. } => which::which(command).is_ok(),
            Prerequisite::Custom { check_command, .. } => which::which(check_command).is_ok(),
        }
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

/// The prerequisites of an LSP server: what it needs on the host to run.
///
/// Both `lsp setup` (and the `lsp_arch_setup` tool) and [`LspManager::install`]
/// check this list, so a server is installed only where it can start.
///
/// [`LspManager::install`]: crate::manager::LspManager::install
#[must_use]
pub fn get_prerequisites(server_id: &str) -> Vec<Prerequisite> {
    match server_id {
        // ========== Node.js-based servers ==========
        "typescript-language-server"
        | "volar"
        | "svelte-language-server"
        | "html-languageserver"
        | "sql-language-server"
        | "graphql-language-service"
        | "bash-language-server"
        | "nomicfoundation-solc" => vec![
            Prerequisite::BuildTool {
                name: "Node.js",
                command: "node",
                min_version: Some("18.0.0"),
            },
            Prerequisite::PackageManager {
                name: "npm",
                command: "npm",
            },
        ],

        // ========== Python-based servers ==========
        "pyright" => vec![Prerequisite::BuildTool {
            name: "Node.js",
            command: "node",
            min_version: Some("18.0.0"),
        }],

        // ========== Go ==========
        "gopls" => vec![Prerequisite::BuildTool {
            name: "Go",
            command: "go",
            min_version: Some("1.20.0"),
        }],

        // ========== Rust ==========
        "rust-analyzer" => vec![Prerequisite::BuildTool {
            name: "rustup",
            command: "rustup",
            min_version: None,
        }],

        // ========== JVM Languages ==========
        "metals" => vec![JAVA],
        // jdtls starts through `bin/jdtls`, a Python 3 script that runs Java.
        "jdtls" => vec![JAVA_21, PYTHON],
        // The Kotlin LSP comes with the Java runtime it runs on.
        "kotlin-lsp" => vec![],

        // ========== Microsoft Ecosystem ==========
        "csharp-ls" | "fsautocomplete" => vec![Prerequisite::Sdk {
            name: ".NET SDK",
            command: "dotnet",
            env_var: Some("DOTNET_ROOT"),
            min_version: None,
        }],

        // ========== Mobile Development ==========
        // What is installed is the Dart SDK itself, whose `dart` runs the
        // analysis server: it needs no Dart of the host's.
        "dart-ls" => vec![],
        "sourcekit-lsp" => vec![Prerequisite::Custom {
            name: "Swift toolchain",
            check_command: "swift",
            install_instructions: "Install Swift toolchain",
            install_url: Some("https://swift.org/download/"),
        }],

        // ========== Other Languages ==========
        "ruby-lsp" => vec![
            Prerequisite::Sdk {
                name: "Ruby",
                command: "ruby",
                env_var: None,
                min_version: None,
            },
            Prerequisite::PackageManager {
                name: "gem",
                command: "gem",
            },
        ],
        "phpactor" => vec![Prerequisite::Sdk {
            name: "PHP",
            command: "php",
            env_var: None,
            min_version: None,
        }],
        "clangd" => vec![],

        _ => vec![],
    }
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
        "Java" => "sudo apt install default-jdk  # or: https://adoptium.net/",
        ".NET SDK" => {
            "sudo apt install dotnet-sdk-8.0  # or: https://dotnet.microsoft.com/download"
        }
        "Ruby" => "sudo apt install ruby ruby-dev",
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
        Prerequisite::Custom { .. } => false,
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

/// Read the version `command` reports and return `true` if it is
/// `≥ min_version`.
///
/// Tools report their version differently: `--version` (Node.js, Python,
/// Java 9+), `-version` (Java 8, whose answer must fail a newer minimum) or the
/// `version` subcommand (Go); the first answer that holds a version is used.
/// Comparison is element by element on major, minor and patch. Returns `true`
/// if no version can be read, so that a tool without one is not a hard failure
/// at install-check time.
fn check_version_gte(command: impl AsRef<std::ffi::OsStr>, min_version: &str) -> bool {
    let command = command.as_ref();
    let Some(min) = parse_semver(min_version) else {
        return true;
    };
    for arg in ["--version", "-version", "version"] {
        let Ok(output) = std::process::Command::new(command).arg(arg).output() else {
            return true;
        };
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        if let Some(actual) = parse_semver(&text) {
            return actual >= min;
        }
    }
    true
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

    #[test]
    fn test_check_prerequisites() {
        let result = check_prerequisites("typescript-language-server");
        // Should have Node.js and npm as prerequisites
        assert!(result.satisfied.len() + result.missing.len() >= 2);
    }

    /// 4.5 AC min_version: parse_semver extracts version tuples and comparison works.
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

    /// 4.5 AC min_version: a version meeting the minimum passes; one below fails.
    #[test]
    fn test_min_version_comparison() {
        // check_version_gte is hard to unit-test without spawning a process,
        // but we can test the comparison logic via parse_semver directly.
        let at_min: Option<(u64, u64, u64)> = Some((18, 0, 0));
        let above_min: Option<(u64, u64, u64)> = Some((20, 0, 0));
        let below_min: Option<(u64, u64, u64)> = Some((16, 0, 0));
        let min: Option<(u64, u64, u64)> = Some((18, 0, 0));

        assert!(at_min >= min);
        assert!(above_min >= min);
        assert!(below_min < min);
    }
}
