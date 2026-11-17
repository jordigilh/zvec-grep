use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};
use zg_codegraph::{
    CallGraphIndex, CodeGraphAffected, CodeGraphArtifact, CodeGraphChange, CodeGraphDirection,
    CodeGraphRelationKind, build_codegraph, read_codegraph, refresh_codegraph, update_codegraph,
};

const LANGUAGES: [&str; 4] = ["go", "rust", "typescript", "python"];

#[derive(Debug, Deserialize)]
struct FixtureTruth {
    schema: String,
    language: String,
    source_files: Vec<String>,
    context_files: Vec<String>,
    source_sha256: BTreeMap<String, String>,
    relations: Vec<RelationQrel>,
    absent_relations: Vec<RelationQrel>,
    call_qrels: Vec<CallQrel>,
    topology_qrels: TopologyQrels,
    affected_qrels: Vec<AffectedQrel>,
    community_qrels: CommunityQrels,
}

#[derive(Debug, Deserialize)]
struct RelationQrel {
    kind: String,
    source: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct CallQrel {
    caller: String,
    target_name: String,
    resolution: String,
    candidate_nodes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TopologyQrels {
    neighbors: Vec<NeighborQrel>,
    paths: Vec<PathQrel>,
}

#[derive(Debug, Deserialize)]
struct NeighborQrel {
    query: String,
    relations: Vec<String>,
    outgoing: Vec<NeighborQrelEdge>,
    incoming: Vec<NeighborQrelEdge>,
}

#[derive(Debug, Deserialize)]
struct NeighborQrelEdge {
    node: String,
    kind: String,
    target_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PathQrel {
    source: String,
    target: String,
    relations: Vec<String>,
    path: Option<Vec<String>>,
    edge_kinds: Vec<String>,
    possible: bool,
}

#[derive(Debug, Deserialize)]
struct CommunityQrels {
    community_count: usize,
    communities: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct AffectedQrel {
    query: String,
    depth: usize,
    relations: Vec<String>,
    include_possible: bool,
    definite: Vec<Vec<NeighborQrelEdge>>,
    possible: Vec<Vec<NeighborQrelEdge>>,
}

#[test]
fn four_language_relation_fixtures_are_source_pinned_and_queryable() {
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codegraph-relations-20260928");

    for language in LANGUAGES {
        let fixture = fixture_root.join(language);
        let truth = load_truth(&fixture, language);
        let workspace = copy_fixture(&fixture, &truth);
        let initial = build_codegraph(workspace.path()).expect("initial fixture graph");

        assert_fixture_shape(&initial, &truth);
        assert_relation_qrels(&initial, &truth);
        let index = CallGraphIndex::new(&initial);
        assert_topology_qrels(&index, &truth);
        assert_affected_qrels(&index, &truth);
        assert_community_qrels(&index, &truth);
        assert_call_qrels(&initial, &truth);

        let (artifact_path, refreshed) =
            refresh_codegraph(workspace.path()).expect("persist fixture graph");
        assert_eq!(refreshed, initial, "{language}: refresh differs from build");
        let persisted: CodeGraphArtifact =
            read_codegraph(&artifact_path).expect("decode persisted graph artifact");
        assert_eq!(persisted, initial, "{language}: persisted graph differs");
        let persisted_bytes = fs::read(&artifact_path).expect("persisted bytes");

        let changed_path = truth
            .source_files
            .first()
            .expect("fixture has a source file");
        let changed_source = workspace.path().join(changed_path);
        let original = fs::read(&changed_source).expect("read source for incremental update");
        let mut changed = original.clone();
        changed.extend_from_slice(b"\n");
        fs::write(&changed_source, &changed).expect("change fixture source");
        let incrementally_updated = update_codegraph(
            &initial,
            workspace.path(),
            &[CodeGraphChange::Upsert(PathBuf::from(changed_path))],
        )
        .expect("incremental fixture graph");
        let fully_updated = build_codegraph(workspace.path()).expect("full updated fixture graph");
        assert_eq!(
            incrementally_updated, fully_updated,
            "{language}: incremental mismatch"
        );

        fs::write(&changed_source, original).expect("restore fixture source");
        let (_, restored) = refresh_codegraph(workspace.path()).expect("restore fixture graph");
        assert_eq!(restored, initial, "{language}: restored graph differs");
        let (_, unchanged) = refresh_codegraph(workspace.path()).expect("unchanged fixture graph");
        assert_eq!(unchanged, initial, "{language}: unchanged refresh differs");
        assert_eq!(
            fs::read(&artifact_path).expect("final persisted bytes"),
            persisted_bytes,
            "{language}: unchanged refresh is not byte-deterministic"
        );
    }
}

fn load_truth(fixture: &Path, language: &str) -> FixtureTruth {
    let truth: FixtureTruth =
        serde_json::from_slice(&fs::read(fixture.join("truth.json")).expect("fixture truth"))
            .expect("valid fixture truth");
    assert_eq!(truth.schema, "zvec-grep.codegraph-relations-v2");
    assert_eq!(truth.language, language);

    let source_files = truth.source_files.iter().cloned().collect::<BTreeSet<_>>();
    let context_files = truth.context_files.iter().cloned().collect::<BTreeSet<_>>();
    assert!(source_files.is_disjoint(&context_files));
    let expected_files = source_files
        .union(&context_files)
        .cloned()
        .collect::<BTreeSet<_>>();
    let hashed_files = truth.source_sha256.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        hashed_files, expected_files,
        "{language}: incomplete source hashes"
    );
    for relative in &expected_files {
        let path = fixture.join(relative);
        assert!(
            path.is_file(),
            "{language}: missing fixture input {relative}"
        );
        assert_eq!(
            sha256(&path),
            truth.source_sha256[relative],
            "{language}: source hash changed for {relative}"
        );
    }
    truth
}

fn copy_fixture(fixture: &Path, truth: &FixtureTruth) -> TempDir {
    let workspace = tempdir().expect("fixture workspace");
    for relative in truth.source_sha256.keys() {
        let source = fixture.join(relative);
        let target = workspace.path().join(relative);
        fs::create_dir_all(target.parent().expect("fixture target parent"))
            .expect("fixture target directory");
        fs::copy(source, target).expect("copy fixture input");
    }
    workspace
}

fn assert_fixture_shape(artifact: &CodeGraphArtifact, truth: &FixtureTruth) {
    assert_eq!(artifact.files.len(), truth.source_files.len());
    assert_eq!(
        artifact
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>(),
        truth.source_files.iter().map(String::as_str).collect()
    );
    assert!(artifact.relation_generation > 0);
    for file in &artifact.files {
        assert_eq!(file.sha256, truth.source_sha256[&file.path]);
    }
}

fn assert_relation_qrels(artifact: &CodeGraphArtifact, truth: &FixtureTruth) {
    for expected in &truth.relations {
        let source = node_by_name(artifact, &expected.source);
        let edge = artifact
            .edges
            .iter()
            .find(|edge| {
                edge.kind == expected.kind
                    && edge.source == source.id
                    && edge.target_name.as_deref() == Some(expected.target.as_str())
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing {} {} -> {}: {:#?}",
                    expected.kind, expected.source, expected.target, artifact.edges
                )
            });
        assert!(edge.resolved, "expected resolved relation: {edge:#?}");
        assert!(edge.target.is_some(), "expected relation target: {edge:#?}");
    }

    for absent in &truth.absent_relations {
        let source = node_by_name(artifact, &absent.source);
        assert!(!artifact.edges.iter().any(|edge| {
            edge.kind == absent.kind
                && edge.source == source.id
                && edge.target_name.as_deref() == Some(absent.target.as_str())
        }));
    }
}

fn assert_call_qrels(artifact: &CodeGraphArtifact, truth: &FixtureTruth) {
    for expected in &truth.call_qrels {
        let source = node_by_name(artifact, &expected.caller);
        let matches = artifact
            .edges
            .iter()
            .filter(|edge| {
                edge.kind == "calls"
                    && edge.source == source.id
                    && edge.target_name.as_deref() == Some(expected.target_name.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "call qrel should identify one edge");
        let edge = matches[0];
        assert_eq!(
            edge.resolution.as_deref(),
            Some(expected.resolution.as_str())
        );
        assert!(edge.target.is_none());
        let actual_candidates = edge
            .ambiguous_candidates
            .iter()
            .map(|id| node_label(node_by_id(artifact, id)))
            .collect::<BTreeSet<_>>();
        let expected_candidates = expected
            .candidate_nodes
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_candidates, expected_candidates);
    }
}

fn assert_topology_qrels(index: &CallGraphIndex, truth: &FixtureTruth) {
    for expected in &truth.topology_qrels.neighbors {
        let relations = parse_relations(&expected.relations);
        let actual = index
            .neighbors(&expected.query, Some(&relations), false)
            .expect("topology neighbors query");
        assert_eq!(actual.relation_filter, expected.relations);
        assert_neighbor_edges(
            &actual.neighbors,
            &expected.outgoing,
            CodeGraphDirection::Outgoing,
        );
        assert_neighbor_edges(
            &actual.neighbors,
            &expected.incoming,
            CodeGraphDirection::Incoming,
        );
        assert_eq!(
            actual
                .neighbors
                .iter()
                .filter(|neighbor| neighbor.direction == CodeGraphDirection::Outgoing)
                .count(),
            expected.outgoing.len()
        );
        assert_eq!(
            actual
                .neighbors
                .iter()
                .filter(|neighbor| neighbor.direction == CodeGraphDirection::Incoming)
                .count(),
            expected.incoming.len()
        );
    }

    for expected in &truth.topology_qrels.paths {
        let relations = parse_relations(&expected.relations);
        let actual = index
            .relation_path(
                &expected.source,
                &expected.target,
                Some(&relations),
                expected.possible,
            )
            .expect("topology path query");
        assert_eq!(actual.path, expected.path);
        assert_eq!(actual.relations, expected.edge_kinds);
        assert_eq!(actual.possible, expected.possible);
    }
}

fn assert_neighbor_edges(
    actual: &[zg_codegraph::CodeGraphNeighbor],
    expected: &[NeighborQrelEdge],
    direction: CodeGraphDirection,
) {
    let actual = actual
        .iter()
        .filter(|neighbor| neighbor.direction == direction)
        .map(|neighbor| {
            (
                node_label(&neighbor.node),
                neighbor.relation.kind.clone(),
                neighbor.relation.target_name.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let expected = expected
        .iter()
        .map(|neighbor| {
            (
                neighbor.node.clone(),
                neighbor.kind.clone(),
                neighbor.target_name.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

fn assert_affected_qrels(index: &CallGraphIndex, truth: &FixtureTruth) {
    for expected in &truth.affected_qrels {
        let relations = parse_relations(&expected.relations);
        let actual: CodeGraphAffected = index
            .affected(
                &expected.query,
                expected.depth,
                Some(&relations),
                expected.include_possible,
            )
            .expect("affected query");
        assert_eq!(actual.relation_filter, expected.relations);
        assert_affected_levels(&actual.affected_by_depth, &expected.definite);
        assert_affected_levels(&actual.possible_affected_by_depth, &expected.possible);
    }
}

fn assert_community_qrels(index: &CallGraphIndex, truth: &FixtureTruth) {
    let actual = index.clustering().expect("community qrel query");
    assert_eq!(
        actual.community_count,
        truth.community_qrels.community_count
    );
    assert_eq!(
        actual
            .communities
            .iter()
            .map(|community| community.iter().cloned().collect::<BTreeSet<_>>())
            .collect::<BTreeSet<_>>(),
        truth
            .community_qrels
            .communities
            .iter()
            .map(|community| community.iter().cloned().collect::<BTreeSet<_>>())
            .collect::<BTreeSet<_>>()
    );
    for assignment in actual.assignments {
        assert!(
            actual.communities[assignment.community_id].contains(&assignment.function),
            "community assignment must point at its returned community"
        );
    }
}

fn assert_affected_levels(
    actual: &[Vec<zg_codegraph::CodeGraphNeighbor>],
    expected: &[Vec<NeighborQrelEdge>],
) {
    assert_eq!(actual.len(), expected.len());
    for (actual_level, expected_level) in actual.iter().zip(expected) {
        let actual_level = actual_level
            .iter()
            .map(|neighbor| (node_label(&neighbor.node), neighbor.relation.kind.clone()))
            .collect::<BTreeSet<_>>();
        let expected_level = expected_level
            .iter()
            .map(|neighbor| (neighbor.node.clone(), neighbor.kind.clone()))
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_level, expected_level);
    }
}

fn parse_relations(values: &[String]) -> Vec<CodeGraphRelationKind> {
    values
        .iter()
        .map(|value| CodeGraphRelationKind::parse(value).expect("known relation qrel"))
        .collect()
}

fn node_by_name<'a>(
    artifact: &'a CodeGraphArtifact,
    name: &str,
) -> &'a zg_codegraph::CodeGraphNode {
    artifact
        .nodes
        .iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("missing node named {name}"))
}

fn node_by_id<'a>(artifact: &'a CodeGraphArtifact, id: &str) -> &'a zg_codegraph::CodeGraphNode {
    artifact
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("missing node id {id}"))
}

fn node_label(node: &zg_codegraph::CodeGraphNode) -> String {
    node.path.as_ref().map_or_else(
        || node.name.clone(),
        |path| format!("{path}::{}", node.name),
    )
}

fn sha256(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(fs::read(path).expect("source bytes"));
    hex::encode(digest.finalize())
}
