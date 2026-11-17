use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::{TempDir, tempdir};
use zg_engine::codegraph::{CODEGRAPH_FILE, read_codegraph};

const LANGUAGES: [&str; 4] = ["go", "rust", "typescript", "python"];

#[test]
fn cli_persists_and_answers_four_language_relation_qrels() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../zg-codegraph/tests/fixtures/codegraph-relations-20260928");

    for language in LANGUAGES {
        let fixture = fixture_root.join(language);
        let truth = read_json(&fixture.join("truth.json"));
        let workspace = copy_fixture(&fixture, &truth);
        let root = workspace.path().to_str().expect("UTF-8 fixture root");
        let graph = run_zg(&["--graph", root]);
        assert!(
            graph.status.success(),
            "{language}: graph build failed: {}",
            String::from_utf8_lossy(&graph.stderr)
        );
        let artifact_path = workspace.path().join(".zvec-grep").join(CODEGRAPH_FILE);
        let artifact = read_graph(&artifact_path);
        assert_eq!(artifact["schema"], "zvec-grep.codegraph");
        assert_eq!(artifact["version"], 2);
        assert!(artifact["relation_generation"].as_u64().unwrap_or_default() > 0);
        assert_cli_call_qrels(&artifact, &truth);

        for qrel in truth["topology_qrels"]["neighbors"]
            .as_array()
            .expect("neighbor qrels")
        {
            let mut args = vec![
                "--graph-query".to_owned(),
                artifact_path.display().to_string(),
                "neighbors".to_owned(),
                qrel["query"].as_str().expect("neighbor query").to_owned(),
            ];
            append_relations(&mut args, &qrel["relations"]);
            let actual = run_query(&args, language);
            assert_query_neighbors(&actual, qrel);
        }

        for qrel in truth["topology_qrels"]["paths"]
            .as_array()
            .expect("path qrels")
        {
            let mut args = vec![
                "--graph-query".to_owned(),
                artifact_path.display().to_string(),
                "relation-path".to_owned(),
                qrel["source"].as_str().expect("path source").to_owned(),
                qrel["target"].as_str().expect("path target").to_owned(),
            ];
            append_relations(&mut args, &qrel["relations"]);
            let actual = run_query(&args, language);
            assert_eq!(actual["path"], qrel["path"]);
            assert_eq!(actual["relations"], qrel["edge_kinds"]);
            assert_eq!(
                actual["possible"].as_bool().unwrap_or(false),
                qrel["possible"]
            );
        }

        for qrel in truth["affected_qrels"].as_array().expect("affected qrels") {
            let mut args = vec![
                "--graph-query".to_owned(),
                artifact_path.display().to_string(),
                "affected".to_owned(),
                qrel["query"].as_str().expect("affected query").to_owned(),
                "--depth".to_owned(),
                qrel["depth"].as_u64().expect("affected depth").to_string(),
            ];
            append_relations(&mut args, &qrel["relations"]);
            if qrel["include_possible"].as_bool().unwrap_or(false) {
                args.push("--include-possible".to_owned());
            }
            let actual = run_query(&args, language);
            assert_affected_levels(&actual["affected_by_depth"], &qrel["definite"]);
            assert_affected_levels(&actual["possible_affected_by_depth"], &qrel["possible"]);
        }
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("JSON file")).expect("valid JSON")
}

fn read_graph(path: &Path) -> Value {
    serde_json::to_value(read_codegraph(path).expect("graph artifact")).expect("graph JSON value")
}

fn copy_fixture(fixture: &Path, truth: &Value) -> TempDir {
    let workspace = tempdir().expect("fixture workspace");
    for relative in truth["source_sha256"]
        .as_object()
        .expect("source hashes")
        .keys()
    {
        let source = fixture.join(relative);
        let target = workspace.path().join(relative);
        fs::create_dir_all(target.parent().expect("fixture target parent"))
            .expect("fixture target directory");
        fs::copy(source, target).expect("copy fixture input");
    }
    workspace
}

fn run_zg(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zg"))
        .args(arguments)
        .output()
        .expect("zg process")
}

