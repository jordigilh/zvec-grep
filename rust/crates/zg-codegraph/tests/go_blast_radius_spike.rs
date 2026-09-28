use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use tempfile::TempDir;
use zg_codegraph::{
    CallGraphIndex, CodeGraphArtifact, GO_CALLFACTS_SCHEMA, GO_CALLFACTS_VERSION, GoCallFact,
    GoCallFactsArtifact, GoCallFactsFile, build_go_codegraph, refresh_codegraph,
};

#[derive(Debug, Deserialize)]
struct FixtureTruth {
    source_sha256: HashMap<String, String>,
    calls: Vec<CallTruth>,
    blast_radius: Vec<BlastTruth>,
}

#[derive(Debug, Deserialize)]
struct CallTruth {
    path: String,
    line: usize,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    start_column: usize,
    end_column: usize,
    caller: String,
    target_name: String,
    target: Option<String>,
    #[serde(default)]
    possible_targets: Vec<String>,
    resolution: String,
}

#[derive(Debug, Deserialize)]
struct BlastTruth {
    target: String,
    depth: usize,
    definite_by_depth: Vec<Vec<String>>,
    #[serde(default)]
    possible_by_depth: Vec<Vec<String>>,
}

#[test]
fn name_only_baseline_misses_frozen_go_call_truth() {
    let truth = fixture_truth();
    let baseline = fixture_graph();
    let symbols = symbol_ids(&baseline);
    let baseline_scores = exact_static_scores(&baseline, &truth, &symbols);

    // One cross-package same-name false positive, two missed receiver-specific
    // calls, and one interface dispatch incorrectly treated as exact.
    assert_eq!(baseline_scores, (5, 7, 8));
    let precision_milli = baseline_scores.0 * 1_000 / baseline_scores.1;
    let recall_milli = baseline_scores.0 * 1_000 / baseline_scores.2;
    eprintln!(
        "baseline exact-edge precision={}.{:03} ({}/{}), recall={}.{:03} ({}/{})",
        precision_milli / 1_000,
        precision_milli % 1_000,
        baseline_scores.0,
        baseline_scores.1,
        recall_milli / 1_000,
        recall_milli % 1_000,
        baseline_scores.0,
        baseline_scores.2,
    );

    let index = CallGraphIndex::new(&baseline);
    assert_eq!(
        index
            .blast_radius("app.Flush", 1)
            .expect("local Flush callers")
            .callers_by_depth,
        [vec![
            "app/calls.go::CallerExternalFlush".to_owned(),
            "app/calls.go::CallerLocalFlush".to_owned(),
        ]]
    );
    assert!(
        index
            .blast_radius("dep.Flush", 1)
            .expect("dependency Flush callers")
            .callers_by_depth
            .is_empty()
    );
    assert_eq!(
        index
            .blast_radius("app.Worker.Execute", 1)
            .expect("baseline Worker.Execute callers")
            .callers_by_depth,
        [vec!["app/calls.go::InterfaceCaller".to_owned()]]
    );
    assert!(
        index
            .blast_radius("app.Alpha.Run", 1)
            .expect("Alpha.Run callers")
            .callers_by_depth
            .is_empty()
    );
    assert_eq!(
        index
            .blast_radius("app.Alpha.Run", 1)
            .expect("Alpha.Run candidates")
            .possible_callers_by_depth,
        [vec![
            "app/calls.go::AlphaCaller".to_owned(),
            "app/calls.go::BetaCaller".to_owned(),
        ]]
    );
}

#[test]
fn generated_go_callfacts_drive_blast_radius_results() {
    let truth = fixture_truth();
    let workspace = copy_fixture();
    write_callfacts(workspace.path(), &truth);
    let artifact = build_go_codegraph(workspace.path()).expect("graph with Go facts sidecar");

    let index = CallGraphIndex::new(&artifact);
    for expected in &truth.blast_radius {
        let target = expected
            .target
            .split_once("::")
            .map_or(expected.target.as_str(), |(_, symbol)| symbol);
        let result = index
            .blast_radius(target, expected.depth)
            .unwrap_or_else(|error| panic!("blast radius for {}: {error}", expected.target));
        assert_eq!(
            result.callers_by_depth,
            display_layers(&expected.definite_by_depth),
            "definite callers for {}",
            expected.target
        );
        assert_eq!(
            result.possible_callers_by_depth,
            display_layers(&expected.possible_by_depth),
            "possible callers for {}",
            expected.target
        );
    }

    let interface_result = index
        .blast_radius("app.Worker.Execute", 2)
        .expect("Worker.Execute callers");
    assert!(interface_result.callers_by_depth.is_empty());
    assert_eq!(
        interface_result.possible_callers_by_depth,
        [
            vec!["app/calls.go::InterfaceCaller".to_owned()],
            vec!["app/calls.go::InterfaceCallerOuter".to_owned()],
        ]
    );
}

