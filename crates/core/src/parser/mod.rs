//! Tree-sitter parser integration with entity cleaning and validation
//!
//! This module provides parsing functionality for extracting entities and relationships
//! from source code files. It includes support for:
//!
//! - **Caching**: LRU cache for parser instances to improve performance
//! - **Cleaning**: Entity normalization and size limiting to prevent memory issues
//! - **Validation**: Entity quality checks to ensure extracted data is valid
//!
//! # Example
//!
//! ```rust,no_run
//! use devscriptor_core::parser::{TreeSitterParser, ParserConfig};
//! use std::path::Path;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let config = ParserConfig {
//!     enable_cache: true,
//!     cache_size: 100,
//!     mmap_threshold: 10 * 1024 * 1024,
//!     enable_cleaning: true,
//!     enable_validation: true,
//!     skip_invalid_entities: true,
//!     emit_decorators: false,
//! };
//!
//! let parser = TreeSitterParser::new(config)?;
//! let result = parser.parse_file(Path::new("src/main.rs")).await?;
//! # Ok(())
//! # }
//! ```

pub mod cache;
pub mod cleaning;
pub mod cross_file;
pub mod entity_extraction;
/// Cross-language import and dependency extraction.
pub mod import_extraction;
pub mod language_config;
pub mod language_loader;
pub mod mmap_parser;
pub mod prose;
pub mod relationship_extraction;
pub mod standard_methods;
pub mod symbol_table;
pub mod validation;

// Re-export cross-file analysis types
pub use cross_file::{CrossFileMode, ExternalSymbol, SymbolResolution};
pub use import_extraction::{ImportInfo, extract_imports};
pub use symbol_table::{SymbolEntry, SymbolTable};

// Re-export mmap parser types
pub use mmap_parser::{MmapReader, MmapStats, MmapStr};

// Re-export shared parsing constants
pub mod constants;
pub use constants::{
    MAX_BODY_HARD_LIMIT as max_body_hard_limit, MAX_BODY_LENGTH as max_body_length,
    MAX_CONFIDENCE_SCORE as max_confidence_score, MAX_EXTENDS_LENGTH as max_extends_length,
    MAX_FIELDS_DICT_LENGTH as max_fields_dict_length,
    MAX_FIELDS_STRING_LENGTH as max_fields_string_length,
    MAX_IMPLEMENTS_LENGTH as max_implements_length, MAX_NAME_LENGTH as max_name_length,
    MAX_PARAMETER_LIST_LENGTH as max_parameter_list_length,
    MAX_PARAMETER_STRING_LENGTH as max_parameter_string_length,
    MAX_RETURN_TYPE_LENGTH as max_return_type_length, MIN_CONFIDENCE_SCORE as min_confidence_score,
    TRUNCATION_MARKER, VALID_MACRO_TYPES,
};

use crate::entities::{Entity, FileInfo, Relationship, SymbolRef};
use crate::error::{Error, Result};
use std::path::Path;
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, warn};
use xxhash_rust::xxh3::xxh3_64;

// Re-export key cleaning functions
pub use cleaning::{body_exceeds_limit, clean_entities, clean_entity, validate_and_clean_name};

// Re-export key validation functions
pub use validation::{
    ValidationFailure, ValidationResult, filter_valid_entities, validate_body_content,
    validate_confidence_score, validate_entity, validate_entity_name_length, validate_line_numbers,
    validate_parameter_string, validate_relationship, validate_return_type,
    validate_source_location,
};

/// Parser configuration options
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Enable parser caching
    pub enable_cache: bool,
    /// Cache size limit (number of parsers to cache)
    pub cache_size: usize,
    /// Memory-map files larger than this threshold (bytes)
    pub mmap_threshold: usize,
    /// Enable entity cleaning (normalization and size limiting)
    pub enable_cleaning: bool,
    /// Enable entity validation
    pub enable_validation: bool,
    /// Skip entities that fail validation (if false, invalid entities are kept with warnings)
    pub skip_invalid_entities: bool,
    /// Emit decorator/annotation nodes as standalone `Decorator` entities.
    ///
    /// When `false` (the default), decorator nodes are **not** emitted as separate
    /// entities.  The decorated function or class is still extracted normally.
    /// `true` emits them too; decorators then dominate the semantic index (54% of
    /// all entities on a Python repository, for example).
    pub emit_decorators: bool,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            // Reduced in test builds: integration tests typically analyse a
            // small, fixed language set and do not need 100 cached parsers.
            cache_size: if cfg!(test) { 10 } else { 50 },
            mmap_threshold: 10 * 1024 * 1024, // 10MB
            enable_cleaning: true,
            enable_validation: true,
            skip_invalid_entities: true,
            emit_decorators: false,
        }
    }
}

/// Language-aware Tree-sitter parser with caching support
#[derive(Clone)]
pub struct TreeSitterParser {
    /// Parser configuration
    config: ParserConfig,
    /// LRU cache for parser instances
    cache: Arc<cache::ParserCache>,
    /// How many of several same-named definitions in a file a reference is
    /// linked to (see [`Self::with_ambiguous_fanout`]).
    ambiguous_fanout: usize,
}

