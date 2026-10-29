//! Server registry - Loads and manages LSP server metadata

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Registry of available LSP servers
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerRegistry {
    pub servers: HashMap<String, ServerInfo>,
}

impl ServerRegistry {
    /// Load the embedded registry
    pub fn load() -> Result<Self> {
        let registry_json = include_str!("registry.json");
        let registry: ServerRegistry = serde_json::from_str(registry_json)
            .context("Failed to parse embedded server registry")?;
        Ok(registry)
    }

    /// Get a server by ID
    #[must_use]
    pub fn get_server(&self, id: &str) -> Option<&ServerInfo> {
        self.servers.get(id)
    }

    /// The embedded registry's entry for `id`, from the registry loaded once
    /// per process.
    #[must_use]
    pub fn entry(id: &str) -> Option<&'static ServerInfo> {
        static REGISTRY: std::sync::OnceLock<Option<ServerRegistry>> = std::sync::OnceLock::new();
        REGISTRY
            .get_or_init(|| Self::load().ok())
            .as_ref()?
            .get_server(id)
    }

    /// List all available servers
    #[must_use]
    pub fn list_servers(&self) -> Vec<&ServerInfo> {
        self.servers.values().collect()
    }

    /// Find servers that support a specific language
    #[must_use]
    pub fn find_by_language(&self, language: &str) -> Option<&ServerInfo> {
        let lang = language.to_lowercase();
        self.servers
            .values()
            .find(|s| s.languages.iter().any(|l| l.eq_ignore_ascii_case(&lang)))
    }

    /// Search servers by name or language
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&ServerInfo> {
        let query_lower = query.to_lowercase();
        self.servers
            .values()
            .filter(|s| {
                s.id.to_lowercase().contains(&query_lower)
                    || s.name.to_lowercase().contains(&query_lower)
                    || s.languages
                        .iter()
                        .any(|l| l.to_lowercase().contains(&query_lower))
            })
            .collect()
    }
}

/// Information about an LSP server
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ServerInfo {
    pub id: String,
    pub name: String,
    pub languages: Vec<String>,
    pub source: SourceInfo,
    pub size_mb: u32,
    pub bin_path: String,
    pub license: String,

    /// The server's license files, which the packaging scripts put in its
    /// mirror archive, under `LICENSES/`.
    #[serde(default)]
    pub license_files: Vec<LicenseFile>,

    /// Arguments that make the server speak LSP over stdio.
    ///
    /// Most servers need one (`--stdio`, `start`, `language-server`, …) and stay
    /// silent without it.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment the server needs in order to find itself.
    ///
    /// `ruby-lsp` is a gem binstub and resolves its own gems only when `GEM_HOME`
    /// points at the install directory.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// LSP `initializationOptions` the server requires.
    ///
    /// `typescript-language-server` and `volar` both refuse to initialize unless
    /// told where a TypeScript installation lives.
    #[serde(default)]
    pub initialization_options: Option<serde_json::Value>,

    /// Per-request timeout, for servers whose first request follows a slow project
    /// load. `csharp-ls` and `fsautocomplete` restore a .NET project before they
    /// answer anything, which comfortably exceeds the 30 s default.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Where one of a server's license files comes from: the server's own files,
/// where it ships one, else its publisher's repository.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LicenseFile {
    /// The file in the installed server, as a glob for a directory named by
    /// a version (`gems/ruby-lsp-[0-9]*/LICENSE.txt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The file at a fixed commit of the publisher's repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The SHA-256 the file at `url` must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl ServerInfo {
    /// The version a `download` source pins: the one build its launch
    /// settings (`args`, `bin_path`) are written for. A build of another
    /// version — the mirror's, or one installed for an earlier registry — may
    /// not start with them: jdtls 1.44.0 cannot run on Java 25, which the
    /// pinned 1.61.0 can. A package manager's version only names what its
    /// fallback installs, so it pins nothing.
    #[must_use]
    pub fn pinned_build(&self) -> Option<&str> {
        match self.source.source_type {
            SourceType::Download => self
                .source
                .version
                .as_deref()
                .filter(|version| *version != "latest"),
            _ => None,
        }
    }

    /// Whether `recorded`, the version an install records in its `.version`,
    /// is outdated: another build than the one this server pins. An install
    /// that records none (one placed by hand) is taken as it is.
    #[must_use]
    pub fn outdated_by(&self, recorded: Option<&str>) -> bool {
        matches!((self.pinned_build(), recorded), (Some(pin), Some(version)) if version.trim() != pin)
    }
}

/// Placeholder substituted with `<install_root>/<server_id>` inside `args`,
/// `env` and `initialization_options`, so the registry can reference files
/// shipped alongside a server without hard-coding an absolute path.
pub const SERVER_DIR_PLACEHOLDER: &str = "{server_dir}";