#[test]
fn refresh_reverts_to_syntax_edges_when_sidecar_is_removed_or_stale() {
    let truth = fixture_truth();
    let workspace = copy_fixture();
    write_callfacts(workspace.path(), &truth);
    let (_, initial) = refresh_codegraph(workspace.path()).expect("initial semantic refresh");

    fs::remove_file(callfacts_path(workspace.path())).expect("remove call-facts sidecar");
    let (_, without_sidecar) = refresh_codegraph(workspace.path()).expect("remove overlay");
    assert_ne!(without_sidecar.manifest_key, initial.manifest_key);
    assert_eq!(
        CallGraphIndex::new(&without_sidecar)
            .blast_radius("app.Worker.Execute", 1)
            .expect("syntax fallback result")
            .callers_by_depth,
        [vec!["app/calls.go::InterfaceCaller".to_owned()]],
        "the syntax fallback should be restored, not retain old possible edges"
    );

    write_callfacts(workspace.path(), &truth);
    let (_, semantic_again) = refresh_codegraph(workspace.path()).expect("reapply overlay");
    assert_eq!(semantic_again.manifest_key, initial.manifest_key);

    let facts_path = callfacts_path(workspace.path());
    let mut invalid_facts: serde_json::Value =
        serde_json::from_slice(&fs::read(&facts_path).expect("read valid sidecar"))
            .expect("decode valid sidecar");
    invalid_facts["calls"][0]["end_byte"] = serde_json::json!(
        invalid_facts["calls"][0]["end_byte"]
            .as_u64()
            .expect("call end byte")
            + 1
    );
    fs::write(
        &facts_path,
        serde_json::to_vec(&invalid_facts).expect("serialize invalid range"),
    )
    .expect("write invalid range");
    let (_, invalid_range_fallback) =
        refresh_codegraph(workspace.path()).expect("inconsistent call-site range should fall back");
    assert_eq!(
        CallGraphIndex::new(&invalid_range_fallback)
            .blast_radius("app.Worker.Execute", 1)
            .expect("invalid-range syntax fallback")
            .callers_by_depth,
        [vec!["app/calls.go::InterfaceCaller".to_owned()]]
    );

    write_callfacts(workspace.path(), &truth);
    let _ = refresh_codegraph(workspace.path()).expect("restore valid sidecar");
    let source = workspace.path().join("app/calls.go");
    let mut contents = fs::read(&source).expect("read source");
    contents.extend_from_slice(b"\n// source changed after sidecar generation\n");
    fs::write(source, contents).expect("change Go source");
    let (_, stale_fallback) = refresh_codegraph(workspace.path()).expect("stale sidecar fallback");
    assert_eq!(
        CallGraphIndex::new(&stale_fallback)
            .blast_radius("app.Worker.Execute", 1)
            .expect("stale syntax fallback")
            .callers_by_depth,
        [vec!["app/calls.go::InterfaceCaller".to_owned()]]
    );

    fs::write(callfacts_path(workspace.path()), b"not valid JSON")
        .expect("corrupt optional sidecar");
    let (_, invalid_fallback) =
        refresh_codegraph(workspace.path()).expect("invalid sidecar should not block graph");
    assert_eq!(
        CallGraphIndex::new(&invalid_fallback)
            .blast_radius("app.Worker.Execute", 1)
            .expect("invalid-sidecar syntax fallback")
            .callers_by_depth,
        [vec!["app/calls.go::InterfaceCaller".to_owned()]]
    );
}

fn fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-blast-radius")
}

fn fixture_truth() -> FixtureTruth {
    serde_json::from_str(include_str!("fixtures/go-blast-radius/truth.json"))
        .expect("fixture truth")
}

fn fixture_graph() -> CodeGraphArtifact {
    let artifact = build_go_codegraph(&fixture_root()).expect("baseline Go graph");
    let truth = fixture_truth();
    for file in &artifact.files {
        assert_eq!(
            truth.source_sha256.get(&file.path),
            Some(&file.sha256),
            "source digest for {}",
            file.path
        );
    }
    artifact
}

