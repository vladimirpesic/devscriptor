use super::*;
use std::path::Component;

/// `path` spelled relative to the current directory: `..` up to what the
/// two have in common, then the rest of `path`. A path on another Windows
/// drive than the current directory has no such spelling.
fn relative_to_cwd(path: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let (cwd, path): (Vec<_>, Vec<_>) = (cwd.components().collect(), path.components().collect());
    let root = |parts: &[Component]| {
        parts
            .iter()
            .take_while(|part| !matches!(part, Component::Normal(_)))
            .count()
    };
    if root(&cwd) != root(&path) || cwd[..root(&cwd)] != path[..root(&path)] {
        return None;
    }
    let common = cwd.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut relative: PathBuf =
        std::iter::repeat_n(Component::ParentDir, cwd.len() - common).collect();
    relative.extend(&path[common..]);
    Some(relative)
}

/// A file whose registry server does not run here is told which server
/// and why; one no server is registered for is told there is none.
#[test]
fn a_file_is_told_why_no_server_answers() {
    let unusable = no_server_for("rb").to_string();
    assert!(unusable.contains("(ruby-lsp) for .rb files"), "{unusable}");
    let why = crate::manager::why_unavailable("ruby-lsp").unwrap().reason;
    assert!(unusable.ends_with(&format!("files {why}")), "{unusable}");
    assert!(matches!(no_server_for("xyz"), LspError::NoServerForFile(_)));
}

// ---- LSP capability detection (turns opaque -32601 into a clear error) ----

fn range(a: (u32, u32), b: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position {
            line: a.0,
            character: a.1,
        },
        end: lsp_types::Position {
            line: b.0,
            character: b.1,
        },
    }
}

/// A range past the document's end is taken to its end: past the last
/// line to the end of the document, past a line's end to that line's end,
/// in UTF-16 code units; one within it stays.
#[test]
fn a_range_reaches_no_further_than_the_document() {
    let range = |start: (u32, u32), end: (u32, u32)| lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    };
    let text = "fn a() {}\nlet é = 1;\r\n";
    assert_eq!(
        within_text(text, range((0, 0), (14, 0))),
        range((0, 0), (2, 0))
    );
    assert_eq!(
        within_text(text, range((1, 40), (1, 99))),
        range((1, 10), (1, 10))
    );
    assert_eq!(
        within_text(text, range((0, 3), (1, 4))),
        range((0, 3), (1, 4))
    );
    assert_eq!(
        within_text("fn a() {}", range((0, 0), (5, 0))),
        range((0, 0), (0, 9))
    );
}

/// FsAutoComplete without Fantomas says it found none, and the error
/// names the command that installs it with Fantomas; other errors stay.
#[test]
fn a_formatter_without_fantomas_is_to_be_installed_with_it() {
    let hinted = with_formatter_hint(LspError::LspServerError {
        code: -32602,
        message: "Fantomas install not found.".into(),
    });
    assert_eq!(
        hinted.to_string(),
        "LSP server error -32602: Fantomas install not found. `devscriptor lsp setup \
             fsautocomplete` installs FsAutoComplete with Fantomas beside it."
    );
    let other = with_formatter_hint(LspError::LspServerError {
        code: -32603,
        message: "parse error".into(),
    });
    assert_eq!(other.to_string(), "LSP server error -32603: parse error");
}

/// A symbol is put at its name: FSAutoComplete's selection range starts
/// at the doc comment, and a flat answer gives the whole declaration; one
/// already at its name stays, as does one whose name is not in its range.
#[test]
fn a_symbol_is_put_at_its_name() {
    let fsharp = "/// Issued sequence numbers, by counter key.\ntype Counters = Map<string, int>\n";
    assert_eq!(
        at_name(fsharp, "Counters", range((0, 0), (1, 32))),
        range((1, 5), (1, 13))
    );
    assert_eq!(
        at_name(fsharp, "Counters", range((1, 5), (1, 13))),
        range((1, 5), (1, 13))
    );
    let java = "package com.tidewater.berths;\n";
    assert_eq!(
        at_name(java, "com.tidewater.berths", range((0, 0), (0, 29))),
        range((0, 8), (0, 28))
    );
    // A comment that names it, a longer word that holds it, a signature.
    let noisy = "// Counters by key\nlet Countersx = 1\nlet Counters(key) = 2\n";
    assert_eq!(
        at_name(noisy, "Counters(string)", range((0, 0), (2, 21))),
        range((2, 4), (2, 12))
    );
    assert_eq!(
        at_name(noisy, "Missing", range((0, 0), (2, 21))),
        range((0, 0), (2, 21))
    );
}

