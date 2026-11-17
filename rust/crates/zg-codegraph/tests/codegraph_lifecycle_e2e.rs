use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};
use zg_codegraph::{
    CODEGRAPH_FILE, CODEGRAPH_PUBLICATION_FILE, CodeGraphArtifact, CodeGraphChange,
    build_codegraph, codegraph_source_stamps, read_codegraph, refresh_codegraph, update_codegraph,
    validate_codegraph_publication,
};

#[derive(Debug, Deserialize)]
struct LifecycleTruth {
    schema: String,
    source_sha256: BTreeMap<String, String>,
    stages: Vec<LifecycleStage>,
}

#[derive(Debug, Deserialize)]
struct LifecycleStage {
    name: String,
    files: Vec<String>,
    nodes: Vec<String>,
}

#[test]
#[allow(clippy::too_many_lines)]
fn lifecycle_fixture_keeps_full_incremental_and_published_snapshots_in_sync() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codegraph-lifecycle-20260930");
    let truth: LifecycleTruth =
        serde_json::from_slice(&fs::read(fixture.join("truth.json")).expect("lifecycle truth"))
            .expect("valid lifecycle truth");
    assert_eq!(truth.schema, "zvec-grep.codegraph-lifecycle-v1");
    assert_eq!(truth.stages.len(), 5);
    assert_fixture_hashes(&fixture, &truth);

    let workspace = copy_fixture(&fixture, &truth);
    let initial = build_codegraph(workspace.path()).expect("initial full graph");
    assert_stage(&initial, &truth.stages[0]);

    let (artifact_path, persisted_initial) =
        refresh_codegraph(workspace.path()).expect("initial publication");
    assert_eq!(persisted_initial, initial);
    assert_eq!(
        artifact_path,
        workspace
            .path()
            .canonicalize()
            .expect("canonical workspace")
            .join(".zvec-grep")
            .join(CODEGRAPH_FILE)
    );
    assert_eq!(
        validate_codegraph_publication(workspace.path()).expect("initial publication validity"),
        initial
    );
    let initial_bytes = fs::read(&artifact_path).expect("initial artifact bytes");

    let (_, unchanged) = refresh_codegraph(workspace.path()).expect("unchanged refresh");
    assert_eq!(unchanged, initial);
    assert_eq!(
        fs::read(&artifact_path).expect("unchanged artifact bytes"),
        initial_bytes,
        "a no-op refresh must not rewrite the published graph"
    );

    fs::write(
        workspace.path().join("caller.go"),
        "package main\n\nfunc caller() {\n\thelper()\n\tadded()\n}\n",
    )
    .expect("modify caller");
    let modified_incremental = update_codegraph(
        &initial,
        workspace.path(),
        &[CodeGraphChange::Upsert(PathBuf::from("caller.go"))],
    )
    .expect("modified incremental graph");
    let modified_full = build_codegraph(workspace.path()).expect("modified full graph");
    assert_same_stage(
        &modified_incremental,
        &modified_full,
        &truth.stages[1],
        "modify",
    );

    fs::write(
        workspace.path().join("added.go"),
        "package main\n\nfunc added() {}\n",
    )
    .expect("add source");
    let added_incremental = update_codegraph(
        &modified_incremental,
        workspace.path(),
        &[CodeGraphChange::Upsert(PathBuf::from("added.go"))],
    )
    .expect("added incremental graph");
    let added_full = build_codegraph(workspace.path()).expect("added full graph");
    assert_same_stage(&added_incremental, &added_full, &truth.stages[2], "add");

    fs::remove_file(workspace.path().join("helper.go")).expect("delete source");
    let deleted_incremental = update_codegraph(
        &added_incremental,
        workspace.path(),
        &[CodeGraphChange::Delete(PathBuf::from("helper.go"))],
    )
    .expect("deleted incremental graph");
    let deleted_full = build_codegraph(workspace.path()).expect("deleted full graph");
    assert_same_stage(
        &deleted_incremental,
        &deleted_full,
        &truth.stages[3],
        "delete",
    );
    assert!(deleted_incremental.edges.iter().any(|edge| {
        edge.kind == "calls"
            && edge.target_name.as_deref() == Some("helper")
            && edge.target.is_none()
            && !edge.resolved
    }));

    fs::rename(
        workspace.path().join("added.go"),
        workspace.path().join("renamed.go"),
    )
    .expect("rename source");
    let renamed_incremental = update_codegraph(
        &deleted_incremental,
        workspace.path(),
        &[
            CodeGraphChange::Delete(PathBuf::from("added.go")),
            CodeGraphChange::Upsert(PathBuf::from("renamed.go")),
        ],
    )
    .expect("renamed incremental graph");
    let renamed_full = build_codegraph(workspace.path()).expect("renamed full graph");
    assert_same_stage(
        &renamed_incremental,
        &renamed_full,
        &truth.stages[4],
        "rename",
    );
    assert!(
        renamed_incremental
            .files
            .iter()
            .any(|file| file.path == "renamed.go")
    );
    assert!(
        !renamed_incremental
            .files
            .iter()
            .any(|file| file.path == "added.go")
    );
    assert!(renamed_incremental.edges.iter().any(|edge| {
        edge.kind == "calls" && edge.target_name.as_deref() == Some("added") && edge.resolved
    }));

    let stamps_before = codegraph_source_stamps(workspace.path()).expect("initial source stamps");
    assert!(!stamps_before.contains_key("dist/generated.go"));
    assert!(!stamps_before.contains_key("node_modules/pkg/index.ts"));
    assert!(stamps_before.contains_key("caller.go"));
    fs::write(
        workspace.path().join("caller.go"),
        "package main\n\nfunc caller() {}\n",
    )
    .expect("change source for cache stamp");
    let stamps_after_source_change =
        codegraph_source_stamps(workspace.path()).expect("changed source stamps");
    assert_ne!(
        stamps_before.get("caller.go"),
        stamps_after_source_change.get("caller.go")
    );

    let sidecar = workspace
        .path()
        .join(".zvec-grep")
        .join("go-callfacts-v2.json");
    fs::create_dir_all(sidecar.parent().expect("sidecar parent")).expect("sidecar directory");
    fs::write(&sidecar, b"generation-one").expect("write cache input");
    let stamps_with_sidecar =
        codegraph_source_stamps(workspace.path()).expect("sidecar source stamps");
    assert!(stamps_with_sidecar.contains_key(".zvec-grep/go-callfacts-v2.json"));
    fs::write(&sidecar, b"generation-two").expect("replace cache input");
    let stamps_after_sidecar_change =
        codegraph_source_stamps(workspace.path()).expect("changed sidecar source stamps");
    assert_ne!(
        stamps_with_sidecar.get(".zvec-grep/go-callfacts-v2.json"),
        stamps_after_sidecar_change.get(".zvec-grep/go-callfacts-v2.json")
    );
    fs::remove_file(&sidecar).expect("remove cache input");

    // Reconciliation repairs a missing publication record while preserving the
    // current graph. The current source is intentionally rebuilt after the
    // stamp-only mutation above so the comparison remains source-current.
    let (_, current) = refresh_codegraph(workspace.path()).expect("current reconciliation");
    let current_full = build_codegraph(workspace.path()).expect("current full graph");
    assert_eq!(current, current_full);
    let publication = workspace
        .path()
        .join(".zvec-grep")
        .join(CODEGRAPH_PUBLICATION_FILE);
    fs::remove_file(&publication).expect("remove publication marker");
    let (_, repaired) = refresh_codegraph(workspace.path()).expect("repair publication");
    assert_eq!(repaired, current);
    assert_eq!(
        validate_codegraph_publication(workspace.path()).expect("repaired publication validity"),
        current
    );

    let graph_bytes = fs::read(&artifact_path).expect("current graph bytes");
    fs::write(
        workspace.path().join("caller.go"),
        "package main\n\nfunc caller() {\n\tadded()\n}\n",
    )
    .expect("mutate published source");
    assert!(validate_codegraph_publication(workspace.path()).is_err());
    fs::write(
        workspace.path().join("caller.go"),
        "package main\n\nfunc caller() {}\n",
    )
    .expect("restore published source");
    fs::write(&artifact_path, {
        let mut bytes = graph_bytes.clone();
        bytes.push(0);
        bytes
    })
    .expect("tamper published graph");
    assert!(validate_codegraph_publication(workspace.path()).is_err());
    fs::write(&artifact_path, graph_bytes).expect("restore published graph");
    assert_eq!(
        read_codegraph(&artifact_path).expect("read restored graph"),
        current
    );
    assert!(validate_codegraph_publication(workspace.path()).is_ok());
}

