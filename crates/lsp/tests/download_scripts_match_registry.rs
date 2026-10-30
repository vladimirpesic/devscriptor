//! The four platform downloaders must agree with `registry.json`.
//!
//! The resolver looks for a server under `<install-root>/<server-id>/`, where the
//! id comes from the registry. A downloader that names its directory anything else
//! produces a server the runtime can never find. That is exactly what happened to
//! `dart-ls`: every script created `dart/`, so the managed-install tier missed it
//! and it silently fell through to system detection.
//!
//! These tests read the scripts as text. They cannot prove a script *runs*, but
//! they do catch the class of drift that is invisible until a release is packaged.
#![cfg(feature = "lsp")]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use devscriptor_lsp::lsp_architecture::prerequisites::get_prerequisites;
use devscriptor_lsp::manager::registry::{ServerRegistry, SourceType};

fn repo_root() -> PathBuf {
    // crates/lsp/tests/ -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

const DOWNLOADERS: &[&str] = &[
    "scripts/linux-x86_64/download-lsp-servers.sh",
    "scripts/linux-aarch64/download-lsp-servers.sh",
    "scripts/macos-x86_64/download-lsp-servers.sh",
    "scripts/macos-aarch64/download-lsp-servers.sh",
    "scripts/windows-x86_64/download-lsp-servers.ps1",
];

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every registry id must appear in every downloader.
///
/// A server absent from one platform's script is a server that platform ships
/// without, which is a packaging decision that should be explicit, not accidental.
#[test]
fn every_downloader_covers_every_registry_server() {
    let registry = ServerRegistry::load().unwrap();
    let ids: Vec<&str> = registry.servers.keys().map(String::as_str).collect();
    assert_eq!(ids.len(), 21);

    for script in DOWNLOADERS {
        let body = read(script);
        let missing: Vec<&&str> = ids.iter().filter(|id| !body.contains(**id)).collect();
        assert!(
            missing.is_empty(),
            "{script} never mentions: {missing:?}\n\
             The resolver looks under <install-root>/<server-id>/, so a directory \
             named anything else is invisible to it."
        );
    }
}

/// `dart-ls` is the id and the directory: nothing may create a `dart` directory.
#[test]
fn no_downloader_uses_the_legacy_dart_directory() {
    for script in DOWNLOADERS {
        let body = read(script);
        for stale in ["LSP_DIR/dart/", "LspDir \"dart\\"] {
            assert!(
                !body.contains(stale),
                "{script} still references `{stale}`; the directory the resolver \
                 looks under is `dart-ls`, the registry id"
            );
        }
    }
}

/// `dart-ls` must be a genuinely self-contained Dart SDK, not a pointer at the
/// build machine: neither a shim that `exec`s the builder's `dart` nor a bare
/// `analysis_server.dart.snapshot`, which only runs on the exact VM that built
/// it.
#[test]
fn dart_downloaders_fetch_the_official_sdk() {
    for script in DOWNLOADERS {
        let body = read(script);
        assert!(
            body.contains("dart-archive/channels/stable/release"),
            "{script} must fetch the Dart SDK from Google's archive"
        );
        assert!(
            body.contains("dartsdk-"),
            "{script} must download a dartsdk-<platform>-release.zip"
        );
        // The comments explain why the snapshot alone is not enough, so forbid
        // the act of copying it rather than every mention of its name.
        for copied in ["snapshots/analysis_server", "snapshots\\analysis_server"] {
            assert!(
                !body.contains(copied),
                "{script} still copies a version-locked snapshot instead of the whole SDK"
            );
        }
        assert!(
            !body.contains("exec \"$DART_BIN\" \"$@\""),
            "{script} must not install a wrapper around the build machine's dart"
        );
    }
}

/// Each downloader must ask for its own platform's SDK.
#[test]
fn dart_downloaders_request_the_right_platform() {
    for (script, platform) in [
        ("scripts/linux-x86_64/download-lsp-servers.sh", "linux-x64"),
        (
            "scripts/linux-aarch64/download-lsp-servers.sh",
            "linux-arm64",
        ),
        ("scripts/macos-x86_64/download-lsp-servers.sh", "macos-x64"),
        (
            "scripts/macos-aarch64/download-lsp-servers.sh",
            "macos-arm64",
        ),
        (
            "scripts/windows-x86_64/download-lsp-servers.ps1",
            "windows-x64",
        ),
    ] {
        let body = read(script);
        assert!(
            body.contains(platform),
            "{script} must request the {platform} Dart SDK"
        );
    }
}

/// A truncated or tampered 200 MB download must never be unpacked and published,
/// nor one whose published checksum could not be fetched.
#[test]
fn dart_downloaders_verify_the_published_checksum() {
    for script in DOWNLOADERS {
        let body = read(script);
        assert!(
            body.contains(".sha256sum"),
            "{script} must verify the SDK archive against its published SHA-256"
        );
        let fails_closed = if script.ends_with(".ps1") {
            "if (-not $expected -or"
        } else {
            "if [ -z \"$DART_SUM_EXPECTED\" ] ||"
        };
        assert!(
            body.contains(fails_closed),
            "{script} must refuse the SDK when its published SHA-256 cannot be fetched"
        );
    }
}

/// Each downloader with the platform it packages for, as `platforms` in the
/// registry keys it.
const DOWNLOADER_PLATFORMS: &[(&str, &str)] = &[
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
    (
        "scripts/windows-x86_64/download-lsp-servers.ps1",
        "windows-x86_64",
    ),
];

/// The mirror packages the builds the registry pins for direct installs, so a
/// server is the same whether it comes from the mirror or from its publisher.
#[test]
fn downloaders_package_the_builds_the_registry_pins() {
    let registry = ServerRegistry::load().unwrap();
    let jdtls = &registry.get_server("jdtls").unwrap().source;
    let jdtls_version = jdtls.version.as_deref().unwrap();
    let jdtls_url = jdtls.url.as_deref().unwrap();
    let kotlin = &registry.get_server("kotlin-lsp").unwrap().source;
    let kotlin_version = kotlin.version.as_deref().unwrap();
    let kotlin_url = kotlin.url.as_deref().unwrap();
    let dart = &registry.get_server("dart-ls").unwrap().source;
    let dart_version = dart.version.as_deref().unwrap();

    for (script, platform) in DOWNLOADER_PLATFORMS {
        let body = read(script);
        let powershell = script.ends_with(".ps1");
        let (jdtls_variable, jdtls_assignment, kotlin_variable, kotlin_assignment, dart_default) =
            if powershell {
                (
                    "$jdtlsVer",
                    format!("$jdtlsVer = \"{jdtls_version}\""),
                    "$kotlinVer",
                    format!("$kotlinVer = \"{kotlin_version}\""),
                    format!("$dartVersion = \"{dart_version}\""),
                )
            } else {
                (
                    "${JDTLS_VERSION}",
                    format!("JDTLS_VERSION=\"{jdtls_version}\""),
                    "${KOTLIN_LSP_VERSION}",
                    format!("KOTLIN_LSP_VERSION=\"{kotlin_version}\""),
                    format!("DART_SDK_VERSION=\"${{DART_SDK_VERSION:-{dart_version}}}\""),
                )
            };
        assert!(
            body.contains(&jdtls_assignment),
            "{script} must package jdtls {jdtls_version}, the registry's"
        );
        let url = jdtls_url.replace("{version}", jdtls_variable);
        assert!(body.contains(&url), "{script} must download {url}");

        assert!(
            body.contains(&kotlin_assignment),
            "{script} must package the Kotlin LSP {kotlin_version}, the registry's"
        );
        let url = kotlin_url
            .replace("{version}", kotlin_variable)
            .replace("{platform}", &kotlin.platforms[*platform].name);
        assert!(body.contains(&url), "{script} must download {url}");

        assert!(
            body.contains(&dart_default),
            "{script} must package Dart SDK {dart_version}, the registry's, by default"
        );
    }
}

/// jdtls and the Kotlin LSP are checked against the SHA-256 their publishers
/// put beside them, and nothing is packaged without it.
#[test]
fn jdtls_and_the_kotlin_lsp_are_verified_when_packaged() {
    for script in DOWNLOADERS {
        let body = read(script);
        let verified = if script.ends_with(".ps1") {
            [
                "Get-Verified \"https://download.eclipse.org/jdtls/",
                "Get-Verified \"https://download.jetbrains.com/",
                "if (-not $expected -or ($expected -ne $actual))",
            ]
        } else {
            [
                "fetch_verified \"$JDTLS_URL\"",
                "fetch_verified \"$KOTLIN_LSP_URL\"",
                "if [ -z \"$expected\" ] || [ \"$expected\" != \"$actual\" ]",
            ]
        };
        for needle in verified {
            assert!(body.contains(needle), "{script} must contain `{needle}`");
        }
        assert!(
            !body.contains("jdt-language-server-latest"),
            "{script} must not fall back to an unpinned jdtls"
        );
    }
}

/// The registry drives the launch, so `dart-ls` must point at the SDK's own
/// `dart` and ask it for `language-server` — no launcher script in between.
#[test]
fn registry_launches_dart_ls_from_the_bundled_sdk() {
    let registry = ServerRegistry::load().unwrap();
    let dart = registry.get_server("dart-ls").expect("dart-ls in registry");

    assert_eq!(dart.bin_path, "bin/dart");
    assert_eq!(dart.args, vec!["language-server".to_string()]);
    assert_eq!(
        dart.source.source_type,
        SourceType::Download,
        "dart-ls is downloaded, not borrowed from the system"
    );
    let prerequisites = get_prerequisites("dart-ls");
    assert!(
        prerequisites.is_empty(),
        "a self-contained SDK requires nothing of the host, got {prerequisites:?}"
    );
}

/// `clangd` bakes the absolute path of its resource directory (the builtin
/// `stddef.h`, `stdarg.h`, intrinsics) in at build time, so the headers must
/// travel with the binary, and clangd must be *told* to use them — shipping
/// them next to the binary is silently ignored. The binary itself comes from
/// clangd's release, for each platform it is published for, not from PATH,
/// where `clangd` can be swiftly's proxy on Linux and is Apple's shim into the
/// Command Line Tools on macOS.
#[test]
fn clangd_ships_from_its_release_and_is_pointed_at_its_resource_dir() {
    let registry = ServerRegistry::load().unwrap();
    let clangd = registry.get_server("clangd").expect("clangd in registry");
    for (script, platform) in DOWNLOADER_PLATFORMS {
        let body = read(script);
        if !clangd.source.published_for.iter().any(|p| p == platform) {
            assert!(
                body.contains("rm -rf \"${LSP_DIR:?}/clangd\""),
                "{script} must remove an earlier download's clangd: clangd publishes no {platform} build"
            );
            continue;
        }
        let fetch = if script.ends_with(".ps1") {
            "Get-ReleaseAsset \"clangd/clangd\""
        } else {
            "fetch_release_asset clangd/clangd"
        };
        assert!(body.contains(fetch), "{script} must fetch clangd's release");
        assert!(
            body.contains("resource-dir")
                && (body.contains("lib/clang") || body.contains("lib\\clang")),
            "{script} must ship the release's lib/clang/<major>/include as resource-dir"
        );
        for copied in [
            "command -v clangd",
            "Get-Command clangd",
            "-print-resource-dir",
        ] {
            assert!(
                !body.contains(copied),
                "{script} still copies the clangd on PATH (`{copied}`)"
            );
        }
    }
    assert_eq!(
        clangd.args,
        vec![
            "--resource-dir={server_dir}/resource-dir".to_string(),
            // The database Devscriptor writes for a project without one.
            "--compile-commands-dir={compile_commands_dir}".to_string(),
        ],
        "clangd must be launched against the resource directory shipped beside it"
    );
}

/// What a downloader takes from a GitHub release is checked against the
/// SHA-256 GitHub publishes for the asset, as Devscriptor's own install is —
/// rust-analyzer's and phpactor's included. phpactor is not packaged for
/// Windows, where PHP has no posix extension, which phpactor needs.
#[test]
fn release_assets_are_verified_when_packaged() {
    for script in DOWNLOADERS {
        let body = read(script);
        let (fetch, definition, verified) = if script.ends_with(".ps1") {
            (
                "Get-ReleaseAsset",
                "function Get-ReleaseAsset(",
                "if (-not $expected -or ($expected -ne $actual)) {",
            )
        } else {
            (
                "fetch_release_asset",
                "fetch_release_asset() {",
                "if [ -z \"$expected\" ] || [ \"$expected\" != \"$actual\" ]; then",
            )
        };
        let start = body
            .find(definition)
            .unwrap_or_else(|| panic!("{script} defines no {fetch}"));
        let function = &body[start..start + body[start..].find("\n}\n").unwrap()];
        assert!(
            function.contains("digest") && function.contains(verified),
            "{script}: {fetch} must check the SHA-256 GitHub publishes"
        );
        let windows = script.ends_with(".ps1");
        for repo in ["rust-lang/rust-analyzer", "phpactor/phpactor"] {
            let fetched = body
                .lines()
                .any(|line| line.contains(fetch) && line.contains(repo));
            let wanted = !(windows && repo == "phpactor/phpactor");
            assert_eq!(fetched, wanted, "{script}: {fetch} of {repo}'s release");
        }
        assert!(
            !body.contains("browser_download_url\": \""),
            "{script} still scrapes a download URL out of the release JSON"
        );
    }
}

/// `~/.cargo/bin/rust-analyzer` is a rustup proxy, and the toolchain's own
/// binary links against `librustc_driver-*.so` from inside that toolchain.
/// Neither survives being copied, so the upstream `*-standalone` asset is the
/// only shippable rust-analyzer.
#[test]
fn rust_analyzer_comes_from_the_upstream_standalone_release() {
    for script in DOWNLOADERS {
        let body = read(script);
        assert!(
            body.contains("rust-lang/rust-analyzer"),
            "{script} must download rust-analyzer from its GitHub releases"
        );
        // The comments explain why the local toolchain is unusable, so match on
        // code that would actually source it rather than on prose.
        for stale in [
            "rustup component add rust-analyzer",
            "find_rust_analyzer",
            "Find-RustAnalyzer",
        ] {
            assert!(
                !body.contains(stale),
                "{script} still sources rust-analyzer from the local toolchain (`{stale}`)"
            );
        }
    }
}

/// macOS may refuse to execute a freshly downloaded Mach-O whose signature it
/// cannot assess, so the macOS downloaders must make the binary runnable
/// before they verify it — otherwise the build fails with "the downloaded
/// binary does not run". Linux has no such policy and must not carry the step.
#[test]
fn macos_makes_the_downloaded_rust_analyzer_runnable() {
    for script in [
        "scripts/macos-x86_64/download-lsp-servers.sh",
        "scripts/macos-aarch64/download-lsp-servers.sh",
    ] {
        let macos = read(script);
        assert!(
            macos.contains("com.apple.quarantine"),
            "{script} must de-quarantine the rust-analyzer binary"
        );
        assert!(
            macos.contains("codesign --force --sign -"),
            "{script} must ad-hoc re-sign the binary when it will not run"
        );
    }

    for linux in [
        "scripts/linux-x86_64/download-lsp-servers.sh",
        "scripts/linux-aarch64/download-lsp-servers.sh",
    ] {
        let body = read(linux);
        assert!(
            !body.contains("codesign"),
            "{linux} must not carry the macOS-only code-signing step"
        );
    }
}

/// The Dart archive's `.sha256sum` sidecar is served as
/// application/octet-stream, so Windows PowerShell hands `.Content` back as a
/// Byte[]. Splitting that would stringify it to byte *values* — the checksum
/// parse would yield "55" (decimal for the hash's leading '7') and every
/// Windows build would fail with a bogus "checksum mismatch". The sidecar must
/// be fetched to a file and read as text instead of `.Content`-split.
#[test]
fn windows_parses_the_dart_checksum_as_text_not_content_bytes() {
    let win = read("scripts/windows-x86_64/download-lsp-servers.ps1");
    assert!(
        !win.contains(".Content -split"),
        "the Windows downloader must not split Invoke-WebRequest .Content \
         (it is a Byte[] for octet-stream responses)"
    );
    assert!(
        win.contains("Get-Content $sumPath -Raw"),
        "the Windows downloader must read the .sha256sum sidecar from a file as text"
    );
}

/// Each server a package manager installs is packaged at the build the
/// registry pins, with the companions it lists, so a server is the same
/// whether it comes from the mirror or from its package manager.
#[test]
fn downloaders_install_the_builds_the_registry_pins() {
    let registry = ServerRegistry::load().unwrap();
    for script in DOWNLOADERS {
        let body = read(script);
        let powershell = script.ends_with(".ps1");
        for info in registry.servers.values() {
            let source = &info.source;
            let version = source.version.as_deref().unwrap_or_default();
            let package = source.package.as_deref().unwrap_or_default();
            let call = match (&source.source_type, powershell) {
                (SourceType::Npm, false) => {
                    let mut call =
                        format!("download_npm \"{}\" \"{package}\" \"{version}\"", info.id);
                    for companion in &source.companions {
                        call.push_str(&format!(" \"{companion}\""));
                    }
                    call
                }
                (SourceType::Npm, true) => {
                    let mut call =
                        format!("Download-Npm \"{}\" \"{package}\" \"{version}\"", info.id);
                    if !source.companions.is_empty() {
                        let companions: Vec<String> = source
                            .companions
                            .iter()
                            .map(|c| format!("\"{c}\""))
                            .collect();
                        call.push_str(&format!(" @({})", companions.join(", ")));
                    }
                    call
                }
                (SourceType::DotnetTool, false) => {
                    format!(
                        "download_dotnet \"{}\" \"{package}\" \"{version}\"",
                        info.id
                    )
                }
                (SourceType::DotnetTool, true) => {
                    format!(
                        "Download-Dotnet \"{}\" \"{package}\" \"{version}\"",
                        info.id
                    )
                }
                (SourceType::Coursier, false) => format!("METALS_VERSION=\"{version}\""),
                (SourceType::Coursier, true) => format!("$metalsVer = \"{version}\""),
                (SourceType::Gem, false) => format!("RUBY_LSP_VERSION=\"{version}\""),
                (SourceType::Gem, true) => format!("$RubyLspVersion = \"{version}\""),
                _ => continue,
            };
            assert!(
                body.lines().any(|line| line.trim() == call),
                "{script} must install {} as `{call}`",
                info.id
            );
            if source.source_type == SourceType::Coursier {
                let coordinates = if powershell {
                    format!("\"{package}:$metalsVer\"")
                } else {
                    format!("\"{package}:$METALS_VERSION\"")
                };
                assert!(
                    body.contains(&format!("bootstrap --standalone {coordinates}")),
                    "{script} must bootstrap {coordinates}, holding its libraries"
                );
                let jar = if powershell {
                    "(Test-FileHolds (Join-Path $metalsDir \"bin\\metals\") $metalsJar)"
                } else {
                    "grep -aqF \"$METALS_JAR\" \"$LSP_DIR/metals/bin/metals\""
                };
                assert!(
                    body.contains(jar),
                    "{script} must replace a launcher that fetches its libraries"
                );
            }
        }
    }
}

/// sourcekit-lsp comes with the Swift toolchain (`system` in the registry) and
/// runs only inside it, so no downloader copies it into a package: each removes
/// an earlier download's copy instead, which the packager would ship.
#[test]
fn downloaders_leave_the_toolchains_server_to_the_toolchain() {
    let registry = ServerRegistry::load().unwrap();
    let system: Vec<&str> = registry
        .servers
        .values()
        .filter(|s| s.source.source_type == SourceType::System)
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(system, ["sourcekit-lsp"]);

    for script in DOWNLOADERS {
        let body = read(script);
        let removal = if script.ends_with(".ps1") {
            "$sourcekitDir = Join-Path $LspDir \"sourcekit-lsp\"\n\
             if (Test-Path $sourcekitDir) { Remove-Item -Recurse -Force $sourcekitDir }"
        } else {
            "rm -rf \"${LSP_DIR:?}/sourcekit-lsp\""
        };
        assert!(
            body.contains(removal),
            "{script} must remove an earlier download's sourcekit-lsp"
        );
        for copy in [
            "SOURCEKIT_BIN",
            "SKLSP",
            "Find-SourceKitLsp",
            "Copy-SystemExe",
        ] {
            assert!(
                !body.contains(copy),
                "{script} still copies sourcekit-lsp (`{copy}`)"
            );
        }
    }
}

/// Packaging keys the manifest by registry id, which names each server's
/// directory: no packager maps another directory name onto one (`dart` onto
/// `dart-ls`), which would hide a directory misnamed for its server.
#[test]
fn packaging_scripts_carry_no_stale_dart_alias() {
    for script in [
        "scripts/linux-x86_64/package-lsp-servers.sh",
        "scripts/linux-aarch64/package-lsp-servers.sh",
        "scripts/macos-x86_64/package-lsp-servers.sh",
        "scripts/macos-aarch64/package-lsp-servers.sh",
        "scripts/windows-x86_64/package-lsp-servers.ps1",
    ] {
        let body = read(script);
        assert!(
            !body.contains("dart) echo \"dart-ls\"")
                && !body.contains("\"dart\" { return \"dart-ls\" }"),
            "{script} still aliases the `dart` directory to `dart-ls`"
        );
    }
}

/// Guard the assumption the whole scheme rests on: ids are safe path components.
#[test]
fn registry_ids_are_plain_directory_names() {
    let registry = ServerRegistry::load().unwrap();
    let seen: HashSet<&str> = registry.servers.keys().map(String::as_str).collect();
    assert_eq!(seen.len(), registry.servers.len(), "duplicate ids");

    for id in seen {
        assert!(
            !id.is_empty()
                && !id.contains('/')
                && !id.contains('\\')
                && !id.contains("..")
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "`{id}` is not a safe directory name"
        );
    }
}

/// The Dart SDK records its build in `.version`, as every pinned build does,
/// which is what the packagers and the outdated check read.
#[test]
fn the_dart_sdk_records_its_build_where_the_others_do() {
    for script in DOWNLOADERS {
        let body = read(script);
        let record = if script.ends_with(".ps1") {
            "Set-Content -Path (Join-Path $target \".version\") -Value $dartVersion"
        } else {
            "> \"$LSP_DIR/dart-ls/.version\""
        };
        assert!(
            body.contains(record),
            "{script} must record the SDK in .version"
        );
        assert!(!body.contains("VERSION\") -Value") && !body.contains("dart-ls/VERSION"));
    }
}

/// A ruby-lsp bundle is kept only when it was built for this machine's Ruby
/// (`a_bundle_built_for_another_ruby_is_replaced` runs the Linux one), and on
/// macOS the Ruby is found before the bundle is checked against it: macOS's
/// own 2.6 is first on PATH there.
#[test]
fn a_ruby_lsp_bundle_is_checked_against_this_ruby() {
    for script in DOWNLOADERS {
        let body = read(script);
        let check = if script.ends_with(".ps1") {
            "(Test-BuiltForThisRuby $RubyLspDir)"
        } else {
            "built_for_this_ruby \"$LSP_DIR/ruby-lsp\""
        };
        assert!(
            body.contains(check),
            "{script} must check the bundle's Ruby"
        );
    }
    for script in [
        "scripts/macos-x86_64/download-lsp-servers.sh",
        "scripts/macos-aarch64/download-lsp-servers.sh",
    ] {
        let macos = read(script);
        let ruby = macos.find("if ! ensure_modern_ruby; then").unwrap();
        let check = macos
            .find("built_for_this_ruby \"$LSP_DIR/ruby-lsp\"")
            .unwrap();
        assert!(ruby < check, "{script}");
    }
}
