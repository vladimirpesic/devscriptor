//! Individual tree-sitter language loader
//!
//! This module provides language loading functionality by directly importing
//! individual tree-sitter language crates from crates.io.
//!
//! Architecture:
//! - Each language crate is declared as a dependency in Cargo.toml
//! - Languages are registered in the LANGUAGE_LOADERS map at compile time
//! - Each grammar crate exports its `LANGUAGE` constant, which tree-sitter 0.27 loads
//! - All 26 languages are statically linked into the binary, via 26 tree-sitter
//!   grammar crates, producing 27 registered parsers (the TypeScript crate
//!   supplies both `typescript` and `tsx`)

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::LazyLock;
use tree_sitter::Language;

/// Type alias for language loader functions
type LanguageLoader = fn() -> Language;

/// Map of language names to their loader functions.
static LANGUAGE_LOADERS: LazyLock<HashMap<&'static str, LanguageLoader>> = LazyLock::new(|| {
    // Every `map.insert` below is gated on a `lang-*` feature, so a build with no
    // language features (`--no-default-features`) never mutates the map. `mut` is
    // required by every build that enables at least one language — i.e. every
    // shipped build — so the binding cannot simply drop it.
    #[allow(unused_mut)]
    let mut map: HashMap<&'static str, LanguageLoader> = HashMap::new();

    // Web/JavaScript ecosystem
    #[cfg(feature = "lang-javascript")]
    map.insert("javascript", || tree_sitter_javascript::LANGUAGE.into());

    // TypeScript / TSX — use the dedicated grammar, not the JS fallback
    #[cfg(feature = "lang-typescript")]
    map.insert("typescript", || {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    });
    #[cfg(feature = "lang-typescript")]
    map.insert("tsx", || tree_sitter_typescript::LANGUAGE_TSX.into());

    // Python
    #[cfg(feature = "lang-python")]
    map.insert("python", || tree_sitter_python::LANGUAGE.into());

    // Rust
    #[cfg(feature = "lang-rust")]
    map.insert("rust", || tree_sitter_rust::LANGUAGE.into());

    // Java
    #[cfg(feature = "lang-java")]
    map.insert("java", || tree_sitter_java::LANGUAGE.into());

    // C/C++
    #[cfg(feature = "lang-c")]
    map.insert("c", || tree_sitter_c::LANGUAGE.into());
    #[cfg(feature = "lang-cpp")]
    map.insert("cpp", || tree_sitter_cpp::LANGUAGE.into());

    // C#
    #[cfg(feature = "lang-c-sharp")]
    map.insert("c_sharp", || tree_sitter_c_sharp::LANGUAGE.into());

    // Ruby
    #[cfg(feature = "lang-ruby")]
    map.insert("ruby", || tree_sitter_ruby::LANGUAGE.into());

    // PHP
    #[cfg(feature = "lang-php")]
    map.insert("php", || tree_sitter_php::LANGUAGE_PHP.into());

    // Swift
    #[cfg(feature = "lang-swift")]
    map.insert("swift", || tree_sitter_swift::LANGUAGE.into());

    // Kotlin
    #[cfg(feature = "lang-kotlin")]
    map.insert("kotlin", || tree_sitter_kotlin_ng::LANGUAGE.into());

    // Scala
    #[cfg(feature = "lang-scala")]
    map.insert("scala", || tree_sitter_scala::LANGUAGE.into());

    // Dart
    #[cfg(feature = "lang-dart")]
    map.insert("dart", || tree_sitter_dart::LANGUAGE.into());

    // Shell/Bash
    #[cfg(feature = "lang-bash")]
    map.insert("bash", || tree_sitter_bash::LANGUAGE.into());

    // HTML
    #[cfg(feature = "lang-html")]
    map.insert("html", || tree_sitter_html::LANGUAGE.into());

    // SQL
    #[cfg(feature = "lang-sql")]
    map.insert("sql", || tree_sitter_sequel::LANGUAGE.into());

    // Solidity
    #[cfg(feature = "lang-solidity")]
    map.insert("solidity", || tree_sitter_solidity::LANGUAGE.into());

    // F#
    #[cfg(feature = "lang-fsharp")]
    map.insert("fsharp", || tree_sitter_fsharp::LANGUAGE_FSHARP.into());

    // Go
    #[cfg(feature = "lang-go")]
    map.insert("go", || tree_sitter_go::LANGUAGE.into());

    // GraphQL
    #[cfg(feature = "lang-graphql")]
    map.insert("graphql", || tree_sitter_graphql::LANGUAGE.into());

    // Objective-C
    #[cfg(feature = "lang-objc")]
    map.insert("objc", || tree_sitter_objc::LANGUAGE.into());

    // CUDA
    #[cfg(feature = "lang-cuda")]
    map.insert("cuda", || tree_sitter_cuda::LANGUAGE.into());

    // Svelte
    #[cfg(feature = "lang-svelte")]
    map.insert("svelte", || tree_sitter_svelte_next::LANGUAGE.into());

    // Vue
    #[cfg(feature = "lang-vue")]
    map.insert("vue", || tree_sitter_vue_next::LANGUAGE.into());

    // VB.NET
    #[cfg(feature = "lang-vb")]
    map.insert("vb_net", || tree_sitter_vb_dotnet::LANGUAGE.into());

    map
});

/// Get a tree-sitter Language by name
///
/// # Arguments
/// * `name` - The language name (e.g., "rust", "python", "javascript")
///
/// # Returns
/// * `Ok(Language)` - The tree-sitter Language object
/// * `Err(Error)` - If the language is not supported
pub fn get_language(name: &str) -> Result<Language> {
    // Normalize language name
    let normalized = normalize_language_name(name);

    LANGUAGE_LOADERS
        .get(normalized)
        .map(|loader| loader())
        .ok_or_else(|| Error::UnsupportedLanguage(format!("Language '{}' is not supported", name)))
}

