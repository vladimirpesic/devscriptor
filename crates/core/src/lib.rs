//! Devscriptor-Core: High-performance code analysis library
//!
//! This library provides functionality for parsing, analyzing, and storing
//! code entities from 26 programming languages via individual tree-sitter parsers.

#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod analysis;
pub mod config;
pub mod context_graph;
pub mod database;
pub mod embeddings;
pub mod entities;
pub mod error;
pub mod graph;
pub mod parser;
pub mod pipeline;
pub mod processing;
pub mod search;
pub mod user_dirs;
pub mod utils;
pub mod watcher;

pub use entities::{
    Class, CodeEntity, Component, Constant, Contract, Decorator, Entity, EntityId, Enum, Extension,
    FileInfo, Function, Interface, Macro, Module, Namespace, Relationship, RelationshipType,
    SourceLocation, Struct, Trait, TypeAlias, Union,
};
pub use error::{Error, Result, sanitize_message};

// Re-export embedding provider trait so external crates can implement
// or swap providers without depending on a concrete backend.
pub use embeddings::provider::{EmbeddingProvider, JinaEmbeddingProvider};
pub use embeddings::{
    EMBEDDING_DIMENSION, EmbeddingConfig, EmbeddingService, MockEmbeddingProvider,
};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
