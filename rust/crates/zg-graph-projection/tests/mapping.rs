//! Mapping unit tests: upstream status/kind/provenance -> our edge contract.

use zg_codegraph::CodeGraphEdge;
use zg_graph_projection::{
    UpstreamEdgeRow, map_edge_row, parse_candidates, qualified_name, symbol_kind,
};

fn row(
    status: &str,
    kind: &str,
    target: Option<&str>,
    provenance: Option<&str>,
) -> UpstreamEdgeRow {
    UpstreamEdgeRow {
        id: 1,
        source: "a".repeat(32),
        target: target.map(str::to_owned),
        kind: kind.to_owned(),
        status: status.to_owned(),
        reference_name: Some("ref".to_owned()),
        receiver_name: None,
        line: Some(7),
        col: Some(4),
        candidates: None,
        provenance: provenance.map(str::to_owned),
    }
}

#[test]
fn resolved_edge_carries_provenance_into_resolution() {
    let edge = map_edge_row(&row(
        "resolved",
        "calls",
        Some(&"b".repeat(32)),
        Some("file_scoped"),
    ))
    .expect("resolved call maps");
    assert_eq!(edge.kind, "calls");
    assert!(edge.resolved);
    assert_eq!(edge.resolution.as_deref(), Some("upstream:file_scoped"));
    let target = format!("upstream:{}", "b".repeat(32));
    assert_eq!(edge.target.as_deref(), Some(target.as_str()));
    assert_eq!(edge.source, format!("upstream:{}", "a".repeat(32)));
    assert_eq!(edge.target_name.as_deref(), Some("ref"));
}

#[test]
fn pending_edge_is_unresolved_without_target() {
    let edge = map_edge_row(&row("pending", "calls", None, None)).expect("pending call maps");
    assert!(!edge.resolved);
    assert_eq!(edge.resolution.as_deref(), Some("upstream:pending"));
    assert!(edge.target.is_none());
    assert_eq!(edge.target_name.as_deref(), Some("ref"));
}

#[test]
fn failed_edge_is_unresolved_and_labelled_failed() {
    let edge = map_edge_row(&row("failed", "imports", None, None)).expect("failed import maps");
    assert!(!edge.resolved);
    assert_eq!(edge.kind, "imports");
    assert_eq!(edge.resolution.as_deref(), Some("upstream:failed"));
}

#[test]
fn extends_maps_to_inherits_and_declares_is_dropped() {
    let edge = map_edge_row(&row(
        "resolved",
        "extends",
        Some(&"b".repeat(32)),
        Some("import_bound"),
    ))
    .expect("extends maps");
    assert_eq!(edge.kind, "inherits");
    assert!(
        map_edge_row(&row(
            "resolved",
            "declares",
            Some(&"b".repeat(32)),
            Some("file_local")
        ))
        .is_none()
    );
}

#[test]
fn unknown_kinds_and_statuses_are_rejected() {
    assert!(map_edge_row(&row("resolved", "teleports", Some(&"b".repeat(32)), None)).is_none());
    assert!(map_edge_row(&row("speculative", "calls", Some(&"b".repeat(32)), None)).is_none());
}

#[test]
fn candidates_parse_to_endpoint_ids() {
    let candidate = format!("\"{}\"", "c".repeat(32));
    let parsed = parse_candidates(&format!("[{candidate}]"));
    assert_eq!(parsed, vec!["c".repeat(32)]);
}

#[test]
fn go_scoped_functions_become_methods_with_dotted_qualified_names() {
    let kind = symbol_kind(
        &Some("function".to_owned()),
        Some("AssetRegistry"),
        &Some("go".to_owned()),
    );
    assert_eq!(kind, "method");
    let qualified = qualified_name(Some("AssetRegistry"), "ListAssets", &Some("go".to_owned()));
    assert_eq!(qualified.as_deref(), Some("AssetRegistry.ListAssets"));
    let bare = symbol_kind(&Some("function".to_owned()), None, &Some("go".to_owned()));
    assert_eq!(bare, "function");
    let unqualified = qualified_name(None, "ListAssets", &Some("go".to_owned()));
    assert_eq!(unqualified, None);
}

#[test]
fn non_go_scoped_functions_use_scope_separators() {
    let kind = symbol_kind(
        &Some("function".to_owned()),
        Some("Widget"),
        &Some("rust".to_owned()),
    );
    assert_eq!(kind, "method");
    let qualified = qualified_name(Some("Widget"), "paint", &Some("rust".to_owned()));
    assert_eq!(qualified.as_deref(), Some("Widget::paint"));
    let class = symbol_kind(&Some("class".to_owned()), None, &Some("go".to_owned()));
    assert_eq!(class, "class");
}

#[test]
fn edge_ranges_carry_line_and_column_with_zero_bytes() {
    let edge = map_edge_row(&row(
        "resolved",
        "calls",
        Some(&"b".repeat(32)),
        Some("file_local"),
    ))
    .expect("maps");
    let range = edge.range.expect("line and col present");
    assert_eq!((range.start_line, range.start_column), (7, 4));
    assert_eq!((range.start_byte, range.end_byte), (0, 0));
}

#[test]
fn definite_possible_split_survives_the_mapping() {
    // The contract the #43 verdict depends on: resolved edges feed the definite
    // call graph, pending edges with candidates feed the possible graph.
    let resolved = map_edge_row(&row(
        "resolved",
        "calls",
        Some(&"b".repeat(32)),
        Some("workspace_unique"),
    ))
    .expect("maps");
    assert!(resolved.resolved);
    assert_eq!(resolved.ambiguous_candidates, Vec::<String>::new());
    let mut pending = map_edge_row(&row("pending", "calls", None, None)).expect("maps");
    pending.ambiguous_candidates = parse_candidates(&format!("[\"{}\"]", "c".repeat(32)));
    assert!(!pending.resolved && pending.target.is_none());
    assert_eq!(pending.ambiguous_candidates.len(), 1);
}

#[test]
fn mapped_edges_match_codegraph_edge_shape() {
    let edge = map_edge_row(&row(
        "resolved",
        "calls",
        Some(&"b".repeat(32)),
        Some("file_local"),
    ))
    .expect("maps");
    let serialized = serde_json::to_string(&edge).expect("serializes");
    let round: CodeGraphEdge = serde_json::from_str(&serialized).expect("deserializes");
    assert_eq!(round, edge);
}
