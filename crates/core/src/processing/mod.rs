//! File processing utilities for Devscriptor
//!
//! Exposes the file scanner used by the pipeline and integration tests; the
//! pipeline itself orchestrates batching, incremental re-indexing and
//! watching.

pub mod scanner;

pub use scanner::{
    EntryPointDetector, FileEntry, FileFilter, FileScanner, PrioritizedFileEntry, PriorityAnalyzer,
    PriorityPattern, PriorityStats,
};