/// Columns count UTF-16 code units, as the server's do: `ü` is one, and
/// two bytes.
#[test]
fn a_symbol_is_put_at_its_name_in_utf16_columns() {
    let text = "fn ü(); fn target() {}\n";
    assert_eq!(
        at_name(text, "target", range((0, 0), (0, 22))),
        range((0, 11), (0, 17))
    );
}

/// The symbols of a tree answer and of a flat one are both put at their
/// names in the file's text.
#[test]
fn document_symbols_are_located_at_their_names() {
    let text = "/// The counters.\ntype Counters = int\n";
    let nested = lsp_types::DocumentSymbolResponse::Nested(vec![
        #[allow(deprecated)]
        lsp_types::DocumentSymbol {
            name: "Counters".into(),
            detail: None,
            kind: lsp_types::SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            range: range((0, 0), (1, 19)),
            selection_range: range((0, 0), (1, 19)),
            children: None,
        },
    ]);
    let symbols = symbols_from_lsp(Some(nested), "/p/a.fs", Some(text));
    assert_eq!(symbols[0].location.range.start.line, 1);
    assert_eq!(symbols[0].location.range.start.character, 5);
    let uri: lsp_types::Uri = "file:///p/a.fs".parse().unwrap();
    #[allow(deprecated)]
    let flat = lsp_types::DocumentSymbolResponse::Flat(vec![lsp_types::SymbolInformation {
        name: "Counters".into(),
        kind: lsp_types::SymbolKind::STRUCT,
        tags: None,
        deprecated: None,
        location: lsp_types::Location {
            uri,
            range: range((0, 0), (1, 19)),
        },
        container_name: None,
    }]);
    let symbols = symbols_from_lsp(Some(flat), "/p/a.fs", Some(text));
    assert_eq!(symbols[0].location.range.start.line, 1);
    assert_eq!(symbols[0].location.range.start.character, 5);
}

#[test]
fn capability_supported_reads_lsp_provider_shapes() {
    use serde_json::json;
    // `true` or an options object → supported.
    let caps = json!({
        "documentSymbolProvider": true,
        "workspaceSymbolProvider": { "resolveProvider": false },
    });
    assert!(capability_supported(&caps, "documentSymbolProvider"));
    assert!(capability_supported(&caps, "workspaceSymbolProvider"));

    // `false`, `null`, or absent → unsupported.
    let caps = json!({
        "documentSymbolProvider": false,
        "hoverProvider": serde_json::Value::Null,
    });
    assert!(!capability_supported(&caps, "documentSymbolProvider"));
    assert!(!capability_supported(&caps, "hoverProvider"));
    assert!(!capability_supported(&caps, "workspaceSymbolProvider")); // absent
    assert!(!capability_supported(&json!({}), "documentSymbolProvider"));
}

// ---- text-edit application (rename / format write-back) ----

fn te(sl: u32, sc: u32, el: u32, ec: u32, new: &str) -> TextEdit {
    TextEdit {
        range: Range {
            start: Position2D {
                line: sl,
                character: sc,
            },
            end: Position2D {
                line: el,
                character: ec,
            },
        },
        new_text: new.to_string(),
    }
}

#[test]
fn position_to_byte_offset_walks_lines_and_clamps() {
    let src = "ab\ncd\n";
    assert_eq!(position_to_byte_offset(src, 0, 0), 0);
    assert_eq!(position_to_byte_offset(src, 0, 2), 2); // end of "ab"
    assert_eq!(position_to_byte_offset(src, 1, 0), 3); // start of "cd"
    assert_eq!(position_to_byte_offset(src, 1, 1), 4);
    assert_eq!(position_to_byte_offset(src, 0, 99), 2); // past EOL clamps
}

