//! One spelling of a path for every caller that names the same file.

use std::path::{Path, PathBuf};

/// `path` as one spelling for every caller: canonical when it exists; when
/// only its directory does (a database not created yet), that directory
/// canonical with the file's name; else absolute.
#[must_use]
pub fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = dunce::canonicalize(path) {
        return path;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = dunce::canonicalize(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })
    {
        return parent.join(name);
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_named_by_its_canonical_directory() {
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join(".").join("index.db");
        assert_eq!(
            canonical(&named),
            dunce::canonicalize(dir.path()).unwrap().join("index.db")
        );
        // Created, it keeps that name.
        std::fs::write(&named, "").unwrap();
        assert_eq!(
            canonical(&named),
            dunce::canonicalize(dir.path()).unwrap().join("index.db")
        );
    }
}
