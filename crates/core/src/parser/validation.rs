//! Entity validation utilities for ensuring parsed entities meet quality standards
//!
//! This module provides functionality to validate entities after parsing,
//! checking for valid names, line numbers, field lengths, and type-specific constraints.

use crate::entities::{
    Class, Component, Constant, Contract, Decorator, Documentation, Entity, Enum, Extension,
    Function, Interface, Macro, Module, Namespace, Relationship, SourceLocation, Struct, Trait,
    TypeAlias, Union,
};
use crate::parser::constants::{
    MAX_BODY_HARD_LIMIT as MAX_BODY_LENGTH, MAX_CONFIDENCE_SCORE, MAX_EXTENDS_LENGTH,
    MAX_FIELDS_DICT_LENGTH, MAX_FIELDS_STRING_LENGTH, MAX_IMPLEMENTS_LENGTH, MAX_NAME_LENGTH,
    MAX_PARAMETER_LIST_LENGTH, MAX_PARAMETER_STRING_LENGTH, MAX_RETURN_TYPE_LENGTH,
    MIN_CONFIDENCE_SCORE, VALID_MACRO_TYPES,
};
use tracing::warn;

/// Result of entity validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// Entity is valid
    Valid,
    /// Entity is invalid with reason
    Invalid(String),
}

/// Record of a validation failure for a specific entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationFailure {
    /// Entity type string
    pub entity_type: String,
    /// Entity name (if known)
    pub entity_name: String,
    /// Reason the entity failed validation
    pub reason: String,
}

impl ValidationResult {
    /// Check if validation passed
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// Check if validation failed
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        !self.is_valid()
    }
}

/// Validate an entity
///
/// # Arguments
///
/// * `entity` - The entity to validate
/// * `file_path` - Expected file path for consistency check
///
/// # Returns
///
/// `ValidationResult::Valid` if the entity passes all validation checks,
/// otherwise `ValidationResult::Invalid` with a reason
pub fn validate_entity(entity: &Entity, file_path: &str) -> ValidationResult {
    // Check file path consistency
    if entity.file_path() != file_path {
        return ValidationResult::Invalid(format!(
            "File path mismatch: {} vs {}",
            entity.file_path(),
            file_path
        ));
    }

    // Validate name
    let name = entity.name();
    if name.trim().is_empty() {
        return ValidationResult::Invalid("Entity has empty name".to_string());
    }
    if name.len() > MAX_NAME_LENGTH {
        return ValidationResult::Invalid(format!(
            "Entity name too long: {} characters",
            name.len()
        ));
    }

    // Type-specific validation
    match entity {
        Entity::Function(f) => validate_function(f),
        Entity::Class(c) => validate_class(c),
        Entity::Table(t) => validate_class(t),
        Entity::Index(i) => validate_class(i),
        Entity::Struct(s) => validate_struct(s),
        Entity::Macro(m) => validate_macro(m),
        Entity::Enum(e) => validate_enum(e),
        Entity::Interface(i) => validate_interface(i),
        Entity::Trait(t) => validate_trait(t),
        Entity::Module(m) => validate_module(m),
        Entity::Namespace(n) => validate_namespace(n),
        Entity::Constant(c) => validate_constant(c),
        Entity::TypeAlias(t) => validate_type_alias(t),
        Entity::Union(u) => validate_union(u),
        Entity::Component(c) => validate_component(c),
        Entity::Contract(c) => validate_contract(c),
        Entity::Extension(e) => validate_extension(e),
        Entity::Decorator(d) => validate_decorator(d),
        Entity::Section(d) => validate_documentation(d),
    }
}

/// Validate a function entity
fn validate_function(entity: &Function) -> ValidationResult {
    // Validate name length
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Function name too long: {} characters",
            entity.name.len()
        ));
    }

    // Validate parameters if present
    if let Some(ref params) = entity.parameters
        && !validate_parameter_string(params)
    {
        return ValidationResult::Invalid("Function parameters too long".to_string());
    }

    // Validate return type if present
    if let Some(ref return_type) = entity.return_type
        && !validate_return_type(return_type)
    {
        return ValidationResult::Invalid("Function return type too long".to_string());
    }

    // Validate body length
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Function body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }

    ValidationResult::Valid
}

