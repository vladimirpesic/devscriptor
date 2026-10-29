//! Entity cleaning utilities for normalizing and sanitizing parsed entities
//!
//! This module provides functionality to clean and normalize entities after parsing,
//! ensuring consistency and preventing memory issues with excessively large fields.

use crate::entities::{
    Class, Component, Constant, Contract, Decorator, Documentation, Entity, Enum, Extension,
    Function, Interface, Macro, Module, Namespace, Struct, Trait, TypeAlias, Union,
};
use crate::parser::constants::{
    MAX_BODY_HARD_LIMIT, MAX_BODY_LENGTH, MAX_EXTENDS_LENGTH, MAX_FIELDS_DICT_LENGTH,
    MAX_FIELDS_STRING_LENGTH, MAX_IMPLEMENTS_LENGTH, MAX_NAME_LENGTH, MAX_PARAMETER_LIST_LENGTH,
    MAX_PARAMETER_STRING_LENGTH, MAX_RETURN_TYPE_LENGTH, TRUNCATION_MARKER,
};

/// Clean and normalize an entity
///
/// # Arguments
///
/// * `entity` - The entity to clean
///
/// # Returns
///
/// The cleaned entity with normalized fields and truncated oversized content
pub fn clean_entity(entity: Entity) -> Entity {
    match entity {
        Entity::Function(f) => Entity::Function(clean_function(f)),
        Entity::Class(c) => Entity::Class(clean_class(c)),
        Entity::Table(t) => Entity::Table(clean_class(t)),
        Entity::Index(i) => Entity::Index(clean_class(i)),
        Entity::Struct(s) => Entity::Struct(clean_struct(s)),
        Entity::Macro(m) => Entity::Macro(clean_macro(m)),
        Entity::Enum(e) => Entity::Enum(clean_enum(e)),
        Entity::Interface(i) => Entity::Interface(clean_interface(i)),
        Entity::Trait(t) => Entity::Trait(clean_trait(t)),
        Entity::Module(m) => Entity::Module(clean_module(m)),
        Entity::Namespace(n) => Entity::Namespace(clean_namespace(n)),
        Entity::Constant(c) => Entity::Constant(clean_constant(c)),
        Entity::TypeAlias(t) => Entity::TypeAlias(clean_type_alias(t)),
        Entity::Union(u) => Entity::Union(clean_union(u)),
        Entity::Component(c) => Entity::Component(clean_component(c)),
        Entity::Contract(c) => Entity::Contract(clean_contract(c)),
        Entity::Extension(e) => Entity::Extension(clean_extension(e)),
        Entity::Decorator(d) => Entity::Decorator(clean_decorator(d)),
        Entity::Section(d) => Entity::Section(clean_documentation(d)),
    }
}

/// Clean a function entity
///
/// Normalizes whitespace and truncates oversized fields.
fn clean_function(mut entity: Function) -> Function {
    // Normalize name whitespace
    entity.name = entity.name.trim().to_string();

    // Normalize file path
    entity.file_path = entity.file_path.trim().to_string();

    // Clean body content - truncate if too large
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);

    // Clean parameters if present
    entity.parameters = entity.parameters.map(|p| {
        if p.len() > MAX_PARAMETER_STRING_LENGTH {
            truncate_string(&p, MAX_PARAMETER_STRING_LENGTH)
        } else {
            p
        }
    });

    // Clean return type if present
    entity.return_type = entity.return_type.map(|r| {
        if r.len() > MAX_RETURN_TYPE_LENGTH {
            truncate_string(&r, MAX_RETURN_TYPE_LENGTH)
        } else {
            r
        }
    });

    // Clean modifiers
    entity.modifiers = entity.modifiers.map(|m| m.trim().to_string());
    entity.access_modifiers = entity.access_modifiers.map(|a| a.trim().to_string());

    entity
}