impl TreeSitterParser {
    /// Create a new parser with the given configuration
    ///
    /// # Arguments
    ///
    /// * `config` - Parser configuration options
    ///
    /// # Returns
    ///
    /// A new `TreeSitterParser` instance
    ///
    /// # Errors
    ///
    /// Returns an error if the parser cache cannot be initialized
    pub fn new(config: ParserConfig) -> Result<Self> {
        let cache = Arc::new(cache::ParserCache::new(config.cache_size));
        Ok(Self {
            config,
            cache,
            ambiguous_fanout: symbol_table::DEFAULT_AMBIGUOUS_FANOUT,
        })
    }

    /// Link a reference to at most `fanout` of several same-named
    /// definitions in its file, earliest first, each edge marked ambiguous;
    /// 1 links the earliest. 0 counts as 1. The default is
    /// [`DEFAULT_AMBIGUOUS_FANOUT`](symbol_table::DEFAULT_AMBIGUOUS_FANOUT).
    #[must_use]
    pub fn with_ambiguous_fanout(mut self, fanout: usize) -> Self {
        self.ambiguous_fanout = fanout.max(1);
        self
    }

    /// Parse a file and extract entities
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file to parse
    ///
    /// # Returns
    ///
    /// A `ParsedFile` containing file info, entities, and relationships
    ///
    /// # Errors
    ///
    /// Returns an error if file reading or parsing fails
    /// Parse a file and extract entities (async, offloads blocking I/O).
    pub async fn parse_file(&self, path: &Path) -> Result<ParsedFile> {
        let this = self.clone();
        let path_buf = path.to_path_buf();
        tokio::task::spawn_blocking(move || this.parse_file_sync(&path_buf))
            .await
            .map_err(|e| Error::Generic(format!("Blocking task failed: {e}")))?
    }

    /// Synchronous file parser — no async or tokio runtime required.
    pub fn parse_file_sync(&self, path: &Path) -> Result<ParsedFile> {
        // Read content first so we can fall back to content sniffing for
        // ambiguous extensions (.m = MATLAB vs. Objective-C).
        let content = self.read_file_content(path)?;
        let language = self.detect_language(path).or_else(|e| {
            // For .m files whose filename gives no signal, sniff the content.
            let is_m = path.extension().and_then(|x| x.to_str()) == Some("m");
            if is_m && language_config::content_is_objc(&content) {
                Ok("objc".to_string())
            } else {
                Err(e)
            }
        })?;
        self.parse_content_large_stack(&content, &language, path)
    }