/// Validate a class entity
fn validate_class(entity: &Class) -> ValidationResult {
    // Validate name length
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Class name too long: {} characters",
            entity.name.len()
        ));
    }

    // Validate extends if present
    if let Some(ref extends) = entity.extends
        && extends.len() > MAX_EXTENDS_LENGTH
    {
        return ValidationResult::Invalid("Class extends information too long".to_string());
    }

    // Validate implements if present
    if let Some(ref implements) = entity.implements
        && implements.len() > MAX_IMPLEMENTS_LENGTH
    {
        return ValidationResult::Invalid("Class implements information too long".to_string());
    }

    // Validate body length
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Class body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }

    ValidationResult::Valid
}

/// Validate a struct entity
fn validate_struct(entity: &Struct) -> ValidationResult {
    // Validate name length
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Struct name too long: {} characters",
            entity.name.len()
        ));
    }

    // Validate body length
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Struct body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }

    ValidationResult::Valid
}

/// Validate a macro entity
fn validate_macro(entity: &Macro) -> ValidationResult {
    // Validate name length
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Macro name too long: {} characters",
            entity.name.len()
        ));
    }

    // Validate body length
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Macro body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }

    ValidationResult::Valid
}

/// Validate an enum entity
fn validate_enum(entity: &Enum) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Enum name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Enum body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate an interface entity
fn validate_interface(entity: &Interface) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Interface name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Interface body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a trait entity
fn validate_trait(entity: &Trait) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Trait name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Trait body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a module entity
fn validate_module(entity: &Module) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Module name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Module body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a namespace entity
fn validate_namespace(entity: &Namespace) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Namespace name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Namespace body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a constant entity
fn validate_constant(entity: &Constant) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Constant name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Constant body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a type alias entity
fn validate_type_alias(entity: &TypeAlias) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Type alias name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Type alias body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a union entity
fn validate_union(entity: &Union) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Union name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Union body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a component entity
fn validate_component(entity: &Component) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Component name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Component body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a contract entity
fn validate_contract(entity: &Contract) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Contract name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Contract body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate an extension entity
fn validate_extension(entity: &Extension) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Extension name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Extension body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a decorator entity
fn validate_decorator(entity: &Decorator) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Decorator name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Decorator body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a documentation entity
fn validate_documentation(entity: &Documentation) -> ValidationResult {
    if !validate_entity_name_length(&entity.name) {
        return ValidationResult::Invalid(format!(
            "Documentation name too long: {} characters",
            entity.name.len()
        ));
    }
    if entity.body.len() > MAX_BODY_LENGTH {
        return ValidationResult::Invalid(format!(
            "Documentation body exceeds size limit: {} bytes",
            entity.body.len()
        ));
    }
    ValidationResult::Valid
}

/// Validate a relationship entity.
///
/// Rejects self-referential relationships (source_id == target_id) which
/// are almost always extraction bugs rather than real code facts.
///
/// # Arguments
///
/// * `relationship` - The relationship to validate
///
/// # Returns
///
/// `ValidationResult::Valid` if the relationship passes all validation checks
pub fn validate_relationship(relationship: &Relationship) -> ValidationResult {
    // Reject self-loops — a function cannot call itself at the granularity
    // we extract, and these are almost always parser false-positives.
    if relationship.source_id == relationship.target_id {
        return ValidationResult::Invalid("Self-referential relationship".to_string());
    }

    // Metadata, if present, must be a valid JSON object or array.
    if let Some(meta) = &relationship.metadata
        && !meta.is_object()
        && !meta.is_array()
        && !meta.is_null()
    {
        return ValidationResult::Invalid(
            "Relationship metadata must be an object or array".to_string(),
        );
    }

    ValidationResult::Valid
}

/// Validate source location
///
/// # Arguments
///
/// * `location` - The source location to validate
///
/// # Returns
///
/// `true` if line numbers are valid and consistent
pub fn validate_source_location(location: &SourceLocation) -> bool {
    validate_line_numbers(location.start_line, location.end_line)
}

/// Validate entity name length
///
/// # Arguments
///
/// * `name` - The name to validate
///
/// # Returns
///
/// `true` if name length is within limits
pub fn validate_entity_name_length(name: &str) -> bool {
    name.len() <= MAX_NAME_LENGTH
}

/// Validate parameter string length
///
/// # Arguments
///
/// * `params` - The parameter string to validate
///
/// # Returns
///
/// `true` if parameter string length is within limits
pub fn validate_parameter_string(params: &str) -> bool {
    params.len() <= MAX_PARAMETER_STRING_LENGTH
}

