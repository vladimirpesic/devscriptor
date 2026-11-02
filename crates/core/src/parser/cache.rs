//! Language cache for Tree-sitter parsers.
//!
//! Tree-sitter `Parser` objects are not `Send` and carry per-parse state, so a
//! single `Mutex<Parser>` shared across threads would serialize all parsing of
//! one language: for a repo that is 80% TypeScript, `max_concurrency =
//! num_cpus` would collapse to 1.
//!
//! So the cache holds the *language* (a cheap FFI pointer), not the parser.
//! Each caller creates a fresh `Parser` from the cached `Language`, calling
//! `Parser::new()` and `set_language()` in O(1), and several threads parse the
//! same language at once.

use crate::error::{Error, Result};
use moka::sync::Cache;
use tree_sitter::{Language, Parser};

/// Thread-safe cache that stores `tree_sitter::Language` objects (FFI
/// pointers, `Copy`) keyed by language name string.
///
/// Callers call [`get_language`](ParserCache::get_language) to get the
/// compiled grammar, then build a throwaway `Parser` from it.  This way
/// `Parser` is never shared across threads.
pub struct ParserCache {
    cache: Cache<String, Language>,
}

impl ParserCache {
    /// Create a new cache with the specified capacity (number of languages).
    pub fn new(capacity: usize) -> Self {
        let cache = Cache::builder().max_capacity(capacity as u64).build();
        Self { cache }
    }

    /// Return the cached `Language` for `language_name`, loading it on first
    /// access.
    ///
    /// # Errors
    /// Returns an error if the language is unsupported or the FFI load fails.
    pub fn get_language(&self, language_name: &str) -> Result<Language> {
        if let Some(lang) = self.cache.get(language_name) {
            tracing::debug!("Language cache hit: {}", language_name);
            return Ok(lang);
        }

        tracing::debug!("Loading tree-sitter language: {}", language_name);
        let lang = super::language_loader::get_language(language_name).map_err(|e| {
            tracing::error!("Failed to get language '{}': {:?}", language_name, e);
            Error::UnsupportedLanguage(format!("{language_name}: {e}"))
        })?;

        self.cache.insert(language_name.to_string(), lang.clone());
        Ok(lang)
    }

    /// Create a ready-to-use `Parser` for `language_name`.
    ///
    /// The returned `Parser` is owned by the caller and never shared — callers
    /// may use it freely without any locking.
    ///
    /// # Errors
    /// Returns an error if the language cannot be loaded or set on the parser.
    pub fn get_parser(&self, language_name: &str) -> Result<Parser> {
        let lang = self.get_language(language_name)?;
        let mut parser = Parser::new();
        parser
            .set_language(&lang)
            .map_err(|e| Error::Parser(format!("Failed to set language '{language_name}': {e}")))?;
        Ok(parser)
    }

    /// Remove all cached language entries.
    pub fn clear(&self) {
        self.cache.invalidate_all();
    }
}
