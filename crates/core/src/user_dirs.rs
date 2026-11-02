//! The folders Devscriptor keeps the user's configuration, data and caches
//! in, each under a `devscriptor` folder of its own.
//!
//! `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME`, set to an absolute
//! path, name them on every platform. Otherwise they are the platform's:
//! `~/.config`, `~/.local/share` and `~/.cache` on Linux;
//! `~/Library/Application Support` for the configuration and data and
//! `~/Library/Caches` on macOS; the roaming `AppData` folder for the
//! configuration and the local one for the data and caches on Windows. On
//! macOS and Windows nothing else moves them for one process, which is what
//! keeps a test away from the user's own index, servers and model.
//! `devscriptor_lsp::user_dirs` follows the same rule.

use std::path::PathBuf;

/// The configuration folder: templates, trusted `devscriptor-lsp.toml` files.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME").or_else(dirs::config_dir)
}

/// The data folder: the shared default database.
#[must_use]
pub fn data_local_dir() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME").or_else(dirs::data_local_dir)
}

/// The cache folder: the embedding model, the installed language servers.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME").or_else(dirs::cache_dir)
}

/// The folder `var` names, when it is an absolute path: a relative one is
/// ignored, as the XDG base directory specification says.
fn xdg(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}
