//! Call Graph Analysis for Devscriptor
//!
//! Provides comprehensive function call graph generation with:
//! - Call graph construction from relationships
//! - Cycle detection in call graphs
//! - Reachability analysis
//! - Entry point identification
//! - Depth analysis
//! - Export to multiple formats (JSON, DOT, Cytoscape)

use crate::analysis::AnalysisGuardConfig;
use crate::database::Database;
use crate::entities::{Class, EntityId, Function, Relationship, RelationshipType};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::{info, warn};

/// Represents a node in the call graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraphNode {
    /// Entity ID
    pub entity_id: EntityId,
    /// Entity name
    pub entity_name: String,
    /// Entity type (function, method, class, etc.)
    pub entity_type: String,
    /// File path
    pub file_path: String,
    /// Start line
    pub start_line: u32,
    /// End line
    pub end_line: u32,
    /// Language
    pub language: String,
    /// Modifiers (e.g., async, static)
    pub modifiers: Option<String>,
    /// Parent class name if a method
    pub parent_class: Option<String>,
    /// Parent function name if nested
    pub parent_function: Option<String>,
    /// Complexity score
    pub complexity_score: Option<f64>,
    /// Whether this is an external library call
    pub is_external: bool,
}

/// Represents an edge in the call graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraphEdge {
    /// Source entity ID
    pub source_id: EntityId,
    /// Target entity ID
    pub target_id: EntityId,
    /// Call type (direct, indirect, callback, polymorphic)
    pub call_type: String,
    /// Line number where call occurs
    pub line_number: Option<u32>,
    /// File path where call occurs
    pub file_path: Option<String>,
    /// Context (code snippet)
    pub context: Option<String>,
    /// Confidence score (0.0-1.0)
    pub confidence_score: f64,
    /// How many times this call occurs
    pub frequency: u32,
}

/// Represents a complete call graph
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallGraph {
    /// Nodes in the graph
    pub nodes: HashMap<EntityId, CallGraphNode>,
    /// Edges in the graph (source, target) -> edge
    pub edges: HashMap<(EntityId, EntityId), CallGraphEdge>,
    /// Adjacency list for O(1) outgoing edge lookups.
    /// Maps source entity ID -> list of edge keys.
    outgoing_edges: HashMap<EntityId, Vec<(EntityId, EntityId)>>,
    /// Adjacency list for O(1) incoming edge lookups.
    /// Maps target entity ID -> list of edge keys.
    incoming_edges: HashMap<EntityId, Vec<(EntityId, EntityId)>>,
    /// Nodes with no incoming edges (entry points)
    pub root_nodes: HashSet<EntityId>,
    /// Nodes with no outgoing edges (leaf functions)
    pub leaf_nodes: HashSet<EntityId>,
    /// Circular dependencies found
    pub cycles: Vec<Vec<EntityId>>,
    /// Maximum depth of the call graph
    pub max_depth: u32,
    /// Total number of nodes
    pub total_nodes: usize,
    /// Total number of edges
    pub total_edges: usize,
}

impl CallGraph {
    /// Create a new empty call graph
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node to the call graph
    pub fn add_node(&mut self, node: CallGraphNode) {
        self.nodes.insert(node.entity_id, node);
    }

    /// Add an edge to the call graph
    pub fn add_edge(&mut self, edge: CallGraphEdge) {
        let key = (edge.source_id, edge.target_id);
        if let Some(existing) = self.edges.get_mut(&key) {
            existing.frequency += edge.frequency;
            existing.confidence_score = (existing.confidence_score + edge.confidence_score) / 2.0;
        } else {
            self.outgoing_edges
                .entry(edge.source_id)
                .or_default()
                .push(key);
            self.incoming_edges
                .entry(edge.target_id)
                .or_default()
                .push(key);
            self.edges.insert(key, edge);
        }
    }

    /// Get node by entity ID
    #[must_use]
    pub fn get_node(&self, entity_id: EntityId) -> Option<&CallGraphNode> {
        self.nodes.get(&entity_id)
    }

    /// Get edges from a source node (O(1) average via adjacency list)
    #[must_use]
    pub fn get_outgoing_edges(&self, entity_id: EntityId) -> Vec<&CallGraphEdge> {
        self.outgoing_edges
            .get(&entity_id)
            .map(|keys| keys.iter().filter_map(|k| self.edges.get(k)).collect())
            .unwrap_or_default()
    }

    /// Get edges to a target node (O(1) average via adjacency list)
    #[must_use]
    pub fn get_incoming_edges(&self, entity_id: EntityId) -> Vec<&CallGraphEdge> {
        self.incoming_edges
            .get(&entity_id)
            .map(|keys| keys.iter().filter_map(|k| self.edges.get(k)).collect())
            .unwrap_or_default()
    }

    /// Find all callers of a function
    #[must_use]
    pub fn find_callers(&self, entity_id: EntityId) -> Vec<&CallGraphNode> {
        self.get_incoming_edges(entity_id)
            .iter()
            .filter_map(|e| self.nodes.get(&e.source_id))
            .collect()
    }

    /// Find all callees of a function
    #[must_use]
    pub fn find_callees(&self, entity_id: EntityId) -> Vec<&CallGraphNode> {
        self.get_outgoing_edges(entity_id)
            .iter()
            .filter_map(|e| self.nodes.get(&e.target_id))
            .collect()
    }

