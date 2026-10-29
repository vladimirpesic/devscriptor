//! LSP (Language Server Protocol) client implementation.
//!
//! This module provides the core LSP communication infrastructure including:
//! - JSON-RPC protocol types
//! - Transport layer for stdio communication
//! - Client with request/response correlation
//! - Server lifecycle management

pub mod client;
pub mod compile_commands;
pub mod lifecycle;
pub mod protocol;
pub mod transport;

pub use client::{LspClient, ServerState};
pub use lifecycle::{LspServer, SpawnResult, spawn_servers, spawn_servers_in};
pub use protocol::{
    InboundMessage, NotificationMessage, RequestId, RequestMessage, ResponseMessage,
    generate_request_id,
};
pub use transport::{LspReader, LspTransport, LspWriter};
