//! End-to-end projection test: a synthetic upstream store (zvec collections +
//! SQLite graph database shaped exactly like upstream `zg --index --graph`
//! output) is projected into a codegraph-v2 artifact and queried through our
//! own [`CallGraphIndex`] surface.

use std::fs;

use rusqlite::Connection;
use zvec_rust::{Collection, CollectionOptions, CollectionSchema, DataType, Doc, FieldSchema};

use zg_codegraph::{CallGraphIndex, read_codegraph};
use zg_graph_projection::{project_upstream_graph, project_upstream_index};

/// The upstream edges DDL (storage/graph/schema.rs @7504720), trimmed to the
/// columns the writer always fills in a fresh index.
const EDGES_DDL: &str = "
CREATE TABLE edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL,
    source TEXT NOT NULL,
    target TEXT,
    kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    reference_name TEXT,
    receiver_name TEXT,
    arity INTEGER,
    line INTEGER,
    col INTEGER,
    candidates TEXT,
    language TEXT NOT NULL DEFAULT 'unknown',
    name_tail TEXT NOT NULL DEFAULT '',
    provenance TEXT,
    metadata TEXT NOT NULL DEFAULT '{}'
);
INSERT INTO edges (file_id, source, target, kind, status, reference_name, line, col, provenance, language, name_tail)
VALUES
  (1, 'aaaa00000000000000000000000000aa', 'bbbb00000000000000000000000000bb', 'calls', 'resolved', 'helper', 3, 2, 'file_local', 'go', 'helper'),
  (1, 'aaaa00000000000000000000000000aa', NULL, 'calls', 'pending', 'external', 4, 2, NULL, 'go', 'external'),
  (1, 'f1', 'cccc00000000000000000000000000cc', 'imports', 'resolved', './util', 1, 0, 'import_scoped', 'go', 'util'),
  (1, 'cccc00000000000000000000000000cc', NULL, 'declares', 'resolved', NULL, NULL, NULL, 'file_local', 'go', '');
";

fn upsert(collection: &Collection, docs: &[Doc]) {
    let refs: Vec<&Doc> = docs.iter().collect();
    let result = collection.upsert(&refs).expect("upsert docs");
    assert_eq!(result.error_count, 0, "upsert must succeed");
    collection.flush().expect("flush collection");
}

fn create_collection(dir: &std::path::Path, name: &str) -> Collection {
    let mut schema = CollectionSchema::new(name).expect("schema");
    for (field, data_type) in [
        ("file_id", DataType::Uint32),
        ("entity_id", DataType::String),
        ("payload", DataType::String),
        ("metadata", DataType::String),
        ("path", DataType::String),
        ("relative_path", DataType::String),
    ] {
        let mut spec = FieldSchema::new(field, data_type, true, 0).expect("field schema");
        spec.set_index_params(&zvec_rust::IndexParams::invert(true, false).expect("index params"))
            .expect("index params");
        schema.add_field(&spec).expect("add field");
    }
    let mut options = CollectionOptions::new().expect("options");
    options.set_read_only(false).expect("writable");
    Collection::create_and_open(
        dir.join(name).to_str().expect("UTF-8 path"),
        &schema,
        Some(&options),
    )
    .expect("create collection")
}

/// `(entity_id, symbol_name, symbol_type, scope, language)` for two Go symbols
/// plus one plain value entity.
fn write_entities(store: &std::path::Path) {
    let collection = create_collection(store, "entities");
    let payload = r#"{"id":"x","file_id":1,"source_range":{"kind":"text","start_line":2,"end_line":5,"start_byte_offset":10,"end_byte_offset":40,"start_byte_column":0,"end_byte_column":1},"content":{"kind":"code","value":"func run() {}"},"fragments":[]}"#;
    let docs: Vec<Doc> = [
        (
            "aaaa00000000000000000000000000aa",
            r#"{"kind":"code","symbol_type":"function","symbol_name":"run","language":"go"}"#,
        ),
        (
            "bbbb00000000000000000000000000bb",
            r#"{"kind":"code","symbol_type":"function","symbol_name":"help","scope":"Helper","language":"go"}"#,
        ),
        (
            "cccc00000000000000000000000000cc",
            r#"{"kind":"code","symbol_type":"class","symbol_name":"Helper","language":"go"}"#,
        ),
    ]
    .iter()
    .map(|&(id, metadata)| {
        let mut doc = Doc::new().expect("doc");
        doc.set_pk(id);
        doc.add_u32("file_id", 1).expect("file id");
        doc.add_string("entity_id", id).expect("entity id");
        doc.add_string("payload", payload).expect("payload");
        doc.add_string("metadata", metadata).expect("metadata");
        doc
    })
    .collect();
    upsert(&collection, &docs);
}

