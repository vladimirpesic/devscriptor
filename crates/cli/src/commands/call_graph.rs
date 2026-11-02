//! Call graph command implementation

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use devscriptor_core::database::{Database, EntityHeader};
use devscriptor_core::entities::EntityId;

use crate::commands::common::CallGraphFormat;

/// Maximum call-graph depth allowed from CLI input.
const MAX_CALL_GRAPH_DEPTH: usize = 20;

/// Generate call graph
pub async fn execute(
    entity: String,
    database: Arc<Database>,
    format: CallGraphFormat,
    max_depth: usize,
    direction: String,
) -> Result<()> {
    // Cap the depth a user asks for, so the walk stays bounded.
    let max_depth = max_depth.min(MAX_CALL_GRAPH_DEPTH);
    // Search for the entity by name
    let entities = database.search_entities(&entity, 10).await?;
    let target = entities
        .first()
        .ok_or_else(|| anyhow::anyhow!("Entity '{entity}' not found in database"))?;

    let entity_id = target.id();
    let entity_name = target.name().to_string();
    let entity_type = target.entity_type();

    // BFS traversal with batched relationship queries: the relationships of the
    // whole frontier are fetched in one query per depth level, not one query per
    // node.
    let mut visited: HashSet<EntityId> = HashSet::new();
    let mut result = Vec::new();
    // A relationship between two nodes of the graph is met from both ends:
    // it is listed once.
    let mut listed: HashSet<(EntityId, EntityId, String)> = HashSet::new();
    let mut frontier: Vec<(EntityId, usize)> = vec![(entity_id, 0)];
    visited.insert(entity_id);

    while !frontier.is_empty() {
        let mut next_frontier = Vec::new();

        // Collect all frontier node ids for this depth
        let ids: Vec<EntityId> = frontier.iter().map(|(id, _)| *id).collect();

        // Single batched query for the entire frontier
        let rels = database.get_relationships_for_ids(&ids).await?;

        for (current_id, depth) in frontier {
            if depth >= max_depth {
                continue;
            }

            for rel in &rels {
                // Only consider relationships where this node is one endpoint
                let is_involved = rel.source_id == current_id || rel.target_id == current_id;
                if !is_involved {
                    continue;
                }

                let include = match direction.as_str() {
                    "incoming" => rel.target_id == current_id,
                    "outgoing" => rel.source_id == current_id,
                    _ => true,
                };

                if include {
                    let next_id = if rel.source_id == current_id {
                        rel.target_id
                    } else {
                        rel.source_id
                    };

                    if listed.insert((
                        rel.source_id,
                        rel.target_id,
                        rel.relationship_type.as_str().to_string(),
                    )) {
                        result.push(rel.clone());
                    }

                    if visited.insert(next_id) {
                        next_frontier.push((next_id, depth + 1));
                    }
                }
            }
        }

        frontier = next_frontier;
    }

    let filtered = result;

    // Name both ends of every relationship: ids alone cannot be read.
    let mut ids: Vec<EntityId> = filtered
        .iter()
        .flat_map(|r| [r.source_id, r.target_id])
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let headers: HashMap<EntityId, EntityHeader> = database
        .get_entity_names_by_ids(&ids)
        .await?
        .into_iter()
        .map(|header| (header.id, header))
        .collect();
    let name = |id: EntityId| -> (String, String) {
        if id == entity_id {
            return (entity_name.clone(), entity_type.to_string());
        }
        headers.get(&id).map_or_else(
            || (id.to_string(), String::new()),
            |header| (header.name.clone(), header.entity_type.clone()),
        )
    };
    let label = |id: EntityId| -> String {
        let (name, kind) = name(id);
        if kind.is_empty() {
            name
        } else {
            format!("{name} ({kind})")
        }
    };

    let output = match format {
        CallGraphFormat::Text => {
            let mut lines = vec![
                format!("Call Graph for: {} ({})", entity_name, entity_type),
                format!("Max depth: {}", max_depth),
                format!("Direction: {}", direction),
                String::new(),
                format!("Relationships found: {}", filtered.len()),
            ];
            for rel in &filtered {
                lines.push(format!(
                    "  {} --{}--> {}",
                    label(rel.source_id),
                    rel.relationship_type.as_str(),
                    label(rel.target_id)
                ));
            }
            lines.join("\n")
        }
        CallGraphFormat::Json => serde_json::json!({
            "entity": entity_name,
            "entity_type": entity_type,
            "entity_id": entity_id.to_string(),
            "max_depth": max_depth,
            "direction": direction,
            "relationships": filtered.iter().map(|r| {
                let (source_name, source_type) = name(r.source_id);
                let (target_name, target_type) = name(r.target_id);
                serde_json::json!({
                    "source": r.source_id.to_string(),
                    "source_name": source_name,
                    "source_type": source_type,
                    "target": r.target_id.to_string(),
                    "target_name": target_name,
                    "target_type": target_type,
                    "type": r.relationship_type.as_str()
                })
            }).collect::<Vec<_>>()
        })
        .to_string(),
        CallGraphFormat::Dot => {
            // Nodes are named by id, so that two entities of one name stay two,
            // and labelled with the name.
            let mut lines = vec!["digraph callgraph {".to_string()];
            let mut nodes: Vec<EntityId> = std::iter::once(entity_id)
                .chain(filtered.iter().flat_map(|r| [r.source_id, r.target_id]))
                .collect();
            nodes.dedup();
            let mut declared = std::collections::HashSet::new();
            for id in nodes {
                if declared.insert(id) {
                    lines.push(format!(
                        "    \"{id}\" [label=\"{}\"];",
                        dot_escape(&label(id))
                    ));
                }
            }
            for rel in &filtered {
                lines.push(format!(
                    "    \"{}\" -> \"{}\" [label=\"{}\"];",
                    rel.source_id,
                    rel.target_id,
                    rel.relationship_type.as_str()
                ));
            }
            lines.push("}".to_string());
            lines.join("\n")
        }
        CallGraphFormat::Mermaid => {
            let node =
                |id: EntityId| format!("E{}[\"{}\"]", id.simple(), mermaid_escape(&label(id)));
            let mut lines = vec![
                "```mermaid".to_string(),
                "graph TD".to_string(),
                format!("    {}", node(entity_id)),
            ];
            for rel in &filtered {
                lines.push(format!(
                    "    {} -->|{}| {}",
                    node(rel.source_id),
                    rel.relationship_type.as_str(),
                    node(rel.target_id)
                ));
            }
            lines.push("```".to_string());
            lines.join("\n")
        }
    };

    println!("{output}");

    Ok(())
}

/// `text` inside a double-quoted DOT string.
fn dot_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `text` inside a double-quoted Mermaid label.
fn mermaid_escape(text: &str) -> String {
    text.replace('"', "#quot;")
}
