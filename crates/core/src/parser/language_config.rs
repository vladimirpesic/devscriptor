//! Language configuration and file extension mappings

use std::collections::HashMap;
use std::sync::LazyLock;

/// Default config returned for unknown languages.
static DEFAULT_CONFIG: LazyLock<LanguageNodeConfig> = LazyLock::new(LanguageNodeConfig::default);

/// All supported language identifiers (must match the arms in `build_language_config`).
const SUPPORTED_LANGS: &[&str] = &[
    "bash",
    "c",
    "c_sharp",
    "cpp",
    "cuda",
    "dart",
    "fsharp",
    "go",
    "graphql",
    "html",
    "java",
    "javascript",
    "kotlin",
    "objc",
    "objective_c",
    "php",
    "python",
    "ruby",
    "rust",
    "scala",
    "solidity",
    "sql",
    "svelte",
    "swift",
    "tsx",
    "typescript",
    "vb_net",
    "vue",
];

/// Static cache of `LanguageNodeConfig` values, built once on first access.
/// Each config is leaked to obtain `&'static` references, so a lookup makes no
/// allocation.
static LANGUAGE_CONFIG_MAP: LazyLock<HashMap<&'static str, &'static LanguageNodeConfig>> =
    LazyLock::new(|| {
        let mut map = HashMap::with_capacity(SUPPORTED_LANGS.len());
        for &lang in SUPPORTED_LANGS {
            let config = build_language_config(lang);
            // Leak the config to get a stable `&'static` pointer.
            let ptr: &'static LanguageNodeConfig = Box::leak(Box::new(config));
            map.insert(lang, ptr);
        }
        map
    });

/// Map file extensions to tree-sitter language names
/// Only includes languages from our supported set
static EXTENSION_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();

    // Web/JS ecosystem
    map.insert("js", "javascript");
    map.insert("jsx", "javascript");
    map.insert("ts", "typescript");
    map.insert("tsx", "tsx");
    map.insert("mjs", "javascript");
    map.insert("cjs", "javascript");

    // Python
    map.insert("py", "python");
    map.insert("pyi", "python");
    map.insert("pyw", "python");

    // Rust
    map.insert("rs", "rust");

    // Go
    map.insert("go", "go");
    map.insert("mod", "gomod");
    map.insert("sum", "gosum");

    // Java
    map.insert("java", "java");

    // C/C++
    map.insert("c", "c");
    map.insert("h", "c");
    map.insert("cpp", "cpp");
    map.insert("cc", "cpp");
    map.insert("cxx", "cpp");
    map.insert("hpp", "cpp");

    // C#
    map.insert("cs", "c_sharp");

    // Ruby
    map.insert("rb", "ruby");
    map.insert("erb", "ruby");

    // PHP
    map.insert("php", "php");

    // Swift
    map.insert("swift", "swift");

    // Kotlin
    map.insert("kt", "kotlin");
    map.insert("kts", "kotlin");

    // Scala
    map.insert("scala", "scala");
    map.insert("sc", "scala");

    // Dart
    map.insert("dart", "dart");

    // Shell
    map.insert("sh", "bash");
    map.insert("bash", "bash");
    map.insert("zsh", "bash");

    // HTML
    map.insert("html", "html");
    map.insert("htm", "html");

    // SQL
    map.insert("sql", "sql");

    // Make
    map.insert("mk", "make");

    // Solidity
    map.insert("sol", "solidity");

    // F#
    map.insert("fs", "fsharp");
    map.insert("fsx", "fsharp");
    map.insert("fsi", "fsharp");

    // GraphQL
    map.insert("gql", "graphql");
    map.insert("graphql", "graphql");

    // Objective-C / Objective-C++
    // NOTE: `.m` is intentionally omitted — it is ambiguous (MATLAB/Octave/ObjC).
    //       Only `.mm` (Objective-C++) is mapped unambiguously.
    map.insert("mm", "objc");

    // CUDA
    map.insert("cu", "cuda");
    map.insert("cuh", "cuda");

    // Svelte
    map.insert("svelte", "svelte");

    // Vue
    map.insert("vue", "vue");

    // VB.NET
    map.insert("vb", "vb_net");

    // Simple/config files (detected but not parsed with tree-sitter)
    // JSON
    map.insert("json", "json");
    // YAML
    map.insert("yaml", "yaml");
    map.insert("yml", "yaml");
    // TOML
    map.insert("toml", "toml");
    // XML
    map.insert("xml", "xml");
    // CSS
    map.insert("css", "css");
    // SCSS
    map.insert("scss", "scss");
    // CMake
    map.insert("cmake", "cmake");
    // Markdown — recorded as a file, not parsed. Documentation is the largest
    // store of intent in most repositories; until it is indexed it stays
    // invisible to every search.
    map.insert("md", "markdown");
    map.insert("mdx", "markdown");
    map.insert("markdown", "markdown");

    map
});

