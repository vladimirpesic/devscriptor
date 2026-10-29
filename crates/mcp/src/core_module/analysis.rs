//! Analysis tool implementations: dead code, complexity, entity details, relationships

use devscriptor_core::entities::RelationshipType;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::{CoreModule, ToolError, ToolResult, ok_json};
use crate::tools::{
    AnalyzeComplexityResponse, ComplexityResult, DeadCodeItem, FindDeadCodeResponse,
    GetRelationshipsResponse, RelationshipInfo,
};

/// One end of a relationship, as `get_relationships` reports it.
struct Endpoint {
    name: String,
    entity_type: String,
    /// The file it is defined in, as stored.
    file: String,
    /// The line it starts on, counted from 1.
    line: u32,
}

/// What `get_entity_details` says of `entity`: where it is, and the
/// structure the index holds of its kind — a function's signature, owner,
/// enclosing function, modifiers and decorators, a type's methods, fields and
/// supertypes — and where the `others` of its name are, which `file_path`
/// tells apart.
fn entity_details(
    entity: &devscriptor_core::entities::Entity,
    others: &[devscriptor_core::entities::Entity],
) -> Value {
    use devscriptor_core::entities::Entity;
    let location = entity.location();
    let mut details = json!({
        "entity_type": entity.entity_type(),
        "name": entity.name(),
        "file_path": entity.file_path(),
        "start_line": location.start_line,
        "end_line": location.end_line,
        "language": entity.language(),
    });
    let structure = match entity {
        Entity::Function(f) => json!({
            "docstring": f.docstring,
            "parameters": f.parameters,
            "return_type": f.return_type,
            "parent_class": f.parent_class,
            "parent_function": f.parent_function,
            "implements": f.implements,
            "modifiers": f.modifiers,
            "access_modifiers": f.access_modifiers,
            "decorators": f.decorators,
        }),
        Entity::Class(c) | Entity::Table(c) | Entity::Index(c) => json!({
            "docstring": c.docstring,
            "methods": c.methods.clone().unwrap_or_default(),
            "fields": c.fields,
            "extends": c.extends,
            "implements": c.implements,
            "access_modifiers": c.access_modifiers,
            "decorators": c.decorators,
        }),
        Entity::Struct(s) => json!({
            "docstring": s.docstring,
            "methods": s.methods.clone().unwrap_or_default(),
            "fields": s.fields,
        }),
        Entity::Interface(i) => json!({
            "docstring": i.docstring,
            "methods": i.methods.clone().unwrap_or_default(),
            "properties": i.properties,
            "extends": i.extends,
        }),
        Entity::Trait(t) => json!({
            "docstring": t.docstring,
            "methods": t.methods.clone().unwrap_or_default(),
        }),
        Entity::Enum(e) => json!({
            "docstring": e.docstring,
            "variants": e.variants,
            "methods": e.methods.clone().unwrap_or_default(),
            "implements": e.implements,
            "backed_type": e.backed_type,
        }),
        _ => json!({}),
    };
    if let (Some(details), Some(structure)) = (details.as_object_mut(), structure.as_object()) {
        details.extend(structure.clone());
        if !others.is_empty() {
            let elsewhere: Vec<String> = others
                .iter()
                .take(10)
                .map(|other| format!("{}:{}", other.file_path(), other.location().start_line))
                .collect();
            details.insert("other_matches".to_string(), json!(elsewhere));
            details.insert("total_matches".to_string(), json!(others.len() + 1));
        }
    }
    details
}

impl CoreModule {
    pub(super) async fn get_entity_details(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let entity_type = args
            .get("entity_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'entity_type' argument".into()))?;

        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'name' argument".into()))?;

        // Optional: disambiguate between same-named entities in different files.
        // Matched as a substring so a relative fragment matches an absolute path.
        let file_path = args.get("file_path").and_then(|v| v.as_str());

        info!(
            "Getting details for {}: {} (file: {:?})",
            entity_type, name, file_path
        );

        let searcher = self.state.searcher();
        let entities = match entity_type {
            "function" => searcher.search_functions(name, true, None, None).await,
            "class" => searcher.search_classes(name, true, None, None).await,
            _ => {
                let opts = devscriptor_core::search::SearchOptions {
                    entity_type: Some(entity_type.to_string()),
                    ..Default::default()
                };
                searcher
                    .search_entities(name, opts)
                    .await
                    .map(|results| results.into_iter().map(|result| result.entity).collect())
            }
        }
        .map_err(|e| {
            warn!("Entity lookup failed: {}", e);
            ToolError::from(e)
        })?;

