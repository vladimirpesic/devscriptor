//! Trust for project-local `devscriptor-lsp.toml` files.
//!
//! A `devscriptor-lsp.toml` names the commands Devscriptor runs as language
//! servers, and `devscriptor mcp` reads one from whatever directory an agent
//! starts it in: a cloned repository could ship one that runs any program. So
//! a project file is used only once the user has trusted that exact content
//! with `devscriptor lsp trust`, and editing it withdraws the trust until it is
//! trusted again. The file `$DEVSCRIPTOR_LSP_CONFIG` names needs no trust:
//! setting the variable is the user's own choice.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{LspError, Result};

/// Trusted files, by canonical path, with the SHA-256 of the content trusted.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct TrustStore {
    trusted: BTreeMap<String, String>,
}

/// `<config dir>/devscriptor/trusted-lsp-configs.json`.
fn default_store() -> Option<PathBuf> {
    crate::user_dirs::config_dir()
        .map(|dir| dir.join("devscriptor").join("trusted-lsp-configs.json"))
}

/// SHA-256 of `content`, as lowercase hex.
fn digest(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

/// The key a file is trusted under: its canonical path.
fn key(path: &Path) -> Result<String> {
    let canonical = dunce::canonicalize(path)
        .map_err(|e| LspError::Config(format!("Cannot resolve {}: {e}", path.display())))?;
    Ok(canonical.to_string_lossy().into_owned())
}

fn read_store(store: &Path) -> TrustStore {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn write_store(store: &Path, trust: &TrustStore) -> Result<()> {
    if let Some(dir) = store.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write beside it and rename, so a crash never leaves a torn store.
    let partial = store.with_extension("json.partial");
    std::fs::write(&partial, serde_json::to_string_pretty(trust)?)?;
    std::fs::rename(&partial, store)?;
    Ok(())
}

/// Whether the file at `path`, read as `content`, is trusted as it stands.
#[must_use]
pub fn is_trusted(path: &Path, content: &[u8]) -> bool {
    default_store().is_some_and(|store| is_trusted_in(&store, path, content))
}

fn is_trusted_in(store: &Path, path: &Path, content: &[u8]) -> bool {
    let Ok(key) = key(path) else {
        return false;
    };
    read_store(store).trusted.get(&key) == Some(&digest(content))
}

/// Trust the file at `path` with exactly `content` — what the user was shown —
/// so a change between showing and trusting is not trusted with it.
///
/// # Errors
///
/// Returns an error if the path cannot be resolved or the store cannot be
/// written.
pub fn trust(path: &Path, content: &str) -> Result<()> {
    let store = default_store()
        .ok_or_else(|| LspError::Config("No configuration directory to keep trust in".into()))?;
    trust_in(&store, path, content)
}

fn trust_in(store: &Path, path: &Path, content: &str) -> Result<()> {
    let mut trust = read_store(store);
    trust.trusted.insert(key(path)?, digest(content.as_bytes()));
    write_store(store, &trust)
}

/// Withdraw trust from the file at `path`; returns whether it was trusted.
///
/// # Errors
///
/// Returns an error if the store cannot be written.
pub fn untrust(path: &Path) -> Result<bool> {
    let Some(store) = default_store() else {
        return Ok(false);
    };
    untrust_in(&store, path)
}

fn untrust_in(store: &Path, path: &Path) -> Result<bool> {
    let mut trust = read_store(store);
    let removed = trust.trusted.remove(&key(path)?).is_some();
    if removed {
        write_store(store, &trust)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_trusted_only_with_the_content_it_was_trusted_with() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("trust.json");
        let file = dir.path().join("devscriptor-lsp.toml");
        std::fs::write(&file, "[[lsp_servers]]\ncommand = \"rust-analyzer\"\n").unwrap();
        let content = std::fs::read(&file).unwrap();

        assert!(
            !is_trusted_in(&store, &file, &content),
            "untrusted by default"
        );
        trust_in(&store, &file, std::str::from_utf8(&content).unwrap()).unwrap();
        assert!(is_trusted_in(&store, &file, &content));

        // An edit withdraws the trust until the new content is trusted.
        std::fs::write(&file, "[[lsp_servers]]\ncommand = \"/tmp/evil\"\n").unwrap();
        let edited = std::fs::read(&file).unwrap();
        assert!(!is_trusted_in(&store, &file, &edited));

        assert!(untrust_in(&store, &file).unwrap());
        assert!(!is_trusted_in(&store, &file, &content));
        assert!(!untrust_in(&store, &file).unwrap(), "already untrusted");
    }

    /// The same content at another path is a different file to trust.
    #[test]
    fn trust_does_not_follow_content_to_another_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("trust.json");
        let a = dir.path().join("a.toml");
        let b = dir.path().join("b.toml");
        std::fs::write(&a, "x").unwrap();
        std::fs::write(&b, "x").unwrap();

        trust_in(&store, &a, "x").unwrap();
        assert!(!is_trusted_in(&store, &b, b"x"));
    }
}
