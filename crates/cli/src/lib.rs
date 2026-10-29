//! Devscriptor CLI library
//!
//! Re-exports all command modules so integration tests can exercise the
//! CLI → Core → Database pipeline without spawning subprocesses.

pub mod commands;