    /// Check if a node is reachable from another node
    #[must_use]
    pub fn is_reachable(&self, from: EntityId, to: EntityId) -> bool {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(from);

        while let Some(current) = queue.pop_front() {
            if current == to {
                return true;
            }

            if !visited.insert(current) {
                continue;
            }

            for edge in self.get_outgoing_edges(current) {
                queue.push_back(edge.target_id);
            }
        }

        false
    }

    /// Find all nodes reachable from a given node
    #[must_use]
    pub fn find_reachable(&self, start: EntityId) -> HashSet<EntityId> {
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);

        while let Some(current) = queue.pop_front() {
            if !reachable.insert(current) {
                continue;
            }

            for edge in self.get_outgoing_edges(current) {
                queue.push_back(edge.target_id);
            }
        }

        reachable
    }
}

/// Information about a function in the call graph report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionInfo {
    /// Function name
    pub name: String,
    /// File path
    pub file_path: String,
    /// Number of times called
    pub call_count: u32,
    /// Line number
    pub start_line: u32,
    /// Complexity score
    pub complexity_score: Option<f64>,
    /// Parent class
    pub parent_class: Option<String>,
}

/// Circular dependency information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircularDependency {
    /// Cycle ID
    pub cycle_id: usize,
    /// Function names in the cycle
    pub functions: Vec<String>,
    /// Length of the cycle
    pub cycle_length: usize,
    /// The file its first function is in
    pub file_path: String,
    /// The line its first function starts on
    pub line: u32,
    /// The files its functions are in, each once
    pub files: Vec<String>,
}

/// Summary statistics for call graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraphSummary {
    /// Total number of functions
    pub total_functions: usize,
    /// Total number of classes
    pub total_classes: usize,
    /// Total number of calls
    pub total_calls: usize,
    /// Number of root functions
    pub root_functions: usize,
    /// Number of leaf functions
    pub leaf_functions: usize,
    /// Number of circular dependencies
    pub circular_dependencies: usize,
    /// Maximum call depth
    pub max_call_depth: u32,
    /// Average calls per function
    pub average_calls_per_function: f64,
    /// Graph density
    pub graph_density: f64,
}

/// Comprehensive call graph analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraphReport {
    /// Summary statistics
    pub summary: CallGraphSummary,
    /// The call graph
    pub call_graph: CallGraph,
    /// Top-level functions (called but don't call others)
    pub top_level_functions: Vec<FunctionInfo>,
    /// Heavily called functions
    pub heavily_called_functions: Vec<FunctionInfo>,
    /// Isolated functions (no calls in or out)
    pub isolated_functions: Vec<FunctionInfo>,
    /// Circular dependencies
    pub circular_dependencies: Vec<CircularDependency>,
    /// Depth analysis (depth -> list of node IDs)
    pub depth_analysis: HashMap<u32, Vec<EntityId>>,
    /// Recommendations
    pub recommendations: Vec<String>,
}

/// Call graph generator
pub struct CallGraphGenerator {
    /// Mapping from entity identifiers to node IDs
    entity_to_node_id: HashMap<String, EntityId>,
    /// Input-size guard configuration
    guard_config: AnalysisGuardConfig,
}

impl Default for CallGraphGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl CallGraphGenerator {
    /// Create a new call graph generator
    #[must_use]
    pub fn new() -> Self {
        Self {
            entity_to_node_id: HashMap::new(),
            guard_config: AnalysisGuardConfig::default(),
        }
    }

    /// Set the guard configuration for this generator.
    #[must_use]
    pub fn with_guard_config(mut self, config: AnalysisGuardConfig) -> Self {
        self.guard_config = config;
        self
    }

    /// Generate a call graph from entities and relationships
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub fn generate_call_graph(
        &mut self,
        functions: &[Function],
        classes: &[Class],
        relationships: &[Relationship],
    ) -> Result<CallGraph> {
        let total_entities = functions.len() + classes.len();
        if total_entities > self.guard_config.max_entities_for_call_graph {
            return Err(Error::resource_exhausted(
                "call graph entities",
                self.guard_config.max_entities_for_call_graph as u64,
                total_entities as u64,
            ));
        }

        info!(
            "Generating call graph for {} functions and {} classes",
            functions.len(),
            classes.len()
        );

        let mut call_graph = CallGraph::new();

        // Add all entities as nodes
        self.add_entities_as_nodes(&mut call_graph, functions, classes);

        // Add relationships as edges
        self.add_relationships_as_edges(&mut call_graph, relationships);

        // Calculate graph metrics
        self.calculate_graph_metrics(&mut call_graph);

        // Find circular dependencies
        self.find_circular_dependencies(&mut call_graph);

        // Update summary statistics
        call_graph.total_nodes = call_graph.nodes.len();
        call_graph.total_edges = call_graph.edges.len();

        info!(
            "Call graph generated with {} nodes and {} edges",
            call_graph.total_nodes, call_graph.total_edges
        );

        Ok(call_graph)
    }

    /// Generate call graph from database
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn generate_from_database(&mut self, db: &Database) -> Result<CallGraph> {
        // Fetch all functions
        let functions = db.get_all_functions().await?;

        // Fetch all classes
        let classes = db.get_all_classes().await?;

        // Fetch all relationships
        let relationships = db.get_all_relationships().await?;

        self.generate_call_graph(&functions, &classes, &relationships)
    }

