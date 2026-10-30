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

    /// Whether the server's builds stop starting some weeks after their
    /// release: JetBrains' Kotlin LSP is a pre-release that expires. Such a
    /// server tracks its latest build (its `version` is `latest`), is not
    /// mirrored, where an expired build would go on being served, and is
    /// brought up to its latest build when it is installed again.
    #[serde(default)]
    pub builds_expire: bool,

    pub size_mb: u32,
    pub bin_path: String,
    pub license: String,

    /// The server's license files, which the packaging scripts put in its
    /// mirror archive, under `LICENSES/`. A server that is not mirrored
    /// ([`Self::is_mirrored`]) names none.
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

    /// Directories put before the `PATH` the server is started with, where
    /// it finds the tools installed beside it ([`SourceInfo::companions`]):
    /// FsAutoComplete formats with Fantomas, which it looks for there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,

    /// Settings the server takes from its client, sent in
    /// `workspace/didChangeConfiguration` once it is initialized and given,
    /// section by section, to its `workspace/configuration` requests.
    ///
    /// Metals asks whether to import a build, a question a client without a
    /// user can only decline; with `autoImportBuilds` it imports the build
    /// without asking.
    #[serde(default)]
    pub configuration: Option<serde_json::Value>,

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
    /// The name the file is given under `LICENSES/`, where its own is that of
    /// another of the server's: Fantomas's `LICENSE.md` beside
    /// FsAutoComplete's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl LicenseFile {
    /// The name the file has under `LICENSES/`: its `name`, else that of the
    /// file at `path` or `url`.
    #[must_use]
    pub fn packaged_name(&self) -> &str {
        self.name.as_deref().unwrap_or_else(|| {
            let from = self
                .path
                .as_deref()
                .or(self.url.as_deref())
                .unwrap_or_default();
            from.rsplit('/').next().unwrap_or(from)
        })
    }
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

    /// Whether the mirror packages the server: one that comes with its
    /// toolchain, or whose builds expire, is installed from elsewhere.
    #[must_use]
    pub fn is_mirrored(&self) -> bool {
        self.source.source_type != SourceType::System && !self.builds_expire
    }

    /// Whether the server installs its publisher's latest build, which
    /// [`downloader::latest_version`](super::downloader::latest_version)
    /// finds.
    #[must_use]
    pub fn tracks_latest(&self) -> bool {
        self.source.source_type == SourceType::Download
            && self.source.version.as_deref() == Some("latest")
    }

    /// This server at `version`, which its install fetches and records: the
    /// latest build of a server that [`tracks_latest`](Self::tracks_latest).
    #[must_use]
    pub fn at_version(&self, version: &str) -> Self {
        let mut server = self.clone();
        server.source.version = Some(version.to_string());
        server
    }

    /// The first of the server's companions ([`SourceInfo::companions`]) its
    /// install in `server_dir` lacks: an npm package without its
    /// `node_modules/<name>/`, a .NET tool without its store at its version.
    /// An install made without its companions is installed again.
    #[must_use]
    pub fn lacks_companion(&self, server_dir: &std::path::Path) -> Option<&str> {
        self.source
            .companions
            .iter()
            .map(String::as_str)
            .find(|companion| {
                let Some((name, version)) = companion
                    .rsplit_once('@')
                    .filter(|(name, _)| !name.is_empty())
                else {
                    return false;
                };
                match self.source.source_type {
                    SourceType::Npm => !server_dir
                        .join("node_modules")
                        .join(name)
                        .join("package.json")
                        .is_file(),
                    SourceType::DotnetTool => !server_dir
                        .join("bin/.store")
                        .join(name.to_ascii_lowercase())
                        .join(version.to_ascii_lowercase())
                        .is_dir(),
                    _ => false,
                }
            })
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
    /// the latest release. A download that tracks its latest build finds it
    /// in `repo`'s latest release.
    pub version: Option<String>,
    /// What comes before the version in the tags of `repo`'s releases, for a
    /// download that tracks its latest build: `kotlin-lsp/v` in the Kotlin
    /// LSP's `kotlin-lsp/v263.7084.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_prefix: Option<String>,
    /// Where the publisher puts the SHA-256 of a download the registry pins
    /// none for, `{url}` being the download's own URL: JetBrains puts each
    /// Kotlin LSP build's beside it, at `{url}.sha256`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum_url: Option<String>,
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
    /// The platforms the server is installed on, keyed as `platforms` is,
    /// where it is not installed on each: those its GitHub release has a
    /// build for (clangd's has none for Linux ARM, and its `clangd-linux` zip
    /// is an x86_64 build), or that it runs on (phpactor needs PHP's posix
    /// extension, which PHP on Windows has not). Empty is every platform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub published_for: Vec<String>,
    /// Packages installed beside the server with its own package manager
    /// (`name@version`), which it needs but does not depend on: npm's
    /// typescript-language-server and volar refuse to start without a
    /// TypeScript to point at, and sql-language-server reaches into a
    /// `vscode-languageserver-protocol` that newer versions hide behind an
    /// `exports` map; the .NET tool FsAutoComplete formats with the Fantomas
    /// installed beside it, which it finds on its `PATH` (`path`).
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
    /// SHA-256 of the build (hex-encoded), where the registry pins the build;
    /// otherwise the source's `checksum_url` has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
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
    /// with the SHA-256 it must match, under a name (its own, or the one it is
    /// given) no other of its files has; a server that comes with its toolchain, or whose builds expire,
    /// is not packaged.
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
            if !server.is_mirrored() {
                assert!(server.license_files.is_empty(), "{id}");
                continue;
            }
            assert!(
                !server.license_files.is_empty(),
                "{id} names no license file"
            );
            let mut names = Vec::new();
            for file in &server.license_files {
                if let Some(name) = &file.name {
                    assert!(
                        !name.contains('/') && !name.contains('\\') && name != "..",
                        "{id}: {name}"
                    );
                }
                match (&file.path, &file.url, &file.sha256) {
                    (Some(path), None, None) => {
                        assert!(
                            !path.starts_with('/')
                                && !path.contains('\\')
                                && !path.split('/').any(|part| part == ".."),
                            "{id}: {path}"
                        );
                    }
                    (None, Some(url), Some(sha256)) => {
                        let commit = url
                            .strip_prefix("https://raw.githubusercontent.com/")
                            .and_then(|rest| rest.split('/').nth(2))
                            .unwrap_or_default();
                        assert!(hex(commit, 40), "{id}: {url} is not at a commit");
                        assert!(hex(sha256, 64), "{id}: {sha256}");
                    }
                    _ => panic!("{id}: a license file has a path, or a url and its sha256"),
                }
                let name = file.packaged_name();
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

    /// An install lacks a companion until it is where its package manager
    /// puts it: an npm package in `node_modules/`, a .NET tool in its tool
    /// path's store, at its version.
    #[test]
    fn an_install_lacks_a_companion_its_package_manager_has_not_put_in() {
        let registry = ServerRegistry::load().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let fsac = registry.get_server("fsautocomplete").unwrap();
        assert_eq!(fsac.lacks_companion(dir.path()), Some("fantomas@7.0.5"));
        std::fs::create_dir_all(dir.path().join("bin/.store/fantomas/7.0.4")).unwrap();
        assert_eq!(fsac.lacks_companion(dir.path()), Some("fantomas@7.0.5"));
        std::fs::create_dir_all(dir.path().join("bin/.store/fantomas/7.0.5")).unwrap();
        assert_eq!(fsac.lacks_companion(dir.path()), None);

        let volar = registry.get_server("volar").unwrap();
        assert_eq!(volar.lacks_companion(dir.path()), Some("typescript@5.9.3"));
        let typescript = dir.path().join("node_modules/typescript");
        std::fs::create_dir_all(&typescript).unwrap();
        std::fs::write(typescript.join("package.json"), "{}").unwrap();
        assert_eq!(volar.lacks_companion(dir.path()), None);

        let pyright = registry.get_server("pyright").unwrap();
        assert_eq!(pyright.lacks_companion(dir.path()), None);
    }

    /// A server whose builds expire tracks its latest build, found in its
    /// repository's releases and checked against the SHA-256 its publisher
    /// puts beside it, and is not mirrored; every other download pins its
    /// build.
    #[test]
    fn a_server_whose_builds_expire_tracks_its_latest_build() {
        let registry = ServerRegistry::load().unwrap();
        for server in registry.list_servers() {
            let id = &server.id;
            let source = &server.source;
            assert_eq!(server.builds_expire, server.tracks_latest(), "{id}");
            if !server.builds_expire {
                assert!(source.tag_prefix.is_none(), "{id}");
                assert!(source.checksum_url.is_none(), "{id}");
                continue;
            }
            assert!(!server.is_mirrored(), "{id}");
            assert_eq!(server.pinned_build(), None, "{id}");
            assert!(source.repo.is_some() && source.tag_prefix.is_some(), "{id}");
            assert!(
                source
                    .checksum_url
                    .as_deref()
                    .is_some_and(|url| url.contains("{url}")),
                "{id}"
            );
            assert!(source.expected_sha256.is_none(), "{id}");
            assert!(
                source
                    .platforms
                    .values()
                    .all(|build| build.sha256.is_none()),
                "{id}"
            );
        }
        let kotlin = registry.get_server("kotlin-lsp").unwrap();
        assert!(kotlin.builds_expire);
        assert!(
            !kotlin.outdated_by(Some("263.6379.0")),
            "kept until installed again"
        );
        assert_eq!(
            kotlin.at_version("263.7084.0").pinned_build(),
            Some("263.7084.0")
        );
    }
}