    /// Run [`Self::parse_content`] on a dedicated thread with a large stack
    /// ([`constants::PARSE_THREAD_STACK_SIZE`]).
    ///
    /// This is the universal SIGBUS/SIGSEGV guard.  Tree-sitter parses input
    /// with deep C-level recursion; on pathological or machine-generated sources
    /// (deeply-nested initializers, long macro chains) this can overflow the
    /// default ~2 MB thread stack and raise an **uncatchable** hardware signal
    /// that aborts the entire process — no matter the codebase.  Giving the parse
    /// ample stack headroom turns those files into successful parses instead of
    /// crashes.  A Rust panic on the parse thread is also caught here and turned
    /// into a recoverable error, so a single bad file can never take down the run.
    fn parse_content_large_stack(
        &self,
        content: &str,
        language: &str,
        path: &Path,
    ) -> Result<ParsedFile> {
        // Clone the (cheap, `Arc`-backed) parser so the scoped thread owns it,
        // avoiding a `Sync` bound on `Self`.  `std::thread::scope` guarantees the
        // thread is joined before this function returns, so the borrowed
        // `content`/`language`/`path` remain valid for its entire lifetime.
        let this = self.clone();
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("ts-parse".to_string())
                .stack_size(constants::PARSE_THREAD_STACK_SIZE)
                .spawn_scoped(scope, move || this.parse_content(content, language, path));

            match spawned {
                Ok(handle) => handle.join().unwrap_or_else(|_| {
                    Err(Error::Generic(format!(
                        "tree-sitter parse thread panicked for {}",
                        path.display()
                    )))
                }),
                Err(e) => Err(Error::Generic(format!(
                    "failed to spawn parse thread for {}: {e}",
                    path.display()
                ))),
            }
        })
    }

    /// Detect programming language from file path extension
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file
    ///
    /// # Returns
    ///
    /// The detected language identifier as a string
    ///
    /// # Errors
    ///
    /// Returns an error if the file extension cannot be detected or is unsupported
    fn detect_language(&self, path: &Path) -> Result<String> {
        // First try filename-based detection for special cases (MATLAB vs Objective-C)
        if let Some(filename) = path.file_name().and_then(|f| f.to_str())
            && let Some(lang) = language_config::detect_language_from_filename(filename)
        {
            return Ok(lang);
        }

        // Try to detect by extension
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            if let Some(lang) = language_config::extension_to_language(extension) {
                return Ok(lang);
            }
            // Extension found but not supported
            return Err(Error::UnsupportedLanguage(extension.to_string()));
        }

        // Try filename-based detection for files without extensions
        if let Some(filename) = path.file_stem().and_then(|f| f.to_str()) {
            let filename_lower = filename.to_lowercase();

            // Try direct match first
            if let Some(lang) = language_config::filename_to_language(&filename_lower) {
                return Ok(lang);
            }

            // Try stripping common test prefixes (e.g., "test_makefile" -> "makefile")
            let stripped = filename_lower
                .trim_start_matches("test_")
                .trim_start_matches("example_")
                .trim_start_matches("sample_");
            if let Some(lang) = language_config::filename_to_language(stripped) {
                return Ok(lang);
            }
        }

        Err(Error::Generic(
            "Could not detect file extension".to_string(),
        ))
    }

    /// Read file content (with memory-mapping for large files)
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file
    ///
    /// # Returns
    ///
    /// The file content as a string
    ///
    /// # Errors
    ///
    /// Returns an error if file reading fails or content is not valid UTF-8.
    ///
    /// Uses mmap for files above the configured threshold.
    ///
    /// **Note:** this performs blocking I/O. The async entry point `parse_file` wraps the
    /// whole parse (this read + tree-sitter) in `tokio::task::spawn_blocking`, and the parse
    /// itself runs on a dedicated large-stack thread (`parse_content_large_stack`). The
    /// read is bounded by the file-size cap, so small files (the common case) complete quickly.
    fn read_file_content(&self, path: &Path) -> Result<String> {
        use crate::utils::validation::DEFAULT_MAX_FILE_SIZE;

        let metadata = std::fs::metadata(path)?;
        let len = metadata.len();

        // Reject files larger than the global max, which could exhaust memory.
        if len > DEFAULT_MAX_FILE_SIZE {
            return Err(Error::Generic(format!(
                "File exceeds maximum size ({} MiB): {}",
                DEFAULT_MAX_FILE_SIZE / 1024 / 1024,
                path.display()
            )));
        }

        // Compare file length as u64 to avoid 32-bit truncation on usize::try_from.
        let content = if len > self.config.mmap_threshold as u64 {
            // The MmapStr wrapper validates UTF-8 once at open time, holds the file
            // alive, and takes an advisory flock on Unix.
            //
            // `to_owned_string` copies the mapping into
            // an owned `String`, so mmap's net benefit here is the advisory flock
            // + lazy fault-in rather than zero-copy. Returning an owned `String`
            // keeps a single read path for both branches; parsing directly off the
            // borrowed `&str` would avoid the copy but is not worth the lifetime
            // plumbing given the file-size cap bounds the allocation.
            let mapped = crate::parser::mmap_parser::MmapStr::open(path)?;
            mapped.to_owned_string()
        } else {
            std::fs::read_to_string(path)?
        };

        // Universal minified/generated-file guard: files that pack enormous
        // content onto a single line are not meaningfully analyzable and can
        // drive tree-sitter into pathological memory/recursion.  Only files
        // larger than the per-line limit can possibly contain such a line, so we
        // scan only those.  Offenders are skipped with a clean, recoverable error
        // — the pipeline logs a warning and moves on, never crashing.
        if len > constants::MAX_LINE_LENGTH as u64
            && let Some(line_len) = content
                .lines()
                .map(str::len)
                .find(|&l| l > constants::MAX_LINE_LENGTH)
        {
            return Err(Error::Generic(format!(
                "File {} contains a {}-byte line exceeding the {}-byte parse limit (likely minified/generated); skipping",
                path.display(),
                line_len,
                constants::MAX_LINE_LENGTH
            )));
        }

        Ok(content)
    }

    /// Parse content with the given language
    ///
    /// This method extracts entities and relationships from source code,
    /// then optionally cleans and validates them based on configuration.
    ///
    /// # Arguments
    ///
    /// * `content` - Source code content to parse
    /// * `language` - Programming language identifier
    /// * `path` - Path to the source file (for metadata)
    ///
    /// # Returns
    ///
    /// A `ParsedFile` containing file info, entities, and relationships
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or entity extraction fails
    fn parse_content(&self, content: &str, language: &str, path: &Path) -> Result<ParsedFile> {
        // Prose documents have no AST worth building, but they do have
        // structure: one entity per heading section. Handled before the
        // simple-file branch below, which would otherwise return them with no
        // entities at all.
        if language_config::is_prose_language(language) {
            let mut entities = prose::extract_sections(content, &path.to_string_lossy(), language);
            // Content-stable ids, as for every other entity: a section keeps
            // its id, and so its vector and the edges into it, while it is
            // unchanged.
            assign_stable_ids(&mut entities, &mut [], &mut [], path);
            let file_info = FileInfo {
                id: uuid::Uuid::new_v4(),
                path: path.to_string_lossy().to_string(),
                language: language.to_string(),
                size_bytes: content.len() as u64,
                line_count: content.lines().count() as u32,
                checksum: compute_checksum(content),
                last_modified: OffsetDateTime::now_utc(),
                entities: Vec::new(),
                created_at: OffsetDateTime::now_utc(),
                updated_at: OffsetDateTime::now_utc(),
            };
            return Ok(ParsedFile {
                file_info,
                entities,
                relationships: Vec::new(),
                refs: Vec::new(),
                imports: Vec::new(),
                tree: None,
                content: content.to_string(),
            });
        }

        // Check if this is a simple/config file that should not be parsed with tree-sitter
        if language_config::is_simple_file_language(language) {
            debug!(
                "Skipping tree-sitter parsing for simple file: {} (language: {})",
                path.display(),
                language
            );

            // Create file info for simple files without AST parsing
            let file_info = FileInfo {
                id: uuid::Uuid::new_v4(),
                path: path.to_string_lossy().to_string(),
                language: language.to_string(),
                size_bytes: content.len() as u64,
                line_count: content.lines().count() as u32,
                checksum: compute_checksum(content),
                last_modified: OffsetDateTime::now_utc(),
                entities: Vec::new(),
                created_at: OffsetDateTime::now_utc(),
                updated_at: OffsetDateTime::now_utc(),
            };

            // Return a ParsedFile with empty entities and no parse tree.
            // No dummy tree is created — we avoid invoking tree-sitter entirely
            // for formats where AST-based extraction provides no value.
            return Ok(ParsedFile {
                file_info,
                entities: Vec::new(),
                relationships: Vec::new(),
                refs: Vec::new(),
                imports: Vec::new(),
                tree: None,
                content: content.to_string(),
            });
        }

        // Check if body exceeds hard limit before parsing
        if cleaning::body_exceeds_limit(content) {
            warn!(
                "File {} exceeds body size limit ({} bytes), parsing may be skipped",
                path.display(),
                content.len()
            );
        }

        // Each call gets its own Parser built from the cached Language.
        // No lock needed — Parser is local to this stack frame.
        let mut parser = self.cache.get_parser(language)?;

        let tree = parser
            .parse(content, None)
            .ok_or_else(|| Error::Parser("Failed to parse content".to_string()))?;

        // Extract entities
        let mut entities = entity_extraction::extract_entities(
            &tree,
            content,
            language,
            path.to_string_lossy().as_ref(),
        )?;

        // Discard standalone Decorator entities unless the caller
        // explicitly opted in.  The decorated function/class is still extracted
        // normally; decorators only pollute the semantic index as bare entities.
        if !self.config.emit_decorators {
            entities.retain(|e| !matches!(e, crate::entities::Entity::Decorator(_)));
        }

        // Extract relationships, and the references they come from, which a
        // name the file's imports bind makes another module's.
        let mut imports = extract_imports(tree.root_node(), content, language);
        let relationship_extraction::Extraction {
            mut relationships,
            mut refs,
            module,
        } = relationship_extraction::extract_relationships(
            &tree,
            content,
            &entities,
            &imports,
            self.ambiguous_fanout,
            language,
            path.to_string_lossy().as_ref(),
        )?;
        entities.extend(module);

        // Svelte's and Vue's grammars leave a `<script>` body as an opaque
        // `raw_text` node, so nothing inside it is extracted. Re-parse the
        // scripts in the language each names and merge their entities,
        // relationships and imports (positions preserved) into the file.
        let mut script_imports = Vec::new();
        if matches!(language, "svelte" | "vue") {
            let (mut script_entities, scripts, mut imports) =
                self.extract_scripts(&tree, content, path, language, &entities)?;
            entities.append(&mut script_entities);
            relationships.extend(scripts.relationships);
            refs.extend(scripts.refs);
            script_imports.append(&mut imports);
        }

        // Apply cleaning if enabled
        if self.config.enable_cleaning {
            debug!(
                "Cleaning {} entities from {}",
                entities.len(),
                path.display()
            );
            entities = cleaning::clean_entities(entities);
        }

        // Apply validation if enabled
        if self.config.enable_validation {
            debug!("Validating entities from {}", path.display());

            if self.config.skip_invalid_entities {
                // Filter out invalid entities
                let file_path_str = path.to_string_lossy();
                let (valid, _failures) =
                    validation::filter_valid_entities(entities, &file_path_str);
                entities = valid;

                // Relationships are extracted before validation, so their source/target
                // IDs may reference entities that were just filtered out. Retain only
                // relationships whose both endpoints still exist in the valid entity set
                // to prevent FK violations when batch_insert_parsed_files runs.
                let valid_ids: std::collections::HashSet<crate::entities::EntityId> =
                    entities.iter().map(|e| e.id()).collect();
                relationships.retain(|r| {
                    valid_ids.contains(&r.source_id) && valid_ids.contains(&r.target_id)
                });
            } else {
                // Log warnings for invalid entities but keep them
                let file_path_str = path.to_string_lossy();
                for entity in &entities {
                    let result = validation::validate_entity(entity, &file_path_str);
                    if let validation::ValidationResult::Invalid(ref reason) = result {
                        warn!(
                            "Entity '{}' in {} failed validation: {}",
                            entity.name(),
                            path.display(),
                            reason
                        );
                    }
                }
            }

            // Validate relationships
            relationships.retain(|rel| {
                let result = validation::validate_relationship(rel);
                if let validation::ValidationResult::Invalid(ref reason) = result {
                    warn!(
                        "Relationship from {} to {} failed validation: {}",
                        rel.source_id, rel.target_id, reason
                    );
                    false
                } else {
                    true
                }
            });
        }

        // The types each entity's signature names are references too; and a
        // reference made by an entity cleaning or validation dropped has no
        // source to resolve from.
        refs.extend(entities.iter().flat_map(symbol_table::type_refs));
        let kept: std::collections::HashSet<crate::entities::EntityId> =
            entities.iter().map(Entity::id).collect();
        refs.retain(|reference| kept.contains(&reference.source_id));

        // Assign content-stable entity ids (and remap relationships to match).
        // This must run on the FINAL entity set so the ids that land in the DB —
        // and therefore the embedding vectors keyed by them — survive re-parses
        // unchanged. It is the foundation for reliable incremental embedding.
        assign_stable_ids(&mut entities, &mut relationships, &mut refs, path);
        imports.append(&mut script_imports);

        // Create file info
        let file_info = FileInfo {
            id: uuid::Uuid::new_v4(),
            path: path.to_string_lossy().to_string(),
            language: language.to_string(),
            size_bytes: content.len() as u64,
            line_count: content.lines().count() as u32,
            checksum: compute_checksum(content),
            last_modified: OffsetDateTime::now_utc(),
            entities: entities.iter().map(|e| e.id()).collect(),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };

        Ok(ParsedFile {
            file_info,
            entities,
            relationships,
            refs,
            imports,
            tree: Some(tree),
            content: content.to_string(),
        })
    }

    /// Re-parse the scripts embedded in a Svelte or Vue component.
    ///
    /// Both grammars expose a `<script>` body as an opaque `raw_text` node, so
    /// functions, classes and their relationships inside it are invisible to
    /// normal extraction. For each language the component's scripts are in
    /// (see [`script_language`]), we build a "shadow" document the same length
    /// as the file — every byte outside those scripts blanked to a space,
    /// newlines preserved — parse it in that language, and keep the resulting
    /// entities. Because the shadow keeps byte offsets and line numbers
    /// identical to the original, each entity's location is already correct
    /// for the component. A script's relationships and imports are read in
    /// its own language, and its entities then re-labelled with the
    /// component's (`svelte`, `vue`): read after, in the component's, none of
    /// its calls would be one. The component (among `outer`, the file's own
    /// entities) takes part, so that the code at the top of a script — most of
    /// a `<script setup>` — is the component's, which spans the file.
    ///
    /// The markup's expressions ([`collect_template_expressions`]) join the
    /// shadow of the component's first script language, each ended with a
    /// `;` written over its closing delimiter, so that a call in the markup
    /// (`{formatUsd(cents)}`, `@click="submit(x)"`) is the component's too.
    /// Without them a function used only in the markup would have no caller.
    fn extract_scripts(
        &self,
        tree: &tree_sitter::Tree,
        content: &str,
        path: &Path,
        component: &str,
        outer: &[Entity],
    ) -> Result<(
        Vec<Entity>,
        relationship_extraction::Extraction,
        Vec<ImportInfo>,
    )> {
        let mut blocks = Vec::new();
        collect_scripts(tree.root_node(), content, &mut blocks);
        let mut template = Vec::new();
        if matches!(component, "svelte" | "vue") {
            collect_template_expressions(tree.root_node(), &mut template);
        }
        let template_language = blocks
            .first()
            .map_or("javascript", |&(language, _)| language);

        let mut entities = Vec::new();
        let mut extraction = relationship_extraction::Extraction::default();
        let mut imports = Vec::new();
        for script_language in SCRIPT_LANGUAGES {
            let ranges: Vec<(usize, usize)> = blocks
                .iter()
                .filter(|(language, _)| *language == script_language)
                .map(|&(_, range)| range)
                .collect();
            let expressions: &[(usize, usize)] = if script_language == template_language {
                &template
            } else {
                &[]
            };
            if ranges.is_empty() && expressions.is_empty() {
                continue;
            }

            // Blank everything to spaces (newlines kept), then restore the
            // script bytes, and the markup's expressions, each a statement.
            let bytes = content.as_bytes();
            let mut shadow: Vec<u8> = bytes
                .iter()
                .map(|&b| if b == b'\n' { b'\n' } else { b' ' })
                .collect();
            for (s, e) in &ranges {
                shadow[*s..*e].copy_from_slice(&bytes[*s..*e]);
            }
            for &(s, e) in expressions {
                shadow[s..e].copy_from_slice(&bytes[s..e]);
                // Over the `}` or `"` that closes it; never over a newline,
                // which would move the lines after it.
                if bytes.get(e).is_some_and(|b| b.is_ascii() && *b != b'\n') {
                    shadow[e] = b';';
                }
            }
            let shadow = String::from_utf8_lossy(&shadow).into_owned();

            let mut parser = self.cache.get_parser(script_language)?;
            let Some(script_tree) = parser.parse(&shadow, None) else {
                continue;
            };
            let mut found = entity_extraction::extract_entities(
                &script_tree,
                &shadow,
                script_language,
                path.to_string_lossy().as_ref(),
            )?;
            let script_imports = extract_imports(script_tree.root_node(), &shadow, script_language);
            let with_component: Vec<Entity> = found
                .iter()
                .chain(outer.iter().filter(|e| matches!(e, Entity::Component(_))))
                .cloned()
                .collect();
            let script = relationship_extraction::extract_relationships(
                &script_tree,
                &shadow,
                &with_component,
                &script_imports,
                self.ambiguous_fanout,
                script_language,
                path.to_string_lossy().as_ref(),
            )?;
            found.extend(script.module);
            imports.extend(script_imports);
            for e in &mut found {
                e.set_language(component.to_string());
            }
            entities.append(&mut found);
            extraction.relationships.extend(script.relationships);
            extraction.refs.extend(script.refs);
        }
        Ok((entities, extraction, imports))
    }

    /// Get parser configuration
    #[must_use]
    pub const fn config(&self) -> &ParserConfig {
        &self.config
    }

    /// Check if cleaning is enabled
    #[must_use]
    pub fn is_cleaning_enabled(&self) -> bool {
        self.config.enable_cleaning
    }

    /// Check if validation is enabled
    #[must_use]
    pub fn is_validation_enabled(&self) -> bool {
        self.config.enable_validation
    }
}