    fn add_entities_as_nodes(
        &mut self,
        call_graph: &mut CallGraph,
        functions: &[Function],
        classes: &[Class],
    ) {
        // Add functions as nodes
        for func in functions {
            let node = CallGraphNode {
                entity_id: func.id,
                entity_name: func.name.clone(),
                entity_type: "function".to_string(),
                file_path: func.file_path.clone(),
                start_line: func.location.start_line,
                end_line: func.location.end_line,
                language: func.language.clone(),
                modifiers: func.modifiers.clone(),
                parent_class: func.parent_class.clone(),
                parent_function: func.parent_function.clone(),
                complexity_score: None,
                is_external: false,
            };
            call_graph.add_node(node);
            self.entity_to_node_id
                .insert(format!("{}::{}", func.file_path, func.name), func.id);
        }

        // Add classes as nodes
        for cls in classes {
            let node = CallGraphNode {
                entity_id: cls.id,
                entity_name: cls.name.clone(),
                entity_type: "class".to_string(),
                file_path: cls.file_path.clone(),
                start_line: cls.location.start_line,
                end_line: cls.location.end_line,
                language: cls.language.clone(),
                modifiers: cls.access_modifiers.clone(),
                parent_class: None,
                parent_function: None,
                complexity_score: None,
                is_external: false,
            };
            call_graph.add_node(node);
            self.entity_to_node_id
                .insert(format!("{}::{}", cls.file_path, cls.name), cls.id);
        }
    }