#[test]
fn apply_text_edits_single_rename() {
    let out = apply_text_edits("const oldName = 1;\n", &[te(0, 6, 0, 13, "renamed")]);
    assert_eq!(out, "const renamed = 1;\n");
}

#[test]
fn apply_text_edits_multiple_edits_out_of_order() {
    // Two edits on different lines, supplied later-first and earlier-second;
    // both must land correctly (offsets stay valid because we apply tail-first).
    let edits = vec![te(2, 0, 2, 3, "baz"), te(0, 0, 0, 3, "baz")];
    let out = apply_text_edits("foo();\nbar();\nfoo();\n", &edits);
    assert_eq!(out, "baz();\nbar();\nbaz();\n");
}

#[test]
fn apply_text_edits_insertion() {
    let out = apply_text_edits("a\nb\n", &[te(1, 0, 1, 0, "X")]);
    assert_eq!(out, "a\nXb\n");
}

fn open_doc(last_used: u64) -> OpenDocument {
    OpenDocument {
        version: 1,
        stamp: None,
        hash: 0,
        read_at: std::time::SystemTime::UNIX_EPOCH,
        last_used,
        saved: None,
    }
}

/// Past the cap, a language's least recently used documents are closed —
/// never the one just opened, and never another language's.
#[test]
fn the_least_recently_used_documents_of_the_language_are_closed() {
    let mut open = HashMap::new();
    open.insert("/p/a.rs".to_string(), open_doc(5));
    open.insert("/p/b.rs".to_string(), open_doc(1));
    open.insert("/p/c.rs".to_string(), open_doc(3));
    open.insert("/p/new.rs".to_string(), open_doc(0));
    open.insert("/p/old.py".to_string(), open_doc(0));
    let language = |path: &str| {
        if path.ends_with(".rs") {
            "rust"
        } else {
            "python"
        }
        .to_string()
    };

    let closed = documents_to_close(&open, language, "rust", "/p/new.rs", 2);
    assert_eq!(closed, ["/p/b.rs", "/p/c.rs"]);
    assert!(documents_to_close(&open, language, "rust", "/p/new.rs", 4).is_empty());
}

/// Edit columns count UTF-16 code units, the only encoding offered to
/// servers: `ñ` and `ú` are one unit (two UTF-8 bytes) each. Read as byte
/// offsets, this rename cut `(a` from the line.
#[test]
fn apply_text_edits_counts_utf16_units_on_non_ascii_lines() {
    let src = "// Déjà vu\nfn ñandú_total(a: i32) -> i32 { a + 1 }\n";
    let out = apply_text_edits(src, &[te(1, 3, 1, 14, "total")]);
    assert_eq!(out, "// Déjà vu\nfn total(a: i32) -> i32 { a + 1 }\n");
}

/// A character outside the Basic Multilingual Plane is two UTF-16 units.
#[test]
fn apply_text_edits_counts_a_surrogate_pair_as_two_units() {
    // `let s = "` is 9 units, the emoji 2, `"; let ` 7: `x` starts at 18.
    let out = apply_text_edits("let s = \"😀\"; let x = 1;\n", &[te(0, 18, 0, 19, "y")]);
    assert_eq!(out, "let s = \"😀\"; let y = 1;\n");
}

// ---- workspace root resolution (lazy, file-rooted LSP startup) ----

/// The real shipped preset, so these tests track the actual markers.
fn rust_config() -> LspServerConfig {
    let c = LspServerConfig::rust_analyzer();
    assert!(
        c.heuristics
            .as_ref()
            .is_some_and(|h| h.markers.iter().any(|m| m == "Cargo.toml")),
        "rust preset must key on Cargo.toml"
    );
    c
}

