//! Query operations over a persisted codegraph snapshot.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::OnceLock,
};

use leiden_rs::{Leiden, LeidenConfig, QualityType, from_petgraph};
use petgraph::{
    Direction, Graph, Undirected,
    algo::astar,
    graph::{DiGraph, NodeIndex},
    visit::EdgeRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CodeGraphArtifact, CodeGraphEdge, CodeGraphError, CodeGraphNode, CodeGraphResult};

/// Versioned relation vocabulary shared by structural and semantic codegraph
/// producers. Unknown serialized relation strings remain readable through the
/// existing `CodeGraphEdge::kind` field; this enum is used by relation-aware
/// query callers that want an explicit allow-list.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CodeGraphRelationKind {
    Defines,
    Imports,
    Calls,
    Inherits,
    Implements,
    Overrides,
    MixesIn,
    References,
    Tests,
    DependsOn,
}

impl CodeGraphRelationKind {
    pub const VERSION: u32 = 1;

    pub const ALL: [Self; 10] = [
        Self::Defines,
        Self::Imports,
        Self::Calls,
        Self::Inherits,
        Self::Implements,
        Self::Overrides,
        Self::MixesIn,
        Self::References,
        Self::Tests,
        Self::DependsOn,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Defines => "defines",
            Self::Imports => "imports",
            Self::Calls => "calls",
            Self::Inherits => "inherits",
            Self::Implements => "implements",
            Self::Overrides => "overrides",
            Self::MixesIn => "mixes_in",
            Self::References => "references",
            Self::Tests => "tests",
            Self::DependsOn => "depends_on",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "defines" => Some(Self::Defines),
            "imports" => Some(Self::Imports),
            "calls" => Some(Self::Calls),
            "inherits" => Some(Self::Inherits),
            "implements" => Some(Self::Implements),
            "overrides" => Some(Self::Overrides),
            "mixes_in" | "mixes-in" => Some(Self::MixesIn),
            "references" => Some(Self::References),
            "tests" => Some(Self::Tests),
            "depends_on" | "depends-on" => Some(Self::DependsOn),
            _ => None,
        }
    }
}