/// Result of parsing a file
#[derive(Debug)]
pub struct ParsedFile {
    /// File information
    pub file_info: FileInfo,
    /// Extracted entities
    pub entities: Vec<Entity>,
    /// Extracted relationships
    pub relationships: Vec<Relationship>,
    /// Every reference the file's entities make, resolved or not: what the
    /// edges into other files are derived from.
    pub refs: Vec<SymbolRef>,
    /// The file's imports, against which its references resolve.
    pub imports: Vec<ImportInfo>,
    /// Parse tree (for further analysis) — `None` for simple/config files
    pub tree: Option<tree_sitter::Tree>,
    /// Source content
    pub content: String,
}

impl From<ParsedFile> for crate::database::FileRecord {
    fn from(parsed: ParsedFile) -> Self {
        Self {
            file_info: parsed.file_info,
            entities: parsed.entities,
            relationships: parsed.relationships,
            refs: parsed.refs,
            imports: parsed.imports,
        }
    }
}

impl ParsedFile {
    /// What the index stores of this file, copied.
    #[must_use]
    pub fn to_record(&self) -> crate::database::FileRecord {
        crate::database::FileRecord {
            file_info: self.file_info.clone(),
            entities: self.entities.clone(),
            relationships: self.relationships.clone(),
            refs: self.refs.clone(),
            imports: self.imports.clone(),
        }
    }