/// A workspace member must resolve to the *workspace* root, not to itself:
/// `crates/lsp/Cargo.toml` inherits `version.workspace = true` and cannot be
/// loaded standalone — rust-analyzer rooted there rejects the file.
#[test]
fn workspace_member_resolves_to_the_repository_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();

    let member = root.join("crates/lsp/src");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(root.join("crates/lsp/Cargo.toml"), "[package]\n").unwrap();

    let file = member.join("lib.rs");
    std::fs::write(&file, "fn main() {}").unwrap();

    let got = Translator::workspace_root_for(&rust_config(), &file).unwrap();
    assert_eq!(got, root, "must pick the outermost Cargo.toml");
}

/// The search must not ascend past the enclosing repository, or a stray
/// `Cargo.toml` in a parent directory would swallow unrelated projects.
#[test]
fn search_stops_at_the_vcs_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    // A decoy marker *above* the repo.
    std::fs::write(tmp.path().join("Cargo.toml"), "[workspace]\n").unwrap();

    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[package]\n").unwrap();
    let src = repo.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("main.rs");
    std::fs::write(&file, "fn main() {}").unwrap();

    let got = Translator::workspace_root_for(&rust_config(), &file).unwrap();
    assert_eq!(got, repo, "must not escape the .git boundary");
}

/// A standalone crate with no VCS still resolves to its own root.
#[test]
fn standalone_crate_resolves_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("crate");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
    let file = root.join("src/main.rs");
    std::fs::write(&file, "fn main() {}").unwrap();

    let got = Translator::workspace_root_for(&rust_config(), &file).unwrap();
    assert_eq!(got, root);
}

/// A loose source file with no project marker anywhere yields `None`, so the
/// caller can tell the agent exactly what is missing instead of reporting
/// "no LSP server configured".
#[test]
fn no_marker_anywhere_yields_none() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let file = tmp.path().join("loose.rs");
    std::fs::write(&file, "fn main() {}").unwrap();

    assert!(Translator::workspace_root_for(&rust_config(), &file).is_none());
}

/// A C file belongs to the project its build file or clangd configuration
/// is in: a Makefile project's, for a file under its `src/`, not `src/`
/// alone.
#[test]
fn a_c_file_belongs_to_its_makefile_project() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let project = tmp.path().join("services/geo-cpp");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("Makefile"), "all:\n").unwrap();
    let file = project.join("src/luhn.c");
    std::fs::write(&file, "int luhn_check_digit(const char *s);\n").unwrap();

    let mut clangd = rust_config();
    clangd.language_id = "cpp".into();
    clangd.heuristics = crate::generated_registry::heuristics("clangd");
    assert_eq!(
        Translator::workspace_root_for(&clangd, &file),
        Some(project)
    );
}

/// A loose file for a non-build-graph language (e.g. Python) with no marker
/// falls back to the file's own directory instead of yielding `None`, so a
/// bare `.py` script is still analysable. Strict languages (Rust) do not.
#[test]
fn marker_less_non_strict_language_falls_back_to_file_directory() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let file = tmp.path().join("scratch.py");
    std::fs::write(&file, "def f():\n    pass\n").unwrap();

    // A Python-shaped config: has heuristics, but none of its markers exist.
    let mut c = rust_config();
    c.language_id = "python".into();
    c.heuristics = Some(crate::config::ServerHeuristics {
        markers: vec!["pyproject.toml".into(), "requirements.txt".into()],
        max_depth: 3,
    });

    let got = Translator::workspace_root_for(&c, &file).unwrap();
    assert_eq!(
        got,
        tmp.path(),
        "marker-less Python roots at the file's dir"
    );

    // A strict, build-graph language stays unavailable.
    let mut strict = c.clone();
    strict.language_id = "csharp".into();
    assert!(
        Translator::workspace_root_for(&strict, &file).is_none(),
        "build-graph languages must not fall back to a bare directory"
    );
}

/// A server without heuristics matches anywhere and is rooted at the file.
#[test]
fn config_without_heuristics_roots_at_the_file_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("x.rs");
    std::fs::write(&file, "").unwrap();

    let mut c = rust_config();
    c.heuristics = None;
    let got = Translator::workspace_root_for(&c, &file).unwrap();
    assert_eq!(got, tmp.path());
}

