//! The configuration and cache folders, as `devscriptor_core::user_dirs`
//! finds them: `XDG_CONFIG_HOME` and `XDG_CACHE_HOME`, set to an absolute
//! path, on every platform, else the platform's own.

use std::path::PathBuf;

/// The configuration folder: trusted `devscriptor-lsp.toml` files.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME").or_else(dirs::config_dir)
}

/// The cache folder: installed language servers and their state.
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