fn copy_fixture() -> TempDir {
    let destination = tempfile::tempdir().expect("temporary fixture root");
    for relative in ["go.mod", "app/calls.go", "dep/dep.go"] {
        let source = fixture_root().join(relative);
        let target = destination.path().join(relative);
        fs::create_dir_all(target.parent().expect("fixture parent")).expect("fixture directory");
        fs::copy(source, target).expect("copy Go fixture source");
    }
    destination
}

fn write_callfacts(root: &Path, truth: &FixtureTruth) {
    let mut files: Vec<GoCallFactsFile> = truth
        .source_sha256
        .iter()
        .filter(|(path, _)| Path::new(path).extension().is_some_and(|ext| ext == "go"))
        .map(|(path, sha256)| GoCallFactsFile {
            path: path.clone(),
            sha256: sha256.clone(),
        })
        .collect();
    files.sort_by(|left: &GoCallFactsFile, right| left.path.cmp(&right.path));
    let calls = truth
        .calls
        .iter()
        .map(|call| GoCallFact {
            path: call.path.clone(),
            start_byte: call.start_byte,
            end_byte: call.end_byte,
            start_line: call.start_line,
            end_line: call.end_line,
            start_column: call.start_column,
            end_column: call.end_column,
            caller: call.caller.clone(),
            target_name: call.target_name.clone(),
            target: call.target.clone(),
            possible_targets: call.possible_targets.clone(),
            resolution: call.resolution.clone(),
        })
        .collect();
    let artifact = GoCallFactsArtifact {
        schema: GO_CALLFACTS_SCHEMA.to_owned(),
        version: GO_CALLFACTS_VERSION,
        files,
        calls,
    };
    let path = callfacts_path(root);
    fs::create_dir_all(path.parent().expect("sidecar directory")).expect("sidecar directory");
    fs::write(
        path,
        serde_json::to_vec_pretty(&artifact).expect("serialize call facts"),
    )
    .expect("write Go call-facts sidecar");
}

fn callfacts_path(root: &Path) -> std::path::PathBuf {
    root.join(".zvec-grep").join("go-callfacts-v1.json")
}

fn display_layers(layers: &[Vec<String>]) -> Vec<Vec<String>> {
    layers
        .iter()
        .map(|layer| {
            layer
                .iter()
                .map(|symbol| display_for_truth(symbol))
                .collect()
        })
        .collect()
}

fn symbol_ids(artifact: &CodeGraphArtifact) -> HashMap<String, String> {
    artifact
        .nodes
        .iter()
        .filter_map(|node| {
            Some((
                format!(
                    "{}::{}",
                    node.path.as_deref()?,
                    node.qualified_name.as_deref()?
                ),
                node.id.clone(),
            ))
        })
        .collect()
}

fn exact_static_scores(
    artifact: &CodeGraphArtifact,
    truth: &FixtureTruth,
    symbols: &HashMap<String, String>,
) -> (usize, usize, usize) {
    let expected_by_site = truth
        .calls
        .iter()
        .map(|call| ((call.path.as_str(), call.line), call))
        .collect::<HashMap<_, _>>();
    let nodes_by_id = artifact
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut correct = 0;
    let mut predicted_definite = 0;
    let static_truth_count = truth
        .calls
        .iter()
        .filter(|call| call.resolution == "static")
        .count();

    for edge in artifact
        .edges
        .iter()
        .filter(|edge| edge.kind == "calls" && edge.target.is_some())
    {
        predicted_definite += 1;
        let Some(caller) = nodes_by_id.get(edge.source.as_str()) else {
            continue;
        };
        let Some(range) = edge.range.as_ref() else {
            continue;
        };
        let Some(expected) =
            expected_by_site.get(&(caller.path.as_deref().unwrap_or_default(), range.start_line))
        else {
            continue;
        };
        if expected.resolution != "static" {
            continue;
        }
        let expected_target = expected
            .target
            .as_ref()
            .and_then(|symbol| symbols.get(symbol));
        if expected_target.is_some_and(|target| edge.target.as_ref() == Some(target)) {
            correct += 1;
        }
    }

    (correct, predicted_definite, static_truth_count)
}

fn display_for_truth(symbol: &str) -> String {
    let (path, qualified_name) = symbol
        .split_once("::")
        .expect("fixture symbol has path and qualified name");
    let name = qualified_name
        .rsplit('.')
        .next()
        .expect("qualified symbol leaf");
    format!("{path}::{name}")
}

#[test]
fn frozen_go_fixture_and_truth_are_present() {
    let root = fixture_root();
    assert!(root.join("go.mod").is_file());
    assert!(root.join("truth.json").is_file());
    assert!(fs::read(root.join("app/calls.go")).is_ok());
}
