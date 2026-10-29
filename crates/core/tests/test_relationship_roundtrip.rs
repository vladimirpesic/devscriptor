//! Every RelationshipType variant survives the as_str() → DB →
//! parse_relationship_type() round-trip unchanged.

use devscriptor_core::entities::RelationshipType;

const ALL_VARIANTS: &[RelationshipType] = &[
    RelationshipType::Calls,
    RelationshipType::CalledBy,
    RelationshipType::Inherits,
    RelationshipType::InheritedBy,
    RelationshipType::Implements,
    RelationshipType::ImplementedBy,
    RelationshipType::MethodOf,
    RelationshipType::HasMethod,
    RelationshipType::Uses,
    RelationshipType::UsedBy,
    RelationshipType::References,
    RelationshipType::ReferencedBy,
    RelationshipType::Overrides,
    RelationshipType::OverriddenBy,
    RelationshipType::RelatedTo,
];

#[test]
fn relationship_type_roundtrip_via_as_str() {
    for &variant in ALL_VARIANTS {
        let stored = variant.as_str();
        let recovered = RelationshipType::parse(stored)
            .unwrap_or_else(|_| panic!("parse failed for as_str() output '{stored}'"));
        assert_eq!(
            variant, recovered,
            "round-trip failed for {variant:?}: as_str()='{stored}' parsed back to {recovered:?}"
        );
    }
}

/// `parse` reads the stored form, `as_str`'s, and nothing else: a type
/// spelled without its underscore is not one.
#[test]
fn relationship_type_parse_takes_only_the_stored_form() {
    for input in ["calledby", "inheritedby", "methodof", "relatedto", "call"] {
        assert!(
            RelationshipType::parse(input).is_err(),
            "parse('{input}') should fail"
        );
    }
    assert_eq!(
        RelationshipType::parse("called_by").unwrap(),
        RelationshipType::CalledBy
    );
}

#[test]
fn parse_unknown_returns_err() {
    assert!(RelationshipType::parse("not_a_thing").is_err());
}