/// Validate parameter list length
///
/// # Arguments
///
/// * `params` - The parameter list to validate
///
/// # Returns
///
/// `true` if parameter list length is within limits
pub fn validate_parameter_list(params: &[String]) -> bool {
    params.len() <= MAX_PARAMETER_LIST_LENGTH
        && params
            .iter()
            .all(|p| p.len() <= MAX_PARAMETER_STRING_LENGTH)
}

/// Validate return type string length
///
/// # Arguments
///
/// * `return_type` - The return type string to validate
///
/// # Returns
///
/// `true` if return type length is within limits
pub fn validate_return_type(return_type: &str) -> bool {
    return_type.len() <= MAX_RETURN_TYPE_LENGTH
}

/// Validate fields dictionary length
///
/// # Arguments
///
/// * `fields_count` - The number of fields to validate
///
/// # Returns
///
/// `true` if fields count is within limits
pub fn validate_fields_count(fields_count: usize) -> bool {
    fields_count <= MAX_FIELDS_DICT_LENGTH
}

/// Validate fields string length
///
/// # Arguments
///
/// * `fields` - The fields string to validate
///
/// # Returns
///
/// `true` if fields string length is within limits
pub fn validate_fields_string(fields: &str) -> bool {
    fields.len() <= MAX_FIELDS_STRING_LENGTH
}

/// Validate body content length
///
/// # Arguments
///
/// * `body` - The body content to validate
///
/// # Returns
///
/// `true` if body length is within limits
pub fn validate_body_content(body: &str) -> bool {
    body.len() <= MAX_BODY_LENGTH
}

/// Validate line numbers
///
/// # Arguments
///
/// * `start_line` - Starting line number
/// * `end_line` - Ending line number
///
/// # Returns
///
/// `true` if line numbers are valid and consistent
pub fn validate_line_numbers(start_line: u32, end_line: u32) -> bool {
    if start_line == 0 || end_line == 0 {
        return false;
    }
    if start_line > end_line {
        return false;
    }
    true
}

/// Validate confidence score
///
/// # Arguments
///
/// * `score` - The confidence score to validate
///
/// # Returns
/// `true` if score is within valid range [0.0, 1.0]
pub fn validate_confidence_score(score: f32) -> bool {
    (MIN_CONFIDENCE_SCORE..=MAX_CONFIDENCE_SCORE).contains(&score)
}

/// Validate macro type
///
/// # Arguments
///
/// * `macro_type` - The macro type to validate
///
/// # Returns
///
/// `true` if macro type is valid
pub fn validate_macro_type(macro_type: &str) -> bool {
    VALID_MACRO_TYPES.contains(&macro_type)
}