/// Check if a language is supported
///
/// # Arguments
/// * `name` - The language name to check
///
/// # Returns
/// `true` if the language is supported, `false` otherwise
pub fn has_language(name: &str) -> bool {
    let normalized = normalize_language_name(name);
    LANGUAGE_LOADERS.contains_key(normalized)
}

/// Get a list of all supported language names
///
/// # Returns
/// A sorted vector of language names
pub fn available_languages() -> Vec<String> {
    let mut languages: Vec<String> = LANGUAGE_LOADERS.keys().map(|&k| k.to_string()).collect();
    languages.sort();
    languages
}

/// Normalize language name for lookup
///
/// Computes lowercase once and reuses it for both alias matching and map lookup,
/// avoiding a redundant `to_lowercase()` allocation in the default case.
fn normalize_language_name(name: &str) -> &str {
    let lower = name.to_lowercase();
    match lower.as_str() {
        // TypeScript aliases → dedicated TS grammar (lang-typescript).
        // Falls back to the JS grammar only when lang-typescript is absent.
        "ts" | "typescript" => {
            #[cfg(feature = "lang-typescript")]
            return "typescript";
            #[cfg(all(not(feature = "lang-typescript"), feature = "lang-javascript"))]
            return "javascript";
            #[cfg(all(not(feature = "lang-typescript"), not(feature = "lang-javascript")))]
            return name;
        }

        // TSX aliases → dedicated TSX grammar (lang-typescript).
        "tsx" => {
            #[cfg(feature = "lang-typescript")]
            return "tsx";
            #[cfg(all(not(feature = "lang-typescript"), feature = "lang-javascript"))]
            return "javascript";
            #[cfg(all(not(feature = "lang-typescript"), not(feature = "lang-javascript")))]
            return name;
        }

        // JavaScript aliases
        "js" => {
            #[cfg(feature = "lang-javascript")]
            return "javascript";
            #[cfg(not(feature = "lang-javascript"))]
            return name;
        }

        // C++ aliases
        "c++" | "cxx" => {
            #[cfg(feature = "lang-cpp")]
            return "cpp";
            #[cfg(not(feature = "lang-cpp"))]
            return name;
        }

        // C# aliases
        "c#" | "csharp" | "cs" => {
            #[cfg(feature = "lang-c-sharp")]
            return "c_sharp";
            #[cfg(not(feature = "lang-c-sharp"))]
            return name;
        }

        // F# aliases
        "f#" | "fsharp" | "fs" => {
            #[cfg(feature = "lang-fsharp")]
            return "fsharp";
            #[cfg(not(feature = "lang-fsharp"))]
            return name;
        }

        // Go aliases
        "golang" => {
            #[cfg(feature = "lang-go")]
            return "go";
            #[cfg(not(feature = "lang-go"))]
            return name;
        }

        // Shell/Bash aliases
        "sh" | "shell" | "zsh" => {
            #[cfg(feature = "lang-bash")]
            return "bash";
            #[cfg(not(feature = "lang-bash"))]
            return name;
        }

        // Objective-C aliases
        "objective-c" | "objectivec" | "objective_c" => {
            #[cfg(feature = "lang-objc")]
            return "objc";
            #[cfg(not(feature = "lang-objc"))]
            return name;
        }

        // Kotlin alias
        "kt" => {
            #[cfg(feature = "lang-kotlin")]
            return "kotlin";
            #[cfg(not(feature = "lang-kotlin"))]
            return name;
        }

        // VB.NET aliases
        "vb" | "visual_basic" | "vbnet" => {
            #[cfg(feature = "lang-vb")]
            return "vb_net";
            #[cfg(not(feature = "lang-vb"))]
            return name;
        }

        // Default: check if lowercase version exists in map
        _ => {
            if LANGUAGE_LOADERS.contains_key(lower.as_str()) {
                // Return a reference that lives long enough
                // This is safe because the key strings in LANGUAGE_LOADERS are static
                return LANGUAGE_LOADERS
                    .keys()
                    .find(|&&k| k == lower.as_str())
                    .copied()
                    .unwrap_or(name);
            }
            name
        }
    }
}

/// Get the number of supported languages
pub fn language_count() -> usize {
    LANGUAGE_LOADERS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_language_supported() {
        // Test that we can get a supported language
        let lang = get_language("rust");
        assert!(lang.is_ok(), "Rust language should be supported");
    }

    #[test]
    fn test_get_language_unsupported() {
        // Test that unsupported languages return an error
        let lang = get_language("unknown_language_xyz");
        assert!(lang.is_err(), "Unknown language should return error");
    }

    #[test]
    fn test_has_language() {
        assert!(has_language("rust"), "Rust should be supported");
        assert!(has_language("python"), "Python should be supported");
        assert!(
            !has_language("unknown_xyz"),
            "Unknown language should not be supported"
        );
    }

    #[test]
    fn test_available_languages() {
        let languages = available_languages();
        assert!(!languages.is_empty(), "Should have some languages");
        assert!(
            languages.contains(&"rust".to_string()),
            "Should contain rust"
        );
        assert!(
            languages.contains(&"python".to_string()),
            "Should contain python"
        );
    }

    #[test]
    fn test_language_count() {
        let count = language_count();
        assert!(count > 0, "Should have at least one language");
        // Default features (`full-lang`) register 27 parsers from 26 grammar
        // crates — the TypeScript crate supplies both `typescript` and `tsx`.
        // With a narrower `lang-*` selection the count is smaller, so assert only
        // that the reported count matches the map.
        assert_eq!(count, LANGUAGE_LOADERS.len(), "Count should match map size");
    }
}
