//! The Windows scripts, read as text: no PowerShell runs these tests, so
//! what would break them under Windows PowerShell 5.1 is checked here.
#![cfg(feature = "lsp")]

use std::path::{Path, PathBuf};

const POWERSHELL_SCRIPTS: &[&str] = &[
    "scripts/windows-x86_64/download-lsp-servers.ps1",
    "scripts/windows-x86_64/install-dependencies.ps1",
    "scripts/windows-x86_64/package-lsp-servers.ps1",
];

fn read(rel: &str) -> String {
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf();
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Windows PowerShell 5.1 reads a script without a byte-order mark in the
/// ANSI code page, where UTF-8's `—` (E2 80 94) is `â€”`: its last character,
/// U+201D, is a closing quote to PowerShell, and ends a string it is in.
#[test]
fn powershell_scripts_are_ascii() {
    for script in POWERSHELL_SCRIPTS {
        for (n, line) in read(script).lines().enumerate() {
            assert!(line.is_ascii(), "{script}:{}: {line}", n + 1);
        }
    }
}

/// In `if (Test-Path $dir -and $x)` PowerShell hands `-and` to `Test-Path`
/// as a parameter, which it has not, and the condition fails with an error:
/// a command in a condition is parenthesised before an operator joins it.
#[test]
fn no_command_takes_an_operator_for_a_parameter() {
    for script in POWERSHELL_SCRIPTS {
        for (n, line) in read(script).lines().enumerate() {
            let Some(start) = line.find("if (") else {
                continue;
            };
            let condition = &line[start + 4..];
            let command = condition.split_whitespace().next().unwrap_or_default();
            let is_command = command.contains('-')
                && command
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase());
            if !is_command {
                continue;
            }
            let unparenthesised = condition.split('(').next().unwrap_or_default();
            for operator in [" -and ", " -or "] {
                assert!(
                    !unparenthesised.contains(operator),
                    "{script}:{}: `{command}` would take `{}` for a parameter: {line}",
                    n + 1,
                    operator.trim()
                );
            }
        }
    }
}

/// The Microsoft Store's `python` alias is on PATH where no Python is
/// installed, so a `python` command is not taken for one: Python is installed
/// unless `python --version` names it.
#[test]
fn the_store_alias_is_not_taken_for_python() {
    let body = read("scripts/windows-x86_64/install-dependencies.ps1");
    assert!(!body.contains("Install-WinGet \"Python.Python.3.12\" \"python\""));
    assert!(body.contains("if ($pythonVersion -match '^Python 3') {"));
}

/// The Windows packager puts each server's license files in its archive, as
/// the Unix packagers do (`package_scripts_run`): before the archive is made,
/// a fetched one checked against the SHA-256 the registry pins, and a server
/// without them not packaged.
#[test]
fn the_windows_packager_adds_the_license_files() {
    let body = read("scripts/windows-x86_64/package-lsp-servers.ps1");
    let added = body
        .find("    if (-not (Add-LicenseFiles $serverId)) { return $false }")
        .expect("licenses are added");
    let archived = body
        .find("        Compress-Archive -Path $dir")
        .expect("the archive is made");
    assert!(added < archived);
    assert!(body.contains("$script:RegistryServers.$serverId.license_files"));
    assert!(
        body.contains("(Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower() -ne $file.sha256")
    );
    assert!(body.contains("registry.json names no license file"));
}

/// The Windows downloader asks GitHub's API with the token the environment
/// gives, which it takes out of the environment before any installer runs,
/// as the Unix downloaders do (`download_scripts_run`), and unpacks jdtls
/// with Windows' own tar: Git's GNU tar, first on PATH under Git Bash, takes
/// the archive's `C:` for a host.
#[test]
fn the_windows_downloader_keeps_its_token_and_unpacks_with_windows_tar() {
    let body = read("scripts/windows-x86_64/download-lsp-servers.ps1");
    let taken = body
        .find("$script:GitHubApiToken = $env:GITHUB_TOKEN\nRemove-Item Env:GITHUB_TOKEN")
        .expect("the token is taken out of the environment");
    let first_install = body.find("& npm.cmd install").expect("npm installs");
    assert!(taken < first_install);
    assert!(body.contains("$headers[\"Authorization\"] = \"Bearer $script:GitHubApiToken\""));
    assert!(body.contains("$tar = Join-Path $env:SystemRoot \"System32\\tar.exe\""));
    assert!(!body.contains("& tar "));
}

/// Coursier writes its launcher to the name it is given and, on Windows, the
/// batch file that runs it with `.bat` added: named `metals.bat`, the launcher
/// would be the POSIX shell one, beside a `metals.bat.bat`.
#[test]
fn metals_is_bootstrapped_under_the_name_its_batch_file_extends() {
    let body = read("scripts/windows-x86_64/download-lsp-servers.ps1");
    assert!(body.contains("-o (Join-Path $mbin \"metals\")"));
    assert!(!body.contains("-o (Join-Path $mbin \"metals.bat\")"));
    // The search a thin launcher is told by is defined before it is used.
    let defined = body
        .find("function Test-FileHolds([string]$Path, [string]$Text)")
        .expect("Test-FileHolds is defined");
    let used = body
        .find("(Test-FileHolds ")
        .expect("Test-FileHolds is used");
    assert!(defined < used);
}

/// A Windows install is made beside the one it replaces and put in place
/// once it is in, as on the Unix downloaders (`a_failed_install_leaves_the_earlier_one`),
/// and ruby-lsp's `.version` is written only for an install this run made.
#[test]
fn a_windows_install_replaces_the_earlier_one_only_once_it_is_in() {
    let body = read("scripts/windows-x86_64/download-lsp-servers.ps1");
    for function in ["Download-Npm", "Download-Dotnet", "Download-Gem"] {
        let start = body
            .find(&format!("function {function}("))
            .unwrap_or_else(|| panic!("no {function}"));
        let end = start + body[start..].find("\n}\n").unwrap();
        let definition = &body[start..end];
        assert!(
            definition.contains("New-Staging"),
            "{function} installs in place"
        );
        assert!(
            definition.contains("Install-Swapped"),
            "{function} never swaps in"
        );
    }
    assert!(body.contains("$staging = New-Staging $metalsDir"));
    assert!(body.contains("if ($script:Downloaded.ContainsKey(\"ruby-lsp\")) {"));
}