    /// Get the number of entities
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Get the number of relationships
    #[must_use]
    pub fn relationship_count(&self) -> usize {
        self.relationships.len()
    }

    /// Get entities of a specific type
    pub fn entities_of_type(&self, type_name: &str) -> Vec<&Entity> {
        self.entities
            .iter()
            .filter(|e| e.entity_type() == type_name)
            .collect()
    }
}

/// Compute a checksum of content for change detection
///
/// # Arguments
///
/// * `content` - The content to hash
///
/// # Returns
///
/// A hexadecimal string representing the hash
fn compute_checksum(content: &str) -> String {
    // Use xxhash3 for fast, stable change detection instead of SHA-256.
    // SHA-256 is cryptographic and overkill for this use case.
    let hash = xxh3_64(content.as_bytes());
    format!("{:016x}", hash)
}

/// Fixed namespace for Devscriptor content-derived entity ids. **Never change**
/// this value — doing so would change every entity id and invalidate every
/// stored embedding across all existing databases.
const ENTITY_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0xe2d7_b3a4_c1f0_4e6a_9b8c_7d5e_3f1a_2b4c);

/// Derive a deterministic [`EntityId`] from a canonical identity key (UUIDv5).
fn stable_entity_id(canonical_key: &str) -> crate::entities::EntityId {
    uuid::Uuid::new_v5(&ENTITY_ID_NAMESPACE, canonical_key.as_bytes())
}