/// Filter valid entities from a batch
///
/// # Arguments
///
/// * `entities` - Vector of entities to validate
/// * `file_path` - Expected file path for consistency check
///
/// # Returns
///
/// Filter valid entities and return both the valid set and validation failures.
///
/// Invalid entities are not dropped silently: `failures` names each, for diagnostics.
pub fn filter_valid_entities(
    entities: Vec<Entity>,
    file_path: &str,
) -> (Vec<Entity>, Vec<ValidationFailure>) {
    let mut valid = Vec::with_capacity(entities.len());
    let mut failures = Vec::new();

    for entity in entities {
        let result = validate_entity(&entity, file_path);
        if let ValidationResult::Invalid(ref reason) = result {
            warn!(
                "Filtering out invalid entity '{}': {}",
                entity.name(),
                reason
            );
            failures.push(ValidationFailure {
                entity_type: entity.entity_type().to_string(),
                entity_name: entity.name().to_string(),
                reason: reason.clone(),
            });
        } else {
            valid.push(entity);
        }
    }

    (valid, failures)
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

/// Get the minimum confidence score
#[must_use]
pub const fn min_confidence_score() -> f32 {
    MIN_CONFIDENCE_SCORE
}

/// Get the maximum confidence score
#[must_use]
pub const fn max_confidence_score() -> f32 {
    MAX_CONFIDENCE_SCORE
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn create_test_function() -> Function {
        Function {
            id: Uuid::new_v4(),
            name: "test_function".to_string(),
            file_path: "/test/file.rs".to_string(),
            body: "fn test() {}".to_string(),
            location: SourceLocation::new(1, 0, 10, 1),
            language: "rust".to_string(),
            parent_function: None,
            parent_class: None,
            parameters: Some("param1: String".to_string()),
            return_type: Some("Result<T>".to_string()),
            modifiers: Some("pub async".to_string()),
            access_modifiers: Some("public".to_string()),
            docstring: None,
            decorators: vec![],
            implements: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn test_validate_entity_valid() {
        let func = create_test_function();
        let entity = Entity::Function(func);

        let result = validate_entity(&entity, "/test/file.rs");
        assert!(result.is_valid());
    }

    #[test]
    fn test_validate_entity_file_path_mismatch() {
        let func = create_test_function();
        let entity = Entity::Function(func);

        let result = validate_entity(&entity, "/different/path.rs");
        assert!(result.is_invalid());
    }

    #[test]
    fn test_validate_entity_empty_name() {
        let mut func = create_test_function();
        func.name = "   ".to_string();
        let entity = Entity::Function(func);

        let result = validate_entity(&entity, "/test/file.rs");
        assert!(result.is_invalid());
    }

    #[test]
    fn test_validate_entity_name_too_long() {
        let mut func = create_test_function();
        func.name = "a".repeat(MAX_NAME_LENGTH + 1);
        let entity = Entity::Function(func);

        let result = validate_entity(&entity, "/test/file.rs");
        assert!(result.is_invalid());
    }

    #[test]
    fn test_validate_line_numbers() {
        assert!(validate_line_numbers(1, 10));
        assert!(validate_line_numbers(5, 5));
        assert!(!validate_line_numbers(0, 10));
        assert!(!validate_line_numbers(10, 5));
        assert!(!validate_line_numbers(5, 0));
    }

    #[test]
    fn test_validate_source_location() {
        let valid = SourceLocation::new(1, 0, 10, 1);
        assert!(validate_source_location(&valid));

        let invalid = SourceLocation::new(10, 0, 1, 1);
        assert!(!validate_source_location(&invalid));
    }

    #[test]
    fn test_validate_confidence_score() {
        assert!(validate_confidence_score(0.0));
        assert!(validate_confidence_score(0.5));
        assert!(validate_confidence_score(1.0));
        assert!(!validate_confidence_score(-0.1));
        assert!(!validate_confidence_score(1.1));
    }

    #[test]
    fn test_validate_parameter_string() {
        assert!(validate_parameter_string("x: i32"));
        assert!(!validate_parameter_string(
            &"x".repeat(MAX_PARAMETER_STRING_LENGTH + 1)
        ));
    }

    #[test]
    fn test_validate_return_type() {
        assert!(validate_return_type("Result<T>"));
        assert!(!validate_return_type(
            &"T".repeat(MAX_RETURN_TYPE_LENGTH + 1)
        ));
    }

    #[test]
    fn test_validate_body_content() {
        assert!(validate_body_content("fn main() {}"));
        assert!(!validate_body_content(&"x".repeat(MAX_BODY_LENGTH + 1)));
    }

    #[test]
    fn test_filter_valid_entities() {
        let func1 = Entity::Function(create_test_function());

        let mut func2 = create_test_function();
        func2.name = "".to_string();
        let func2_entity = Entity::Function(func2);

        let entities = vec![func1, func2_entity];
        let (valid, failures) = filter_valid_entities(entities, "/test/file.rs");

        assert_eq!(valid.len(), 1);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].entity_type, "function");
    }

    #[test]
    fn test_validate_relationship() {
        // Relationship struct doesn't have confidence_score field
        // so we just test that it validates successfully
        let relationship = Relationship {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            relationship_type: crate::entities::RelationshipType::Calls,
            metadata: None,
            created_at: OffsetDateTime::now_utc(),
        };

        // Currently Relationship has no fields that need validation
        // beyond what we can check (all fields are valid UUIDs by construction)
        // The validate_relationship function currently only checks confidence_score
        // which doesn't exist, so we skip this test for now
        // In the future, if confidence_score is added, this test should be updated
        assert!(validate_relationship(&relationship).is_valid());
    }

    #[test]
    fn test_validation_result() {
        let valid = ValidationResult::Valid;
        assert!(valid.is_valid());
        assert!(!valid.is_invalid());

        let invalid = ValidationResult::Invalid("test reason".to_string());
        assert!(!invalid.is_valid());
        assert!(invalid.is_invalid());
    }
}
