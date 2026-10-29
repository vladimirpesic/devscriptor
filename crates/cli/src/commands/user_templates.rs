//! Templates and profiles the user creates, kept beside the built-in set.
//!
//! `template create` and `profile create` write
//! `<config dir>/devscriptor/templates/<name>.json` (`~/.config/...` on Linux),
//! and every `template` and `profile` command loads that directory over the
//! built-ins, so what one command creates the next one sees. Templates and
//! profiles are the same objects; the two command groups share the directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use devscriptor_core::config::profiles::{ConfigurationTemplates, TemplateConfig};

/// The directory user templates live in.
pub(crate) fn dir() -> Option<PathBuf> {
    devscriptor_core::user_dirs::config_dir().map(|dir| dir.join("devscriptor").join("templates"))
}

/// The built-in templates plus the user's.
pub(crate) fn load() -> ConfigurationTemplates {
    dir().map_or_else(ConfigurationTemplates::new, |dir| load_from(&dir))
}

fn load_from(dir: &Path) -> ConfigurationTemplates {
    let mut templates = ConfigurationTemplates::new();
    if dir.is_dir()
        && let Err(e) = templates.load_templates_from_dir(dir)
    {
        eprintln!("⚠️  Skipping your templates in {}: {e}", dir.display());
    }
    templates
}

/// A template name doubles as a file name, so it is held to safe characters.
fn check_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    anyhow::ensure!(
        valid,
        "'{name}' is not a usable name: use up to 64 letters, digits, '-', '_' or '.', \
         not starting with '.'"
    );
    Ok(())
}

/// Create and save the user template `name`, a copy of `based_on` or a blank
/// one; `kind` ("template" or "profile") words the messages. Returns the file.
pub(crate) fn create(name: &str, based_on: Option<&str>, kind: &str) -> Result<PathBuf> {
    let dir = dir().context("No configuration directory to keep templates in")?;
    create_in(&dir, name, based_on, kind)
}

fn create_in(dir: &Path, name: &str, based_on: Option<&str>, kind: &str) -> Result<PathBuf> {
    check_name(name)?;
    let templates = load_from(dir);
    anyhow::ensure!(
        !templates.has_template(name),
        "A {kind} named '{name}' already exists"
    );
    let template = match based_on {
        Some(base) => {
            let mut template = templates
                .get_template(base)
                .cloned()
                .with_context(|| format!("Base {kind} '{base}' not found"))?;
            template.name = name.to_string();
            template
        }
        None => TemplateConfig::new(name, format!("Custom {kind}"), "Custom"),
    };

    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&template)?)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(path)
}

/// Delete the user template `name`. Built-in ones cannot be deleted.
pub(crate) fn delete(name: &str, kind: &str) -> Result<PathBuf> {
    let dir = dir().context("No configuration directory holding templates")?;
    delete_in(&dir, name, kind)
}

fn delete_in(dir: &Path, name: &str, kind: &str) -> Result<PathBuf> {
    // Find the file by the name inside it, as loading does, so a hand-placed
    // file with another file name is found too.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let template: Option<TemplateConfig> = match path.extension().and_then(|e| e.to_str()) {
                Some("json") => TemplateConfig::parse(&text, false).ok(),
                Some("yaml" | "yml") => TemplateConfig::parse(&text, true).ok(),
                _ => None,
            };
            if template.is_some_and(|t| t.name == name) {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to delete {}", path.display()))?;
                return Ok(path);
            }
        }
    }
    if ConfigurationTemplates::new().has_template(name) {
        anyhow::bail!("'{name}' is a built-in {kind} and cannot be deleted");
    }
    anyhow::bail!("No {kind} named '{name}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_created_template_is_seen_by_the_next_load_and_deleted_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = ConfigurationTemplates::new()
            .list_templates()
            .first()
            .map(|n| n.to_string())
            .expect("a built-in template");

        let path = create_in(dir.path(), "mine", Some(&builtin), "template").unwrap();
        assert!(path.is_file());
        let loaded = load_from(dir.path());
        assert_eq!(loaded.get_template("mine").unwrap().name, "mine");

        assert!(
            create_in(dir.path(), "mine", None, "template").is_err(),
            "taken"
        );
        assert_eq!(delete_in(dir.path(), "mine", "profile").unwrap(), path);
        assert!(!load_from(dir.path()).has_template("mine"));
    }

    #[test]
    fn built_ins_cannot_be_deleted_and_names_stay_file_names() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = ConfigurationTemplates::new()
            .list_templates()
            .first()
            .map(|n| n.to_string())
            .unwrap();
        let err = delete_in(dir.path(), &builtin, "profile").unwrap_err();
        assert!(err.to_string().contains("built-in"), "{err}");

        for bad in ["", "../x", "a/b", ".hidden"] {
            assert!(
                create_in(dir.path(), bad, None, "template").is_err(),
                "{bad:?}"
            );
        }
    }
}