/// Replace each entity's id with a **content-stable** id derived from its
/// identity (file path, type, name, disambiguator, and an in-file ordinal for
/// genuine duplicates), and remap relationship endpoints to match.
///
/// Determinism is what makes incremental embedding reliable: the same source
/// always yields the same ids, so re-parsing an unchanged (or merely edited
/// elsewhere) file preserves each entity's id — and therefore the embedding
/// vector keyed by it — while truly new/removed entities get/lose ids cleanly.
/// The ordinal is assigned in extraction order; since tree-sitter traversal is
/// deterministic for a given source, identical-key duplicates keep stable
/// ordinals unless they are reordered in the source (a rare, self-correcting
/// edge case that the embedding cache absorbs).
fn assign_stable_ids(
    entities: &mut [Entity],
    relationships: &mut [Relationship],
    refs: &mut [SymbolRef],
    path: &Path,
) {
    use std::collections::HashMap;

    let file = path.to_string_lossy();
    let mut ordinals: HashMap<String, u32> = HashMap::new();
    let mut remap: HashMap<crate::entities::EntityId, crate::entities::EntityId> =
        HashMap::with_capacity(entities.len());

    for entity in entities.iter_mut() {
        let base = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            file,
            entity.entity_type(),
            entity.name(),
            entity.identity_disambiguator()
        );
        let ord = ordinals.entry(base.clone()).or_insert(0);
        let key = format!("{base}\u{1f}{ord}");
        *ord += 1;

        let new_id = stable_entity_id(&key);
        let old_id = entity.id();
        if old_id != new_id {
            remap.insert(old_id, new_id);
            crate::match_entity!(entity, e => {
                e.id = new_id;
            });
        }
    }

    if !remap.is_empty() {
        for rel in relationships.iter_mut() {
            if let Some(&new) = remap.get(&rel.source_id) {
                rel.source_id = new;
            }
            if let Some(&new) = remap.get(&rel.target_id) {
                rel.target_id = new;
            }
        }
        for reference in refs.iter_mut() {
            if let Some(&new) = remap.get(&reference.source_id) {
                reference.source_id = new;
            }
        }
    }
}