fn run_query(arguments: &[String], language: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_zg"))
        .args(arguments)
        .output()
        .expect("zg graph query process");
    assert!(
        output.status.success(),
        "{language}: graph query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("graph query JSON")
}

fn append_relations(arguments: &mut Vec<String>, relations: &Value) {
    for relation in relations.as_array().expect("relation list") {
        arguments.push("--relation".to_owned());
        arguments.push(relation.as_str().expect("relation name").to_owned());
    }
}

fn assert_cli_call_qrels(artifact: &Value, truth: &Value) {
    let nodes = artifact["nodes"].as_array().expect("artifact nodes");
    let edges = artifact["edges"].as_array().expect("artifact edges");
    for qrel in truth["call_qrels"].as_array().expect("call qrels") {
        let caller_id = nodes
            .iter()
            .find(|node| node["name"] == qrel["caller"])
            .and_then(|node| node["id"].as_str())
            .expect("call qrel caller node");
        let edge = edges
            .iter()
            .find(|edge| {
                edge["kind"] == "calls"
                    && edge["source"] == caller_id
                    && edge["target_name"] == qrel["target_name"]
            })
            .expect("call qrel edge");
        assert_eq!(edge["resolution"], qrel["resolution"]);
        assert!(edge["target"].is_null());
        let actual = edge["ambiguous_candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|candidate| {
                let candidate_id = candidate.as_str().expect("candidate id");
                let node = nodes
                    .iter()
                    .find(|node| node["id"] == candidate_id)
                    .expect("candidate node");
                node_label(node)
            })
            .collect::<BTreeSet<_>>();
        let expected = qrel["candidate_nodes"]
            .as_array()
            .expect("expected candidates")
            .iter()
            .map(|candidate| candidate.as_str().expect("expected candidate").to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }
}

fn assert_query_neighbors(actual: &Value, qrel: &Value) {
    assert_eq!(actual["relation_filter"], qrel["relations"]);
    for direction in ["outgoing", "incoming"] {
        let actual_edges = actual["neighbors"]
            .as_array()
            .expect("neighbors")
            .iter()
            .filter(|neighbor| neighbor["direction"] == direction)
            .map(|neighbor| {
                (
                    node_label(&neighbor["node"]),
                    neighbor["relation"]["kind"]
                        .as_str()
                        .expect("relation kind")
                        .to_owned(),
                    neighbor["relation"]["target_name"]
                        .as_str()
                        .map(str::to_owned),
                )
            })
            .collect::<BTreeSet<_>>();
        let expected_edges = qrel[direction]
            .as_array()
            .expect("expected neighbors")
            .iter()
            .map(|neighbor| {
                (
                    neighbor["node"].as_str().expect("expected node").to_owned(),
                    neighbor["kind"]
                        .as_str()
                        .expect("expected relation kind")
                        .to_owned(),
                    neighbor["target_name"].as_str().map(str::to_owned),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_edges, expected_edges);
    }
}

fn assert_affected_levels(actual: &Value, expected: &Value) {
    let actual_levels = actual.as_array().expect("affected levels");
    let expected_levels = expected.as_array().expect("expected affected levels");
    assert_eq!(actual_levels.len(), expected_levels.len());
    for (actual_level, expected_level) in actual_levels.iter().zip(expected_levels) {
        let actual_nodes = actual_level
            .as_array()
            .expect("affected level")
            .iter()
            .map(|neighbor| {
                (
                    node_label(&neighbor["node"]),
                    neighbor["relation"]["kind"]
                        .as_str()
                        .expect("relation kind")
                        .to_owned(),
                )
            })
            .collect::<BTreeSet<_>>();
        let expected_nodes = expected_level
            .as_array()
            .expect("expected affected level")
            .iter()
            .map(|neighbor| {
                (
                    neighbor["node"].as_str().expect("expected node").to_owned(),
                    neighbor["kind"]
                        .as_str()
                        .expect("expected relation kind")
                        .to_owned(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_nodes, expected_nodes);
    }
}

fn node_label(node: &Value) -> String {
    node["path"].as_str().map_or_else(
        || node["name"].as_str().expect("node name").to_owned(),
        |path| format!("{path}::{}", node["name"].as_str().expect("node name")),
    )
}
