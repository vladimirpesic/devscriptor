//! Whether a gem-installed server can run on this machine's Ruby.
//!
//! A gem's native extensions are built for one Ruby API version, and RubyGems
//! keeps them under `extensions/<platform>/<api version>/`. Another Ruby skips
//! a gem whose extensions were built for a version not its own, as if the gem
//! were missing: a `ruby-lsp` built with Ruby 3.2 does not find its own `prism`
//! under Ruby 3.3, and exits before answering `initialize`.

use std::path::Path;
use std::sync::OnceLock;

/// This machine's Ruby extension API version (`Gem.extension_api_version`,
/// `3.3.0`), asked of the `ruby` on `PATH` once per process; `None` without a
/// Ruby that answers.
pub(crate) fn local_extension_api() -> Option<&'static str> {
    static API: OnceLock<Option<String>> = OnceLock::new();
    API.get_or_init(|| {
        let output = std::process::Command::new("ruby")
            .args(["-e", "print Gem.extension_api_version"])
            .output()
            .ok()?;
        let api = String::from_utf8(output.stdout).ok()?;
        (output.status.success() && !api.trim().is_empty()).then(|| api.trim().to_string())
    })
    .as_deref()
}

/// The Ruby API versions the native extensions of the gem install in `dir`
/// were built for: the names under its `extensions/<platform>/`, sorted;
/// none for an install whose gems have no native extensions.
pub(crate) fn bundle_extension_apis(dir: &Path) -> Vec<String> {
    let mut apis: Vec<String> = std::fs::read_dir(dir.join("extensions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|platform| std::fs::read_dir(platform.path()).ok())
        .flatten()
        .flatten()
        .filter(|api| api.path().is_dir())
        .map(|api| api.file_name().to_string_lossy().into_owned())
        .collect();
    apis.sort();
    apis.dedup();
    apis
}

/// The Ruby versions the gem install in `dir` was built for, when `local`,
/// this machine's extension API version, is not among them: what it would
/// need, and cannot load. `None` when it can run here — its extensions are
/// `local`'s, or it has none — or when there is no local Ruby to compare
/// with, which the prerequisites report.
pub(crate) fn built_for_another_ruby(dir: &Path, local: Option<&str>) -> Option<String> {
    let local = local?;
    let apis = bundle_extension_apis(dir);
    (!apis.is_empty() && !apis.iter().any(|api| api == local)).then(|| apis.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gem install whose native extensions were built for one Ruby runs on
    /// that Ruby alone; one with no native extensions runs on any.
    #[test]
    fn a_gem_install_runs_on_the_ruby_it_was_built_for() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(built_for_another_ruby(dir.path(), Some("3.3.0")), None);
        std::fs::create_dir_all(
            dir.path()
                .join("extensions/x86_64-linux-gnu/3.2.0/prism-1.9.0"),
        )
        .unwrap();
        assert_eq!(bundle_extension_apis(dir.path()), ["3.2.0"]);
        assert_eq!(
            built_for_another_ruby(dir.path(), Some("3.3.0")).as_deref(),
            Some("3.2.0")
        );
        assert_eq!(built_for_another_ruby(dir.path(), Some("3.2.0")), None);
        assert_eq!(built_for_another_ruby(dir.path(), None), None);
    }
}