/// Parse a file with default configuration
///
/// This is a convenience function for quick parsing without setting up
/// a full parser instance.
///
/// # Arguments
///
/// * `path` - Path to the file to parse
///
/// # Returns
///
/// A `ParsedFile` containing file info, entities, and relationships
///
/// # Errors
///
/// Returns an error if file reading or parsing fails
pub async fn parse_file(path: &Path) -> Result<ParsedFile> {
    let parser = TreeSitterParser::new(ParserConfig::default())?;
    parser.parse_file(path).await
}

/// The languages a component's scripts can be in: the grammars
/// [`script_language`] names.
const SCRIPT_LANGUAGES: [&str; 3] = ["javascript", "typescript", "tsx"];

/// The script blocks of a Svelte or Vue component, each with its language:
/// the body of a `<script>` is the `raw_text` child directly under a
/// `script_element` (a `<style>`'s `raw_text` is excluded by the parent
/// check). A block in a language no grammar here parses is left out.
fn collect_scripts(
    node: tree_sitter::Node,
    content: &str,
    out: &mut Vec<(&'static str, (usize, usize))>,
) {
    if node.kind() == "raw_text"
        && let Some(script) = node.parent().filter(|p| p.kind() == "script_element")
        && let Some(language) = script_language(script, content)
    {
        out.push((language, (node.start_byte(), node.end_byte())));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_scripts(child, content, out);
    }
}

/// The JavaScript expressions in a Svelte or Vue component's markup, as byte
/// ranges: Svelte's `{…}` — in text, in attribute values, in `{@html …}` and
/// in block tags, but not the binding of an `{#each … as item}` — and Vue's
/// `{{ … }}` and directive values (`:disabled="…"`, `@click="…"`,
/// `v-for="…"`).
fn collect_template_expressions(node: tree_sitter::Node, out: &mut Vec<(usize, usize)>) {
    let parent = node.parent();
    let expression = match node.kind() {
        "svelte_raw_text" => {
            parent.is_some_and(|parent| parent.child_by_field_name("parameter") != Some(node))
        }
        "raw_text" => parent.is_some_and(|parent| parent.kind() == "interpolation"),
        "attribute_value" => parent.is_some_and(|parent| {
            parent.kind() == "directive_attribute"
                || (parent.kind() == "quoted_attribute_value"
                    && parent
                        .parent()
                        .is_some_and(|attribute| attribute.kind() == "directive_attribute"))
        }),
        _ => false,
    };
    if expression {
        out.push((node.start_byte(), node.end_byte()));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_template_expressions(child, out);
    }
}

/// The language of the `<script>` element `script`, by its `lang`
/// attribute: TypeScript for `ts` and `typescript`, TSX for `tsx`, and
/// JavaScript for `js`, `javascript`, `jsx` or no `lang` at all; `None` for
/// any other (`coffee`). Both grammars spell an attribute as an
/// `attribute_name` and an `attribute_value`, quoted or not.
fn script_language(script: tree_sitter::Node, content: &str) -> Option<&'static str> {
    let text = |node: tree_sitter::Node| node.utf8_text(content.as_bytes()).unwrap_or("");
    let mut cursor = script.walk();
    let start_tag = script
        .children(&mut cursor)
        .find(|child| child.kind() == "start_tag")?;
    let mut cursor = start_tag.walk();
    for attribute in start_tag
        .children(&mut cursor)
        .filter(|child| child.kind() == "attribute")
    {
        let mut name = "";
        let mut value = "";
        let mut parts = attribute.walk();
        for part in attribute.children(&mut parts) {
            match part.kind() {
                "attribute_name" => name = text(part),
                "attribute_value" => value = text(part),
                "quoted_attribute_value" => {
                    value = part.named_child(0).map_or("", text);
                }
                _ => {}
            }
        }
        if name.trim().eq_ignore_ascii_case("lang") {
            return match value.trim().to_ascii_lowercase().as_str() {
                "" | "js" | "javascript" | "jsx" => Some("javascript"),
                "ts" | "typescript" => Some("typescript"),
                "tsx" => Some("tsx"),
                _ => None,
            };
        }
    }
    Some("javascript")
}

/// Synchronous convenience function for parsing a file.
pub fn parse_file_sync(path: &Path) -> Result<ParsedFile> {
    let parser = TreeSitterParser::new(ParserConfig::default())?;
    parser.parse_file_sync(path)
}