        let entities: Vec<_> = match file_path {
            Some(needle) => entities
                .into_iter()
                .filter(|e| e.file_path().contains(needle))
                .collect(),
            None => entities,
        };

        let Some(entity) = entities.first() else {
            let error = match file_path {
                Some(f) => {
                    format!("No {entity_type} found with name '{name}' in a file matching '{f}'")
                }
                None => format!("No {entity_type} found with name: {name}"),
            };
            return ok_json(&json!({
                "error": error,
                "suggestion": "Try a broader search or check the name spelling",
            }));
        };

        ok_json(&entity_details(entity, &entities[1..]))
    }

    pub(super) async fn get_relationships(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let (response, _) = self.relationships(arguments).await?;
        ok_json(&response)
    }

    /// The answer of `get_relationships`, and the files it was read from:
    /// those of the entities asked about, then those of the edges' other ends.
    pub(super) async fn relationships(
        &self,
        arguments: Option<Value>,
    ) -> Result<(GetRelationshipsResponse, Vec<String>), ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let entity_name = args
            .get("entity_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'entity_name' argument".into()))?;

        // Every filter is optional. `entity_type` and `file_path` narrow which
        // same-named entities are considered (the file matched as a substring,
        // as `get_entity_details` matches it); `relationship_type` narrows the
        // edges returned. None has a default: applying one silently would hide
        // results for callers that did not ask for a filter.
        let entity_type = args.get("entity_type").and_then(|v| v.as_str());
        let file_path = args.get("file_path").and_then(|v| v.as_str());
        let relationship_type = args.get("relationship_type").and_then(|v| v.as_str());

        info!(
            "Getting relationships for: {} (entity_type: {:?}, file_path: {:?}, \
             relationship_type: {:?})",
            entity_name, entity_type, file_path, relationship_type
        );

        let db = self.state.database();

        // MCP-7 FIX: Use exact-name lookup first; fall back to fuzzy search only if needed.
        let matching_entities = match db.get_entities_by_exact_name(entity_name, 100).await {
            Ok(entities) if !entities.is_empty() => entities,
            _ => match db.search_entities(entity_name, 100).await {
                Ok(entities) => entities,
                Err(e) => {
                    warn!("Entity search for relationships failed: {}", e);
                    return Err(ToolError::from(e));
                }
            },
        };

        let matching_entities: Vec<_> = matching_entities
            .into_iter()
            .filter(|e| entity_type.is_none_or(|wanted| e.entity_type() == wanted))
            .filter(|e| file_path.is_none_or(|needle| e.file_path().contains(needle)))
            .collect();

        let mut entity_ids = Vec::new();
        let mut id_to_name: std::collections::HashMap<String, Endpoint> =
            std::collections::HashMap::new();
        let mut files = Vec::new();
        let mut seen_files = std::collections::HashSet::new();
        for e in &matching_entities {
            let id_str = e.id().to_string();
            entity_ids.push(e.id());
            id_to_name.insert(
                id_str.clone(),
                Endpoint {
                    name: e.name().to_string(),
                    entity_type: e.entity_type().to_string(),
                    file: e.file_path().to_string(),
                    line: e.location().start_line,
                },
            );
            if seen_files.insert(e.file_path().to_string()) {
                files.push(e.file_path().to_string());
            }
        }

        if entity_ids.is_empty() {
            let response = GetRelationshipsResponse {
                entity: entity_name.to_string(),
                entity_type: entity_type.unwrap_or("any").to_string(),
                total_relationships: 0,
                relationships: vec![],
            };
            return Ok((response, files));
        }

        // First pass: gather the raw, de-duplicated edges as id triples, each
        // with its ambiguity mark and the lines it is made on. Names are
        // resolved afterwards in one batched lookup so an edge's *target*
        // (which is almost never in this entity's own match set) comes back as
        // a name rather than a bare UUID. Unless a type is asked for, each fact
        // is given once: a reverse row (`called_by` of a `calls` edge) and
        // `method_of`, the twin of `has_method`, repeat an edge the answer
        // holds, as the edges of both ends are read.
        let mut raw_edges: Vec<(String, String, String, bool, Vec<u32>)> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for entity_id in entity_ids {
            match db.get_relationships(entity_id).await {
                Ok(rels) => {
                    for rel in rels {
                        let rel_type = rel.relationship_type.as_str().to_string();
                        match relationship_type {
                            Some(wanted) if !rel_type.eq_ignore_ascii_case(wanted) => continue,
                            None if rel.relationship_type.is_reverse()
                                || rel.relationship_type == RelationshipType::MethodOf =>
                            {
                                continue;
                            }
                            _ => {}
                        }
                        let source_id = rel.source_id.to_string();
                        let target_id = rel.target_id.to_string();
                        let key = format!("{source_id}->{target_id}:{rel_type}");
                        if !seen.insert(key) {
                            continue;
                        }
                        let lines = rel
                            .metadata
                            .as_ref()
                            .and_then(|metadata| metadata["lines"].as_array())
                            .map(|lines| {
                                lines
                                    .iter()
                                    .filter_map(Value::as_u64)
                                    .filter_map(|line| u32::try_from(line).ok())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let ambiguous = rel.is_ambiguous();
                        raw_edges.push((source_id, target_id, rel_type, ambiguous, lines));
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to get relationships for entity {}: {}",
                        entity_id, e
                    );
                    return Err(ToolError::from(e));
                }
            }
        }

        // Cap before resolving so we only name the edges we will actually return.
        raw_edges.truncate(100);

        // Resolve every endpoint id not already known (targets, cross-file
        // sources) in a single batched DB lookup.
        let unresolved: Vec<devscriptor_core::entities::EntityId> = raw_edges
            .iter()
            .flat_map(|(s, t, _, _, _)| [s, t])
            .filter(|id| !id_to_name.contains_key(*id))
            .filter_map(|id| uuid::Uuid::parse_str(id).ok())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        if !unresolved.is_empty() {
            match db.get_entity_names_by_ids(&unresolved).await {
                Ok(headers) => {
                    for header in headers {
                        if seen_files.insert(header.file_path.clone()) {
                            files.push(header.file_path.clone());
                        }
                        id_to_name.insert(
                            header.id.to_string(),
                            Endpoint {
                                name: header.name,
                                entity_type: header.entity_type,
                                file: header.file_path,
                                line: header.start_line,
                            },
                        );
                    }
                }
                Err(e) => warn!("Failed to resolve relationship endpoint names: {e}"),
            }
        }

        let filtered: Vec<RelationshipInfo> = raw_edges
            .into_iter()
            .map(|(source_id, target_id, rel_type, ambiguous, lines)| {
                let source = id_to_name.get(&source_id);
                let target = id_to_name.get(&target_id);
                RelationshipInfo {
                    source: source.map_or_else(|| source_id.clone(), |e| e.name.clone()),
                    target: target.map_or_else(|| target_id.clone(), |e| e.name.clone()),
                    rel_type,
                    source_type: Some(
                        source.map_or_else(|| "unknown".to_string(), |e| e.entity_type.clone()),
                    ),
                    target_type: Some(
                        target.map_or_else(|| "unknown".to_string(), |e| e.entity_type.clone()),
                    ),
                    source_file: source.map(|e| e.file.clone()),
                    source_line: source.map(|e| e.line),
                    target_file: target.map(|e| e.file.clone()),
                    target_line: target.map(|e| e.line),
                    lines,
                    ambiguous,
                }
            })
            .collect();

        let response = GetRelationshipsResponse {
            entity: entity_name.to_string(),
            entity_type: entity_type.unwrap_or("any").to_string(),
            total_relationships: filtered.len(),
            relationships: filtered,
        };

        Ok((response, files))
    }

    pub(super) async fn find_dead_code(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.unwrap_or_default();

        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(20) as usize;
        let path = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(".");
        // `.` is the whole index; another path keeps the entities under it.
        // Passed on as given, a relative path would be resolved against nothing it
        // is meant to be, and one that does not exist would find no dead code.
        let target = Self::scope_directory(path)?
            .map_or_else(|| ".".to_string(), |dir| dir.to_string_lossy().into_owned());
        // Findings below this confidence are dropped: an unreferenced private
        // entity scores 0.9, an exported one 0.5, one whose name is written
        // outside its definition 0.3, so the default of 0.5 keeps the first two.
        let min_confidence = args
            .get("min_confidence")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.5);

        info!(
            "Finding dead code in {} (min_confidence: {})",
            path, min_confidence
        );

        let results = self
            .state
            .analysis_api()
            .analyze_dead_code(target)
            .await
            .map_err(|e| {
                warn!("Dead code analysis failed: {}", e);
                ToolError::from(e)
            })?;
        let mut findings: Vec<(DeadCodeItem, bool)> = results
            .findings
            .iter()
            .filter_map(|f| {
                let meta = f.metadata.as_ref()?;
                let confidence = meta.get("confidence")?.as_f64()?;
                if confidence < min_confidence {
                    return None;
                }
                let item = DeadCodeItem {
                    name: meta.get("entity_name")?.as_str()?.to_string(),
                    file_path: f.file_path.clone()?,
                    line: f.line_number.unwrap_or(0),
                    confidence,
                    reason: meta.get("reason")?.as_str()?.to_string(),
                };
                Some((item, meta.get("entity_type")?.as_str()? == "class"))
            })
            .collect();
        // The surest first, so that `limit` keeps them; then by place.
        findings.sort_by(|(a, _), (b, _)| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then(a.line.cmp(&b.line))
        });

        let total_unused_classes = findings.iter().filter(|(_, is_class)| *is_class).count();
        let total_unused_functions = findings.len() - total_unused_classes;

        let (unused_classes, unused_functions): (Vec<_>, Vec<_>) = findings
            .into_iter()
            .take(limit)
            .partition(|(_, is_class)| *is_class);

        ok_json(&FindDeadCodeResponse {
            total_entities_analyzed: results.summary.entities_analyzed,
            unused_functions: unused_functions.into_iter().map(|(item, _)| item).collect(),
            unused_classes: unused_classes.into_iter().map(|(item, _)| item).collect(),
            total_unused_functions,
            total_unused_classes,
        })
    }

    pub(super) async fn analyze_complexity(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        use devscriptor_core::analysis::complexity::{ComplexityAnalyzer, ComplexityMetrics};

        let args = arguments.unwrap_or_default();

        let top_n = args
            .get("top_n")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;
        let path = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(".");
        // Optional: restrict results to a single file (substring match, so a
        // relative fragment matches the absolute stored path).
        let file_path = args.get("file_path").and_then(serde_json::Value::as_str);
        // `.` is the whole index; another path keeps the functions under it.
        let under = Self::scope_directory(path)?;

        info!("Analyzing complexity in {} (file: {:?})", path, file_path);

        let analyzer = ComplexityAnalyzer::new();
        let threshold = analyzer.thresholds().max_cyclomatic_complexity;
        let report = match analyzer.analyze(self.state.database()).await {
            Ok(report) => report,
            Err(e) => {
                warn!("Complexity analysis failed: {}", e);
                return Err(ToolError::from(e));
            }
        };

        // Every function in scope, those under the threshold included: the
        // most complex are listed whatever their score, and the counts are of
        // what was analysed, not of what was flagged.
        let mut functions: Vec<&ComplexityMetrics> = report
            .function_metrics
            .iter()
            .filter(|m| {
                under
                    .as_ref()
                    .is_none_or(|dir| m.file_path.starts_with(&*dir.to_string_lossy()))
            })
            .filter(|m| file_path.is_none_or(|needle| m.file_path.contains(needle)))
            .collect();
        let total = functions.len();
        let sum: u64 = functions
            .iter()
            .map(|m| u64::from(m.cyclomatic_complexity))
            .sum();
        let average = if total > 0 {
            sum as f64 / total as f64
        } else {
            0.0
        };
        let above = functions
            .iter()
            .filter(|m| m.cyclomatic_complexity > threshold)
            .count();

        functions.sort_by_key(|m| std::cmp::Reverse(m.cyclomatic_complexity));
        functions.truncate(top_n);
        let most_complex = functions
            .into_iter()
            .map(|m| ComplexityResult {
                name: m.entity_name.clone(),
                file_path: m.file_path.clone(),
                line: m.start_line,
                complexity_score: m.cyclomatic_complexity,
                risk_level: if m.cyclomatic_complexity > 2 * threshold {
                    "high"
                } else if m.cyclomatic_complexity > threshold {
                    "medium"
                } else {
                    "low"
                }
                .to_string(),
            })
            .collect();

        ok_json(&AnalyzeComplexityResponse {
            total_functions_analyzed: total,
            average_complexity: average,
            threshold,
            functions_above_threshold: above,
            most_complex_functions: most_complex,
        })
    }
}
