//! Bridge layer for translating between MCP and LSP protocols.
//!
//! This module provides the translation infrastructure that converts
//! MCP tool requests into LSP requests and back.

pub mod client_registry;
pub mod state;
pub mod translator;

pub use client_registry::ClientRegistry;
pub use state::{Document, DocumentState};
pub use translator::{Translator, path_to_uri, uri_to_path};