fn assert_fixture_hashes(fixture: &Path, truth: &LifecycleTruth) {
    for (relative, expected) in &truth.source_sha256 {
        let path = fixture.join(relative);
        assert!(path.is_file(), "missing lifecycle fixture input {relative}");
        assert_eq!(
            sha256(&path),
            *expected,
            "fixture hash changed for {relative}"
        );
    }
}

fn copy_fixture(fixture: &Path, truth: &LifecycleTruth) -> TempDir {
    let workspace = tempdir().expect("lifecycle workspace");
    for relative in truth.source_sha256.keys() {
        let source = fixture.join(relative);
        let target = workspace.path().join(relative);
        fs::create_dir_all(target.parent().expect("fixture target parent"))
            .expect("fixture target directory");
        fs::copy(source, target).expect("copy lifecycle fixture input");
    }
    workspace
}

fn assert_same_stage(
    incremental: &CodeGraphArtifact,
    full: &CodeGraphArtifact,
    stage: &LifecycleStage,
    transition: &str,
) {
    assert_eq!(incremental, full, "{transition}: incremental/full mismatch");
    assert_stage(incremental, stage);
}

fn assert_stage(artifact: &CodeGraphArtifact, stage: &LifecycleStage) {
    assert_eq!(
        artifact
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>(),
        stage.files.iter().cloned().collect::<BTreeSet<_>>(),
        "{}: source file set",
        stage.name
    );
    let names = artifact
        .nodes
        .iter()
        .map(|node| node.name.clone())
        .collect::<BTreeSet<_>>();
    for expected in &stage.nodes {
        assert!(
            names.contains(expected),
            "{}: missing node {expected}",
            stage.name
        );
    }
    assert!(!names.contains("ignoredGenerated"));
    assert!(!names.contains("ignoredDependency"));
}

fn sha256(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(fs::read(path).expect("fixture bytes"));
    hex::encode(digest.finalize())
}
