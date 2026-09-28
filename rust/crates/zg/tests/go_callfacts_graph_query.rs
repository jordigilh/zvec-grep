use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};
use tempfile::tempdir;

fn run_zg(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zg"))
        .args(arguments)
        .output()
        .expect("zg process")
}

#[test]
fn cli_graph_query_uses_pinned_go_callfacts_and_falls_back_after_removal() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../zg-codegraph/tests/fixtures/go-blast-radius");
    let workspace = tempdir().expect("fixture workspace");
    for relative in ["go.mod", "app/calls.go", "dep/dep.go"] {
        let source = fixture.join(relative);
        let target = workspace.path().join(relative);
        fs::create_dir_all(target.parent().expect("fixture parent")).expect("fixture directory");
        fs::copy(source, target).expect("copy Go fixture");
    }

    let truth: Value =
        serde_json::from_slice(&fs::read(fixture.join("truth.json")).expect("fixture truth"))
            .expect("decode truth");
    let files = truth["source_sha256"]
        .as_object()
        .expect("source digests")
        .iter()
        .filter(|(path, _)| {
            Path::new(path)
                .extension()
                .is_some_and(|extension| extension == "go")
        })
        .map(|(path, digest)| json!({"path": path, "sha256": digest}))
        .collect::<Vec<_>>();
    let sidecar = json!({
        "schema": "zvec-grep.go-callfacts",
        "version": 1,
        "files": files,
        "calls": truth["calls"],
    });
    let sidecar_path = workspace.path().join(".zvec-grep/go-callfacts-v1.json");
    fs::create_dir_all(sidecar_path.parent().expect("sidecar parent")).expect("sidecar directory");
    fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&sidecar).expect("serialize sidecar"),
    )
    .expect("write sidecar");

    let root = workspace.path().to_str().expect("UTF-8 test path");
    let graph_build = run_zg(&["--graph", root]);
    assert!(
        graph_build.status.success(),
        "graph build failed: {}",
        String::from_utf8_lossy(&graph_build.stderr)
    );
    let artifact = workspace.path().join(".zvec-grep/codegraph-v1.json");
    let artifact_value: Value =
        serde_json::from_slice(&fs::read(&artifact).expect("graph artifact"))
            .expect("decode graph artifact");
    let semantic_manifest = artifact_value["manifest_key"]
        .as_str()
        .expect("semantic manifest")
        .to_owned();

    let target_result = query(&artifact, "app.Target", 3);
    assert_eq!(
        target_result["callers_by_depth"],
        json!([
            ["app/calls.go::Direct"],
            ["app/calls.go::Transitive"],
            ["app/calls.go::Deep"]
        ])
    );
    let interface_result = query(&artifact, "app.Worker.Execute", 2);
    assert_eq!(interface_result["callers_by_depth"], json!([]));
    assert_eq!(
        interface_result["possible_callers_by_depth"],
        json!([
            ["app/calls.go::InterfaceCaller"],
            ["app/calls.go::InterfaceCallerOuter"]
        ])
    );

    fs::remove_file(sidecar_path).expect("remove opt-in sidecar");
    let syntax_graph = run_zg(&["--graph", root]);
    assert!(
        syntax_graph.status.success(),
        "syntax fallback graph build failed"
    );
    let fallback_artifact: Value =
        serde_json::from_slice(&fs::read(&artifact).expect("fallback graph artifact"))
            .expect("decode fallback graph");
    assert_ne!(
        fallback_artifact["manifest_key"].as_str(),
        Some(semantic_manifest.as_str())
    );
    assert_eq!(
        query(&artifact, "app.Worker.Execute", 1)["callers_by_depth"],
        json!([["app/calls.go::InterfaceCaller"]])
    );
}

fn query(artifact: &Path, target: &str, depth: usize) -> Value {
    let artifact = artifact.to_str().expect("UTF-8 artifact path");
    let depth = depth.to_string();
    let output = run_zg(&[
        "--graph-query",
        artifact,
        "blast-radius",
        target,
        "--depth",
        &depth,
    ]);
    assert!(
        output.status.success(),
        "graph query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("graph query JSON")
}
