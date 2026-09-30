use std::{collections::BTreeMap, fs, path::Path, process::Command};

use serde_json::{Value, json};
use tempfile::tempdir;
use zg_engine::codegraph::{
    CODEGRAPH_FILE, GO_CALLFACTS_FILE, GO_CALLFACTS_SCHEMA, GO_CALLFACTS_VERSION,
    GoCallFactsContext, read_codegraph,
};

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
    let (sidecar_path, context_sha256) = write_fixture_sidecar(&fixture, workspace.path());

    let root = workspace.path().to_str().expect("UTF-8 test path");
    let graph_build = run_zg(&["--graph", root]);
    assert!(
        graph_build.status.success(),
        "graph build failed: {}",
        String::from_utf8_lossy(&graph_build.stderr)
    );
    let artifact = workspace.path().join(".zvec-grep").join(CODEGRAPH_FILE);
    let artifact_value: Value =
        serde_json::to_value(read_codegraph(&artifact).expect("decode graph artifact"))
            .expect("graph JSON value");
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
    assert_eq!(target_result["go_callfacts_context_sha256"], context_sha256);

    fs::remove_file(sidecar_path).expect("remove opt-in sidecar");
    let syntax_graph = run_zg(&["--graph", root]);
    assert!(
        syntax_graph.status.success(),
        "syntax fallback graph build failed"
    );
    let fallback_artifact: Value =
        serde_json::to_value(read_codegraph(&artifact).expect("decode fallback graph"))
            .expect("fallback graph JSON value");
    assert_ne!(
        fallback_artifact["manifest_key"].as_str(),
        Some(semantic_manifest.as_str())
    );
    assert_eq!(
        query(&artifact, "app.Worker.Execute", 1)["callers_by_depth"],
        json!([["app/calls.go::InterfaceCaller"]])
    );
}

fn write_fixture_sidecar(fixture: &Path, root: &Path) -> (std::path::PathBuf, String) {
    for relative in ["go.mod", "app/calls.go", "dep/dep.go"] {
        let source = fixture.join(relative);
        let target = root.join(relative);
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
    let context_files = truth["source_sha256"]
        .as_object()
        .expect("source digests")
        .iter()
        .filter(|(path, _)| {
            matches!(
                Path::new(path).file_name().and_then(|name| name.to_str()),
                Some("go.mod" | "go.sum" | "go.work" | "go.work.sum")
            )
        })
        .map(|(path, digest)| zg_engine::codegraph::GoCallFactsFile {
            path: path.clone(),
            sha256: digest.as_str().expect("context digest").to_owned(),
        })
        .collect();
    let context = GoCallFactsContext {
        go_version: "go1.26.0".to_owned(),
        go_mod: "go.mod".to_owned(),
        go_work: String::new(),
        settings: fixture_context_settings(),
        context_files,
    };
    let context_sha256 = context.fingerprint();
    let sidecar = json!({
        "schema": GO_CALLFACTS_SCHEMA,
        "version": GO_CALLFACTS_VERSION,
        "context": context,
        "context_sha256": context_sha256.clone(),
        "files": files,
        "calls": truth["calls"],
    });
    let sidecar_path = root.join(format!(".zvec-grep/{GO_CALLFACTS_FILE}"));
    fs::create_dir_all(sidecar_path.parent().expect("sidecar parent")).expect("sidecar directory");
    fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&sidecar).expect("serialize sidecar"),
    )
    .expect("write sidecar");
    (sidecar_path, context_sha256)
}

fn fixture_context_settings() -> BTreeMap<String, String> {
    [
        "GO111MODULE",
        "GO386",
        "GOAMD64",
        "GOARCH",
        "GOARM",
        "GOARM64",
        "CGO_ENABLED",
        "GOEXPERIMENT",
        "GOFLAGS",
        "GOMIPS",
        "GOMIPS64",
        "GOOS",
        "GOPPC64",
        "GORISCV64",
        "GOTOOLCHAIN",
        "GOWASM",
    ]
    .into_iter()
    .map(|name| {
        let value = match name {
            "GOOS" => "darwin",
            "GOARCH" => "arm64",
            "CGO_ENABLED" => "1",
            _ => "",
        };
        (name.to_owned(), value.to_owned())
    })
    .collect()
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