/// Set of simple/config file languages that are detected but not parsed with tree-sitter
/// These languages have extension mappings for file detection but no AST-based entity extraction
pub static SIMPLE_FILE_LANGUAGES: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| {
        let mut set = std::collections::HashSet::new();
        set.insert("json");
        set.insert("yaml");
        set.insert("toml");
        set.insert("xml");
        set.insert("css");
        set.insert("scss");
        set.insert("cmake");
        set.insert("make");
        set.insert("dockerfile");
        set.insert("gitignore"); // .gitignore, .gitattributes — no AST value
        set.insert("ini"); // editorconfig, npmrc
        // Prose, not config — but it belongs here for the same reason: there is
        // no tree-sitter grammar for it. Without an entry every `.md` file would
        // fail the scan with "Language not supported: md" and be left out of the
        // `files` table; with one, the file is recorded, and its sections come
        // from `PROSE_LANGUAGES` below.
        set.insert("markdown");
        set
    });

/// Languages whose files are prose: indexed as heading sections, not as an AST.
pub static PROSE_LANGUAGES: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| {
        let mut set = std::collections::HashSet::new();
        set.insert("markdown");
        set
    });

/// Whether this language is prose, extracted section by section.
///
/// Checked before [`is_simple_file_language`]: markdown is listed there too, so
/// that a build without prose extraction still records the file rather than
/// failing the scan.
#[must_use]
pub fn is_prose_language(language: &str) -> bool {
    PROSE_LANGUAGES.contains(language)
}

/// Check if a language is a simple/config file that should not be parsed with tree-sitter
///
/// Simple files are detected for file type identification but don't have AST-based
/// entity extraction. They are treated as plain text files.
pub fn is_simple_file_language(language: &str) -> bool {
    SIMPLE_FILE_LANGUAGES.contains(language)
}

/// Map filenames (without extension) to tree-sitter language names
/// Used for files like "Makefile", "Dockerfile", etc.
static FILENAME_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();

    // Make
    map.insert("makefile", "make");
    map.insert("gnumakefile", "make");

    // Dockerfile
    map.insert("dockerfile", "dockerfile");

    // Docker Compose
    map.insert("docker-compose", "yaml");
    map.insert("compose", "yaml");

    // Git config files
    map.insert("gitconfig", "yaml");
    map.insert("gitattributes", "xml");
    map.insert("gitignore", "gitignore");

    // Editor config
    map.insert("editorconfig", "ini");

    // npm/pnpm
    map.insert("npmrc", "ini");

    // Vagrant
    map.insert("vagrantfile", "ruby");

    // Brewfile
    map.insert("brewfile", "ruby");

    // Gemfile
    map.insert("gemfile", "ruby");

    // Rakefile
    map.insert("rakefile", "ruby");

    // Procfile
    map.insert("procfile", "yaml");

    map
});

/// Configuration for language-specific AST node types
#[derive(Debug, Clone, Default)]
pub struct LanguageNodeConfig {
    /// Function/method node types
    pub function_nodes: Vec<&'static str>,
    /// Class node types
    pub class_nodes: Vec<&'static str>,
    /// Struct node types
    pub struct_nodes: Vec<&'static str>,
    /// Enum node types
    pub enum_nodes: Vec<&'static str>,
    /// Interface node types
    pub interface_nodes: Vec<&'static str>,
    /// Trait node types
    pub trait_nodes: Vec<&'static str>,
    /// Module node types
    pub module_nodes: Vec<&'static str>,
    /// Namespace node types
    pub namespace_nodes: Vec<&'static str>,
    /// Type alias node types
    pub type_alias_nodes: Vec<&'static str>,
    /// Constant node types
    pub constant_nodes: Vec<&'static str>,
    /// Macro node types
    pub macro_nodes: Vec<&'static str>,
    /// Import/use node types
    pub import_nodes: Vec<&'static str>,
    /// Call expression node types
    pub call_nodes: Vec<&'static str>,
    /// Documentation/comment node types
    pub documentation_nodes: Vec<&'static str>,
    /// Component node types: the node that is a framework file's one
    /// component (Vue's and Svelte's `document`), or an HTML element. A JSX
    /// element is not one: it renders a component, which `call_nodes` records.
    pub component_nodes: Vec<&'static str>,
    /// Property/field node types
    pub property_nodes: Vec<&'static str>,
    /// Variable declaration node types
    /// Constructor node types
    pub constructor_nodes: Vec<&'static str>,
}

/// Get language node configuration for a given language
pub fn get_language_config(language: &str) -> &'static LanguageNodeConfig {
    LANGUAGE_CONFIG_MAP
        .get(language)
        .copied()
        .unwrap_or_else(|| &DEFAULT_CONFIG)
}