/// Placeholder substituted with Devscriptor's cache directory
/// (`~/.cache/devscriptor`) in the same places, for state a server keeps
/// between runs that belongs in no install of it: the Kotlin LSP's caches and
/// indexes, which it would otherwise leave in a new temporary directory at
/// every start.
pub const CACHE_DIR_PLACEHOLDER: &str = "{cache_dir}";

/// What the registry's placeholders stand for in one launch.
#[derive(Debug, Clone, Copy)]
pub struct Placeholders<'a> {
    /// The value of [`SERVER_DIR_PLACEHOLDER`].
    pub server_dir: &'a str,
    /// The value of [`CACHE_DIR_PLACEHOLDER`].
    pub cache_dir: &'a str,
}

impl Placeholders<'_> {
    /// `text` with every placeholder replaced.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        text.replace(SERVER_DIR_PLACEHOLDER, self.server_dir)
            .replace(CACHE_DIR_PLACEHOLDER, self.cache_dir)
    }

    /// `value` with every placeholder replaced throughout.
    #[must_use]
    pub fn apply_json(&self, value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::String(s) => serde_json::Value::String(self.apply(s)),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|v| self.apply_json(v)).collect())
            }
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.apply_json(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Source information for downloading a server
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SourceInfo {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub repo: Option<String>,
    pub package: Option<String>,
    pub url: Option<String>,
    /// The pinned version (a GitHub release's tag), or none or `latest` for
    /// the latest release.
    pub version: Option<String>,
    pub asset_pattern: Option<String>,
    /// Expected SHA-256 checksum of the downloaded artifact (hex-encoded),
    /// where one artifact serves every platform.
    #[serde(default)]
    pub expected_sha256: Option<String>,
    /// A download's build for each platform it is published for, keyed
    /// `<os>-<arch>` (`linux-x86_64`, `macos-aarch64`, `windows-x86_64`, …),
    /// where the artifact differs by platform.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub platforms: HashMap<String, PlatformBuild>,
    /// The platforms a GitHub release has a build for, keyed as `platforms`
    /// is, where it does not have one for each: clangd's has none for Linux
    /// ARM, and its `clangd-linux` zip is an x86_64 build. Empty is every
    /// platform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub published_for: Vec<String>,
    /// npm packages installed beside the server (`name@version`), which it
    /// needs but does not depend on: typescript-language-server and volar
    /// refuse to start without a TypeScript to point at, and
    /// sql-language-server reaches into a `vscode-languageserver-protocol`
    /// that newer versions hide behind an `exports` map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub companions: Vec<String>,
}

/// One platform's build of a download.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PlatformBuild {
    /// What `{platform}` in the URL becomes: the part of the file name the
    /// publisher gives this platform's build (`linux-x64` in the Dart SDK's
    /// `dartsdk-linux-x64-release.zip`, `-aarch64.tar.gz` in the Kotlin LSP's
    /// `kotlin-server-<version>-aarch64.tar.gz`).
    pub name: String,
    /// SHA-256 of the build (hex-encoded).
    pub sha256: String,
}