/// A strict-root server starting for a monorepo whose root is no project
/// of its kind gets the projects inside the root; another server gets the
/// root. Rooted at such a root, rust-analyzer answered `file not found`.
#[test]
fn startup_folders_root_a_strict_server_at_the_projects_inside() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("services/pricing-rust/src")).unwrap();
    std::fs::write(root.join("services/pricing-rust/Cargo.toml"), "[package]\n").unwrap();

    let mut python = rust_config();
    python.language_id = "python".into();
    let mut translator = Translator::new(Config {
        workspace: crate::config::WorkspaceConfig::default(),
        servers: vec![rust_config(), python.clone()],
        lifecycle: crate::config::LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![root.clone()]);

    assert_eq!(
        translator.startup_folders(&rust_config()),
        vec![root.join("services/pricing-rust")]
    );
    assert_eq!(translator.startup_folders(&python), vec![root]);
}

/// kotlin-lsp imports a Gradle build only at the root of a workspace
/// folder: starting for a monorepo it gets the projects inside, and for a
/// tree that holds none, the root, where it still serves each file.
#[test]
fn startup_folders_give_kotlin_the_projects_inside() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("depot");
    let write = |rel: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    };
    std::fs::create_dir_all(root.join(".git")).unwrap();
    write("services/reefer-kotlin/build.gradle.kts");
    write("services/reefer-kotlin/src/main/kotlin/SetPoint.kt");
    // A Groovy build, a Maven build and a multi-project build hold
    // Kotlin as much as a build written in the Kotlin DSL.
    write("services/rates-groovy/build.gradle");
    write("services/rates-groovy/src/main/kotlin/Rate.kt");
    write("services/ledger-maven/pom.xml");
    write("services/ledger-maven/src/main/kotlin/Ledger.kt");
    write("platform/settings.gradle.kts");
    write("platform/core/build.gradle.kts");
    write("platform/core/src/main/kotlin/Core.kt");
    // Builds without Kotlin are no project of kotlin-lsp's: its build
    // script is no Kotlin source.
    write("services/berths-java/pom.xml");
    write("services/berths-java/src/main/java/Berth.java");
    write("services/billing-java/build.gradle.kts");
    write("services/billing-java/src/main/java/Bill.java");
    let loose = dunce::canonicalize(tmp.path()).unwrap().join("scripts");
    std::fs::create_dir_all(&loose).unwrap();
    std::fs::write(loose.join("tool.kt"), "fun main() {}\n").unwrap();

    let mut kotlin = rust_config();
    kotlin.language_id = "kotlin".into();
    kotlin.extensions = vec!["kt".into(), "kts".into()];
    kotlin.heuristics = crate::generated_registry::heuristics("kotlin-lsp");
    let mut translator = Translator::new(Config {
        workspace: crate::config::WorkspaceConfig::default(),
        servers: vec![kotlin.clone()],
        lifecycle: crate::config::LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![root.clone()]);
    assert_eq!(
        translator.startup_folders(&kotlin),
        [
            "platform",
            "services/ledger-maven",
            "services/rates-groovy",
            "services/reefer-kotlin",
        ]
        .map(|project| root.join(project))
    );
    assert_eq!(
        translator.folders_for_file(&kotlin, root.join("services/reefer-kotlin")),
        translator.startup_folders(&kotlin)
    );
    assert!(Translator::knows_only_its_folders("kotlin"));

    translator.set_workspace_roots(vec![loose.clone()]);
    assert_eq!(translator.startup_folders(&kotlin), vec![loose]);
}

