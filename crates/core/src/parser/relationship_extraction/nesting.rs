//! Nesting and reverse edges: the HasMethod / MethodOf pair of every entity
//! inside another, and the reverse of each other relationship.

use super::*;

/// Extract hierarchical relationships (HasMethod / MethodOf) between entities
/// that are nested within each other (e.g., methods inside a class).
pub(super) fn extract_hierarchical_relationships(
    entities: &[Entity],
    line_index: &Extents,
    id_map: &std::collections::HashMap<Uuid, &str>,
) -> Vec<Relationship> {
    let mut relationships = Vec::new();
    let now = OffsetDateTime::now_utc();

    // For each entity, check if it is contained within a larger entity
    for entity in entities {
        let loc = entity.location();
        let entity_id = entity.id();

        // Skip if entity spans the whole file or is invalid
        if loc.start_line == 0 || loc.end_line < loc.start_line {
            continue;
        }

        // The innermost entity whose extent holds this one's and is larger: two
        // entities on the same lines would each hold the other by lines alone, so
        // a field and its one-line type would each be the other's member.
        let (start, end) = (
            (loc.start_line, loc.start_column),
            (loc.end_line, loc.end_column),
        );
        let parent_id = line_index.innermost(start, end, |(c_start, c_end, _)| {
            *c_start <= start && end <= *c_end && (*c_start, *c_end) != (start, end)
        });

        if let Some(parent) = parent_id {
            let parent_name = id_map.get(&parent).copied().unwrap_or("");
            // Create HasMethod: parent -> child
            relationships.push(Relationship {
                id: Uuid::new_v4(),
                source_id: parent,
                target_id: entity_id,
                relationship_type: RelationshipType::HasMethod,
                metadata: Some(serde_json::json!({
                    "container_name": parent_name,
                    "contained_name": entity.name(),
                    "contained_type": entity.entity_type(),
                })),
                created_at: now,
            });

            // Create MethodOf: child -> parent
            relationships.push(Relationship {
                id: Uuid::new_v4(),
                source_id: entity_id,
                target_id: parent,
                relationship_type: RelationshipType::MethodOf,
                metadata: Some(serde_json::json!({
                    "member_name": entity.name(),
                    "parent_name": parent_name,
                })),
                created_at: now,
            });
        }
    }

    relationships
}

// =============================================================================
// REVERSE RELATIONSHIPS
// =============================================================================

/// Generate reverse relationships for every forward relationship.
///
/// Forward -> Reverse mapping:
///   Calls       -> CalledBy
///   Inherits    -> InheritedBy
///   Implements  -> ImplementedBy
///   Uses        -> UsedBy
///   References  -> ReferencedBy
///   Overrides   -> OverriddenBy
///
/// NOTE: HasMethod / MethodOf are intentionally skipped here because
/// `extract_hierarchical_relationships()` already creates them as a
/// bidirectional pair, so reversing them again would produce duplicates.
pub(crate) fn generate_reverse_relationships(relationships: &[Relationship]) -> Vec<Relationship> {
    let mut reverse = Vec::with_capacity(relationships.len());
    let now = OffsetDateTime::now_utc();

    for rel in relationships {
        if let Some(reverse_type) = reverse_of(&rel.relationship_type) {
            reverse.push(Relationship {
                id: Uuid::new_v4(),
                source_id: rel.target_id,
                target_id: rel.source_id,
                relationship_type: reverse_type,
                metadata: rel.metadata.clone(),
                created_at: now,
            });
        }
    }

    reverse
}

/// Map a forward relationship type to its reverse.
///
/// HasMethod and MethodOf are intentionally excluded from this mapping
/// because they are already created as a bidirectional pair by
/// `extract_hierarchical_relationships()`, so reversing them again
/// would produce duplicate relationships.
pub(crate) fn reverse_of(rel_type: &RelationshipType) -> Option<RelationshipType> {
    match rel_type {
        RelationshipType::Calls => Some(RelationshipType::CalledBy),
        RelationshipType::Inherits => Some(RelationshipType::InheritedBy),
        RelationshipType::Implements => Some(RelationshipType::ImplementedBy),
        RelationshipType::Uses => Some(RelationshipType::UsedBy),
        RelationshipType::References => Some(RelationshipType::ReferencedBy),
        RelationshipType::Overrides => Some(RelationshipType::OverriddenBy),
        // HasMethod / MethodOf are already bidirectional — don't reverse again
        RelationshipType::HasMethod | RelationshipType::MethodOf => None,
        // Reverse types themselves don't get another reverse
        RelationshipType::CalledBy
        | RelationshipType::InheritedBy
        | RelationshipType::ImplementedBy
        | RelationshipType::UsedBy
        | RelationshipType::ReferencedBy
        | RelationshipType::OverriddenBy
        | RelationshipType::RelatedTo => None,
    }
}