    fn add_relationships_as_edges(
        &self,
        call_graph: &mut CallGraph,
        relationships: &[Relationship],
    ) {
        for rel in relationships {
            // Process call relationships
            if matches!(rel.relationship_type, RelationshipType::Calls) {
                let source_id = rel.source_id;
                let target_id = rel.target_id;

                // Only add edge if both nodes exist
                if call_graph.nodes.contains_key(&source_id)
                    && call_graph.nodes.contains_key(&target_id)
                {
                    // Extract optional fields from metadata if available
                    let line_number = rel
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("line_number"))
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32);
                    let file_path = rel
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("file_path"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let confidence_score = rel
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("confidence_score"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.8);

                    let edge = CallGraphEdge {
                        source_id,
                        target_id,
                        call_type: "direct".to_string(),
                        line_number,
                        file_path,
                        context: None,
                        confidence_score,
                        frequency: 1,
                    };
                    call_graph.add_edge(edge);
                }
            }
        }
    }

    fn calculate_graph_metrics(&self, call_graph: &mut CallGraph) {
        // Find root nodes (no incoming edges)
        let all_target_ids: HashSet<EntityId> =
            call_graph.edges.values().map(|e| e.target_id).collect();
        let all_source_ids: HashSet<EntityId> =
            call_graph.edges.values().map(|e| e.source_id).collect();
        call_graph.root_nodes = all_source_ids
            .difference(&all_target_ids)
            .copied()
            .collect();

        // Find leaf nodes (no outgoing edges)
        call_graph.leaf_nodes = all_target_ids
            .difference(&all_source_ids)
            .copied()
            .collect();

        // Calculate max depth using BFS from root nodes
        call_graph.max_depth = self.calculate_max_depth(call_graph);
    }

    fn calculate_max_depth(&self, call_graph: &CallGraph) -> u32 {
        if call_graph.edges.is_empty() {
            return 0;
        }

        // Build adjacency list for outgoing edges
        let mut adjacency: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
        for edge in call_graph.edges.values() {
            adjacency
                .entry(edge.source_id)
                .or_default()
                .push(edge.target_id);
        }

        // BFS from all root nodes
        let mut max_depth = 0;
        for &root_id in &call_graph.root_nodes {
            let depth = self.bfs_depth(&adjacency, root_id);
            max_depth = max_depth.max(depth);
        }

        max_depth
    }

    fn bfs_depth(&self, adjacency: &HashMap<EntityId, Vec<EntityId>>, start_id: EntityId) -> u32 {
        let mut queue = VecDeque::new();
        queue.push_back((start_id, 0u32));
        let mut visited = HashSet::new();
        let mut max_depth = 0u32;

        while let Some((node_id, depth)) = queue.pop_front() {
            if !visited.insert(node_id) {
                continue;
            }

            max_depth = max_depth.max(depth);

            // Add neighbors with incremented depth
            if let Some(neighbors) = adjacency.get(&node_id) {
                for &neighbor_id in neighbors {
                    if !visited.contains(&neighbor_id) {
                        queue.push_back((neighbor_id, depth + 1));
                    }
                }
            }
        }

        max_depth
    }

    fn find_circular_dependencies(&self, call_graph: &mut CallGraph) {
        // Build adjacency list
        let mut adjacency: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
        for edge in call_graph.edges.values() {
            adjacency
                .entry(edge.source_id)
                .or_default()
                .push(edge.target_id);
        }

        // Find cycles with an iterative DFS. `visited` persists across start
        // nodes; the per-start traversal tracks its own path.
        let mut visited = HashSet::new();
        let mut cycles = Vec::new();

        for &node_id in call_graph.nodes.keys() {
            if !visited.contains(&node_id) {
                Self::collect_cycles_from(node_id, &adjacency, &mut visited, &mut cycles);
            }
        }

        call_graph.cycles = cycles;
    }

    /// Iterative DFS cycle detection from `start`.
    ///
    /// An explicit work stack rather than recursion, so a pathologically deep
    /// call chain (e.g. machine-generated code) cannot overflow the thread stack.
    /// `on_path` is the set of nodes on the current DFS path (the "recursion
    /// stack"); `path` is the ordered path that reconstructs a cycle when a
    /// back-edge is found.
    fn collect_cycles_from(
        start: EntityId,
        adjacency: &HashMap<EntityId, Vec<EntityId>>,
        visited: &mut HashSet<EntityId>,
        cycles: &mut Vec<Vec<EntityId>>,
    ) {
        // Each frame is `(node, index of the next neighbour to visit)`.
        let mut stack: Vec<(EntityId, usize)> = vec![(start, 0)];
        let mut on_path: HashSet<EntityId> = HashSet::new();
        let mut path: Vec<EntityId> = Vec::new();

        visited.insert(start);
        on_path.insert(start);
        path.push(start);

        while let Some(&(node, idx)) = stack.last() {
            match adjacency.get(&node).and_then(|ns| ns.get(idx).copied()) {
                Some(neighbor) => {
                    // Advance this frame to its next neighbour.
                    if let Some(frame) = stack.last_mut() {
                        frame.1 += 1;
                    }
                    if on_path.contains(&neighbor) {
                        // Back-edge → cycle from `neighbor`'s position to the end.
                        if let Some(cycle_start) = path.iter().position(|&id| id == neighbor) {
                            let cycle = path[cycle_start..].to_vec();
                            if !cycles.contains(&cycle) {
                                cycles.push(cycle);
                            }
                        }
                    } else if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        on_path.insert(neighbor);
                        path.push(neighbor);
                        stack.push((neighbor, 0));
                    }
                }
                None => {
                    // All neighbours processed → leave the node.
                    stack.pop();
                    on_path.remove(&node);
                    path.pop();
                }
            }
        }
    }

    /// Generate a comprehensive call graph report
    ///
    /// # Errors
    /// Returns an error if the call graph is invalid
    pub fn generate_report(&self, call_graph: &CallGraph) -> Result<CallGraphReport> {
        info!("Generating call graph report");

        // Summary statistics
        let summary = self.generate_summary(call_graph);

        // Top-level functions
        let top_level_functions = self.find_top_level_functions(call_graph);

        // Heavily called functions
        let heavily_called_functions = self.find_heavily_called_functions(call_graph);

        // Isolated functions
        let isolated_functions = self.find_isolated_functions(call_graph);

        // Circular dependencies
        let circular_dependencies = self.format_circular_dependencies(call_graph);

        // Depth analysis
        let depth_analysis = self.perform_depth_analysis(call_graph);

        // Recommendations — skip centrality on very large graphs
        let recommendations =
            if call_graph.nodes.len() <= self.guard_config.max_nodes_for_call_graph_centrality {
                self.generate_recommendations(
                    call_graph,
                    &top_level_functions,
                    &heavily_called_functions,
                )
            } else {
                warn!(
                    "Skipping centrality-based recommendations: {} nodes exceed limit of {}.",
                    call_graph.nodes.len(),
                    self.guard_config.max_nodes_for_call_graph_centrality
                );
                vec![format!(
                    "Graph has {} nodes which exceeds the centrality-analysis limit of {}. \
                 Recommendations based on betweenness centrality were skipped.",
                    call_graph.nodes.len(),
                    self.guard_config.max_nodes_for_call_graph_centrality
                )]
            };

        let report = CallGraphReport {
            summary,
            call_graph: call_graph.clone(),
            top_level_functions,
            heavily_called_functions,
            isolated_functions,
            circular_dependencies,
            depth_analysis,
            recommendations,
        };

        info!("Call graph report generated successfully");
        Ok(report)
    }

    fn generate_summary(&self, call_graph: &CallGraph) -> CallGraphSummary {
        let total_functions = call_graph
            .nodes
            .values()
            .filter(|n| n.entity_type == "function")
            .count();
        let total_classes = call_graph
            .nodes
            .values()
            .filter(|n| n.entity_type == "class")
            .count();
        let total_nodes = call_graph.nodes.len().max(1);

        CallGraphSummary {
            total_functions,
            total_classes,
            total_calls: call_graph.edges.len(),
            root_functions: call_graph.root_nodes.len(),
            leaf_functions: call_graph.leaf_nodes.len(),
            circular_dependencies: call_graph.cycles.len(),
            max_call_depth: call_graph.max_depth,
            average_calls_per_function: call_graph.edges.len() as f64 / total_nodes as f64,
            graph_density: if total_nodes > 1 {
                call_graph.edges.len() as f64 / (total_nodes as f64 * (total_nodes as f64 - 1.0))
            } else {
                0.0
            },
        }
    }

    fn find_top_level_functions(&self, call_graph: &CallGraph) -> Vec<FunctionInfo> {
        let mut top_level = Vec::new();

        // Functions that are targets but not sources
        let target_ids: HashSet<EntityId> =
            call_graph.edges.values().map(|e| e.target_id).collect();
        let source_ids: HashSet<EntityId> =
            call_graph.edges.values().map(|e| e.source_id).collect();

        for node_id in target_ids.difference(&source_ids) {
            if let Some(node) = call_graph.nodes.get(node_id)
                && node.entity_type == "function"
            {
                let call_count = call_graph
                    .edges
                    .values()
                    .filter(|e| e.target_id == *node_id)
                    .map(|e| e.frequency)
                    .sum();

                top_level.push(FunctionInfo {
                    name: node.entity_name.clone(),
                    file_path: node.file_path.clone(),
                    call_count,
                    start_line: node.start_line,
                    complexity_score: node.complexity_score,
                    parent_class: node.parent_class.clone(),
                });
            }
        }

        // Sort by call count
        top_level.sort_by_key(|b| std::cmp::Reverse(b.call_count));
        top_level.truncate(20); // Return top 20

        top_level
    }

    fn find_heavily_called_functions(&self, call_graph: &CallGraph) -> Vec<FunctionInfo> {
        let mut call_counts: HashMap<EntityId, u32> = HashMap::new();

        // Count calls to each function
        for edge in call_graph.edges.values() {
            *call_counts.entry(edge.target_id).or_insert(0) += edge.frequency;
        }

        // Find functions with high call counts
        let mut heavily_called: Vec<(EntityId, u32)> = call_counts.into_iter().collect();
        heavily_called.sort_by_key(|b| std::cmp::Reverse(b.1));
        heavily_called.truncate(20);

        heavily_called
            .into_iter()
            .filter_map(|(node_id, call_count)| {
                call_graph.nodes.get(&node_id).map(|node| FunctionInfo {
                    name: node.entity_name.clone(),
                    file_path: node.file_path.clone(),
                    call_count,
                    start_line: node.start_line,
                    complexity_score: node.complexity_score,
                    parent_class: node.parent_class.clone(),
                })
            })
            .collect()
    }

    fn find_isolated_functions(&self, call_graph: &CallGraph) -> Vec<FunctionInfo> {
        let all_node_ids: HashSet<EntityId> = call_graph.nodes.keys().copied().collect();
        let edge_node_ids: HashSet<EntityId> = call_graph
            .edges
            .values()
            .flat_map(|e| [e.source_id, e.target_id])
            .collect();
        let isolated_ids: Vec<EntityId> =
            all_node_ids.difference(&edge_node_ids).copied().collect();

        isolated_ids
            .into_iter()
            .filter_map(|node_id| {
                call_graph.nodes.get(&node_id).and_then(|node| {
                    if node.entity_type == "function" {
                        Some(FunctionInfo {
                            name: node.entity_name.clone(),
                            file_path: node.file_path.clone(),
                            call_count: 0,
                            start_line: node.start_line,
                            complexity_score: node.complexity_score,
                            parent_class: node.parent_class.clone(),
                        })
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    fn format_circular_dependencies(&self, call_graph: &CallGraph) -> Vec<CircularDependency> {
        let mut circular_deps = Vec::new();

        for (i, cycle) in call_graph.cycles.iter().take(10).enumerate() {
            let nodes: Vec<_> = cycle
                .iter()
                .filter_map(|node_id| call_graph.nodes.get(node_id))
                .collect();
            let mut files: Vec<String> = Vec::new();
            for node in &nodes {
                if !files.contains(&node.file_path) {
                    files.push(node.file_path.clone());
                }
            }
            circular_deps.push(CircularDependency {
                cycle_id: i + 1,
                functions: nodes
                    .iter()
                    .map(|node| format!("{} ({})", node.entity_name, node.file_path))
                    .collect(),
                cycle_length: cycle.len(),
                file_path: nodes
                    .first()
                    .map(|node| node.file_path.clone())
                    .unwrap_or_default(),
                line: nodes.first().map_or(0, |node| node.start_line),
                files,
            });
        }

        circular_deps
    }

    fn perform_depth_analysis(&self, call_graph: &CallGraph) -> HashMap<u32, Vec<EntityId>> {
        if call_graph.edges.is_empty() {
            let all_ids: Vec<EntityId> = call_graph.nodes.keys().copied().collect();
            let mut map = HashMap::new();
            map.insert(0, all_ids);
            return map;
        }

        // Build adjacency list for outgoing edges
        let mut adjacency: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
        for edge in call_graph.edges.values() {
            adjacency
                .entry(edge.source_id)
                .or_default()
                .push(edge.target_id);
        }

        // Calculate depth for each node using BFS from root nodes
        let mut node_depths: HashMap<EntityId, u32> = HashMap::new();

        for &root_id in &call_graph.root_nodes {
            self.bfs_node_depths(&adjacency, root_id, &mut node_depths);
        }

        // Group nodes by depth
        let mut depth_analysis: HashMap<u32, Vec<EntityId>> = HashMap::new();
        for (node_id, depth) in node_depths {
            depth_analysis.entry(depth).or_default().push(node_id);
        }

        depth_analysis
    }

    fn bfs_node_depths(
        &self,
        adjacency: &HashMap<EntityId, Vec<EntityId>>,
        start_id: EntityId,
        node_depths: &mut HashMap<EntityId, u32>,
    ) {
        let mut queue = VecDeque::new();
        queue.push_back((start_id, 0u32));
        let mut visited = HashSet::new();

        while let Some((node_id, depth)) = queue.pop_front() {
            if !visited.insert(node_id) {
                continue;
            }

            // Update depth if this is a deeper path to this node
            let current_depth = node_depths.entry(node_id).or_insert(0);
            if depth > *current_depth {
                *current_depth = depth;
            }

            // Add neighbors with incremented depth
            if let Some(neighbors) = adjacency.get(&node_id) {
                for &neighbor_id in neighbors {
                    if !visited.contains(&neighbor_id) {
                        queue.push_back((neighbor_id, depth + 1));
                    }
                }
            }
        }
    }

    fn generate_recommendations(
        &self,
        call_graph: &CallGraph,
        _top_level_functions: &[FunctionInfo],
        heavily_called_functions: &[FunctionInfo],
    ) -> Vec<String> {
        let mut recommendations = Vec::new();

        // Check for circular dependencies
        if !call_graph.cycles.is_empty() {
            recommendations.push(format!(
                "⚠️ Found {} circular dependencies. Consider refactoring to eliminate cycles.",
                call_graph.cycles.len()
            ));
        }

        // Check for deeply nested call graphs
        if call_graph.max_depth > 10 {
            recommendations.push(format!(
                "🔧 Call graph has maximum depth of {}. Consider flattening deeply nested calls.",
                call_graph.max_depth
            ));
        }

        // Check for heavily called functions
        let very_heavily_called: Vec<&FunctionInfo> = heavily_called_functions
            .iter()
            .filter(|f| f.call_count > 50)
            .collect();
        if !very_heavily_called.is_empty() {
            recommendations.push(format!(
                "🔧 {} functions are called more than 50 times. Consider caching or optimization.",
                very_heavily_called.len()
            ));
        }

        // Check for isolated functions
        if call_graph.total_nodes > 0 {
            let all_node_ids: std::collections::HashSet<_> =
                call_graph.nodes.keys().copied().collect();
            let edge_node_ids: std::collections::HashSet<_> = call_graph
                .edges
                .values()
                .flat_map(|e| [e.source_id, e.target_id])
                .collect();
            let isolated_count = all_node_ids
                .difference(&edge_node_ids)
                .filter(|id| {
                    call_graph
                        .nodes
                        .get(id)
                        .is_some_and(|n| n.entity_type == "function")
                })
                .count();
            let isolated_ratio = isolated_count as f64 / call_graph.total_nodes as f64;
            if isolated_ratio > 0.3 {
                recommendations.push(format!(
                    "⚠️ {} of {} functions are isolated (no calls). Consider reviewing for dead code.",
                    isolated_count, call_graph.total_nodes
                ));
            }
        }

        if recommendations.is_empty() {
            recommendations.push(
                "✅ Call graph analysis shows healthy function call patterns with good modularity."
                    .to_string(),
            );
        }

        recommendations
    }

    /// Visualize call graph as a string
    #[must_use]
    pub fn visualize_call_graph(&self, graph: &HashMap<String, Vec<String>>) -> String {
        let mut lines = vec!["Call Graph Visualization:".to_string(), "=".repeat(30)];

        for (caller, callees) in graph {
            let callee_str = if callees.is_empty() {
                "no calls".to_string()
            } else {
                callees.join(", ")
            };
            lines.push(format!("{} -> {}", caller, callee_str));
        }

        lines.join("\n")
    }

    /// Export call graph in various formats
    ///
    /// # Errors
    /// Returns an error if the format is unsupported
    pub fn export_call_graph(&self, call_graph: &CallGraph, format: &str) -> Result<String> {
        match format {
            "json" => Ok(self.export_json(call_graph)),
            "dot" => Ok(self.export_dot(call_graph)),
            "cytoscape" => Ok(self.export_cytoscape(call_graph)),
            _ => Err(crate::error::Error::Analysis(format!(
                "Unsupported export format: {format}"
            ))),
        }
    }

    fn export_json(&self, call_graph: &CallGraph) -> String {
        let nodes: Vec<_> = call_graph
            .nodes
            .values()
            .map(|node| {
                serde_json::json!({
                    "id": node.entity_id,
                    "name": node.entity_name,
                    "type": node.entity_type,
                    "file_path": node.file_path,
                    "start_line": node.start_line,
                    "end_line": node.end_line,
                    "language": node.language,
                    "parent_class": node.parent_class,
                    "parent_function": node.parent_function,
                })
            })
            .collect();

        let edges: Vec<_> = call_graph
            .edges
            .values()
            .map(|edge| {
                serde_json::json!({
                    "source": edge.source_id,
                    "target": edge.target_id,
                    "call_type": edge.call_type,
                    "line_number": edge.line_number,
                    "file_path": edge.file_path,
                    "confidence_score": edge.confidence_score,
                    "frequency": edge.frequency,
                })
            })
            .collect();

        let data = serde_json::json!({
            "nodes": nodes,
            "edges": edges,
            "metadata": {
                "total_nodes": call_graph.total_nodes,
                "total_edges": call_graph.total_edges,
                "root_nodes": call_graph.root_nodes.iter().collect::<Vec<_>>(),
                "leaf_nodes": call_graph.leaf_nodes.iter().collect::<Vec<_>>(),
                "max_depth": call_graph.max_depth,
            }
        });

        serde_json::to_string_pretty(&data).unwrap_or_default()
    }

    fn export_dot(&self, call_graph: &CallGraph) -> String {
        let mut lines = vec![
            "digraph CallGraph {".to_string(),
            "  rankdir=LR;".to_string(),
            "  node [shape=box];".to_string(),
        ];

        // Add nodes
        for node in call_graph.nodes.values() {
            let label = format!(
                "{}\\n{}",
                node.entity_name,
                std::path::Path::new(&node.file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&node.file_path)
            );
            lines.push(format!(r#"  {:?} [label="{}"];"#, node.entity_id, label));
        }

        // Add edges
        for edge in call_graph.edges.values() {
            lines.push(format!(
                r#"  {:?} -> {:?} [label="{}"];"#,
                edge.source_id, edge.target_id, edge.call_type
            ));
        }

        lines.push("}".to_string());
        lines.join("\n")
    }

    fn export_cytoscape(&self, call_graph: &CallGraph) -> String {
        let mut elements = Vec::new();

        // Add nodes
        for node in call_graph.nodes.values() {
            elements.push(serde_json::json!({
                "data": {
                    "id": node.entity_id.to_string(),
                    "name": node.entity_name,
                    "type": node.entity_type,
                    "file_path": node.file_path,
                    "start_line": node.start_line,
                }
            }));
        }

        // Add edges
        for edge in call_graph.edges.values() {
            elements.push(serde_json::json!({
                "data": {
                    "id": format!("{}-{}", edge.source_id, edge.target_id),
                    "source": edge.source_id.to_string(),
                    "target": edge.target_id.to_string(),
                    "call_type": edge.call_type,
                    "frequency": edge.frequency,
                }
            }));
        }

        serde_json::to_string_pretty(&elements).unwrap_or_default()
    }
}

/// Call graph analyzer for performing advanced analysis
pub struct CallGraphAnalyzer;

impl CallGraphAnalyzer {
    /// Find all entry points in the call graph
    #[must_use]
    pub fn find_entry_points(call_graph: &CallGraph) -> Vec<&CallGraphNode> {
        call_graph
            .root_nodes
            .iter()
            .filter_map(|id| call_graph.nodes.get(id))
            .collect()
    }

    /// Find all dead ends in the call graph (functions that call nothing)
    #[must_use]
    pub fn find_dead_ends(call_graph: &CallGraph) -> Vec<&CallGraphNode> {
        call_graph
            .leaf_nodes
            .iter()
            .filter_map(|id| call_graph.nodes.get(id))
            .collect()
    }

    /// Calculate the call graph diameter (longest shortest path)
    ///
    /// Skips computation and returns 0 for graphs larger than 5 000 nodes
    /// to avoid unbounded O(n²) runtime.
    #[must_use]
    pub fn calculate_diameter(call_graph: &CallGraph) -> u32 {
        const MAX_NODES_FOR_DIAMETER: usize = 5_000;

        if call_graph.nodes.is_empty() {
            return 0;
        }
        if call_graph.nodes.len() > MAX_NODES_FOR_DIAMETER {
            warn!(
                "Skipping diameter calculation: {} nodes exceed limit of {}.",
                call_graph.nodes.len(),
                MAX_NODES_FOR_DIAMETER
            );
            return 0;
        }

        let mut max_distance = 0u32;

        for &start_id in call_graph.nodes.keys() {
            let distances = Self::calculate_distances(call_graph, start_id);
            for &dist in distances.values() {
                max_distance = max_distance.max(dist);
            }
        }

        max_distance
    }

    /// Calculate distances from a starting node to all other nodes
    fn calculate_distances(call_graph: &CallGraph, start_id: EntityId) -> HashMap<EntityId, u32> {
        let mut distances = HashMap::new();
        let mut queue = VecDeque::new();
        queue.push_back((start_id, 0u32));
        distances.insert(start_id, 0u32);

        while let Some((current, distance)) = queue.pop_front() {
            for edge in call_graph.get_outgoing_edges(current) {
                let neighbor_id = edge.target_id;
                if let std::collections::hash_map::Entry::Vacant(e) = distances.entry(neighbor_id) {
                    e.insert(distance + 1);
                    queue.push_back((neighbor_id, distance + 1));
                }
            }
        }

        distances
    }

    /// Find the most central functions using Brandes' algorithm for betweenness
    /// centrality on unweighted directed graphs.
    ///
    /// Runs in **O(n·m)** time and **O(n + m)** space per source node,
    /// replacing the previous O(n²) all-pairs explicit shortest-path enumeration.
    ///
    /// Returns an empty vector for graphs larger than 5 000 nodes to avoid
    /// unbounded runtime.
    #[must_use]
    pub fn find_most_central(call_graph: &CallGraph, top_n: usize) -> Vec<&CallGraphNode> {
        const MAX_NODES_FOR_CENTRALITY: usize = 5_000;

        if call_graph.nodes.is_empty() {
            return Vec::new();
        }
        if call_graph.nodes.len() > MAX_NODES_FOR_CENTRALITY {
            warn!(
                "Skipping betweenness-centrality: {} nodes exceed limit of {}.",
                call_graph.nodes.len(),
                MAX_NODES_FOR_CENTRALITY
            );
            return Vec::new();
        }

        let mut centrality: HashMap<EntityId, f64> =
            call_graph.nodes.keys().map(|&id| (id, 0.0)).collect();

        for &s in call_graph.nodes.keys() {
            // 1. BFS from s
            let mut distance: HashMap<EntityId, i32> = HashMap::new();
            let mut sigma: HashMap<EntityId, f64> = HashMap::new();
            let mut predecessors: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
            let mut queue = VecDeque::new();

            distance.insert(s, 0);
            sigma.insert(s, 1.0);
            queue.push_back(s);

            while let Some(v) = queue.pop_front() {
                let d_v = distance[&v];
                for edge in call_graph.get_outgoing_edges(v) {
                    let w = edge.target_id;
                    if let std::collections::hash_map::Entry::Vacant(e) = distance.entry(w) {
                        e.insert(d_v + 1);
                        queue.push_back(w);
                    }
                    if distance[&w] == d_v + 1 {
                        *sigma.entry(w).or_insert(0.0) += sigma.get(&v).copied().unwrap_or(0.0);
                        predecessors.entry(w).or_default().push(v);
                    }
                }
            }

            // 2. Dependency accumulation (back-propagation in order of non-increasing distance)
            let mut delta: HashMap<EntityId, f64> = HashMap::new();
            let mut stack: Vec<EntityId> = distance.keys().copied().collect();
            stack.sort_by_key(|id| std::cmp::Reverse(distance[id]));

            for &w in &stack {
                if let Some(preds) = predecessors.get(&w) {
                    for &v in preds {
                        let c =
                            (sigma[&v] / sigma[&w]) * (1.0 + delta.get(&w).copied().unwrap_or(0.0));
                        *delta.entry(v).or_insert(0.0) += c;
                    }
                }
                if w != s {
                    *centrality.get_mut(&w).unwrap() += delta.get(&w).copied().unwrap_or(0.0);
                }
            }
        }

        // Sort by centrality score descending
        let mut scored: Vec<(EntityId, f64)> = centrality.into_iter().collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_n);

        scored
            .into_iter()
            .filter_map(|(id, _)| call_graph.nodes.get(&id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::SourceLocation;
    use time::OffsetDateTime;

    fn create_test_function(id: EntityId, name: &str, file_path: &str) -> Function {
        Function {
            id,
            name: name.to_string(),
            file_path: file_path.to_string(),
            body: String::new(),
            location: SourceLocation::new(1, 0, 10, 0),
            language: "rust".to_string(),
            parent_function: None,
            parent_class: None,
            parameters: None,
            return_type: None,
            modifiers: None,
            access_modifiers: None,
            docstring: None,
            decorators: vec![],
            implements: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    fn create_test_relationship(source: EntityId, target: EntityId) -> Relationship {
        Relationship {
            id: EntityId::new_v4(),
            source_id: source,
            target_id: target,
            relationship_type: RelationshipType::Calls,
            metadata: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn test_call_graph_construction() {
        let func1 = create_test_function(EntityId::new_v4(), "func1", "test.rs");
        let func2 = create_test_function(EntityId::new_v4(), "func2", "test.rs");
        let func3 = create_test_function(EntityId::new_v4(), "func3", "test.rs");

        let rel1 = create_test_relationship(func1.id, func2.id);
        let rel2 = create_test_relationship(func2.id, func3.id);

        let mut generator = CallGraphGenerator::new();
        let graph = generator
            .generate_call_graph(&[func1, func2, func3], &[], &[rel1, rel2])
            .unwrap();

        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn test_cycle_detection() {
        let func1 = create_test_function(EntityId::new_v4(), "func1", "test.rs");
        let func2 = create_test_function(EntityId::new_v4(), "func2", "test.rs");
        let func3 = create_test_function(EntityId::new_v4(), "func3", "test.rs");

        // Create cycle: func1 -> func2 -> func3 -> func1
        let rel1 = create_test_relationship(func1.id, func2.id);
        let rel2 = create_test_relationship(func2.id, func3.id);
        let rel3 = create_test_relationship(func3.id, func1.id);

        let mut generator = CallGraphGenerator::new();
        let graph = generator
            .generate_call_graph(&[func1, func2, func3], &[], &[rel1, rel2, rel3])
            .unwrap();

        assert!(!graph.cycles.is_empty());
    }

    #[test]
    fn test_reachability() {
        let func1 = create_test_function(EntityId::new_v4(), "func1", "test.rs");
        let func2 = create_test_function(EntityId::new_v4(), "func2", "test.rs");
        let func3 = create_test_function(EntityId::new_v4(), "func3", "test.rs");

        // Capture IDs before moving the functions
        let func1_id = func1.id;
        let func3_id = func3.id;

        let rel1 = create_test_relationship(func1.id, func2.id);
        let rel2 = create_test_relationship(func2.id, func3.id);

        let mut generator = CallGraphGenerator::new();
        let graph = generator
            .generate_call_graph(&[func1, func2, func3], &[], &[rel1, rel2])
            .unwrap();

        assert!(graph.is_reachable(func1_id, func3_id));
        assert!(!graph.is_reachable(func3_id, func1_id));
    }

    #[test]
    fn test_export_json() {
        let func1 = create_test_function(EntityId::new_v4(), "func1", "test.rs");
        let func2 = create_test_function(EntityId::new_v4(), "func2", "test.rs");

        let rel1 = create_test_relationship(func1.id, func2.id);

        let mut generator = CallGraphGenerator::new();
        let graph = generator
            .generate_call_graph(&[func1, func2], &[], &[rel1])
            .unwrap();

        let json = generator.export_call_graph(&graph, "json").unwrap();
        assert!(json.contains("nodes"));
        assert!(json.contains("edges"));
        assert!(json.contains("metadata"));
    }
}