/// sourcekit-lsp is given the Swift packages inside a monorepo's root: given
/// the root, it answers every request for a file of a package below it
/// with `No language service`. Where there is no package, the root.
#[test]
fn startup_folders_give_swift_the_packages_inside() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("depot");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("apps/reefer-ios/Sources/ReeferKit")).unwrap();
    std::fs::write(root.join("apps/reefer-ios/Package.swift"), "").unwrap();
    std::fs::write(
        root.join("apps/reefer-ios/Sources/ReeferKit/SetPointRange.swift"),
        "",
    )
    .unwrap();
    let loose = dunce::canonicalize(tmp.path()).unwrap().join("scripts");
    std::fs::create_dir_all(&loose).unwrap();
    std::fs::write(loose.join("tool.swift"), "let limit = 3\n").unwrap();

    let mut swift = rust_config();
    swift.language_id = "swift".into();
    swift.extensions = vec!["swift".into()];
    swift.heuristics = Some(crate::config::ServerHeuristics {
        markers: vec!["Package.swift".into()],
        max_depth: 3,
    });
    let mut translator = Translator::new(Config {
        workspace: crate::config::WorkspaceConfig::default(),
        servers: vec![swift.clone()],
        lifecycle: crate::config::LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![root.clone()]);
    assert_eq!(
        translator.startup_folders(&swift),
        vec![root.join("apps/reefer-ios")]
    );
    assert!(Translator::knows_only_its_folders("swift"));

    translator.set_workspace_roots(vec![loose.clone()]);
    assert_eq!(translator.startup_folders(&swift), vec![loose]);
}

/// A server whose project markers are absent still starts where files of
/// its language are, as a request for one of them would start it; a server
/// that needs its manifest does not.
#[test]
fn a_server_starts_where_its_files_are_unless_it_needs_its_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("shop");
    std::fs::create_dir_all(root.join("services/recommender-python")).unwrap();
    std::fs::write(
        root.join("services/recommender-python/recommender.py"),
        "x = 1\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("services/loose-rust")).unwrap();
    std::fs::write(root.join("services/loose-rust/main.rs"), "fn main() {}\n").unwrap();

    let mut translator = Translator::new(Config {
        workspace: crate::config::WorkspaceConfig::default(),
        servers: vec![LspServerConfig::pyright(), rust_config()],
        lifecycle: crate::config::LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![root]);
    assert_eq!(
        translator.startable_languages(["python".to_string(), "rust".to_string()]),
        ["python"],
        "pyright needs no marker; rust-analyzer needs a Cargo.toml"
    );
}

/// Folders are absolute whatever the spelling of the workspace root, so
/// that a file's project, resolved from its absolute path, is found among
/// them.
#[test]
fn startup_folders_are_absolute_for_a_relative_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("services/pricing-rust/src")).unwrap();
    std::fs::write(root.join("services/pricing-rust/Cargo.toml"), "[package]\n").unwrap();
    let file = root.join("services/pricing-rust/src/lib.rs");
    std::fs::write(&file, "").unwrap();

    let mut translator = Translator::new(Config {
        workspace: crate::config::WorkspaceConfig::default(),
        servers: vec![rust_config()],
        lifecycle: crate::config::LifecycleConfig::default(),
    });
    // `root` spelled relative to the current directory, as `.` is; a root
    // on another Windows drive has no such spelling to try.
    let Some(relative) = relative_to_cwd(&root) else {
        return;
    };
    translator.set_workspace_roots(vec![relative]);

    let folders = translator.startup_folders(&rust_config());
    let project = Translator::workspace_root_for(&rust_config(), &file).unwrap();
    assert_eq!(folders, vec![project]);
}

/// Documents of the TypeScript server are opened as their own languages;
/// others as their server's.
#[test]
fn documents_are_opened_as_their_own_language() {
    for (file, id) in [
        ("CartBanner.tsx", "typescriptreact"),
        ("pricing.ts", "typescript"),
        ("app.jsx", "javascriptreact"),
        ("app.mjs", "javascript"),
    ] {
        assert_eq!(
            document_language_id(Path::new(file), "typescript"),
            id,
            "{file}"
        );
    }
    assert_eq!(document_language_id(Path::new("lib.rs"), "rust"), "rust");
}