/// Clean a class entity
///
/// Normalizes whitespace and truncates oversized fields.
fn clean_class(mut entity: Class) -> Class {
    // Normalize name whitespace
    entity.name = entity.name.trim().to_string();

    // Normalize file path
    entity.file_path = entity.file_path.trim().to_string();

    // Clean body content
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);

    // Clean extends if present
    entity.extends = entity.extends.map(|e| {
        if e.len() > MAX_EXTENDS_LENGTH {
            truncate_string(&e, MAX_EXTENDS_LENGTH)
        } else {
            e
        }
    });

    // Clean implements if present
    entity.implements = entity.implements.map(|i| {
        if i.len() > MAX_IMPLEMENTS_LENGTH {
            truncate_string(&i, MAX_IMPLEMENTS_LENGTH)
        } else {
            i
        }
    });

    // Clean constants if present
    entity.constants = entity.constants.map(|c| {
        if c.len() > MAX_FIELDS_STRING_LENGTH {
            truncate_string(&c, MAX_FIELDS_STRING_LENGTH)
        } else {
            c
        }
    });

    // Clean access modifiers
    entity.access_modifiers = entity.access_modifiers.map(|a| a.trim().to_string());

    entity
}

/// Clean a struct entity
///
/// Normalizes whitespace and truncates oversized fields.
fn clean_struct(mut entity: Struct) -> Struct {
    // Normalize name whitespace
    entity.name = entity.name.trim().to_string();

    // Normalize file path
    entity.file_path = entity.file_path.trim().to_string();

    // Clean body content
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);

    entity
}

/// Clean a macro entity
///
/// Normalizes whitespace and truncates oversized fields.
fn clean_macro(mut entity: Macro) -> Macro {
    // Normalize name whitespace
    entity.name = entity.name.trim().to_string();

    // Normalize file path
    entity.file_path = entity.file_path.trim().to_string();

    // Clean body content
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);

    entity
}