/// Internal builder used to populate the static cache once.
fn build_language_config(language: &str) -> LanguageNodeConfig {
    match language {
        "python" => LanguageNodeConfig {
            function_nodes: vec!["function_definition"],
            class_nodes: vec!["class_definition"],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec!["module"],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec!["type_alias_statement"],
            constant_nodes: vec!["assignment"],
            macro_nodes: vec![],
            import_nodes: vec!["import_statement", "import_from_statement"],
            call_nodes: vec!["call"],
            documentation_nodes: vec!["comment", "string", "expression_statement"],
            component_nodes: vec![],
            property_nodes: vec!["attribute"],
            constructor_nodes: vec!["function_definition"],
        },
        "javascript" => LanguageNodeConfig {
            function_nodes: vec![
                "function_declaration",
                "function_expression",
                "arrow_function",
                "method_definition",
            ],
            class_nodes: vec!["class_declaration", "class"],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec!["variable_declarator", "field_definition"],
            macro_nodes: vec![],
            import_nodes: vec!["import_statement"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_definition"],
            constructor_nodes: vec![],
        },
        "typescript" => LanguageNodeConfig {
            function_nodes: vec![
                "function_declaration",
                "function_expression",
                "arrow_function",
                "method_definition",
                "method_signature",
            ],
            class_nodes: vec!["class_declaration", "abstract_class_declaration", "class"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec!["interface_declaration"],
            trait_nodes: vec![],
            module_nodes: vec!["module"],
            namespace_nodes: vec!["internal_module"],
            struct_nodes: vec![],
            type_alias_nodes: vec!["type_alias_declaration"],
            constant_nodes: vec!["variable_declarator", "public_field_definition"],
            macro_nodes: vec![],
            import_nodes: vec!["import_statement", "import_require_clause", "import_alias"],
            call_nodes: vec!["call_expression", "new_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_signature"],
            constructor_nodes: vec!["construct_signature"],
        },
        "tsx" => LanguageNodeConfig {
            function_nodes: vec![
                "function_declaration",
                "function_expression",
                "arrow_function",
                "method_definition",
                "method_signature",
            ],
            class_nodes: vec!["class_declaration", "abstract_class_declaration", "class"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec!["interface_declaration"],
            trait_nodes: vec![],
            module_nodes: vec!["module"],
            namespace_nodes: vec!["internal_module"],
            struct_nodes: vec![],
            type_alias_nodes: vec!["type_alias_declaration"],
            constant_nodes: vec!["variable_declarator", "public_field_definition"],
            macro_nodes: vec![],
            import_nodes: vec!["import_statement", "import_require_clause"],
            call_nodes: vec![
                "call_expression",
                "new_expression",
                "jsx_opening_element",
                "jsx_self_closing_element",
            ],
            documentation_nodes: vec!["comment", "jsx_expression"],
            component_nodes: vec![],
            property_nodes: vec!["property_signature", "jsx_attribute"],
            constructor_nodes: vec!["construct_signature"],
        },
        "java" => LanguageNodeConfig {
            function_nodes: vec![
                "method_declaration",
                "constructor_declaration",
                "lambda_expression",
            ],
            class_nodes: vec!["class_declaration", "record_declaration"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec!["interface_declaration", "annotation_type_declaration"],
            trait_nodes: vec![],
            module_nodes: vec!["module_declaration"],
            namespace_nodes: vec!["package_declaration"],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec!["constant_declaration", "field_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["import_declaration"],
            call_nodes: vec![
                "method_invocation",
                "object_creation_expression",
                "explicit_constructor_invocation",
            ],
            documentation_nodes: vec!["line_comment", "block_comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_declaration", "field_access"],
            constructor_nodes: vec!["constructor_declaration"],
        },
        "rust" => LanguageNodeConfig {
            // A definition, and a declaration without a body (a trait's
            // required method, an `extern` function).
            function_nodes: vec!["function_item", "function_signature_item"],
            class_nodes: vec![],
            enum_nodes: vec!["enum_item"],
            interface_nodes: vec!["trait_item"],
            trait_nodes: vec![],
            module_nodes: vec!["mod_item"],
            namespace_nodes: vec![],
            struct_nodes: vec!["struct_item"],
            type_alias_nodes: vec!["type_item"],
            constant_nodes: vec!["const_item", "static_item"],
            // Definitions only: an invocation (`vec![…]`) is a call (`call_nodes`),
            // and a `macro_rule` is one arm of its definition.
            macro_nodes: vec!["macro_definition"],
            import_nodes: vec!["use_declaration"],
            call_nodes: vec!["call_expression", "macro_invocation"],
            documentation_nodes: vec!["line_comment", "block_comment", "doc_comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_declaration", "shorthand_field_identifier"],
            constructor_nodes: vec!["function_item"],
        },
        "go" => LanguageNodeConfig {
            function_nodes: vec!["function_declaration", "method_declaration", "func_literal"],
            class_nodes: vec![],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            // A `type_spec` is a struct, an interface or a type of its own by
            // its `type` (`extract_go_type_spec`).
            struct_nodes: vec!["type_spec"],
            type_alias_nodes: vec!["type_alias"],
            constant_nodes: vec!["const_declaration", "const_spec"],
            macro_nodes: vec![],
            import_nodes: vec!["import_declaration", "import_spec"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_declaration", "selector_expression"],
            constructor_nodes: vec!["function_declaration"],
        },
        "c" => LanguageNodeConfig {
            function_nodes: vec!["function_definition"],
            class_nodes: vec![],
            enum_nodes: vec!["enum_specifier"],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec!["struct_specifier"],
            type_alias_nodes: vec!["type_definition"],
            constant_nodes: vec!["declaration", "init_declarator"],
            macro_nodes: vec!["preproc_def", "preproc_function_def", "preproc_call"],
            import_nodes: vec!["preproc_include"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_declaration"],
            constructor_nodes: vec![],
        },
        "cpp" => LanguageNodeConfig {
            function_nodes: vec!["function_definition", "lambda_expression"],
            class_nodes: vec!["class_specifier"],
            enum_nodes: vec!["enum_specifier"],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec!["namespace_definition", "namespace_alias_definition"],
            struct_nodes: vec!["struct_specifier"],
            type_alias_nodes: vec!["type_definition", "alias_declaration"],
            constant_nodes: vec!["declaration", "init_declarator"],
            macro_nodes: vec!["preproc_def", "preproc_function_def", "preproc_call"],
            import_nodes: vec!["preproc_include"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_declaration"],
            constructor_nodes: vec!["function_definition"],
        },
        "c_sharp" => LanguageNodeConfig {
            function_nodes: vec![
                "method_declaration",
                "constructor_declaration",
                "destructor_declaration",
                "local_function_statement",
            ],
            class_nodes: vec!["class_declaration", "record_declaration"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec!["interface_declaration"],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec!["namespace_declaration", "file_scoped_namespace_declaration"],
            struct_nodes: vec!["struct_declaration"],
            type_alias_nodes: vec!["using_directive", "using_statement"],
            constant_nodes: vec!["field_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["using_directive"],
            call_nodes: vec!["invocation_expression", "object_creation_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_declaration", "field_declaration"],
            constructor_nodes: vec!["constructor_declaration"],
        },
        "php" => LanguageNodeConfig {
            function_nodes: vec![
                "function_definition",
                "method_declaration",
                "arrow_function",
            ],
            class_nodes: vec!["class_declaration"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec!["interface_declaration"],
            trait_nodes: vec!["trait_declaration"],
            module_nodes: vec![],
            namespace_nodes: vec!["namespace_definition", "namespace_use_declaration"],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec!["const_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["use_declaration", "namespace_use_declaration"],
            call_nodes: vec![
                "function_call_expression",
                "member_call_expression",
                "nullsafe_member_call_expression",
                "scoped_call_expression",
                "object_creation_expression",
            ],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_declaration", "property_element"],
            constructor_nodes: vec!["function_definition"],
        },
        "ruby" => LanguageNodeConfig {
            function_nodes: vec!["method", "singleton_method", "lambda", "block"],
            class_nodes: vec!["class", "singleton_class"],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec!["module"],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec!["assignment"],
            macro_nodes: vec![],
            import_nodes: vec![],
            call_nodes: vec!["call"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["instance_variable", "class_variable"],
            constructor_nodes: vec!["method"],
        },
        "swift" => LanguageNodeConfig {
            function_nodes: vec![
                "function_declaration",
                "init_declaration",
                "deinit_declaration",
                "subscript_declaration",
                // A protocol's requirement.
                "protocol_function_declaration",
            ],
            // A class, actor, struct, enum or extension by its
            // `declaration_kind` (`extract_swift_type`).
            class_nodes: vec!["class_declaration"],
            enum_nodes: vec![],
            interface_nodes: vec!["protocol_declaration"],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec!["typealias_declaration"],
            // A `let`, not a `var` (`is_immutable_binding`).
            constant_nodes: vec!["property_declaration"],
            macro_nodes: vec!["macro_declaration"],
            import_nodes: vec!["import_declaration"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment", "multiline_comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_declaration"],
            constructor_nodes: vec!["init_declaration"],
        },
        "kotlin" => LanguageNodeConfig {
            function_nodes: vec!["function_declaration", "secondary_constructor"],
            // An interface and an enum are `class_declaration`s too, told by
            // their keyword (`kotlin_node_is_interface`, `kotlin_node_is_enum`).
            class_nodes: vec![
                "class_declaration",
                "object_declaration",
                "companion_object",
            ],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec!["type_alias"],
            constant_nodes: vec!["property_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["import"],
            call_nodes: vec!["call_expression", "constructor_invocation"],
            documentation_nodes: vec!["line_comment", "block_comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_declaration"],
            constructor_nodes: vec!["secondary_constructor"],
        },
        // Node kinds of tree-sitter-dart 0.2. A function or member with a body
        // is the `*_declaration` holding its signature and body. One without —
        // an abstract method, or a constructor that is only initializers or a
        // redirection — is the `declaration` around its signature. A
        // `declaration` of fields has no signature, so `extract_dart_name`
        // gives it no name and it is not taken for a function.
        "dart" => LanguageNodeConfig {
            function_nodes: vec![
                "function_declaration",
                "local_function_declaration",
                "getter_declaration",
                "setter_declaration",
                "method_declaration",
                "external_function_declaration",
                "external_getter_declaration",
                "external_setter_declaration",
                "declaration",
            ],
            class_nodes: vec!["class_declaration", "extension_type_declaration"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec![],
            trait_nodes: vec!["mixin_declaration"],
            module_nodes: vec!["library_name"],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec!["type_alias"],
            // A top-level `const`/`final`, or a `static const`/`static final`
            // member; locals and instance fields are other kinds.
            constant_nodes: vec!["static_final_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["import_or_export", "import_specification"],
            // Calls, a constructor invoked without `new` or `const` included.
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment", "documentation_block_comment"],
            component_nodes: vec![],
            property_nodes: vec!["initialized_identifier"],
            constructor_nodes: vec![
                "constructor_signature",
                "constant_constructor_signature",
                "factory_constructor_signature",
                "redirecting_factory_constructor_signature",
            ],
        },
        "scala" => LanguageNodeConfig {
            function_nodes: vec!["function_definition", "function_declaration"],
            class_nodes: vec!["class_definition", "object_definition"],
            enum_nodes: vec!["enum_definition"],
            interface_nodes: vec!["trait_definition"],
            trait_nodes: vec![],
            module_nodes: vec!["object_definition"],
            namespace_nodes: vec!["package_clause", "package_object"],
            struct_nodes: vec!["class_definition"],
            type_alias_nodes: vec!["type_definition"],
            constant_nodes: vec!["val_definition", "val_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["import_declaration"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["val_definition", "var_definition"],
            constructor_nodes: vec!["function_definition"],
        },
        // A `type_definition` holds one or more definitions (`type A … and
        // B …`), each of its own kind; a discriminated union is a
        // `union_type_defn` (`defines_a_union`).
        "fsharp" => LanguageNodeConfig {
            function_nodes: vec![
                "function_or_value_defn",
                // A member, and an abstract one.
                "method_or_prop_defn",
                "member_signature",
            ],
            class_nodes: vec!["anon_type_defn", "exception_definition"],
            enum_nodes: vec!["enum_type_defn"],
            interface_nodes: vec!["interface_type_defn"],
            trait_nodes: vec![],
            module_nodes: vec!["named_module", "module_defn"],
            namespace_nodes: vec!["namespace"],
            struct_nodes: vec!["record_type_defn"],
            type_alias_nodes: vec!["type_abbrev_defn", "delegate_type_defn"],
            constant_nodes: vec!["function_or_value_defn"],
            macro_nodes: vec![],
            import_nodes: vec!["import_decl"],
            call_nodes: vec!["application_expression"],
            documentation_nodes: vec!["line_comment", "block_comment"],
            component_nodes: vec![],
            property_nodes: vec!["record_field"],
            constructor_nodes: vec!["additional_constr_defn"],
        },
        "solidity" => LanguageNodeConfig {
            function_nodes: vec![
                "function_definition",
                "constructor_definition",
                // `receive()` and `fallback()`.
                "fallback_receive_definition",
                "modifier_definition",
            ],
            // `interface_declaration` is intentionally NOT here: the class branch
            // is checked before `interface_nodes`, so listing it would mislabel a
            // Solidity `interface` as a class and turn `contract X is IFace` into
            // `inherits` instead of `implements`. It is resolved via
            // `interface_nodes` below.
            class_nodes: vec!["contract_declaration", "library_declaration"],
            enum_nodes: vec!["enum_declaration"],
            interface_nodes: vec!["interface_declaration"],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec!["struct_declaration"],
            type_alias_nodes: vec!["user_defined_type_definition"],
            // A state variable is one when it is `constant` or `immutable`; a
            // file-level `constant` always.
            constant_nodes: vec![
                "state_variable_declaration",
                "constant_variable_declaration",
            ],
            // A modifier's definition is a function (`function_nodes`); a use
            // in a function's header is a call of it, not a definition.
            macro_nodes: vec![],
            import_nodes: vec!["import_directive", "using_directive"],
            call_nodes: vec!["call_expression", "modifier_invocation"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["state_variable_declaration", "event_parameter"],
            constructor_nodes: vec!["constructor_definition"],
        },
        "graphql" => LanguageNodeConfig {
            // Definitions only. In a query document the selected `field`s and
            // `directive` uses are references into the schema, like
            // `named_type` below: indexed as definitions, every selection would be a
            // function, every directive use a macro and every argument a constant.
            // They are call nodes instead.
            function_nodes: vec![
                "field_definition",
                "operation_definition",
                "fragment_definition",
            ],
            // An `extend type` is an extension of the type (`defines_an_extension`).
            class_nodes: vec!["object_type_definition", "interface_type_definition"],
            enum_nodes: vec!["enum_type_definition", "enum_value_definition"],
            interface_nodes: vec!["interface_type_definition"],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec!["input_object_type_definition"],
            // `named_type` is a reference (`subtotal: Cents!`), not a definition:
            // listed here, every use of a type would be a type alias of its own, and a
            // call into the schema would resolve to whichever of a file's same-named
            // "definitions" sorted first.
            type_alias_nodes: vec!["scalar_type_definition"],
            constant_nodes: vec!["enum_value_definition"],
            macro_nodes: vec!["directive_definition"],
            import_nodes: vec![],
            call_nodes: vec!["field", "directive"],
            documentation_nodes: vec!["comment", "description", "string_value"],
            component_nodes: vec![],
            property_nodes: vec!["field_definition"],
            constructor_nodes: vec![],
        },
        "objc" | "objective_c" => LanguageNodeConfig {
            function_nodes: vec![
                "function_definition",
                "method_declaration",
                "method_definition",
            ],
            class_nodes: vec![
                "class_interface",
                "class_implementation",
                "class_declaration",
            ],
            enum_nodes: vec!["enum_specifier"],
            interface_nodes: vec!["protocol_declaration"],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec!["struct_specifier"],
            type_alias_nodes: vec!["type_definition"],
            constant_nodes: vec!["declaration", "property_declaration"],
            macro_nodes: vec!["preproc_def", "preproc_function_def", "preproc_call"],
            import_nodes: vec!["preproc_include", "module_import"],
            call_nodes: vec![
                "call_expression",
                "message_expression",
                "selector_expression",
            ],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_declaration", "property_attribute"],
            constructor_nodes: vec!["method_definition", "function_definition"],
        },
        "sql" => LanguageNodeConfig {
            function_nodes: vec!["create_function"],
            class_nodes: vec![
                "create_table",
                "create_view",
                "create_materialized_view",
                // Re-tagged to `Entity::Index` by the extractor.
                "create_index",
            ],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec!["create_schema"],
            namespace_nodes: vec![],
            // Columns are attributes of their table, not standalone structs.
            // Extracting each produced collision-prone noise (`id` × N) typed
            // misleadingly as `struct`; they remain in the table entity's body,
            // so they are still searchable/embedded through the table itself.
            struct_nodes: vec![],
            // `CREATE TYPE … AS ENUM` is an enum (`extract_sql_type`).
            type_alias_nodes: vec!["create_type"],
            constant_nodes: vec![],
            macro_nodes: vec![],
            import_nodes: vec![],
            // A function's use.
            call_nodes: vec!["invocation"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["column_definition", "column"],
            constructor_nodes: vec![],
        },
        "html" => LanguageNodeConfig {
            function_nodes: vec!["script_element"],
            class_nodes: vec![],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec![],
            macro_nodes: vec![],
            import_nodes: vec![],
            call_nodes: vec![],
            documentation_nodes: vec!["comment", "doctype"],
            component_nodes: vec!["element", "self_closing_tag"],
            property_nodes: vec!["attribute"],
            constructor_nodes: vec![],
        },
        // Note: "make" and other simple/config languages (json, yaml, toml, xml, css, scss, cmake)
        // are detected for file type identification but not parsed with tree-sitter.
        // They are handled as "simple files" without AST-based entity extraction.
        // See SIMPLE_FILE_LANGUAGES and is_simple_file_language() for the list.
        // ========== NEW LANGUAGE CONFIGURATIONS ==========
        // Web framework languages
        // The kinds of the component's own document: its scripts are parsed in
        // their own languages (`extract_scripts`).
        "vue" => LanguageNodeConfig {
            function_nodes: vec![],
            class_nodes: vec![],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec![],
            macro_nodes: vec![],
            import_nodes: vec![],
            call_nodes: vec![],
            documentation_nodes: vec!["comment", "text"],
            component_nodes: vec!["document"],
            property_nodes: vec!["attribute", "directive_attribute"],
            constructor_nodes: vec![],
        },
        // The kinds of the component's own document: its scripts are parsed in
        // their own languages (`extract_scripts`).
        "svelte" => LanguageNodeConfig {
            function_nodes: vec![],
            class_nodes: vec![],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec![],
            macro_nodes: vec![],
            import_nodes: vec![],
            call_nodes: vec![],
            documentation_nodes: vec!["comment", "text"],
            component_nodes: vec!["document"],
            property_nodes: vec!["attribute"],
            constructor_nodes: vec![],
        },
        // .NET languages
        "vb_net" => LanguageNodeConfig {
            function_nodes: vec!["method_declaration", "constructor_declaration"],
            class_nodes: vec!["class_block"],
            enum_nodes: vec!["enum_block"],
            interface_nodes: vec!["interface_block"],
            trait_nodes: vec![],
            module_nodes: vec!["module_block"],
            namespace_nodes: vec!["namespace_block"],
            struct_nodes: vec!["structure_block"],
            type_alias_nodes: vec!["delegate_declaration"],
            constant_nodes: vec!["const_declaration", "field_declaration"],
            macro_nodes: vec![],
            import_nodes: vec!["imports_statement"],
            call_nodes: vec!["invocation"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["property_declaration", "field_declaration"],
            constructor_nodes: vec!["constructor_declaration"],
        },
        // Systems languages
        "cuda" => LanguageNodeConfig {
            function_nodes: vec!["function_definition"],
            class_nodes: vec!["class_specifier"],
            enum_nodes: vec!["enum_specifier"],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec!["namespace_definition"],
            struct_nodes: vec!["struct_specifier"],
            type_alias_nodes: vec!["type_definition"],
            constant_nodes: vec!["declaration", "init_declarator"],
            macro_nodes: vec!["preproc_def", "preproc_function_def"],
            import_nodes: vec!["preproc_include"],
            call_nodes: vec!["call_expression"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec!["field_declaration"],
            constructor_nodes: vec!["function_definition"],
        },
        // Shell scripting
        "bash" => LanguageNodeConfig {
            // "function_definition" covers both forms: `name() { ... }` and
            // `function name { ... }`. Not "function", the keyword child of
            // function_definition, nor "command" and "pipeline", which would make every
            // shell invocation a function-extraction attempt.
            function_nodes: vec!["function_definition"],
            class_nodes: vec![],
            enum_nodes: vec![],
            interface_nodes: vec![],
            trait_nodes: vec![],
            module_nodes: vec![],
            namespace_nodes: vec![],
            struct_nodes: vec![],
            type_alias_nodes: vec![],
            constant_nodes: vec!["variable_assignment", "declaration_command"],
            macro_nodes: vec![],
            import_nodes: vec!["command_substitution"],
            call_nodes: vec!["command", "subshell", "command_substitution"],
            documentation_nodes: vec!["comment"],
            component_nodes: vec![],
            property_nodes: vec![],
            constructor_nodes: vec![],
        },
        _ => LanguageNodeConfig::default(),
    }
}

/// Every mapped extension with the language it maps to.
pub fn mapped_extensions() -> impl Iterator<Item = (&'static str, &'static str)> {
    EXTENSION_MAP.iter().map(|(&ext, &lang)| (ext, lang))
}

/// Convert file extension to tree-sitter language name
pub fn extension_to_language(extension: &str) -> Option<String> {
    EXTENSION_MAP
        .get(extension.to_lowercase().as_str())
        .map(|&s| s.to_string())
}

/// Convert filename (without extension) to tree-sitter language name
/// Used for files like "Makefile", "Dockerfile", etc.
pub fn filename_to_language(filename: &str) -> Option<String> {
    FILENAME_MAP
        .get(filename.to_lowercase().as_str())
        .map(|&s| s.to_string())
}

/// The language of `filename` by its extension, except that a `.m` file whose
/// name says Objective-C (`objc`, `objective`) is Objective-C. `.m` is not in
/// the extension map, since MATLAB uses it too: any other `.m` file is told by
/// its content ([`content_is_objc`]).
pub fn detect_language_from_filename(filename: &str) -> Option<String> {
    let path = std::path::Path::new(filename);
    let stem = path.file_stem()?.to_str()?.to_lowercase();
    let ext = path.extension()?.to_str()?.to_lowercase();
    if ext == "m" && (stem.contains("objective") || stem.contains("objc")) {
        return Some("objc".to_string());
    }
    extension_to_language(&ext)
}

/// Content-based sniffer: returns `true` when the content looks like
/// Objective-C (presence of `@interface`, `@implementation`, or `#import`).
/// It resolves the `.m` ambiguity (MATLAB vs. Objective-C) when the filename
/// alone doesn't identify the language.
pub fn content_is_objc(content: &str) -> bool {
    // Scan only the first 4 KiB — enough to see any real ObjC header. Byte
    // 4096 may fall inside a character, so cut at the boundary before it.
    let head = &content[..content.floor_char_boundary(4096)];
    head.contains("@interface") || head.contains("@implementation") || head.contains("#import")
}

/// The group of languages whose code can name each other's symbols: the
/// web languages (JavaScript, TypeScript and the Vue and Svelte components
/// that script in them), the C family (C, C++, CUDA, Objective-C), the JVM
/// languages and the .NET ones. Any other language is a group of its own. A
/// name used in another group's file is another symbol of the same name.
#[must_use]
pub fn reference_family(language: &str) -> &str {
    match language {
        "javascript" | "typescript" | "tsx" | "jsx" | "vue" | "svelte" => "web",
        "c" | "cpp" | "cuda" | "objc" | "objective_c" => "c",
        "java" | "kotlin" | "scala" => "jvm",
        "c_sharp" | "fsharp" | "vb_net" => "dotnet",
        other => other,
    }
}

/// Get all supported languages
pub fn supported_languages() -> Vec<String> {
    super::language_loader::available_languages()
}

/// Check if a language is supported
pub fn is_language_supported(language: &str) -> bool {
    super::language_loader::has_language(language)
}

/// Check if a node type matches any of the given patterns
pub fn matches_node_type(node_type: &str, patterns: &[&str]) -> bool {
    patterns.contains(&node_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vue and Svelte script in JavaScript or TypeScript, so a name in one
    /// can be a symbol of the other; Kotlin calls Java; a Python name is
    /// never a JavaScript symbol.
    #[test]
    fn reference_families_group_the_languages_that_name_each_others_symbols() {
        for language in ["javascript", "typescript", "tsx", "vue", "svelte"] {
            assert_eq!(reference_family(language), "web", "{language}");
        }
        assert_eq!(reference_family("kotlin"), reference_family("java"));
        assert_eq!(reference_family("cuda"), reference_family("cpp"));
        assert_eq!(reference_family("fsharp"), reference_family("c_sharp"));
        assert_eq!(reference_family("python"), "python");
        assert_ne!(reference_family("python"), reference_family("javascript"));
    }

    /// Every node kind a language's configuration names is one its grammar
    /// has. A misspelt or outdated kind matches nothing, and says nothing: a
    /// whole kind of declaration would go unindexed.
    #[test]
    fn every_configured_kind_is_one_its_grammar_has() {
        let mut dead = Vec::new();
        for &lang in SUPPORTED_LANGS {
            // A grammar this build leaves out has nothing to check against.
            let Ok(grammar) = crate::parser::language_loader::get_language(lang) else {
                continue;
            };
            let config = get_language_config(lang);
            let lists = [
                ("function_nodes", &config.function_nodes),
                ("class_nodes", &config.class_nodes),
                ("enum_nodes", &config.enum_nodes),
                ("interface_nodes", &config.interface_nodes),
                ("trait_nodes", &config.trait_nodes),
                ("module_nodes", &config.module_nodes),
                ("namespace_nodes", &config.namespace_nodes),
                ("struct_nodes", &config.struct_nodes),
                ("type_alias_nodes", &config.type_alias_nodes),
                ("constant_nodes", &config.constant_nodes),
                ("macro_nodes", &config.macro_nodes),
                ("import_nodes", &config.import_nodes),
                ("call_nodes", &config.call_nodes),
                ("documentation_nodes", &config.documentation_nodes),
                ("component_nodes", &config.component_nodes),
                ("property_nodes", &config.property_nodes),
                ("constructor_nodes", &config.constructor_nodes),
            ];
            for (list, kinds) in lists {
                for kind in kinds {
                    if grammar.id_for_node_kind(kind, true) == 0 {
                        dead.push(format!("{lang}.{list}: {kind}"));
                    }
                }
            }
        }
        assert!(
            dead.is_empty(),
            "kinds no grammar has:\n{}",
            dead.join("\n")
        );
    }

    #[test]
    fn test_extension_to_language() {
        assert_eq!(extension_to_language("rs"), Some("rust".to_string()));
        assert_eq!(extension_to_language("py"), Some("python".to_string()));
        assert_eq!(extension_to_language("js"), Some("javascript".to_string()));
        assert_eq!(extension_to_language("unknown"), None);
    }

    #[test]
    fn test_language_config() {
        let config = get_language_config("python");
        assert!(!config.function_nodes.is_empty());
        assert!(config.function_nodes.contains(&"function_definition"));
    }

    #[test]
    fn test_matches_node_type() {
        assert!(matches_node_type(
            "function_definition",
            &["function_definition", "class_definition"]
        ));
        assert!(!matches_node_type(
            "function_definition",
            &["class_definition"]
        ));
    }

    /// 4.4 AC: content_is_objc detects ObjC markers and ignores MATLAB content.
    #[test]
    fn test_content_is_objc_sniffer() {
        assert!(content_is_objc(
            "#import <Foundation/Foundation.h>\n@interface Foo : NSObject\n@end"
        ));
        assert!(content_is_objc(
            "@interface ViewController : UIViewController\n@end"
        ));
        assert!(content_is_objc("@implementation Foo\n- (void)bar {}\n@end"));
        // MATLAB content must NOT be detected as ObjC
        assert!(!content_is_objc(
            "function y = myFunc(x)\n  y = x * 2;\nend"
        ));
        assert!(!content_is_objc(""));
    }

    /// The 4 KiB cut falls on a character boundary: at byte 4096 a multi-byte
    /// character straddling it would panic.
    #[test]
    fn content_is_objc_cuts_at_a_character_boundary() {
        // 4095 ASCII bytes, then `é` (two bytes) across the 4096 cut.
        let content = format!("%{}é\nfunction y = f(x)\nend\n", "x".repeat(4094));
        assert!(!content_is_objc(&content));
    }
}