/// Type of source for an LSP server
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceType {
    GithubRelease,
    Npm,
    Pip,
    GoInstall,
    DotnetTool,
    Gem,
    Coursier,
    Download,
    System,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every packaged server names its license files, each either a file of
    /// the installed server or one at a commit of its publisher's repository
    /// with the SHA-256 it must match, under a name no other of its files
    /// has; a server that comes with its toolchain is not packaged.
    #[test]
    fn every_packaged_server_names_its_license_files() {
        let hex = |s: &str, len: usize| {
            s.len() == len
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let registry = ServerRegistry::load().unwrap();
        for server in registry.list_servers() {
            let id = &server.id;
            if server.source.source_type == SourceType::System {
                assert!(server.license_files.is_empty(), "{id}");
                continue;
            }
            assert!(
                !server.license_files.is_empty(),
                "{id} names no license file"
            );
            let mut names = Vec::new();
            for file in &server.license_files {
                let name = match (&file.path, &file.url, &file.sha256) {
                    (Some(path), None, None) => {
                        assert!(
                            !path.starts_with('/')
                                && !path.contains('\\')
                                && !path.split('/').any(|part| part == ".."),
                            "{id}: {path}"
                        );
                        path.rsplit('/').next().unwrap()
                    }
                    (None, Some(url), Some(sha256)) => {
                        let commit = url
                            .strip_prefix("https://raw.githubusercontent.com/")
                            .and_then(|rest| rest.split('/').nth(2))
                            .unwrap_or_default();
                        assert!(hex(commit, 40), "{id}: {url} is not at a commit");
                        assert!(hex(sha256, 64), "{id}: {sha256}");
                        url.rsplit('/').next().unwrap()
                    }
                    _ => panic!("{id}: a license file has a path, or a url and its sha256"),
                };
                assert!(!name.is_empty() && !name.contains('*'), "{id}: {name}");
                names.push(name);
            }
            let count = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), count, "{id}: two license files of one name");
        }
    }

    /// Every registry server must declare how to launch it.
    ///
    /// Without `args`, the 14 servers that wait for `--stdio`, `start`,
    /// `language-server` or similar would start but never speak LSP.
    #[test]
    fn every_server_declares_a_launch_spec() {
        let registry = ServerRegistry::load().unwrap();
        assert_eq!(registry.servers.len(), 21);

        // Servers that speak stdio with no arguments at all.
        let no_args = [
            "rust-analyzer",
            "gopls",
            "metals",
            "csharp-ls",
            "fsautocomplete",
            "sourcekit-lsp",
            "ruby-lsp",
        ];
        for s in registry.list_servers() {
            assert!(!s.bin_path.is_empty(), "{}: bin_path", s.id);
            if !no_args.contains(&s.id.as_str()) {
                assert!(
                    !s.args.is_empty(),
                    "{} must declare the args that put it in stdio mode",
                    s.id
                );
            }
        }
    }

    /// The servers that cannot start without extra configuration.
    #[test]
    fn servers_needing_configuration_declare_it() {
        let r = ServerRegistry::load().unwrap();

        // Both TypeScript-backed servers refuse to initialize without a tsdk.
        for id in ["typescript-language-server", "volar"] {
            let opts = r.get_server(id).unwrap().initialization_options.as_ref();
            assert!(opts.is_some(), "{id} needs initializationOptions");
            assert!(
                serde_json::to_string(opts.unwrap())
                    .unwrap()
                    .contains("{server_dir}"),
                "{id} should locate TypeScript relative to its own install dir"
            );
        }

        // FSAC answers "Couldn't find <file>" until the workspace is loaded.
        let fsac = r.get_server("fsautocomplete").unwrap();
        assert_eq!(
            fsac.initialization_options.as_ref().unwrap()["AutomaticWorkspaceInit"],
            serde_json::Value::Bool(true)
        );

        // ruby-lsp is a gem binstub; it finds its gems only via GEM_HOME.
        let ruby = r.get_server("ruby-lsp").unwrap();
        assert_eq!(
            ruby.env.get("GEM_HOME").map(String::as_str),
            Some("{server_dir}")
        );

        // .NET servers load a project before answering; 30s is not enough.
        // Each runs on a newer .NET than it targets where its own is not
        // installed: csharp-ls 0.16.0 is a .NET 8 tool, and on a machine with
        // .NET 10 alone it would say "You must install or update .NET".
        for id in ["csharp-ls", "fsautocomplete"] {
            let server = r.get_server(id).unwrap();
            assert!(
                server.timeout_seconds.unwrap_or(30) > 30,
                "{id} needs a longer request timeout"
            );
            assert_eq!(
                server.env.get("DOTNET_ROLL_FORWARD").map(String::as_str),
                Some("Major"),
                "{id}"
            );
        }

        // jdtls writes `.project`, `.classpath` and `.settings/` into the
        // project unless told at JVM start to keep them in its workspace.
        let jdtls = r.get_server("jdtls").unwrap();
        assert!(jdtls.args.contains(
            &"--jvm-arg=-Djava.import.generatesMetadataFilesAtProjectRoot=false".to_string()
        ));

        // The Kotlin LSP keeps caches and indexes in a new temporary directory
        // at every start, and leaves it, unless given one to reuse.
        let kotlin = r.get_server("kotlin-lsp").unwrap();
        assert_eq!(
            kotlin.args,
            vec!["--stdio", "--system-path={cache_dir}/kotlin-lsp"]
        );

        // rust-analyzer builds in `target/rust-analyzer`: in the project's own
        // `target/` it holds Cargo's lock and blocks the user's `cargo build`.
        let rust = r.get_server("rust-analyzer").unwrap();
        assert_eq!(
            rust.initialization_options.as_ref().unwrap()["cargo"]["targetDir"],
            serde_json::Value::Bool(true)
        );
    }

    #[test]
    fn placeholders_are_substituted_everywhere() {
        let placeholders = Placeholders {
            server_dir: "/opt/ds/volar",
            cache_dir: "/home/u/.cache/devscriptor",
        };
        let v = serde_json::json!({
            "typescript": {"tsdk": "{server_dir}/node_modules/typescript/lib"},
            "list": ["{server_dir}/x", 1, "{cache_dir}/state"],
        });
        let out = placeholders.apply_json(&v);
        assert_eq!(
            out["typescript"]["tsdk"],
            "/opt/ds/volar/node_modules/typescript/lib"
        );
        assert_eq!(out["list"][0], "/opt/ds/volar/x");
        assert_eq!(out["list"][1], 1);
        assert_eq!(out["list"][2], "/home/u/.cache/devscriptor/state");
        assert_eq!(
            placeholders.apply("--system-path={cache_dir}/kotlin-lsp"),
            "--system-path=/home/u/.cache/devscriptor/kotlin-lsp"
        );
    }

    #[test]
    fn test_load_registry() {
        let registry = ServerRegistry::load().unwrap();

        // Check that we have some servers
        assert!(!registry.servers.is_empty());

        // Check for some expected servers
        assert!(registry.get_server("rust-analyzer").is_some());
        assert!(registry.get_server("typescript-language-server").is_some());
        assert!(registry.get_server("pyright").is_some());
    }

    #[test]
    fn test_find_by_language() {
        let registry = ServerRegistry::load().unwrap();

        let rust_server = registry.find_by_language("rust");
        assert!(rust_server.is_some());
        assert_eq!(rust_server.unwrap().name, "Rust Analyzer");

        let python_server = registry.find_by_language("python");
        assert!(python_server.is_some());
    }

    #[test]
    fn test_search() {
        let registry = ServerRegistry::load().unwrap();

        let results = registry.search("rust");
        assert!(!results.is_empty());

        let results = registry.search("typescript");
        assert!(!results.is_empty());
    }

    /// The bridge routes files by `generated_registry.rs`, which is kept in
    /// step with `registry.json` by hand (its generator is not in this
    /// repository): both must name the same extensions and project markers.
    #[test]
    fn generated_registry_matches_registry_json() {
        use crate::generated_registry;

        let json: serde_json::Value = serde_json::from_str(include_str!("registry.json")).unwrap();
        let servers = json["servers"].as_object().unwrap();
        assert_eq!(servers.len(), generated_registry::SERVER_IDS.len());
        let strings = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        for id in generated_registry::SERVER_IDS {
            let entry = &servers[*id];
            let extensions: Vec<String> = generated_registry::extensions(id)
                .unwrap()
                .iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(
                strings(&entry["extensions"]),
                extensions,
                "{id}: extensions"
            );

            let heuristics = generated_registry::heuristics(id);
            match entry.get("heuristics") {
                Some(h) => {
                    let generated = heuristics.unwrap_or_else(|| panic!("{id}: heuristics"));
                    assert_eq!(strings(&h["markers"]), generated.markers, "{id}: markers");
                    assert_eq!(
                        h["max_depth"].as_u64(),
                        Some(generated.max_depth as u64),
                        "{id}: max_depth"
                    );
                }
                None => assert!(heuristics.is_none(), "{id}: no heuristics in the registry"),
            }
        }
    }

    /// Each server is routed only the files it serves: csharp-ls no VB.NET
    /// files, which it never answers (a request for one would wait out the
    /// request timeout), and clangd Objective-C++ as well as C and C++.
    #[test]
    fn servers_are_routed_the_languages_they_serve() {
        use crate::generated_registry::extensions;

        let csharp = extensions("csharp-ls").unwrap();
        assert!(!csharp.contains(&"vb"), "csharp-ls serves C# only");
        let clangd = extensions("clangd").unwrap();
        // `.m` is MATLAB's too: the bridge sends clangd only an Objective-C one.
        for ext in ["c", "h", "cpp", "cc", "cxx", "hpp", "cu", "cuh", "mm", "m"] {
            assert!(clangd.contains(&ext), "clangd should take .{ext}");
        }

        let registry = ServerRegistry::load().unwrap();
        assert!(registry.find_by_language("vb").is_none());
        assert_eq!(
            registry
                .find_by_language("objective-cpp")
                .map(|s| s.id.as_str()),
            Some("clangd")
        );
        let clangd = registry.get_server("clangd").unwrap();
        assert!(clangd.languages.iter().any(|l| l == "objective-c"));
    }

    /// A `download` source pins one build; an install that records another
    /// is outdated, one that records none is taken as it is, and a package
    /// manager's version pins nothing.
    #[test]
    fn an_install_of_another_build_than_the_pinned_one_is_outdated() {
        let registry = ServerRegistry::load().unwrap();
        let jdtls = registry.get_server("jdtls").unwrap();
        let pin = jdtls.pinned_build().expect("jdtls pins its build");
        assert!(jdtls.outdated_by(Some("1.44.0")));
        assert!(!jdtls.outdated_by(Some(pin)));
        assert!(!jdtls.outdated_by(Some(&format!("{pin}\n"))));
        assert!(!jdtls.outdated_by(None), "an install placed by hand");

        let pyright = registry.get_server("pyright").unwrap();
        assert_eq!(pyright.pinned_build(), None);
        assert!(!pyright.outdated_by(Some("0.1.0")));
    }
}
