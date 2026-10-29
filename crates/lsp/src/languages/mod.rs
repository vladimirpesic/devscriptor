//! Language server presets for 36 programming languages.
//!
//! This module provides pre-configured LSP server configurations
//! for a wide variety of programming languages.

pub mod presets;

use std::io::Read;
use std::path::Path;

/// Whether the `.m` file at `path` is Objective-C, by the markers the index
/// reads it by (`content_is_objc` in devscriptor-core, which this crate does
/// not depend on): `@interface`, `@implementation` or `#import` in its first
/// 4 KiB. MATLAB uses `.m` too, and clangd would read a MATLAB file as broken
/// Objective-C, so only an Objective-C one goes to it.
#[must_use]
pub fn is_objective_c(path: &Path) -> bool {
    let mut head = Vec::with_capacity(4096);
    if std::fs::File::open(path)
        .and_then(|file| file.take(4096).read_to_end(&mut head))
        .is_err()
    {
        return false;
    }
    let head = String::from_utf8_lossy(&head);
    head.contains("@interface") || head.contains("@implementation") || head.contains("#import")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Objective-C `.m` is told from a MATLAB one by its content.
    #[test]
    fn an_objective_c_source_is_told_from_matlab() {
        let dir = tempfile::tempdir().unwrap();
        let objc = dir.path().join("TWWeightLimits.m");
        std::fs::write(
            &objc,
            "#import \"TWWeightLimits.h\"\n\n@implementation TWWeightLimits\n@end\n",
        )
        .unwrap();
        let matlab = dir.path().join("limits.m");
        std::fs::write(&matlab, "function y = limits(x)\n  y = x * 2;\nend\n").unwrap();
        assert!(is_objective_c(&objc));
        assert!(!is_objective_c(&matlab));
        assert!(!is_objective_c(&dir.path().join("missing.m")));
    }
}