fn write_files(store: &std::path::Path) {
    let collection = create_collection(store, "files");
    let payload = r#"{"id":1,"relative_path":"main.go","snapshot":{"size_bytes":120,"modified_epoch_ms":null,"content_hash":"cafe01"},"index_status":{"kind":"indexed","indexed_epoch_ms":1,"entity_count":3}}"#;
    let mut doc = Doc::new().expect("doc");
    doc.set_pk("1");
    doc.add_u32("file_id", 1).expect("file id");
    doc.add_string("path", "main.go").expect("path");
    doc.add_string("payload", payload).expect("payload");
    upsert(&collection, &[doc]);
}

fn write_graph(store: &std::path::Path) {
    let graph_dir = store.join("graph");
    fs::create_dir_all(&graph_dir).expect("graph dir");
    let connection = Connection::open(graph_dir.join("graph.sqlite")).expect("graph database");
    connection.execute_batch(EDGES_DDL).expect("seed edges");
}

#[test]
fn projects_synthetic_store_and_answers_queries() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store = workspace.path().join(".zvec-grep").join("index");
    fs::create_dir_all(&store).expect("store dir");
    write_files(&store);
    write_entities(&store);
    write_graph(&store);

    let (index, report) = project_upstream_index(&store).expect("projection succeeds");
    assert_eq!(report.symbol_nodes, 3);
    assert_eq!(report.files, 1);
    assert_eq!(report.edges, 3, "the declares row is dropped");
    assert_eq!(report.dropped_declares, 1);
    assert_eq!(report.resolved_edges, 2);
    assert_eq!(report.pending_edges, 1);
    assert_eq!(report.failed_edges, 0);
    assert_eq!(report.dangling_endpoints, 0);

    // Definite/possible split: one resolved call, one pending reference.
    let radius = index
        .blast_radius("main.go::help", None)
        .expect("blast radius");
    assert_eq!(radius.total_calls, 2);
    assert_eq!(radius.unresolved_calls, 1);
    let callers: Vec<String> = radius.callers_by_depth.concat();
    assert!(
        callers.contains(&"main.go::run".to_owned()),
        "callers: {callers:?}"
    );

    // Method mapping: scoped Go function became a method with a dotted name.
    let help = index
        .node("main.go::help")
        .expect("method node found")
        .node
        .clone();
    assert_eq!(help.kind, "method");
    assert_eq!(help.qualified_name.as_deref(), Some("Helper.help"));
    assert_eq!(help.range.as_ref().map(|range| range.start_line), Some(2));

    // File-scope endpoint nodes exist for the import edge.
    let file_node = index
        .node("upstream:f1")
        .expect("file-scope endpoint is a node")
        .node
        .clone();
    assert_eq!(file_node.kind, "file");
    assert_eq!(file_node.path.as_deref(), Some("main.go"));

    // Writing the sidecar round-trips through the standard artifact reader.
    let output = workspace.path().join("out").join("codegraph-v2.json.zst");
    let (path, artifact, report) =
        project_upstream_graph(&store, Some(&output)).expect("write projection");
    assert_eq!(report.edges, 3);
    assert!(path.is_file());
    let reloaded = read_codegraph(&path).expect("reload artifact");
    assert_eq!(reloaded, artifact);
    let reindex = CallGraphIndex::new(&reloaded);
    assert_eq!(
        reindex
            .blast_radius("main.go::help", None)
            .expect("radius")
            .total_calls,
        2
    );
}

#[test]
fn default_output_lands_beside_store_not_inside_it() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store = workspace.path().join(".zvec-grep").join("index");
    fs::create_dir_all(&store).expect("store dir");
    write_files(&store);
    write_entities(&store);
    write_graph(&store);

    let (path, _, _) = project_upstream_graph(&store, None).expect("default-output projection");
    // The documented default matches zg-codegraph's native sidecar location:
    // `<workspace>/.zvec-grep/codegraph-v2.json.zst`, NOT inside the store
    // (`<workspace>/.zvec-grep/index/...`) where `zg --graph-query` would not
    // find it.
    let expected = workspace
        .path()
        .join(".zvec-grep")
        .join("codegraph-v2.json.zst");
    assert_eq!(path, expected);
    assert!(path.is_file());
    assert!(
        !store.join("codegraph-v2.json.zst").is_file(),
        "artifact must not be written inside the store directory"
    );
}

#[test]
fn projection_is_deterministic_across_runs() {
    let first = tempfile::tempdir().expect("workspace");
    let second = tempfile::tempdir().expect("workspace");
    let mut artifacts = Vec::new();
    for workspace in [&first, &second] {
        let store = workspace.path().join(".zvec-grep").join("index");
        fs::create_dir_all(&store).expect("store dir");
        write_files(&store);
        write_entities(&store);
        write_graph(&store);
        let output = workspace.path().join("codegraph-v2.json.zst");
        let (_, artifact, _) = project_upstream_graph(&store, Some(&output)).expect("projection");
        artifacts.push(artifact);
    }
    assert_eq!(artifacts[0], artifacts[1]);
    assert_eq!(artifacts[0].manifest_key, artifacts[1].manifest_key);
}
