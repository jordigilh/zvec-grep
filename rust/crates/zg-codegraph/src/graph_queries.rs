//! Query operations over a persisted codegraph snapshot.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
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

use crate::{CodeGraphArtifact, CodeGraphError, CodeGraphNode, CodeGraphResult};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphBlastRadius {
    pub function: String,
    pub callers_by_depth: Vec<Vec<String>>,
    pub unresolved_calls: usize,
    pub total_calls: usize,
    pub ambiguous_calls: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphPath {
    pub source: String,
    pub target: String,
    pub path: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphCluster {
    pub function: String,
    pub cluster_id: usize,
    pub community_count: usize,
    pub quality: f64,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CallGraphClustering {
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

/// Reusable graph algorithms over the resolved function/method call edges.
///
/// Leiden runs lazily and is cached for this index instance. Construct one
/// index per artifact and reuse it for related queries.
#[derive(Debug)]
pub struct CallGraphIndex {
    nodes: Vec<CodeGraphNode>,
    graph: DiGraph<usize, ()>,
    by_id: HashMap<String, NodeIndex>,
    by_display_name: HashMap<String, Vec<NodeIndex>>,
    by_qualified_name: HashMap<String, Vec<NodeIndex>>,
    by_name: HashMap<String, Vec<NodeIndex>>,
    manifest_key: String,
    total_calls: usize,
    unresolved_calls: usize,
    ambiguous_calls: usize,
    clusters: OnceLock<Result<ClusterPartition, String>>,
}

impl CallGraphIndex {
    /// Builds an in-memory call graph from a serialized codegraph artifact.
    #[must_use]
    pub fn new(artifact: &CodeGraphArtifact) -> Self {
        let mut definitions = artifact
            .nodes
            .iter()
            .filter(|node| matches!(node.kind.as_str(), "function" | "method"))
            .cloned()
            .collect::<Vec<_>>();
        definitions.sort_by_key(|node| (display_name(node), node.id.clone()));

        let mut graph = DiGraph::<usize, ()>::new();
        let mut nodes = Vec::new();
        let mut by_id = HashMap::new();
        let mut by_display_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        let mut by_qualified_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        let mut by_name: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        for node in &definitions {
            let display = display_name(node);
            let index = by_display_name
                .get(&display)
                .and_then(|indices| indices.first())
                .copied()
                .unwrap_or_else(|| {
                    let position = nodes.len();
                    nodes.push(node.clone());
                    graph.add_node(position)
                });
            by_id.insert(node.id.clone(), index);
            insert_index(&mut by_display_name, display, index);
            if let Some(qualified_name) = &node.qualified_name {
                insert_index(&mut by_qualified_name, qualified_name.clone(), index);
            }
            insert_index(&mut by_name, node.name.clone(), index);
        }

        let mut call_pairs = BTreeSet::new();
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
                continue;
            };
            let Some(target) = by_id.get(target_id).copied() else {
                unresolved_calls += 1;
                continue;
            };
            call_pairs.insert((source.index(), target.index()));
        }
        for (source, target) in call_pairs {
            graph.add_edge(NodeIndex::new(source), NodeIndex::new(target), ());
        }

        Self {
            nodes,
            graph,
            by_id,
            by_display_name,
            by_qualified_name,
            by_name,
            manifest_key: artifact.manifest_key.clone(),
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
        let mut frontier = vec![target];
        let mut callers_by_depth = Vec::new();

        for _ in 0..depth {
            let mut next = Vec::new();
            for node in frontier {
                for caller in self.graph.neighbors_directed(node, Direction::Incoming) {
                    if seen.insert(caller) {
                        next.push(caller);
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

        Ok(CallGraphBlastRadius {
            function: self.display_for_index(target),
            callers_by_depth,
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
                    function: display_name(node),
                    community_id: partition.membership[index.index()],
                }
            })
            .collect::<Vec<_>>();
        assignments.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        Ok(CallGraphClustering {
            community_count: partition.community_count,
            quality: partition.quality,
            communities,
            assignments,
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

    fn display_for_index(&self, index: NodeIndex) -> String {
        display_name(&self.nodes[self.node_position(index)])
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
    use crate::{CodeGraphError, build_go_codegraph};

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
    fn analysis_projection_matches_cocoindex_path_and_method_name_nodes() {
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
            1,
            "the graph query view mirrors CocoIndex's path/name key"
        );
        assert_eq!(
            index
                .by_display_name
                .get("main.go::Value")
                .expect("method display node")
                .len(),
            1
        );
    }
}
