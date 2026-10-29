//! Profile command implementations

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::commands::template::{print_details, write_config};
use crate::commands::user_templates;

/// List profiles
pub fn list() -> Result<()> {
    let templates = user_templates::load();
    let template_names = templates.list_templates();

    println!("\n{}", "═".repeat(60));
    println!(" Available Profiles");
    println!("{}", "═".repeat(60));

    if template_names.is_empty() {
        println!("\nNo profiles found.");
    } else {
        for name in &template_names {
            if let Some(template) = templates.get_template(name) {
                println!("\n• {}", template.name);
                println!("  Category: {}", template.category);
            }
        }
    }

    Ok(())
}

/// Show a profile: what it is for, and the configuration it applies.
pub fn show(name: String) -> Result<()> {
    let templates = user_templates::load();
    let template = templates
        .get_template(&name)
        .with_context(|| format!("Profile not found: {name}"))?;

    println!("\n{}", "═".repeat(60));
    println!(" Profile: {name}");
    println!("{}", "═".repeat(60));
    print_details(template)
}

/// Create a new profile, saved with the user's templates
pub fn create(name: String, based_on: Option<String>) -> Result<()> {
    let path = user_templates::create(&name, based_on.as_deref(), "profile")?;
    match based_on {
        Some(base) => println!(
            "✅ Profile '{name}' created from '{base}' in {}",
            path.display()
        ),
        None => println!("✅ Profile '{name}' created in {}", path.display()),
    }
    Ok(())
}

/// Apply a profile: write its project configuration to `output`.
pub fn apply(name: String, output: PathBuf) -> Result<()> {
    let templates = user_templates::load();
    let config = templates
        .apply_template(&name)
        .with_context(|| format!("Profile not found: {name}"))?;
    write_config(config, &output, &format!("the profile `{name}`."))?;
    println!("   Profile '{name}' applied.");
    Ok(())
}

/// Delete a profile the user created (built-in ones cannot be deleted)
pub fn delete(name: String) -> Result<()> {
    let path = user_templates::delete(&name, "profile")?;
    println!("✅ Profile '{name}' deleted ({})", path.display());
    Ok(())
}