/// Clean an enum entity
fn clean_enum(mut entity: Enum) -> Enum {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean an interface entity
fn clean_interface(mut entity: Interface) -> Interface {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a trait entity
fn clean_trait(mut entity: Trait) -> Trait {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a module entity
fn clean_module(mut entity: Module) -> Module {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a namespace entity
fn clean_namespace(mut entity: Namespace) -> Namespace {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a constant entity
fn clean_constant(mut entity: Constant) -> Constant {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a type alias entity
fn clean_type_alias(mut entity: TypeAlias) -> TypeAlias {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a union entity
fn clean_union(mut entity: Union) -> Union {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a component entity
fn clean_component(mut entity: Component) -> Component {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a contract entity
fn clean_contract(mut entity: Contract) -> Contract {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean an extension entity
fn clean_extension(mut entity: Extension) -> Extension {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a decorator entity
fn clean_decorator(mut entity: Decorator) -> Decorator {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Clean a documentation entity
fn clean_documentation(mut entity: Documentation) -> Documentation {
    entity.name = entity.name.trim().to_string();
    entity.file_path = entity.file_path.trim().to_string();
    entity.body = truncate_string(&entity.body, MAX_BODY_LENGTH);
    entity
}

/// Truncate a string to the specified maximum length
///
/// # Arguments
///
/// * `s` - The string to truncate
/// * `max_len` - Maximum allowed length
///
/// # Returns
///
/// The truncated string with a truncation marker if truncated
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated_len = max_len.saturating_sub(TRUNCATION_MARKER.len());
        // Use char_indices to ensure we split on a valid UTF-8 boundary
        if let Some((byte_idx, _)) = s.char_indices().nth(truncated_len) {
            format!("{}{}", &s[..byte_idx], TRUNCATION_MARKER)
        } else {
            s.to_string()
        }
    }
}

/// Clean a batch of entities
///
/// # Arguments
///
/// * `entities` - Vector of entities to clean
///
/// # Returns
///
/// Vector of cleaned entities
pub fn clean_entities(entities: Vec<Entity>) -> Vec<Entity> {
    entities.into_iter().map(clean_entity).collect()
}

/// Validate and clean entity names
///
/// Checks if the name is valid and returns a cleaned version.
///
/// # Arguments
///
/// * `name` - The name to validate and clean
///
/// # Returns
///
/// `Some(cleaned_name)` if valid, `None` if empty after trimming
pub fn validate_and_clean_name(name: &str) -> Option<String> {
    let cleaned = name.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

/// Check if entity body exceeds hard limit
///
/// # Arguments
///
/// * `body` - The body content to check
///
/// # Returns
///
/// `true` if body exceeds hard limit
pub fn body_exceeds_limit(body: &str) -> bool {
    body.len() > MAX_BODY_HARD_LIMIT
}

/// Get the maximum allowed name length
#[must_use]
pub const fn max_name_length() -> usize {
    MAX_NAME_LENGTH
}

/// Get the maximum allowed body length
#[must_use]
pub const fn max_body_length() -> usize {
    MAX_BODY_LENGTH
}

/// Get the hard limit for body length
#[must_use]
pub const fn max_body_hard_limit() -> usize {
    MAX_BODY_HARD_LIMIT
}

/// Get the maximum allowed parameter string length
#[must_use]
pub const fn max_parameter_string_length() -> usize {
    MAX_PARAMETER_STRING_LENGTH
}

/// Get the maximum allowed parameter list length
#[must_use]
pub const fn max_parameter_list_length() -> usize {
    MAX_PARAMETER_LIST_LENGTH
}

/// Get the maximum allowed fields dict length
#[must_use]
pub const fn max_fields_dict_length() -> usize {
    MAX_FIELDS_DICT_LENGTH
}

/// Get the maximum allowed fields string length
#[must_use]
pub const fn max_fields_string_length() -> usize {
    MAX_FIELDS_STRING_LENGTH
}

/// Get the maximum allowed return type length
#[must_use]
pub const fn max_return_type_length() -> usize {
    MAX_RETURN_TYPE_LENGTH
}

/// Get the maximum allowed extends length
#[must_use]
pub const fn max_extends_length() -> usize {
    MAX_EXTENDS_LENGTH
}

/// Get the maximum allowed implements length
#[must_use]
pub const fn max_implements_length() -> usize {
    MAX_IMPLEMENTS_LENGTH
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::SourceLocation;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn create_test_function() -> Function {
        Function {
            id: Uuid::new_v4(),
            name: "  test_function  ".to_string(),
            file_path: "  /test/file.rs  ".to_string(),
            body: "fn test() {}".to_string(),
            location: SourceLocation::new(1, 0, 10, 1),
            language: "rust".to_string(),
            parent_function: None,
            parent_class: None,
            parameters: Some("  param1: String  ".to_string()),
            return_type: Some("  Result<T>  ".to_string()),
            modifiers: Some("  pub async  ".to_string()),
            access_modifiers: Some("  public  ".to_string()),
            docstring: None,
            decorators: vec![],
            implements: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn test_clean_function_trims_whitespace() {
        let func = create_test_function();
        let cleaned = clean_function(func);

        assert_eq!(cleaned.name, "test_function");
        assert_eq!(cleaned.file_path, "/test/file.rs");
        // Parameters are only truncated if too long, not trimmed for whitespace
        assert_eq!(cleaned.parameters, Some("  param1: String  ".to_string()));
        // Return type is only truncated if too long, not trimmed for whitespace
        assert_eq!(cleaned.return_type, Some("  Result<T>  ".to_string()));
        assert_eq!(cleaned.modifiers, Some("pub async".to_string()));
        assert_eq!(cleaned.access_modifiers, Some("public".to_string()));
    }

    #[test]
    fn test_truncate_string() {
        let short = "short";
        assert_eq!(truncate_string(short, 100), "short");

        let long = "a".repeat(1000);
        let truncated = truncate_string(&long, 100);
        assert!(truncated.len() <= 100);
        assert!(truncated.ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn test_validate_and_clean_name() {
        assert_eq!(
            validate_and_clean_name("  valid_name  "),
            Some("valid_name".to_string())
        );
        assert_eq!(validate_and_clean_name("   "), None);
        assert_eq!(validate_and_clean_name(""), None);
    }

    #[test]
    fn test_body_exceeds_limit() {
        let small_body = "x".repeat(100);
        assert!(!body_exceeds_limit(&small_body));

        let large_body = "x".repeat(MAX_BODY_HARD_LIMIT + 1);
        assert!(body_exceeds_limit(&large_body));
    }

    #[test]
    fn test_clean_entity_dispatches_correctly() {
        let func = create_test_function();
        let entity = Entity::Function(func);

        let cleaned = clean_entity(entity);

        match cleaned {
            Entity::Function(f) => {
                assert_eq!(f.name, "test_function");
            }
            _ => panic!("Expected Function entity"),
        }
    }

    #[test]
    fn test_clean_entities_batch() {
        let func1 = Entity::Function(create_test_function());
        let func2 = Entity::Function(create_test_function());

        let entities = vec![func1, func2];
        let cleaned = clean_entities(entities);

        assert_eq!(cleaned.len(), 2);
        for entity in cleaned {
            match entity {
                Entity::Function(f) => assert_eq!(f.name, "test_function"),
                _ => panic!("Expected Function entity"),
            }
        }
    }
}