/// Direction used when inspecting generic codegraph neighbors.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeGraphDirection {
    Incoming,
    Outgoing,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphQueryMetadata {
    pub manifest_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_callfacts_context_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphNodeResult {
    pub query: String,
    pub node: CodeGraphNode,
    pub incoming: Vec<CodeGraphEdge>,
    pub outgoing: Vec<CodeGraphEdge>,
    pub metadata: CodeGraphQueryMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphNeighbor {
    pub node: CodeGraphNode,
    pub relation: CodeGraphEdge,
    pub direction: CodeGraphDirection,
    /// True when the neighbor is reached through an ambiguous/possible
    /// candidate rather than a definite edge target.
    #[serde(default, skip_serializing_if = "is_false")]
    pub possible: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphNeighbors {
    pub query: String,
    pub node: CodeGraphNode,
    pub relation_filter: Vec<String>,
    pub neighbors: Vec<CodeGraphNeighbor>,
    pub metadata: CodeGraphQueryMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphRelationPath {
    pub source: String,
    pub target: String,
    pub relation_filter: Vec<String>,
    pub path: Option<Vec<String>>,
    pub relations: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub possible: bool,
    pub metadata: CodeGraphQueryMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphExplanation {
    pub query: String,
    pub node: CodeGraphNode,
    pub incoming: Vec<CodeGraphEdge>,
    pub outgoing: Vec<CodeGraphEdge>,
    pub relation_counts: BTreeMap<String, usize>,
    pub metadata: CodeGraphQueryMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphBlastRadius {
    pub function: String,
    /// Fingerprint of the Go analysis context when Go call facts were applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_callfacts_context_sha256: Option<String>,
    /// Fingerprint of the Rust analysis context when Rust call facts were applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_callfacts_context_sha256: Option<String>,
    /// Fingerprint of the TypeScript analysis context when TypeScript call facts were applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript_callfacts_context_sha256: Option<String>,
    /// Fingerprint of the Python analysis context when Python call facts were applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_callfacts_context_sha256: Option<String>,
    pub callers_by_depth: Vec<Vec<String>>,
    /// Possible callers reached through an ambiguous edge. These are not
    /// included in `callers_by_depth`, which contains only edges the artifact
    /// marked resolved. A resolved name match is not necessarily type-checked.
    pub possible_callers_by_depth: Vec<Vec<String>>,
    pub unresolved_calls: usize,
    pub total_calls: usize,
    pub ambiguous_calls: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphPath {
    pub source: String,
    pub target: String,
    pub path: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_callfacts_context_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphCluster {
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_callfacts_context_sha256: Option<String>,
    pub cluster_id: usize,
    pub community_count: usize,
    pub quality: f64,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphClustering {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_callfacts_context_sha256: Option<String>,
    pub community_count: usize,
    pub quality: f64,
    pub communities: Vec<Vec<String>>,
    pub assignments: Vec<CallGraphAssignment>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphAssignment {
    pub node_id: String,
    pub function: String,
    pub community_id: usize,
}

#[derive(Debug)]
struct ClusterPartition {
    membership: Vec<usize>,
    community_count: usize,
    quality: f64,
}

struct CallPairSummary {
    call_pairs: BTreeSet<(usize, usize)>,
    possible_call_pairs: BTreeSet<(usize, usize)>,
    total_calls: usize,
    unresolved_calls: usize,
    ambiguous_calls: usize,
}

struct AllNodeIndex {
    nodes: Vec<CodeGraphNode>,
    display_names: Vec<String>,
    by_id: HashMap<String, usize>,
    by_display_name: HashMap<String, Vec<usize>>,
    by_qualified_name: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
}

impl AllNodeIndex {
    fn new(source_nodes: &[CodeGraphNode]) -> Self {
        let mut nodes = source_nodes.to_vec();
        nodes.sort_by(|left, right| left.id.cmp(&right.id));
        let mut display_counts = HashMap::new();
        for node in &nodes {
            *display_counts.entry(display_name(node)).or_insert(0_usize) += 1;
        }
        let mut display_names = Vec::with_capacity(nodes.len());
        let mut by_id = HashMap::new();
        let mut by_display_name = HashMap::new();
        let mut by_qualified_name = HashMap::new();
        let mut by_name = HashMap::new();
        for (position, node) in nodes.iter().enumerate() {
            let base_display = display_name(node);
            let display = if display_counts
                .get(&base_display)
                .copied()
                .unwrap_or_default()
                > 1
            {
                node.qualified_name.as_ref().map_or_else(
                    || format!("{base_display} [{}]", node.id),
                    |qualified_name| format!("{base_display} [{qualified_name}]"),
                )
            } else {
                base_display.clone()
            };
            display_names.push(display.clone());
            by_id.insert(node.id.clone(), position);
            insert_position_index(&mut by_display_name, base_display, position);
            insert_position_index(&mut by_display_name, display, position);
            if let Some(qualified_name) = &node.qualified_name {
                insert_position_index(&mut by_qualified_name, qualified_name.clone(), position);
            }
            insert_position_index(&mut by_name, node.name.clone(), position);
        }
        Self {
            nodes,
            display_names,
            by_id,
            by_display_name,
            by_qualified_name,
            by_name,
        }
    }
}

/// Reusable graph algorithms over the resolved function/method call edges.
///
/// Leiden runs lazily and is cached for this index instance. Construct one
/// index per artifact and reuse it for related queries.
#[derive(Debug)]
pub struct CallGraphIndex {
    nodes: Vec<CodeGraphNode>,
    display_names: Vec<String>,
    graph: DiGraph<usize, ()>,
    possible_graph: DiGraph<usize, ()>,
    by_id: HashMap<String, NodeIndex>,
    by_display_name: HashMap<String, Vec<NodeIndex>>,
    by_qualified_name: HashMap<String, Vec<NodeIndex>>,
    by_name: HashMap<String, Vec<NodeIndex>>,
    manifest_key: String,
    go_callfacts_context_sha256: Option<String>,
    rust_callfacts_context_sha256: Option<String>,
    typescript_callfacts_context_sha256: Option<String>,
    python_callfacts_context_sha256: Option<String>,
    all_nodes: Vec<CodeGraphNode>,
    all_display_names: Vec<String>,
    all_by_id: HashMap<String, usize>,
    all_by_display_name: HashMap<String, Vec<usize>>,
    all_by_qualified_name: HashMap<String, Vec<usize>>,
    all_by_name: HashMap<String, Vec<usize>>,
    all_edges: Vec<CodeGraphEdge>,
    total_calls: usize,
    unresolved_calls: usize,
    ambiguous_calls: usize,
    clusters: OnceLock<Result<ClusterPartition, String>>,
}

impl CallGraphIndex {
    /// Builds an in-memory call graph from a serialized codegraph artifact.
    #[must_use]
    pub fn new(artifact: &CodeGraphArtifact) -> Self {
        let AllNodeIndex {
            nodes: all_nodes,
            display_names: all_display_names,
            by_id: all_by_id,
            by_display_name: all_by_display_name,
            by_qualified_name: all_by_qualified_name,
            by_name: all_by_name,
        } = AllNodeIndex::new(&artifact.nodes);

        let mut definitions = artifact
            .nodes
            .iter()
            .filter(|node| matches!(node.kind.as_str(), "function" | "method"))
            .cloned()
            .collect::<Vec<_>>();
        definitions.sort_by_key(|node| (display_name(node), node.id.clone()));

        let mut display_counts = HashMap::new();
        for node in &definitions {
            *display_counts.entry(display_name(node)).or_insert(0_usize) += 1;
        }

        let mut graph = DiGraph::<usize, ()>::new();
        let mut possible_graph = DiGraph::<usize, ()>::new();
        let mut nodes = Vec::new();
        let mut display_names = Vec::new();
        let mut by_id = HashMap::new();
        let mut by_display_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        let mut by_qualified_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        let mut by_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        for node in &definitions {
            let base_display = display_name(node);
            let display = if display_counts
                .get(&base_display)
                .copied()
                .unwrap_or_default()
                > 1
            {
                node.qualified_name.as_ref().map_or_else(
                    || format!("{base_display} [{}]", node.id),
                    |qualified_name| format!("{base_display} [{qualified_name}]"),
                )
            } else {
                base_display.clone()
            };
            let position = nodes.len();
            nodes.push(node.clone());
            display_names.push(display.clone());
            let index = graph.add_node(position);
            let possible_index = possible_graph.add_node(position);
            debug_assert_eq!(index, possible_index);
            by_id.insert(node.id.clone(), index);
            insert_index(&mut by_display_name, base_display, index);
            insert_index(&mut by_display_name, display, index);
            if let Some(qualified_name) = &node.qualified_name {
                insert_index(&mut by_qualified_name, qualified_name.clone(), index);
            }
            insert_index(&mut by_name, node.name.clone(), index);
        }

        let call_pair_summary = collect_call_pairs(artifact, &by_id);
        let CallPairSummary {
            call_pairs,
            possible_call_pairs,
            total_calls,
            unresolved_calls,
            ambiguous_calls,
        } = call_pair_summary;
        for (source, target) in call_pairs {
            graph.add_edge(NodeIndex::new(source), NodeIndex::new(target), ());
        }
        for (source, target) in possible_call_pairs {
            possible_graph.add_edge(NodeIndex::new(source), NodeIndex::new(target), ());
        }

        Self {
            nodes,
            display_names,
            graph,
            possible_graph,
            by_id,
            by_display_name,
            by_qualified_name,
            by_name,
            manifest_key: artifact.manifest_key.clone(),
            go_callfacts_context_sha256: artifact.go_callfacts_context_sha256.clone(),
            rust_callfacts_context_sha256: artifact.rust_callfacts_context_sha256.clone(),
            typescript_callfacts_context_sha256: artifact
                .typescript_callfacts_context_sha256
                .clone(),
            python_callfacts_context_sha256: artifact.python_callfacts_context_sha256.clone(),
            all_nodes,
            all_display_names,
            all_by_id,
            all_by_display_name,
            all_by_qualified_name,
            all_by_name,
            all_edges: artifact.edges.clone(),
            total_calls,
            unresolved_calls,
            ambiguous_calls,
            clusters: OnceLock::new(),
        }
    }

    /// Finds callers of a function by depth, like `CocoIndex`'s blast-radius query.
    ///
    /// # Errors
    ///
    /// Returns [`CodeGraphError::GraphNodeNotFound`] or
    /// [`CodeGraphError::GraphNodeAmbiguous`] if `function` cannot be resolved.
    pub fn blast_radius(
        &self,
        function: &str,
        depth: usize,
    ) -> CodeGraphResult<CallGraphBlastRadius> {
        let target = self.resolve_node(function)?;
        let mut seen = HashSet::from([target]);
        let mut definite_depth = HashMap::from([(target, 0_usize)]);
        let mut frontier = vec![target];
        let mut callers_by_depth = Vec::new();

        for _ in 0..depth {
            let mut next = Vec::new();
            for node in frontier {
                for caller in self.graph.neighbors_directed(node, Direction::Incoming) {
                    if seen.insert(caller) {
                        next.push(caller);
                        definite_depth.insert(caller, callers_by_depth.len() + 1);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            next.sort_by_key(|index| self.display_for_index(*index));
            callers_by_depth.push(
                next.iter()
                    .map(|index| self.display_for_index(*index))
                    .collect(),
            );
            frontier = next;
        }

        let mut possible_depth = HashMap::new();
        let mut queue = VecDeque::from([(target, false, 0_usize)]);
        let mut visited_states = HashSet::from([(target, false)]);
        while let Some((node, uncertain, distance)) = queue.pop_front() {
            if distance >= depth {
                continue;
            }
            for caller in self.graph.neighbors_directed(node, Direction::Incoming) {
                let state = (caller, uncertain);
                if visited_states.insert(state) {
                    let next_distance = distance + 1;
                    if uncertain {
                        possible_depth.insert(caller, next_distance);
                    }
                    queue.push_back((caller, uncertain, next_distance));
                }
            }
            for caller in self
                .possible_graph
                .neighbors_directed(node, Direction::Incoming)
            {
                let state = (caller, true);
                if visited_states.insert(state) {
                    let next_distance = distance + 1;
                    possible_depth.insert(caller, next_distance);
                    queue.push_back((caller, true, next_distance));
                }
            }
        }
        let max_possible_depth = possible_depth
            .iter()
            .filter(|(node, _)| !definite_depth.contains_key(node) && **node != target)
            .map(|(_, distance)| *distance)
            .max()
            .unwrap_or_default();
        let mut possible_callers_by_depth = vec![Vec::new(); max_possible_depth];
        for current_depth in 1..=max_possible_depth {
            let mut callers = possible_depth
                .iter()
                .filter(|(node, found_depth)| {
                    **found_depth == current_depth
                        && !definite_depth.contains_key(node)
                        && **node != target
                })
                .map(|(node, _)| self.display_for_index(*node))
                .collect::<Vec<_>>();
            callers.sort();
            possible_callers_by_depth[current_depth - 1] = callers;
        }

        Ok(CallGraphBlastRadius {
            function: self.display_for_index(target),
            go_callfacts_context_sha256: self.go_callfacts_context_sha256.clone(),
            rust_callfacts_context_sha256: self.rust_callfacts_context_sha256.clone(),
            typescript_callfacts_context_sha256: self.typescript_callfacts_context_sha256.clone(),
            python_callfacts_context_sha256: self.python_callfacts_context_sha256.clone(),
            callers_by_depth,
            possible_callers_by_depth,
            unresolved_calls: self.unresolved_calls,
            total_calls: self.total_calls,
            ambiguous_calls: self.ambiguous_calls,
        })
    }

    /// Finds a shortest directed call path between two functions.
    ///
    /// A missing path is returned as `path: None`; an unknown or ambiguous
    /// function name is an error.
    ///
    /// # Errors
    ///
    /// Returns [`CodeGraphError::GraphNodeNotFound`] or
    /// [`CodeGraphError::GraphNodeAmbiguous`] if either endpoint cannot be resolved.
    pub fn shortest_path(&self, source: &str, target: &str) -> CodeGraphResult<CallGraphPath> {
        let source_index = self.resolve_node(source)?;
        let target_index = self.resolve_node(target)?;
        let path = astar(
            &self.graph,
            source_index,
            |candidate| candidate == target_index,
            |_| 1_usize,
            |_| 0_usize,
        )
        .map(|(_, path)| {
            path.into_iter()
                .map(|index| self.display_for_index(index))
                .collect()
        });
        Ok(CallGraphPath {
            source: self.display_for_index(source_index),
            target: self.display_for_index(target_index),
            path,
            go_callfacts_context_sha256: self.go_callfacts_context_sha256.clone(),
            rust_callfacts_context_sha256: self.rust_callfacts_context_sha256.clone(),
            typescript_callfacts_context_sha256: self.typescript_callfacts_context_sha256.clone(),
            python_callfacts_context_sha256: self.python_callfacts_context_sha256.clone(),
        })
    }

    /// Returns the Leiden community containing a function and its members.
    ///
    /// Leiden uses a deterministic manifest-derived seed and modularity,
    /// matching `CocoIndex`'s `ModularityVertexPartition` semantics.
    ///
    /// # Errors
    ///
    /// Returns a node lookup error or a Leiden algorithm error.
    pub fn cluster(&self, function: &str) -> CodeGraphResult<CallGraphCluster> {
        let function_index = self.resolve_node(function)?;
        let partition = self.cluster_partition()?;
        let cluster_id = partition.membership[function_index.index()];
        let mut members = self
            .graph
            .node_indices()
            .filter(|index| partition.membership[index.index()] == cluster_id)
            .map(|index| self.display_for_index(index))
            .collect::<Vec<_>>();
        members.sort();
        Ok(CallGraphCluster {
            function: self.display_for_index(function_index),
            go_callfacts_context_sha256: self.go_callfacts_context_sha256.clone(),
            rust_callfacts_context_sha256: self.rust_callfacts_context_sha256.clone(),
            typescript_callfacts_context_sha256: self.typescript_callfacts_context_sha256.clone(),
            python_callfacts_context_sha256: self.python_callfacts_context_sha256.clone(),
            cluster_id,
            community_count: partition.community_count,
            quality: partition.quality,
            members,
        })
    }

    /// Returns the full Leiden partition. Useful for comparing or caching a
    /// snapshot's communities without rerunning clustering for each function.
    ///
    /// # Errors
    ///
    /// Returns a Leiden algorithm error if clustering fails.
    pub fn clustering(&self) -> CodeGraphResult<CallGraphClustering> {
        let partition = self.cluster_partition()?;
        let mut communities: Vec<Vec<String>> = vec![Vec::new(); partition.community_count];
        for index in self.graph.node_indices() {
            communities[partition.membership[index.index()]].push(self.display_for_index(index));
        }
        for community in &mut communities {
            community.sort();
        }
        communities.sort_by(|left, right| left.first().cmp(&right.first()));
        let mut assignments = self
            .graph
            .node_indices()
            .map(|index| {
                let node = &self.nodes[self.node_position(index)];
                CallGraphAssignment {
                    node_id: node.id.clone(),
                    function: self.display_for_index(index),
                    community_id: partition.membership[index.index()],
                }
            })
            .collect::<Vec<_>>();
        assignments.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        Ok(CallGraphClustering {
            go_callfacts_context_sha256: self.go_callfacts_context_sha256.clone(),
            rust_callfacts_context_sha256: self.rust_callfacts_context_sha256.clone(),
            typescript_callfacts_context_sha256: self.typescript_callfacts_context_sha256.clone(),
            python_callfacts_context_sha256: self.python_callfacts_context_sha256.clone(),
            community_count: partition.community_count,
            quality: partition.quality,
            communities,
            assignments,
        })
    }

    /// Returns one node and all directly attached relations, regardless of
    /// whether the node is a function. This is the generic inspection surface
    /// behind relation-aware codegraph queries.
    ///
    /// # Errors
    ///
    /// Returns a node-not-found or node-ambiguous error when `query` does not
    /// identify exactly one node in the snapshot.
    pub fn node(&self, query: &str) -> CodeGraphResult<CodeGraphNodeResult> {
        let position = self.resolve_all_node(query)?;
        let node_id = self.all_nodes[position].id.as_str();
        let mut incoming = self
            .all_edges
            .iter()
            .filter(|edge| {
                edge.target.as_deref() == Some(node_id)
                    || (edge.target.is_none()
                        && edge
                            .ambiguous_candidates
                            .iter()
                            .any(|candidate| candidate == node_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut outgoing = self
            .all_edges
            .iter()
            .filter(|edge| edge.source == node_id)
            .cloned()
            .collect::<Vec<_>>();
        sort_edges(&mut incoming);
        sort_edges(&mut outgoing);
        Ok(CodeGraphNodeResult {
            query: query.to_owned(),
            node: self.all_nodes[position].clone(),
            incoming,
            outgoing,
            metadata: self.query_metadata(),
        })
    }

    /// Returns a provenance-bearing explanation for any codegraph node.
    ///
    /// # Errors
    ///
    /// Returns the same node resolution errors as [`Self::node`].
    pub fn explain(&self, query: &str) -> CodeGraphResult<CodeGraphExplanation> {
        let result = self.node(query)?;
        let mut relation_counts = BTreeMap::new();
        for edge in result.incoming.iter().chain(result.outgoing.iter()) {
            *relation_counts.entry(edge.kind.clone()).or_insert(0) += 1;
        }
        Ok(CodeGraphExplanation {
            query: result.query,
            node: result.node,
            incoming: result.incoming,
            outgoing: result.outgoing,
            relation_counts,
            metadata: result.metadata,
        })
    }

    /// Returns incoming and outgoing neighbors for a generic code node.
    ///
    /// When `include_possible` is true, ambiguous candidate targets are
    /// projected as possible neighbors without changing the serialized edge's
    /// definite target fields.
    ///
    /// # Errors
    ///
    /// Returns a node-not-found or node-ambiguous error when `query` does not
    /// identify exactly one node in the snapshot.
    pub fn neighbors(
        &self,
        query: &str,
        relation_filter: Option<&[CodeGraphRelationKind]>,
        include_possible: bool,
    ) -> CodeGraphResult<CodeGraphNeighbors> {
        let position = self.resolve_all_node(query)?;
        let mut neighbors = Vec::new();
        for direction in [CodeGraphDirection::Incoming, CodeGraphDirection::Outgoing] {
            for (neighbor, edge, possible) in
                self.traversable_edges(position, direction, relation_filter, include_possible)
            {
                neighbors.push(CodeGraphNeighbor {
                    node: self.all_nodes[neighbor].clone(),
                    relation: edge,
                    direction,
                    possible,
                });
            }
        }
        neighbors.sort_by(|left, right| {
            (
                direction_name(left.direction),
                self.all_display_names
                    .get(self.all_by_id[left.node.id.as_str()])
                    .map(String::as_str)
                    .unwrap_or_default(),
                left.relation.kind.as_str(),
                left.possible,
            )
                .cmp(&(
                    direction_name(right.direction),
                    self.all_display_names
                        .get(self.all_by_id[right.node.id.as_str()])
                        .map(String::as_str)
                        .unwrap_or_default(),
                    right.relation.kind.as_str(),
                    right.possible,
                ))
        });
        Ok(CodeGraphNeighbors {
            query: query.to_owned(),
            node: self.all_nodes[position].clone(),
            relation_filter: relation_filter_names(relation_filter),
            neighbors,
            metadata: self.query_metadata(),
        })
    }

    /// Finds a shortest directed path over all selected relation kinds.
    ///
    /// The default relation set is every known and unknown serialized edge
    /// kind. Set `include_possible` to include ambiguous candidate targets.
    ///
    /// # Errors
    ///
    /// Returns a node-not-found or node-ambiguous error when either endpoint
    /// does not identify exactly one node in the snapshot.
    ///
    /// # Panics
    ///
    /// This method relies on the internal breadth-first-search invariant that
    /// every reachable endpoint has a recorded parent.
    pub fn relation_path(
        &self,
        source: &str,
        target: &str,
        relation_filter: Option<&[CodeGraphRelationKind]>,
        include_possible: bool,
    ) -> CodeGraphResult<CodeGraphRelationPath> {
        let source_position = self.resolve_all_node(source)?;
        let target_position = self.resolve_all_node(target)?;
        let mut parent: HashMap<usize, (usize, CodeGraphEdge, bool)> = HashMap::new();
        let mut queue = VecDeque::from([source_position]);
        while let Some(position) = queue.pop_front() {
            if position == target_position {
                break;
            }
            for (neighbor, edge, possible) in self.traversable_edges(
                position,
                CodeGraphDirection::Outgoing,
                relation_filter,
                include_possible,
            ) {
                if parent.contains_key(&neighbor) || neighbor == source_position {
                    continue;
                }
                parent.insert(neighbor, (position, edge, possible));
                queue.push_back(neighbor);
            }
        }

        let (path, relations, possible) = if source_position == target_position {
            (
                Some(vec![self.all_display_names[source_position].clone()]),
                Vec::new(),
                false,
            )
        } else if parent.contains_key(&target_position) {
            let mut positions = vec![target_position];
            let mut reversed_relations = Vec::new();
            let mut any_possible = false;
            let mut current = target_position;
            while current != source_position {
                let (previous, edge, edge_possible) = parent
                    .remove(&current)
                    .expect("path parent exists for reachable node");
                any_possible |= edge_possible;
                reversed_relations.push(edge.kind);
                positions.push(previous);
                current = previous;
            }
            positions.reverse();
            reversed_relations.reverse();
            (
                Some(
                    positions
                        .into_iter()
                        .map(|position| self.all_display_names[position].clone())
                        .collect(),
                ),
                reversed_relations,
                any_possible,
            )
        } else {
            (None, Vec::new(), false)
        };

        Ok(CodeGraphRelationPath {
            source: self.all_display_names[source_position].clone(),
            target: self.all_display_names[target_position].clone(),
            relation_filter: relation_filter_names(relation_filter),
            path,
            relations,
            possible,
            metadata: self.query_metadata(),
        })
    }

    fn resolve_node(&self, query: &str) -> CodeGraphResult<NodeIndex> {
        if let Some(index) = self.by_id.get(query) {
            return Ok(*index);
        }
        let candidates = self
            .by_display_name
            .get(query)
            .or_else(|| self.by_qualified_name.get(query))
            .or_else(|| self.by_name.get(query));
        let Some(candidates) = candidates else {
            return Err(CodeGraphError::GraphNodeNotFound {
                query: query.to_owned(),
            });
        };
        if let [index] = candidates.as_slice() {
            return Ok(*index);
        }
        let mut candidates = candidates
            .iter()
            .map(|index| {
                let node = &self.nodes[self.node_position(*index)];
                format!("{} [{}]", display_name(node), node.id)
            })
            .collect::<Vec<_>>();
        candidates.sort();
        Err(CodeGraphError::GraphNodeAmbiguous {
            query: query.to_owned(),
            candidates,
        })
    }

    fn resolve_all_node(&self, query: &str) -> CodeGraphResult<usize> {
        if let Some(position) = self.all_by_id.get(query) {
            return Ok(*position);
        }
        let candidates = self
            .all_by_display_name
            .get(query)
            .or_else(|| self.all_by_qualified_name.get(query))
            .or_else(|| self.all_by_name.get(query));
        let Some(candidates) = candidates else {
            return Err(CodeGraphError::GraphNodeNotFound {
                query: query.to_owned(),
            });
        };
        if let [position] = candidates.as_slice() {
            return Ok(*position);
        }
        let mut candidates = candidates
            .iter()
            .map(|position| {
                format!(
                    "{} [{}]",
                    display_name(&self.all_nodes[*position]),
                    self.all_nodes[*position].id
                )
            })
            .collect::<Vec<_>>();
        candidates.sort();
        Err(CodeGraphError::GraphNodeAmbiguous {
            query: query.to_owned(),
            candidates,
        })
    }

    fn query_metadata(&self) -> CodeGraphQueryMetadata {
        CodeGraphQueryMetadata {
            manifest_key: self.manifest_key.clone(),
            go_callfacts_context_sha256: self.go_callfacts_context_sha256.clone(),
            rust_callfacts_context_sha256: self.rust_callfacts_context_sha256.clone(),
            typescript_callfacts_context_sha256: self.typescript_callfacts_context_sha256.clone(),
            python_callfacts_context_sha256: self.python_callfacts_context_sha256.clone(),
        }
    }

    fn traversable_edges(
        &self,
        position: usize,
        direction: CodeGraphDirection,
        relation_filter: Option<&[CodeGraphRelationKind]>,
        include_possible: bool,
    ) -> Vec<(usize, CodeGraphEdge, bool)> {
        let node_id = self.all_nodes[position].id.as_str();
        let mut result = Vec::new();
        for edge in &self.all_edges {
            if !edge_matches_filter(edge, relation_filter) {
                continue;
            }
            match direction {
                CodeGraphDirection::Outgoing if edge.source == node_id => {
                    if let Some(target) =
                        edge.target.as_deref().and_then(|id| self.all_by_id.get(id))
                    {
                        result.push((*target, edge.clone(), false));
                    } else if include_possible && edge.target.is_none() {
                        for candidate in &edge.ambiguous_candidates {
                            if let Some(target) = self.all_by_id.get(candidate) {
                                result.push((*target, edge.clone(), true));
                            }
                        }
                    }
                }
                CodeGraphDirection::Incoming => {
                    if edge.target.as_deref() == Some(node_id) {
                        if let Some(source) = self.all_by_id.get(edge.source.as_str()) {
                            result.push((*source, edge.clone(), false));
                        }
                    } else if include_possible
                        && edge.target.is_none()
                        && edge
                            .ambiguous_candidates
                            .iter()
                            .any(|candidate| candidate == node_id)
                        && let Some(source) = self.all_by_id.get(edge.source.as_str())
                    {
                        result.push((*source, edge.clone(), true));
                    }
                }
                CodeGraphDirection::Outgoing => {}
            }
        }
        result.sort_by(|left, right| {
            (
                self.all_display_names[left.0].as_str(),
                left.1.kind.as_str(),
                left.2,
                left.1.range.as_ref().map_or(0, |range| range.start_byte),
            )
                .cmp(&(
                    self.all_display_names[right.0].as_str(),
                    right.1.kind.as_str(),
                    right.2,
                    right.1.range.as_ref().map_or(0, |range| range.start_byte),
                ))
        });
        result
    }

    fn display_for_index(&self, index: NodeIndex) -> String {
        self.display_names[self.node_position(index)].clone()
    }

    fn node_position(&self, index: NodeIndex) -> usize {
        *self
            .graph
            .node_weight(index)
            .expect("call graph index points to an existing node")
    }

    fn cluster_partition(&self) -> CodeGraphResult<&ClusterPartition> {
        self.clusters
            .get_or_init(|| self.compute_clusters().map_err(|error| error.clone()))
            .as_ref()
            .map_err(|error| CodeGraphError::Clustering(error.clone()))
    }

    fn compute_clusters(&self) -> Result<ClusterPartition, String> {
        if self.nodes.is_empty() {
            return Ok(ClusterPartition {
                membership: Vec::new(),
                community_count: 0,
                quality: 0.0,
            });
        }

        let mut graph = Graph::<usize, f64, Undirected>::new_undirected();
        for index in 0..self.nodes.len() {
            graph.add_node(index);
        }
        let mut undirected_edges = BTreeSet::new();
        for edge in self.graph.edge_references() {
            let source = edge.source().index();
            let target = edge.target().index();
            undirected_edges.insert((source.min(target), source.max(target)));
        }
        for (source, target) in undirected_edges {
            graph.add_edge(NodeIndex::new(source), NodeIndex::new(target), 1.0);
        }

        let graph_data = from_petgraph(&graph).map_err(|error| error.to_string())?;
        let seed = self
            .manifest_key
            .get(..16)
            .and_then(|prefix| u64::from_str_radix(prefix, 16).ok())
            .unwrap_or_default();
        let result = Leiden::new(LeidenConfig {
            seed: Some(seed),
            quality: QualityType::Modularity,
            ..LeidenConfig::default()
        })
        .run(&graph_data)
        .map_err(|error| error.to_string())?;
        let membership = (0..self.nodes.len())
            .map(|index| result.partition.community_of(index))
            .collect();
        Ok(ClusterPartition {
            membership,
            community_count: result.partition.num_communities(),
            quality: result.quality,
        })
    }
}

fn insert_index(index: &mut HashMap<String, Vec<NodeIndex>>, key: String, node_index: NodeIndex) {
    let indices = index.entry(key).or_default();
    if !indices.contains(&node_index) {
        indices.push(node_index);
    }
}

fn insert_position_index(index: &mut HashMap<String, Vec<usize>>, key: String, position: usize) {
    let positions = index.entry(key).or_default();
    if !positions.contains(&position) {
        positions.push(position);
    }
}

fn edge_matches_filter(
    edge: &CodeGraphEdge,
    relation_filter: Option<&[CodeGraphRelationKind]>,
) -> bool {
    relation_filter.is_none_or(|kinds| {
        CodeGraphRelationKind::parse(&edge.kind).is_some_and(|kind| kinds.contains(&kind))
    })
}

fn relation_filter_names(relation_filter: Option<&[CodeGraphRelationKind]>) -> Vec<String> {
    relation_filter.map_or_else(Vec::new, |kinds| {
        kinds.iter().map(|kind| kind.as_str().to_owned()).collect()
    })
}

fn direction_name(direction: CodeGraphDirection) -> &'static str {
    match direction {
        CodeGraphDirection::Incoming => "incoming",
        CodeGraphDirection::Outgoing => "outgoing",
    }
}

fn sort_edges(edges: &mut [CodeGraphEdge]) {
    edges.sort_by(|left, right| {
        (
            left.source.as_str(),
            left.kind.as_str(),
            left.target.as_deref().unwrap_or_default(),
            left.target_name.as_deref().unwrap_or_default(),
            left.resolution.as_deref().unwrap_or_default(),
            left.range.as_ref().map_or(0, |range| range.start_byte),
            left.range.as_ref().map_or(0, |range| range.end_byte),
        )
            .cmp(&(
                right.source.as_str(),
                right.kind.as_str(),
                right.target.as_deref().unwrap_or_default(),
                right.target_name.as_deref().unwrap_or_default(),
                right.resolution.as_deref().unwrap_or_default(),
                right.range.as_ref().map_or(0, |range| range.start_byte),
                right.range.as_ref().map_or(0, |range| range.end_byte),
            ))
    });
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn collect_call_pairs(
    artifact: &CodeGraphArtifact,
    by_id: &HashMap<String, NodeIndex>,
) -> CallPairSummary {
    let mut call_pairs = BTreeSet::new();
    let mut possible_call_pairs = BTreeSet::new();
    let mut total_calls = 0;
    let mut unresolved_calls = 0;
    let mut ambiguous_calls = 0;
    for edge in artifact.edges.iter().filter(|edge| edge.kind == "calls") {
        let Some(source) = by_id.get(&edge.source).copied() else {
            continue;
        };
        total_calls += 1;
        if !edge.ambiguous_candidates.is_empty() {
            ambiguous_calls += 1;
        }
        let Some(target_id) = edge.target.as_deref() else {
            unresolved_calls += usize::from(edge.ambiguous_candidates.is_empty());
            for candidate_id in &edge.ambiguous_candidates {
                if let Some(candidate) = by_id.get(candidate_id.as_str()).copied() {
                    possible_call_pairs.insert((source.index(), candidate.index()));
                }
            }
            continue;
        };
        if let Some(target) = by_id.get(target_id).copied() {
            call_pairs.insert((source.index(), target.index()));
        } else {
            unresolved_calls += 1;
        }
    }
    CallPairSummary {
        call_pairs,
        possible_call_pairs,
        total_calls,
        unresolved_calls,
        ambiguous_calls,
    }
}

fn display_name(node: &CodeGraphNode) -> String {
    node.path.as_ref().map_or_else(
        || node.name.clone(),
        |path| format!("{path}::{}", node.name),
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs};

    use tempfile::tempdir;

    use super::CallGraphIndex;
    use crate::{CodeGraphError, CodeGraphRelationKind, build_go_codegraph};

    #[test]
    fn answers_blast_radius_and_shortest_path_queries() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\nfunc target() {}\nfunc direct() { target() }\nfunc middle() { target() }\nfunc root() { middle() }\n",
        )
        .expect("Go source");
        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let index = CallGraphIndex::new(&artifact);

        let blast = index.blast_radius("target", 3).expect("blast radius");
        assert_eq!(blast.function, "main.go::target");
        assert_eq!(
            blast.callers_by_depth,
            [
                vec!["main.go::direct".to_owned(), "main.go::middle".to_owned()],
                vec!["main.go::root".to_owned()],
            ]
        );
        assert_eq!(blast.total_calls, 3);
        assert_eq!(blast.unresolved_calls, 0);

        let path = index
            .shortest_path("root", "target")
            .expect("shortest path");
        assert_eq!(
            path.path,
            Some(vec![
                "main.go::root".to_owned(),
                "main.go::middle".to_owned(),
                "main.go::target".to_owned(),
            ])
        );
        assert_eq!(
            index
                .shortest_path("target", "root")
                .expect("unreachable path")
                .path,
            None
        );
    }

    #[test]
    fn leiden_groups_disconnected_call_communities_and_is_reproducible() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\nfunc a1() { a2(); a3() }\nfunc a2() { a1(); a3() }\nfunc a3() { a1(); a2() }\nfunc b1() { b2(); b3() }\nfunc b2() { b1(); b3() }\nfunc b3() { b1(); b2() }\n",
        )
        .expect("Go source");
        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let index = CallGraphIndex::new(&artifact);

        let a_cluster = index.cluster("a1").expect("a cluster");
        let b_cluster = index.cluster("b1").expect("b cluster");
        let a_members = a_cluster.members.iter().cloned().collect::<BTreeSet<_>>();
        assert_eq!(
            a_members,
            BTreeSet::from([
                "main.go::a1".to_owned(),
                "main.go::a2".to_owned(),
                "main.go::a3".to_owned(),
            ])
        );
        assert_ne!(a_cluster.cluster_id, b_cluster.cluster_id);
        assert_eq!(a_cluster.community_count, 2);
        assert_eq!(index.cluster("a1").expect("repeat cluster"), a_cluster);
        let clustering = index.clustering().expect("full clustering");
        assert_eq!(clustering.community_count, 2);
        assert_eq!(clustering.communities.len(), 2);
    }

    #[test]
    fn queries_report_missing_and_ambiguous_function_names() {
        let directory = tempdir().expect("workspace");
        fs::create_dir_all(directory.path().join("first")).expect("first package");
        fs::create_dir_all(directory.path().join("second")).expect("second package");
        fs::write(
            directory.path().join("first/first.go"),
            "package first\n\nfunc duplicate() {}\n",
        )
        .expect("first source");
        fs::write(
            directory.path().join("second/second.go"),
            "package second\n\nfunc duplicate() {}\n",
        )
        .expect("second source");
        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let index = CallGraphIndex::new(&artifact);

        assert!(matches!(
            index.blast_radius("duplicate", 1),
            Err(CodeGraphError::GraphNodeAmbiguous { .. })
        ));
        assert!(matches!(
            index.blast_radius("missing", 1),
            Err(CodeGraphError::GraphNodeNotFound { .. })
        ));
    }

    #[test]
    fn callgraph_index_preserves_distinct_receiver_methods() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\ntype alpha struct{}\ntype beta struct{}\nfunc (alpha) Value() {}\nfunc (beta) Value() {}\nfunc callAlpha(a alpha) { a.Value() }\nfunc callBeta(b beta) { b.Value() }\n",
        )
        .expect("Go source");
        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let method_ids = artifact
            .nodes
            .iter()
            .filter(|node| node.kind == "method" && node.name == "Value")
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(method_ids.len(), 2, "the artifact keeps receiver identity");

        let index = CallGraphIndex::new(&artifact);
        let projected = method_ids
            .iter()
            .map(|id| index.by_id.get(id).expect("projected method index"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            projected.len(),
            2,
            "receiver-distinct methods must remain separate graph vertices"
        );
        assert_eq!(
            index
                .by_display_name
                .get("main.go::Value")
                .expect("ambiguous short display alias")
                .len(),
            2
        );
        assert_eq!(index.by_qualified_name["main.alpha.Value"].len(), 1);
        assert_eq!(index.by_qualified_name["main.beta.Value"].len(), 1);
        let method_displays = projected
            .iter()
            .map(|node_index| index.display_for_index(**node_index))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            method_displays,
            BTreeSet::from([
                "main.go::Value [main.alpha.Value]".to_owned(),
                "main.go::Value [main.beta.Value]".to_owned(),
            ])
        );
        let clustered_methods = index
            .clustering()
            .expect("method communities")
            .assignments
            .into_iter()
            .filter(|assignment| method_ids.contains(&assignment.node_id))
            .map(|assignment| assignment.function)
            .collect::<BTreeSet<_>>();
        assert_eq!(clustered_methods, method_displays);
        assert!(matches!(
            index.blast_radius("main.go::Value", 1),
            Err(CodeGraphError::GraphNodeAmbiguous { .. })
        ));
    }

    #[test]
    fn generic_queries_cover_nodes_relations_and_possible_targets() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("first.go"),
            "package demo\n\ntype Base struct{}\ntype Child struct { Base }\nfunc target() {}\n",
        )
        .expect("first source");
        fs::write(
            directory.path().join("second.go"),
            "package demo\n\nfunc target() {}\n",
        )
        .expect("second source");
        fs::write(
            directory.path().join("caller.go"),
            "package demo\n\nfunc caller() { target() }\n",
        )
        .expect("caller source");

        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let index = CallGraphIndex::new(&artifact);
        let calls = [CodeGraphRelationKind::Calls];

        let explanation = index.node("caller.go::caller").expect("caller node");
        assert_eq!(explanation.node.kind, "function");
        assert_eq!(explanation.outgoing.len(), 1);
        assert_eq!(explanation.outgoing[0].kind, "calls");

        let neighbors = index
            .neighbors("caller.go::caller", Some(&calls), true)
            .expect("possible call neighbors");
        assert_eq!(neighbors.neighbors.len(), 2);
        assert!(neighbors.neighbors.iter().all(|neighbor| neighbor.possible));

        let target_ids = artifact
            .nodes
            .iter()
            .filter(|node| node.kind == "function" && node.name == "target")
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(target_ids.len(), 2);
        let target = target_ids.first().expect("target node");
        let path = index
            .relation_path("caller.go::caller", target, Some(&calls), true)
            .expect("possible relation path");
        assert!(path.path.is_some());
        assert!(path.possible);
        assert_eq!(path.relations, ["calls"]);

        let inheritance = [CodeGraphRelationKind::Inherits];
        let inheritance_path = index
            .relation_path(
                "first.go::Child",
                "first.go::Base",
                Some(&inheritance),
                false,
            )
            .expect("structural relation path");
        assert!(inheritance_path.path.is_some());
        assert!(!inheritance_path.possible);
        assert_eq!(inheritance_path.relations, ["inherits"]);

        let definition_filter = [CodeGraphRelationKind::Defines];
        let file_neighbors = index
            .neighbors("caller.go", Some(&definition_filter), false)
            .expect("file definitions");
        assert_eq!(file_neighbors.neighbors.len(), 1);
        assert_eq!(file_neighbors.neighbors[0].node.name, "caller");

        let explanation = index.explain("caller.go::caller").expect("explanation");
        assert_eq!(explanation.relation_counts["calls"], 1);
        assert_eq!(explanation.metadata.manifest_key, artifact.manifest_key);
    }
}
