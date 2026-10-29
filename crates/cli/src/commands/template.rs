//! Template command implementations

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use devscriptor_core::config::{ProjectConfig, StackDetectionResult, TechStack, TemplateConfig};
use devscriptor_core::utils::validation::PathValidationOptions;

use crate::commands::common::validate_path;
use crate::commands::user_templates;

/// List templates
pub fn list(category: Option<String>, search: Option<String>) -> Result<()> {
    let templates = user_templates::load();

    println!("\n{}", "═".repeat(60));
    println!(" Available Templates");
    println!("{}", "═".repeat(60));

    let filtered: Vec<&TemplateConfig> = templates
        .list_templates()
        .into_iter()
        .filter_map(|name| templates.get_template(name))
        .filter(|template| {
            category
                .as_ref()
                .is_none_or(|category| template.category.eq_ignore_ascii_case(category))
        })
        .filter(|template| {
            search
                .as_ref()
                .is_none_or(|search| template.name.contains(search.as_str()))
        })
        .collect();

    if filtered.is_empty() {
        println!("\nNo templates found.");
    } else {
        for template in filtered {
            println!("\n• {}", template.name);
            println!("  Category: {}", template.category);
            println!("  Description: {}", template.description);
        }
    }

    Ok(())
}

/// Show a template: what it is for, and the configuration it applies.
pub fn show(name: String) -> Result<()> {
    let templates = user_templates::load();
    let template = templates
        .get_template(&name)
        .with_context(|| format!("Template not found: {name}"))?;

    println!("\n{}", "═".repeat(60));
    println!(" Template: {name}");
    println!("{}", "═".repeat(60));
    print_details(template)
}

/// A template's details and the `devscriptor.yaml` it writes.
pub(crate) fn print_details(template: &TemplateConfig) -> Result<()> {
    println!("Category: {}", template.category);
    println!("Description: {}", template.description);
    if !template.tags.is_empty() {
        println!("Tags: {}", template.tags.join(", "));
    }
    if !template.use_cases.is_empty() {
        println!("Use cases: {}", template.use_cases.join("; "));
    }
    println!("\nApplies:\n");
    print!(
        "{}",
        template
            .config
            .to_yaml(&format!("From the template `{}`.", template.name))?
    );
    Ok(())
}

/// Write `config` to `output` as a project configuration file headed by
/// `comment`, and say what was done.
pub(crate) fn write_config(config: &ProjectConfig, output: &Path, comment: &str) -> Result<()> {
    let _ = validate_path(output, &PathValidationOptions::default())?;
    let replaced = output.exists();
    let yaml = config.to_yaml(&format!(
        "Devscriptor project configuration: {comment}\n`devscriptor validate` checks it."
    ))?;
    std::fs::write(output, yaml)
        .with_context(|| format!("Failed to write to {}", output.display()))?;
    println!(
        "✅ {} {}",
        if replaced { "Replaced" } else { "Wrote" },
        output.display()
    );
    Ok(())
}

/// Apply a template: write its project configuration to `output`.
pub fn apply(name: String, output: PathBuf) -> Result<()> {
    let templates = user_templates::load();
    let config = templates
        .apply_template(&name)
        .with_context(|| format!("Template not found: {name}"))?;
    write_config(config, &output, &format!("the template `{name}`."))?;
    println!("   Template '{name}' applied.");
    Ok(())
}

/// Create a new template, saved with the user's templates
pub fn create(name: String, based_on: Option<String>) -> Result<()> {
    let path = user_templates::create(&name, based_on.as_deref(), "template")?;
    match based_on {
        Some(base) => println!(
            "✅ Template '{name}' created from '{base}' in {}",
            path.display()
        ),
        None => println!("✅ Template '{name}' created in {}", path.display()),
    }
    Ok(())
}

/// Export template
pub fn export(name: String, output: Option<PathBuf>) -> Result<()> {
    let templates = user_templates::load();

    let template = templates
        .get_template(&name)
        .ok_or_else(|| anyhow::anyhow!("Template '{name}' not found"))?;

    let export_path = match output {
        Some(path) => {
            // Validate output path for dangerous patterns
            let _ = validate_path(&path, &PathValidationOptions::default())?;
            path
        }
        None => PathBuf::from(format!("{name}.template.json")),
    };

    let json = serde_json::to_string_pretty(template)?;
    std::fs::write(&export_path, json)
        .with_context(|| format!("Failed to write to {}", export_path.display()))?;

    println!(
        "✅ Template '{}' exported to {}",
        name,
        export_path.display()
    );

    Ok(())
}

/// Analyze a project: what it is built with, the template that suits it, and
/// the configuration its stacks suggest, written to `output` when given.
pub fn analyze(project_path: PathBuf, output: Option<PathBuf>) -> Result<()> {
    let validated = validate_path(&project_path, &PathValidationOptions::must_be_directory())?;
    println!("\n{}", "═".repeat(60));
    println!(" Project Template Analysis");
    println!("{}", "═".repeat(60));
    println!("\n📁 Project: {}", validated.display());

    let result = StackDetectionResult::detect(&validated);
    if result.primary_stack == TechStack::Unknown {
        println!("\nNo stack recognised: no build file, and no dominant language.");
        println!("Every supported language is indexed by default; no configuration is needed.");
        return Ok(());
    }
    let stacks: Vec<&str> = result
        .all_stacks
        .iter()
        .map(TechStack::display_name)
        .collect();
    println!("\n🧩 Stacks: {}", stacks.join(", "));

    let templates = user_templates::load();
    match result
        .recommended_template
        .as_deref()
        .filter(|name| templates.has_template(name))
    {
        Some(name) => {
            println!("📦 Template: {name} (`devscriptor template apply {name}` writes it)")
        }
        None => println!("📦 Template: none of the templates suits it better than the suggestion"),
    }

    let config = result.suggested_config();
    println!("\nSuggested configuration:\n");
    print!(
        "{}",
        config.to_yaml(&format!("Suggested for {}.", stacks.join(", ")))?
    );
    match output {
        Some(output) => write_config(
            &config,
            &output,
            &format!("suggested for {}.", stacks.join(", ")),
        )?,
        None => println!(
            "\n`devscriptor template analyze {} --output devscriptor.yaml` writes it.",
            project_path.display()
        ),
    }
    Ok(())
}
