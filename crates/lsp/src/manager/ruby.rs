//! Whether a gem-installed server can run on this machine's Ruby.
//!
//! A gem's native extensions are built for one Ruby, and RubyGems keeps them
//! under `extensions/<platform>/<api version>/`. A Ruby looks for them only
//! under its own platform's name and API version, and skips a gem whose
//! extensions are not there, as if the gem were missing: a `ruby-lsp` built
//! with Ruby 3.2 does not find its own `prism` under Ruby 3.3, nor does one
//! built by Debian's Ruby (`x86_64-linux-gnu`) under a Ruby built from source,
//! as rbenv, asdf, mise and Docker's images build it (`x86_64-linux`). Either
//! way it exits before answering `initialize`.
//!
//! Extensions found under the right names can still fail to load: each is
//! linked against the Ruby library of the Ruby that built it, and Debian's
//! (`libruby-3.3.so.3.3`) is no other Ruby's. So the extensions a Ruby would
//! use are also opened with that Ruby, which links them without running them
//! ([`built_for_another_ruby`]).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// A Ruby, as RubyGems keeps native extensions for it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RubyBuild {
    /// `Gem::Platform.local`: `x86_64-linux`, `arm64-darwin-24`.
    pub(crate) platform: String,
    /// `Gem.extension_api_version`: `3.3.0`.
    pub(crate) api: String,
}

impl fmt::Display for RubyBuild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on {}", self.api, self.platform)
    }
}

/// This machine's Ruby, asked of the `ruby` on `PATH` once per process;
/// `None` without a Ruby that answers.
pub(crate) fn local_ruby() -> Option<&'static RubyBuild> {
    static LOCAL: OnceLock<Option<RubyBuild>> = OnceLock::new();
    LOCAL
        .get_or_init(|| {
            let output = std::process::Command::new("ruby")
                .args([
                    "-e",
                    "print Gem::Platform.local.to_s, \"\\n\", Gem.extension_api_version",
                ])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let text = String::from_utf8(output.stdout).ok()?;
            let (platform, api) = text.trim().split_once('\n')?;
            let (platform, api) = (platform.trim(), api.trim());
            (!platform.is_empty() && !api.is_empty()).then(|| RubyBuild {
                platform: platform.to_string(),
                api: api.to_string(),
            })
        })
        .as_ref()
}

/// The Rubies the native extensions of the gem install in `dir` were built
/// for: the `extensions/<platform>/<api>/` directories it holds, sorted; none
/// for an install whose gems have no native extensions.
pub(crate) fn bundle_builds(dir: &Path) -> Vec<RubyBuild> {
    let mut builds: Vec<RubyBuild> = Vec::new();
    for platform in std::fs::read_dir(dir.join("extensions"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for api in std::fs::read_dir(platform.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if api.path().is_dir() {
                builds.push(RubyBuild {
                    platform: platform.file_name().to_string_lossy().into_owned(),
                    api: api.file_name().to_string_lossy().into_owned(),
                });
            }
        }
    }
    builds.sort();
    builds.dedup();
    builds
}

/// The Rubies the gem install in `dir` was built for, when `local`, this
/// machine's Ruby, cannot load its native extensions: what it would need.
/// `None` when it can run here: it has no native extensions, or `local` finds
/// them under its own platform and API version and can link them. `None` too
/// when there is no local Ruby to compare with, which the prerequisites
/// report.
pub(crate) fn built_for_another_ruby(dir: &Path, local: Option<&RubyBuild>) -> Option<String> {
    let local = local?;
    let builds = bundle_builds(dir);
    if builds.is_empty() || (builds.contains(local) && links_here(dir, local)) {
        return None;
    }
    Some(
        builds
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// Whether the `ruby` on `PATH`, `local`, links the native extensions the gem
/// install in `dir` holds for it: each is opened with Fiddle, which loads it
/// and the libraries it needs without initializing it. Asked once per
/// install and process. A Ruby without Fiddle, or one that cannot be run,
/// is not asked, and the extensions count as linking.
fn links_here(dir: &Path, local: &RubyBuild) -> bool {
    static LINKS: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let cache = LINKS.get_or_init(Mutex::default);
    if let Some(known) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(dir)
    {
        return *known;
    }
    let libraries = extension_libraries(
        &dir.join("extensions")
            .join(&local.platform)
            .join(&local.api),
    );
    let links = libraries.is_empty() || open_with_ruby(&libraries);
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(dir.to_path_buf(), links);
    links
}

/// The compiled extensions under `dir`: `.so` files, and `.bundle` files on
/// macOS.
fn extension_libraries(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext == "so" || ext == "bundle")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Whether the `ruby` on `PATH` opens every one of `libraries` with Fiddle.
/// `true` when it cannot be asked: no Ruby, or one without Fiddle.
fn open_with_ruby(libraries: &[PathBuf]) -> bool {
    const PROBE: &str = "begin; require 'fiddle'; rescue LoadError; exit 0; end; \
                         ARGV.each { |library| Fiddle.dlopen(library).close }";
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(PROBE)
        .args(libraries)
        .output();
    match output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            tracing::debug!(
                "This Ruby does not link the native extensions {}: {}",
                libraries
                    .iter()
                    .map(|library| library.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            false
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ruby(platform: &str, api: &str) -> RubyBuild {
        RubyBuild {
            platform: platform.to_string(),
            api: api.to_string(),
        }
    }

    /// A gem install whose native extensions were built for one Ruby runs on
    /// that Ruby alone, the same API version on the same platform; one with no
    /// native extensions runs on any.
    #[test]
    fn a_gem_install_runs_on_the_ruby_it_was_built_for() {
        let dir = tempfile::tempdir().unwrap();
        let built = ruby("x86_64-linux-gnu", "3.2.0");
        assert_eq!(
            built_for_another_ruby(dir.path(), Some(&ruby("x86_64-linux-gnu", "3.3.0"))),
            None
        );
        std::fs::create_dir_all(
            dir.path()
                .join("extensions/x86_64-linux-gnu/3.2.0/prism-1.9.0"),
        )
        .unwrap();
        assert_eq!(bundle_builds(dir.path()), std::slice::from_ref(&built));
        for other in [
            ruby("x86_64-linux-gnu", "3.3.0"),
            ruby("x86_64-linux", "3.2.0"),
        ] {
            assert_eq!(
                built_for_another_ruby(dir.path(), Some(&other)).as_deref(),
                Some("3.2.0 on x86_64-linux-gnu"),
                "{other}"
            );
        }
        // No compiled library in it: nothing for the Ruby to link.
        assert_eq!(built_for_another_ruby(dir.path(), Some(&built)), None);
        assert_eq!(built_for_another_ruby(dir.path(), None), None);
    }

    /// The extensions a Ruby would use are opened with it: one that does not
    /// link, as a library built against another Ruby's does not, makes the
    /// install another Ruby's, though its names are this Ruby's.
    #[test]
    fn an_extension_this_ruby_cannot_link_is_another_rubys() {
        let has_fiddle = std::process::Command::new("ruby")
            .args(["-e", "require 'fiddle'"])
            .output()
            .is_ok_and(|output| output.status.success());
        let Some(local) = local_ruby().filter(|_| has_fiddle) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let ext = dir
            .path()
            .join("extensions")
            .join(&local.platform)
            .join(&local.api)
            .join("broken-1.0.0");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("broken.so"), b"not a library").unwrap();
        assert_eq!(
            built_for_another_ruby(dir.path(), Some(local)),
            Some(local.to_string())
        );
    }
}