/// A hierarchy item keeps the range of its name through the round trip
/// to the agent and back: servers resolve a passed-back item by it.
#[test]
fn hierarchy_items_keep_their_selection_range() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("lib.rs");
    std::fs::write(&file, "pub fn apply_bulk_discount() {}\n").unwrap();
    let span = |l0, c0, l1, c1| lsp_types::Range {
        start: lsp_types::Position {
            line: l0,
            character: c0,
        },
        end: lsp_types::Position {
            line: l1,
            character: c1,
        },
    };
    let lsp_item = lsp_types::CallHierarchyItem {
        name: "apply_bulk_discount".into(),
        kind: lsp_types::SymbolKind::FUNCTION,
        tags: None,
        detail: None,
        uri: path_to_uri(&file).unwrap(),
        range: span(40, 0, 47, 1),
        selection_range: span(41, 7, 41, 26),
        data: None,
    };

    let ours = call_hierarchy_item_from_lsp(lsp_item.clone());
    let back = call_hierarchy_item_to_lsp(&ours).unwrap();
    assert_eq!(back.selection_range, lsp_item.selection_range);
    assert_eq!(back.range, lsp_item.range);

    // One an agent built without it is selected by its whole range.
    let built = CallHierarchyItem {
        selection_range: None,
        ..ours
    };
    assert_eq!(
        call_hierarchy_item_to_lsp(&built).unwrap().selection_range,
        lsp_item.range
    );
}

/// A Windows path is written in a `file:` URI with `/`, after a `/`, and a
/// UNC path with its server as the host; and read back from either
/// spelling of the drive.
#[test]
fn windows_paths_and_their_uris() {
    assert_eq!(windows_uri_path(r"C:\Users\a b\x.rs"), "/C:/Users/a b/x.rs");
    assert_eq!(windows_uri_path(r"\\srv\share\x.rs"), "srv/share/x.rs");
    assert_eq!(windows_path("/C:/Users/a b/x.rs"), r"C:\Users\a b\x.rs");
    assert_eq!(windows_path("/c:/Users/x.rs"), r"C:\Users\x.rs");
    assert_eq!(windows_path("/D:"), r"D:\");
    assert_eq!(windows_path("srv/share/x.rs"), r"\\srv\share\x.rs");
}

#[cfg(unix)]
#[test]
fn test_uri_to_path_unix() {
    let uri: lsp_types::Uri = "file:///home/user/test.rs".parse().unwrap();
    assert_eq!(uri_to_path(&uri), "/home/user/test.rs");
}

#[test]
fn test_range_from_lsp() {
    let lsp_range = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 5,
            character: 10,
        },
    };

    let range = range_from_lsp(lsp_range);
    assert_eq!(range.start.line, 0);
    assert_eq!(range.start.character, 0);
    assert_eq!(range.end.line, 5);
    assert_eq!(range.end.character, 10);
}

#[test]
fn test_marked_string_to_string_plain() {
    let ms = lsp_types::MarkedString::String("hello world".into());
    assert_eq!(marked_string_to_string(ms), "hello world");
}

#[test]
fn test_marked_string_to_string_language() {
    let ms = lsp_types::MarkedString::LanguageString(lsp_types::LanguageString {
        language: "rust".into(),
        value: "fn main() {}".into(),
    });
    assert_eq!(marked_string_to_string(ms), "```rust\nfn main() {}\n```");
}

/// path_to_uri percent-encodes spaces and special characters.
#[test]
fn path_to_uri_percent_encodes_spaces() {
    let path = std::path::Path::new("/home/user/my project/file.rs");
    let uri = path_to_uri(path).expect("path_to_uri should succeed");
    let uri_str = uri.to_string();
    assert!(
        !uri_str.contains(' '),
        "URI must not contain raw spaces; got: {uri_str}"
    );
    assert!(
        uri_str.contains("%20"),
        "space must be percent-encoded as %20; got: {uri_str}"
    );
}

/// uri_to_path decodes percent-encoded paths back correctly.
#[test]
fn uri_to_path_decodes_percent_encoding() {
    let uri: lsp_types::Uri = "file:///home/user/my%20project/file.rs".parse().unwrap();
    let path = uri_to_path(&uri);
    assert!(
        path.contains("my project"),
        "decoded path must contain 'my project'; got: {path}"
    );
}