/// Parse content with default configuration
///
/// This is a convenience function for quick parsing without setting up
/// a full parser instance.
///
/// # Arguments
///
/// * `content` - Source code content to parse
/// * `language` - Programming language identifier
/// * `path` - Path to the source file (for metadata)
///
/// # Returns
///
/// A `ParsedFile` containing file info, entities, and relationships
///
/// # Errors
///
/// Returns an error if parsing or entity extraction fails
pub fn parse_content_sync(content: &str, language: &str, path: &Path) -> Result<ParsedFile> {
    let parser = TreeSitterParser::new(ParserConfig::default())?;
    parser.parse_content(content, language, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_config_default() {
        let config = ParserConfig::default();
        assert!(config.enable_cache);
        // In test builds cache_size is reduced to 10; in production it is 50.
        let expected = if cfg!(test) { 10 } else { 50 };
        assert_eq!(config.cache_size, expected);
        assert_eq!(config.mmap_threshold, 10 * 1024 * 1024);
        assert!(config.enable_cleaning);
        assert!(config.enable_validation);
        assert!(config.skip_invalid_entities);
    }

    #[test]
    fn test_compute_checksum() {
        let content = "test content";
        let checksum1 = compute_checksum(content);
        let checksum2 = compute_checksum(content);
        assert_eq!(checksum1, checksum2);

        let different = compute_checksum("different content");
        assert_ne!(checksum1, different);
    }

    #[test]
    fn stable_ids_are_deterministic_across_reparses() {
        use std::path::Path;
        let src = "fn alpha() -> u8 { 7 }\nfn beta(x: i32) -> i32 { x + 1 }\n";
        let p1 = parse_content_sync(src, "rust", Path::new("lib.rs")).expect("parse 1");
        let p2 = parse_content_sync(src, "rust", Path::new("lib.rs")).expect("parse 2");
        assert!(!p1.entities.is_empty(), "should extract entities");
        let ids1: Vec<_> = p1.entities.iter().map(|e| e.id()).collect();
        let ids2: Vec<_> = p2.entities.iter().map(|e| e.id()).collect();
        assert_eq!(
            ids1, ids2,
            "the same source must yield identical entity ids"
        );
        assert!(ids1.iter().all(|id| !id.is_nil()), "ids must not be nil");
    }

    /// Markdown sections take content-stable ids, so a re-index of a document
    /// keeps the sections that did not change, their vectors and the edges into
    /// them.
    #[test]
    fn stable_ids_hold_for_prose_sections() {
        use std::path::Path;
        let src =
            "# Guide\n\n## Install\n\nRun it.\n\n## Usage\n\nCall it.\n\n## Install\n\nAgain.\n";
        let p1 = parse_content_sync(src, "markdown", Path::new("docs/guide.md")).expect("parse 1");
        let p2 = parse_content_sync(src, "markdown", Path::new("docs/guide.md")).expect("parse 2");
        let ids1: Vec<_> = p1.entities.iter().map(|e| e.id()).collect();
        let ids2: Vec<_> = p2.entities.iter().map(|e| e.id()).collect();
        assert_eq!(ids1.len(), 4, "a section per heading: {:?}", p1.entities);
        assert_eq!(ids1, ids2, "the same document yields the same ids");
        let unique: std::collections::HashSet<_> = ids1.iter().collect();
        assert_eq!(
            unique.len(),
            ids1.len(),
            "headings of one name get distinct ids"
        );

        // Text added to one section changes no section's identity.
        let edited = src.replace("Call it.", "Call it, then check the log.");
        let p3 =
            parse_content_sync(&edited, "markdown", Path::new("docs/guide.md")).expect("parse 3");
        let ids3: Vec<_> = p3.entities.iter().map(|e| e.id()).collect();
        assert_eq!(ids1, ids3, "an edit of a section's text keeps the ids");
    }

    #[test]
    fn stable_ids_survive_unrelated_edits() {
        use std::path::Path;
        let id_of = |p: &ParsedFile, name: &str| {
            p.entities
                .iter()
                .find(|e| e.name() == name)
                .map(super::Entity::id)
        };

        let a = parse_content_sync(
            "fn alpha() -> u8 { 7 }\nfn beta() -> u8 { 8 }\n",
            "rust",
            Path::new("lib.rs"),
        )
        .unwrap();
        // A leading comment shifts every line; `gamma` is inserted between them.
        // `alpha` and `beta` are otherwise unchanged.
        let b = parse_content_sync(
            "// a new comment\nfn alpha() -> u8 { 7 }\nfn gamma() -> u8 { 9 }\nfn beta() -> u8 { 8 }\n",
            "rust",
            Path::new("lib.rs"),
        )
        .unwrap();

        assert!(id_of(&a, "alpha").is_some() && id_of(&a, "beta").is_some());
        assert_eq!(
            id_of(&a, "alpha"),
            id_of(&b, "alpha"),
            "alpha id must survive line shifts and insertions"
        );
        assert_eq!(
            id_of(&a, "beta"),
            id_of(&b, "beta"),
            "beta id must survive line shifts and insertions"
        );

        // The same entity in a different file gets a different id (file-scoped).
        let c =
            parse_content_sync("fn alpha() -> u8 { 7 }\n", "rust", Path::new("other.rs")).unwrap();
        assert_ne!(
            id_of(&a, "alpha"),
            id_of(&c, "alpha"),
            "ids are file-scoped"
        );
    }

    #[test]
    fn test_parser_config_custom() {
        let config = ParserConfig {
            enable_cache: false,
            cache_size: 50,
            mmap_threshold: 5 * 1024 * 1024,
            enable_cleaning: false,
            enable_validation: false,
            skip_invalid_entities: false,
            emit_decorators: false,
        };

        assert!(!config.enable_cache);
        assert_eq!(config.cache_size, 50);
        assert!(!config.enable_cleaning);
        assert!(!config.enable_validation);
        assert!(!config.skip_invalid_entities);
    }
}
