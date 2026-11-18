//! Projection reader: upstream zvec-grep in-engine graph store -> codegraph-v2.
//!
//! Issue #43's endorsed verdict is "ours in front, theirs underneath": keep our
//! MCP/CLI query surface, and let it consume graphs extracted by upstream's
//! integrated substrate without migrating. This crate is the reader half of
//! that: given a workspace index built by upstream `zg --index --graph`
//! (origin/main @7504720), it projects their SQLite graph plus the zvec
//! entity/file collections into a [`CodeGraphArtifact`] our
//! [`CallGraphIndex`](zg_codegraph::CallGraphIndex) can query.
//!
//! What the mapping preserves (and what it cannot):
//!
//! | upstream store | codegraph-v2 |
//! |---|---|
//! | zvec entity id (32-hex, content-hashed) | node id `upstream:<entity_id>` |
//! | file scope endpoint `f{file_id}` | node id `upstream:f{file_id}` (kind `file`) |
//! | `CodeMetadata` symbol_name / scope / signature | node name / qualified_name / signature |
//! | `SymbolType::Function` + scope | node kind `method` |
//! | `SymbolType::Function` without scope | node kind `function` |
//! | payload `source_range` | node range |
//! | files snapshot `content_hash` / `size_bytes` | file sha256 / bytes |
//! | edge kind `calls`/`contains`/`imports`/`implements` | same kind |
//! | edge kind `extends` | kind `inherits` |
//! | edge kind `declares` (internal binding evidence) | dropped |
//! | status `resolved` + `provenance` | `resolved=true`, `resolution=upstream:<provenance>` |
//! | status `pending` + `reference_name`/`candidates` | `resolved=false`, `target_name`, `ambiguous_candidates` |
//! | status `failed` (external-module import refs) | `resolved=false`, `resolution=upstream:failed` |
//!
//! The definite/possible split survives: resolved edges land in the definite
//! call graph, pending edges with recorded candidates land in the possible
//! graph, and upstream's 9-value provenance finally reaches a consumer surface
//! through our `edge.resolution` labels.
//!
//! Not covered (out of scope for this spike): automatic freshness — the reader
//! projects a store someone else built; regenerating the sidecar when the
//! upstream index changes is follow-up work.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zvec_rust::{Collection, CollectionOptions};

use zg_codegraph::{
    CODEGRAPH_RELATION_GENERATION, CODEGRAPH_SCHEMA, CODEGRAPH_VERSION, CallGraphIndex,
    CodeGraphArtifact, CodeGraphEdge, CodeGraphFile, CodeGraphNode, CodeGraphRange, write_codegraph_artifact,
};

/// Prefix for every node id minted by the projection. Upstream entity ids are
/// opaque 32-hex strings and file-scope endpoints are `f{file_id}`; the prefix
/// keeps the projected namespace disjoint from our native `file:`/`symbol:` ids
/// if both are ever loaded side by side.
pub const UPSTREAM_NODE_PREFIX: &str = "upstream:";

/// The index store directory upstream builds inside a workspace.
pub const UPSTREAM_INDEX_DIR: &str = ".zvec-grep/index";

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("upstream store is missing {what}: {path}")]
    MissingStore { what: &'static str, path: PathBuf },
    #[error("cannot read upstream graph database {path}: {source}")]
    GraphDatabase {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("invalid upstream graph row {id}: {reason}")]
    InvalidRow { id: i64, reason: String },
    #[error("zvec store read failed: {0}")]
    Zvec(String),
    #[error("projection I/O while {operation} {path}: {source}")]
    Io {
        operation: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing projected codegraph: {0}")]
    Write(#[from] zg_codegraph::CodeGraphError),
}

pub type ProjectionResult<T> = Result<T, ProjectionError>;

/// Counts describing one projection run, for reporting and tests.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProjectionReport {
    /// Source files recorded in the store that own at least one symbol node.
    pub files: usize,
    /// Entities in the collection, whether or not they carry symbol metadata.
    pub entities: usize,
    /// Symbol nodes emitted (entities with code metadata and a symbol name).
    pub symbol_nodes: usize,
    /// File-scope nodes emitted.
    pub file_nodes: usize,
    /// Edges emitted after kind filtering.
    pub edges: usize,
    /// Edges with upstream status `resolved`.
    pub resolved_edges: usize,
    /// Edges with upstream status `pending`.
    pub pending_edges: usize,
    /// Edges with upstream status `failed`.
    pub failed_edges: usize,
    /// `declares` rows dropped (internal binding evidence, not in our vocabulary).
    pub dropped_declares: usize,
    /// Edge endpoints whose entity was absent from the entities collection.
    pub dangling_endpoints: usize,
}

/// One row of upstream's `edges` table, as the projection sees it. Mirrors the
/// SQLite columns so the mapping is unit-testable without a database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamEdgeRow {
    pub id: i64,
    pub source: String,
    pub target: Option<String>,
    pub kind: String,
    pub status: String,
    pub reference_name: Option<String>,
    pub receiver_name: Option<String>,
    pub line: Option<i64>,
    pub col: Option<i64>,
    pub candidates: Option<String>,
    pub provenance: Option<String>,
}

/// One entity of upstream's entities collection, decoded to the fields the
/// projection needs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct UpstreamEntity {
    entity_id: String,
    file_id: u32,
    metadata: Option<serde_json::Value>,
    source_range: Option<CodeGraphRange>,
}

/// One file of upstream's files collection.
#[derive(Clone, Debug, Eq, PartialEq)]
struct UpstreamFile {
    file_id: u32,
    path: String,
    size_bytes: Option<u64>,
    content_hash: Option<String>,
}

/// Locates the upstream index store for a workspace root
/// (`<root>/.zvec-grep/index`), if one exists.
#[must_use]
pub fn locate_upstream_store(workspace_root: &Path) -> Option<PathBuf> {
    let store = workspace_root.join(".zvec-grep").join("index");
    store.join("graph").join("graph.sqlite").is_file().then_some(store)
}

/// Projects the upstream index store at `store` (the `<workspace>/.zvec-grep/
/// index` directory) into a codegraph-v2 artifact and writes it to `output`
/// (or `<store>/../codegraph-v2.json.zst` when `output` is `None`).
///
/// # Errors
/// Returns an error when the store is missing, unreadable, or holds rows the
/// mapping rejects, or when writing the artifact fails.
pub fn project_upstream_graph(
    store: &Path,
    output: Option<&Path>,
) -> ProjectionResult<(PathBuf, CodeGraphArtifact, ProjectionReport)> {
    let (artifact, report) = build_projection(store)?;
    let default_output = store.join("..").join(zg_codegraph::CODEGRAPH_FILE);
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| normalize_store_output(&default_output));
    let (path, artifact) = write_codegraph_artifact(&output, artifact)?;
    Ok((path, artifact, report))
}

fn normalize_store_output(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

/// Builds the projection in memory: reads the SQLite graph, the entities
/// collection and the files collection, and assembles the artifact.
fn build_projection(store: &Path) -> ProjectionResult<(CodeGraphArtifact, ProjectionReport)> {
    let graph_path = store.join("graph").join("graph.sqlite");
    if !graph_path.is_file() {
        return Err(ProjectionError::MissingStore {
            what: "graph database",
            path: graph_path,
        });
    }
    let files = read_files_collection(&store.join("files"))?;
    let entities = read_entities_collection(&store.join("entities"))?;
    let rows = read_edge_rows(&graph_path)?;

    let mut report = ProjectionReport {
        entities: entities.len(),
        ..ProjectionReport::default()
    };

    let by_file_id: BTreeMap<u32, &UpstreamFile> =
        files.iter().map(|file| (file.file_id, file)).collect();

    // File nodes for every file that owns a symbol, plus files referenced as
    // endpoints (`f{file_id}` import targets).
    let mut file_nodes: BTreeMap<u32, CodeGraphNode> = BTreeMap::new();
    let mut node_files: BTreeMap<String, u32> = BTreeMap::new();

    let mut nodes: Vec<CodeGraphNode> = Vec::new();
    let mut entity_kinds: BTreeMap<String, String> = BTreeMap::new();
    for entity in &entities {
        let Some(metadata) = entity.metadata.as_ref().and_then(code_metadata) else {
            continue;
        };
        let Some(symbol_name) = metadata.symbol_name.clone() else {
            continue;
        };
        let Some(file) = by_file_id.get(&entity.file_id) else {
            continue;
        };
        let kind = symbol_kind(&metadata.symbol_type, metadata.scope.as_deref(), &metadata.language);
        let qualified_name = qualified_name(
            metadata.scope.as_deref(),
            &symbol_name,
            &metadata.language,
        );
        nodes.push(CodeGraphNode {
            id: format!("{UPSTREAM_NODE_PREFIX}{}", entity.entity_id),
            kind: kind.clone(),
            path: Some(file.path.clone()),
            name: symbol_name,
            qualified_name,
            range: entity.source_range.clone(),
            signature: metadata.signature,
        });
        entity_kinds.insert(entity.entity_id.clone(), kind);
        node_files.insert(entity.entity_id.clone(), entity.file_id);
        file_nodes.entry(entity.file_id).or_insert_with(|| file_node(file));
    }
    report.symbol_nodes = nodes.len();

    let mut edges: Vec<CodeGraphEdge> = Vec::new();
    let mut referenced_files: BTreeSet<u32> = BTreeSet::new();
    let known_entities: std::collections::BTreeSet<&str> = entities
        .iter()
        .map(|entity| entity.entity_id.as_str())
        .collect();
    for row in &rows {
        if row.kind == "declares" {
            report.dropped_declares += 1;
            continue;
        }
        let Some(mut edge) = map_edge_row(row) else {
            continue;
        };
        if is_file_endpoint(&row.source) {
            if let Some(file_id) = file_endpoint_id(&row.source) {
                referenced_files.insert(file_id);
            }
        } else if !known_entities.contains(row.source.as_str()) {
            report.dangling_endpoints += 1;
        }
        if let Some(target) = row.target.as_deref() {
            if is_file_endpoint(target) {
                if let Some(file_id) = file_endpoint_id(target) {
                    referenced_files.insert(file_id);
                }
            } else if !known_entities.contains(target) {
                report.dangling_endpoints += 1;
            }
        }
        match row.status.as_str() {
            "resolved" => report.resolved_edges += 1,
            "pending" => report.pending_edges += 1,
            "failed" => report.failed_edges += 1,
            _ => {}
        }
        report.edges += 1;
        if let Some(candidates) = row.candidates.as_deref() {
            edge.ambiguous_candidates = parse_candidates(candidates)
                .iter()
                .map(|candidate| endpoint_node_id(candidate))
                .collect();
        }
        edges.push(edge);
    }

    for file_id in referenced_files {
        if let Some(file) = by_file_id.get(&file_id) {
            file_nodes.entry(file_id).or_insert_with(|| file_node(file));
        }
    }
    let mut file_nodes: Vec<CodeGraphNode> = file_nodes.into_values().collect();
    file_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    report.file_nodes = file_nodes.len();
    nodes.extend(file_nodes);
    nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let symbol_file_ids: std::collections::BTreeSet<u32> =
        node_files.values().copied().collect();
    let mut artifact_files: Vec<CodeGraphFile> = files
        .iter()
        .filter(|file| symbol_file_ids.contains(&file.file_id))
        .map(|file| CodeGraphFile {
            path: file.path.clone(),
            language: language_for_path(&file.path),
            sha256: file.content_hash.clone().unwrap_or_default(),
            bytes: usize::try_from(file.size_bytes.unwrap_or_default()).unwrap_or_default(),
            package: None,
        })
        .collect();
    artifact_files.sort_by(|left, right| left.path.cmp(&right.path));
    report.files = artifact_files.len();

    edges.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.range.as_ref().map_or(0, |range| range.start_line).cmp(&right.range.as_ref().map_or(0, |range| range.start_line)))
    });

    let artifact = CodeGraphArtifact {
        schema: CODEGRAPH_SCHEMA.to_owned(),
        version: CODEGRAPH_VERSION,
        manifest_key: projection_manifest_key(&artifact_files, &edges),
        relation_generation: CODEGRAPH_RELATION_GENERATION,
        go_callfacts_context_sha256: None,
        rust_callfacts_context_sha256: None,
        typescript_callfacts_context_sha256: None,
        python_callfacts_context_sha256: None,
        files: artifact_files,
        nodes,
        edges,
    };
    Ok((artifact, report))
}

/// The symbol facts the projection reads from upstream's `CodeMetadata` JSON.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SymbolFacts {
    symbol_name: Option<String>,
    symbol_type: Option<String>,
    scope: Option<String>,
    signature: Option<String>,
    language: Option<String>,
}

fn code_metadata(metadata: &serde_json::Value) -> Option<SymbolFacts> {
    // Upstream's `EntityMetadata` is serde-tagged: {"kind":"code", ...} or
    // {"kind":"markdown", ...}; only code entities carry symbol facts.
    if metadata.get("kind").and_then(serde_json::Value::as_str) != Some("code") {
        return None;
    }
    let code = metadata;
    Some(SymbolFacts {
        symbol_name: code
            .get("symbol_name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        symbol_type: code
            .get("symbol_type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        scope: code
            .get("scope")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        signature: code
            .get("signature")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        language: code
            .get("language")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

/// Maps upstream's symbol facts to our node kind. Only `function` and `method`
/// nodes are callable in [`CallGraphIndex`]; upstream has no method symbol
/// type, so a scoped function (a Go receiver method, a class member) becomes
/// our `method` and a bare function stays `function`.
fn symbol_kind(symbol_type: &Option<String>, scope: Option<&str>, language: &Option<String>) -> String {
    let is_method = scope.is_some()
        && language.as_deref().map_or(true, |language| {
            // Java/PHP/Python/JavaScript members also arrive as scoped
            // functions; Go methods carry the receiver type as scope.
            !matches!(language, "markdown")
        });
    match symbol_type.as_deref() {
        Some("function") if is_method => "method".to_owned(),
        Some("function") => "function".to_owned(),
        Some("class") => "class".to_owned(),
        Some("interface") => "interface".to_owned(),
        Some("enum") => "enum".to_owned(),
        Some("module") => "module".to_owned(),
        Some("alias") => "alias".to_owned(),
        Some("value") => "value".to_owned(),
        _ => "value".to_owned(),
    }
}

/// Qualified name from upstream's scope breadcrumb. Upstream renders scopes
/// `Type::member` for every language; Go truth identity (and our native
/// sidecar) spells receiver methods `Receiver.Method`, so Go gets the dot.
fn qualified_name(scope: Option<&str>, name: &str, language: &Option<String>) -> Option<String> {
    let scope = scope?;
    let separator = if language.as_deref() == Some("go") {
        "."
    } else {
        "::"
    };
    Some(format!("{scope}{separator}{name}"))
}

/// Maps one edge row. Returns `None` for `declares` rows (internal binding
/// evidence, not in our 14-relation vocabulary). See the crate docs table.
#[must_use]
pub fn map_edge_row(row: &UpstreamEdgeRow) -> Option<CodeGraphEdge> {
    let kind = match row.kind.as_str() {
        "calls" => "calls",
        "contains" => "contains",
        "imports" => "imports",
        "implements" => "implements",
        "extends" => "inherits",
        // `declares` is internal binding evidence, not a relation we publish.
        _ => return None,
    }
    .to_owned();
    let (resolved, resolution) = match row.status.as_str() {
        "resolved" => (
            true,
            format!("upstream:{}", row.provenance.as_deref().unwrap_or("resolved")),
        ),
        "pending" => (false, "upstream:pending".to_owned()),
        "failed" => (false, "upstream:failed".to_owned()),
        _ => return None,
    };
    let target = row.target.as_deref().map(endpoint_node_id);
    Some(CodeGraphEdge {
        kind,
        source: endpoint_node_id(&row.source),
        target,
        target_name: row.reference_name.clone(),
        resolved,
        ambiguous_candidates: Vec::new(),
        resolution: Some(resolution),
        range: edge_range(row.line, row.col),
    })
}

/// Upstream records 1-based lines and 0-based columns but no byte offsets for
/// edge sites; byte offsets are left zero and documented as unavailable.
fn edge_range(line: Option<i64>, col: Option<i64>) -> Option<CodeGraphRange> {
    let line = line?;
    let col = col.unwrap_or_default();
    Some(CodeGraphRange {
        start_byte: 0,
        end_byte: 0,
        start_line: usize::try_from(line).unwrap_or_default(),
        end_line: usize::try_from(line).unwrap_or_default(),
        start_column: usize::try_from(col.max(0)).unwrap_or_default(),
        end_column: usize::try_from(col.max(0)).unwrap_or_default(),
    })
}

/// Ambiguity candidates are stored as a JSON array of upstream endpoint ids.
fn parse_candidates(candidates: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(candidates).unwrap_or_default()
}

/// Upstream endpoints are either 32-hex entity ids or `f{file_id}` file
/// scopes. A file endpoint is `f` followed by decimal digits only; a 32-hex
/// entity id is 32 characters and cannot collide with that shape in practice
/// (it would need 31 leading zeros).
fn is_file_endpoint(endpoint: &str) -> bool {
    file_endpoint_id(endpoint).is_some()
}

fn file_endpoint_id(endpoint: &str) -> Option<u32> {
    let rest = endpoint.strip_prefix('f')?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    rest.parse::<u32>().ok()
}

fn endpoint_node_id(endpoint: &str) -> String {
    format!("{UPSTREAM_NODE_PREFIX}{endpoint}")
}

fn file_node(file: &UpstreamFile) -> CodeGraphNode {
    CodeGraphNode {
        id: format!("{UPSTREAM_NODE_PREFIX}f{}", file.file_id),
        kind: "file".to_owned(),
        path: Some(file.path.clone()),
        name: file.path.clone(),
        qualified_name: None,
        range: None,
        signature: None,
    }
}

fn language_for_path(path: &str) -> String {
    match path.rsplit('.').next().unwrap_or_default() {
        "go" => "go".to_owned(),
        "rs" => "rust".to_owned(),
        "ts" => "typescript".to_owned(),
        "tsx" => "tsx".to_owned(),
        "py" => "python".to_owned(),
        "c" | "h" => "c".to_owned(),
        "cpp" | "cc" | "cxx" | "hpp" => "cpp".to_owned(),
        "java" => "java".to_owned(),
        "js" | "jsx" | "mjs" => "javascript".to_owned(),
        "php" => "php".to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// Deterministic manifest key over the projected files and edges, so equal
/// stores project to byte-equal artifacts (our native key derives from package
/// manifests, which the projected store does not carry).
fn projection_manifest_key(files: &[CodeGraphFile], edges: &[CodeGraphEdge]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.sha256.as_bytes());
        hasher.update([0]);
    }
    for edge in edges {
        hasher.update(edge.kind.as_bytes());
        hasher.update([0]);
        hasher.update(edge.source.as_bytes());
        hasher.update([0]);
        if let Some(target) = edge.target.as_deref() {
            hasher.update(target.as_bytes());
        }
        hasher.update([0]);
        hasher.update(edge.resolution.as_deref().unwrap_or_default().as_bytes());
        hasher.update([0]);
    }
    format!("upstream-projection:{}", hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

// --- store readers ---------------------------------------------------------

fn read_edge_rows(graph_path: &Path) -> ProjectionResult<Vec<UpstreamEdgeRow>> {
    let connection = Connection::open_with_flags(
        graph_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|source| ProjectionError::GraphDatabase {
        path: graph_path.to_path_buf(),
        source,
    })?;
    let mut statement = connection
        .prepare(
            "SELECT id, source, target, kind, status, reference_name, receiver_name,
                    line, col, candidates, provenance
             FROM edges ORDER BY id",
        )
        .map_err(|source| ProjectionError::GraphDatabase {
            path: graph_path.to_path_buf(),
            source,
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok(UpstreamEdgeRow {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                kind: row.get(3)?,
                status: row.get(4)?,
                reference_name: row.get(5)?,
                receiver_name: row.get(6)?,
                line: row.get(7)?,
                col: row.get(8)?,
                candidates: row.get(9)?,
                provenance: row.get(10)?,
            })
        })
        .map_err(|source| ProjectionError::GraphDatabase {
            path: graph_path.to_path_buf(),
            source,
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| ProjectionError::GraphDatabase {
            path: graph_path.to_path_buf(),
            source,
        })
}

fn open_collection_read_only(path: &Path, what: &'static str) -> ProjectionResult<Collection> {
    if !path.is_dir() {
        return Err(ProjectionError::MissingStore {
            what,
            path: path.to_path_buf(),
        });
    }
    zvec_init()?;
    let text = path
        .to_str()
        .ok_or_else(|| ProjectionError::Zvec(format!("{what} path is not UTF-8: {}", path.display())))?
        .to_owned();
    let mut options =
        CollectionOptions::new().map_err(|error| ProjectionError::Zvec(error.to_string()))?;
    options
        .set_read_only(true)
        .map_err(|error| ProjectionError::Zvec(error.to_string()))?;
    Collection::open(&text, Some(&options))
        .map_err(|error| ProjectionError::Zvec(format!("open {what}: {error}")))
}

fn zvec_init() -> ProjectionResult<()> {
    zvec_rust::initialize(None).or_else(|error| {
        // The zvec-rust-build helper initializes the native library before
        // main for binaries that link it; a second initialization is success.
        if error.code == zvec_rust::ErrorCode::AlreadyExists {
            Ok(())
        } else {
            Err(ProjectionError::Zvec(error.to_string()))
        }
    })
}

fn iterate_collection(
    collection: &Collection,
    fields: &[&str],
    what: &'static str,
) -> ProjectionResult<Vec<zvec_rust::Doc>> {
    let iterator = collection
        .iter_with_options(Some(fields), false)
        .map_err(|error| ProjectionError::Zvec(format!("iterate {what}: {error}")))?;
    let mut docs = Vec::new();
    for doc in iterator {
        docs.push(doc.map_err(|error| ProjectionError::Zvec(format!("read {what}: {error}")))?);
    }
    Ok(docs)
}

fn doc_string(doc: &zvec_rust::Doc, field: &str, what: &'static str) -> ProjectionResult<String> {
    doc.get_string(field)
        .map_err(|error| ProjectionError::Zvec(format!("read {what} field {field}: {error}")))?
        .ok_or_else(|| ProjectionError::Zvec(format!("{what} record is missing field {field}")))
}

fn doc_u32(doc: &zvec_rust::Doc, field: &str, what: &'static str) -> ProjectionResult<u32> {
    doc.get_u32(field)
        .map_err(|error| ProjectionError::Zvec(format!("read {what} field {field}: {error}")))?
        .ok_or_else(|| ProjectionError::Zvec(format!("{what} record is missing field {field}")))
}

fn read_entities_collection(path: &Path) -> ProjectionResult<Vec<UpstreamEntity>> {
    let collection = open_collection_read_only(path, "entities collection")?;
    let docs = iterate_collection(
        &collection,
        &["entity_id", "file_id", "metadata", "payload"],
        "entities collection",
    )?;
    let mut entities = Vec::with_capacity(docs.len());
    for doc in &docs {
        let entity_id = doc_string(doc, "entity_id", "entities collection")?;
        let file_id = doc_u32(doc, "file_id", "entities collection")?;
        let metadata = if doc.has_field("metadata") && !doc.is_field_null("metadata") {
            let json = doc_string(doc, "metadata", "entities collection")?;
            serde_json::from_str::<serde_json::Value>(&json)
                .map(Some)
                .map_err(|error| {
                    ProjectionError::Zvec(format!("entity {entity_id} has invalid metadata: {error}"))
                })?
        } else {
            None
        };
        let source_range = doc
            .get_string("payload")
            .ok()
            .flatten()
            .and_then(|payload| payload_source_range(&payload));
        entities.push(UpstreamEntity {
            entity_id,
            file_id,
            metadata,
            source_range,
        });
    }
    Ok(entities)
}

/// Extracts the `source_range` of upstream's entity payload JSON
/// (`{"id","file_id","source_range","content","fragments"}`).
fn payload_source_range(payload: &str) -> Option<CodeGraphRange> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum RangePayload {
        Full {},
        Text {
            start_line: usize,
            end_line: usize,
            start_byte_offset: usize,
            end_byte_offset: usize,
            start_byte_column: usize,
            end_byte_column: usize,
        },
    }
    #[derive(Deserialize)]
    struct PayloadRecord {
        source_range: RangePayload,
    }
    let record: PayloadRecord = serde_json::from_str(payload).ok()?;
    match record.source_range {
        RangePayload::Text {
            start_line,
            end_line,
            start_byte_offset,
            end_byte_offset,
            start_byte_column,
            end_byte_column,
        } => Some(CodeGraphRange {
            start_byte: start_byte_offset,
            end_byte: end_byte_offset,
            start_line,
            end_line,
            start_column: start_byte_column,
            end_column: end_byte_column,
        }),
        RangePayload::Full {} => None,
    }
}

fn read_files_collection(path: &Path) -> ProjectionResult<Vec<UpstreamFile>> {
    let collection = open_collection_read_only(path, "files collection")?;
    let docs = iterate_collection(
        &collection,
        &["file_id", "path", "relative_path", "payload"],
        "files collection",
    )?;
    let mut files = Vec::with_capacity(docs.len());
    for doc in &docs {
        let file_id = doc_u32(doc, "file_id", "files collection")?;
        // `relative_path` is the Unicode projection of the native path; the
        // `path` field encodes the native path as JSON components and is only
        // a fallback.
        let path = doc
            .get_string("relative_path")
            .ok()
            .flatten()
            .or_else(|| doc.get_string("path").ok().flatten())
            .and_then(|value| {
                if value.starts_with('{') {
                    decode_native_path(&value)
                } else {
                    Some(value)
                }
            })
            .ok_or_else(|| {
                ProjectionError::Zvec(format!(
                    "files collection record {file_id} has no usable path"
                ))
            })?;
        let (size_bytes, content_hash) = doc
            .get_string("payload")
            .ok()
            .flatten()
            .and_then(|payload| file_snapshot(&payload))
            .unwrap_or((None, None));
        files.push(UpstreamFile {
            file_id,
            path,
            size_bytes,
            content_hash,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path).then(left.file_id.cmp(&right.file_id)));
    Ok(files)
}

/// Extracts `(size_bytes, content_hash)` from a files payload JSON.
fn file_snapshot(payload: &str) -> Option<(Option<u64>, Option<String>)> {
    #[derive(Deserialize)]
    struct SnapshotRecord {
        size_bytes: Option<u64>,
        content_hash: Option<String>,
    }
    #[derive(Deserialize)]
    struct PayloadRecord {
        snapshot: SnapshotRecord,
    }
    let record: PayloadRecord = serde_json::from_str(payload).ok()?;
    Some((record.snapshot.size_bytes, record.snapshot.content_hash))
}

/// Joins a native path record (`{"components":["internal","main.go"]}`).
fn decode_native_path(value: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct NativePath {
        components: Vec<String>,
    }
    let path: NativePath = serde_json::from_str(value).ok()?;
    Some(path.components.join("/"))
}

/// Convenience wrapper: projects and immediately builds the query index.
///
/// # Errors
/// Returns an error when the projection fails.
pub fn project_upstream_index(
    store: &Path,
) -> ProjectionResult<(CallGraphIndex, ProjectionReport)> {
    let (artifact, report) = build_projection(store)?;
    Ok((CallGraphIndex::new(&artifact), report))
}

/// Ensures a path's parent exists (used by the binary for explicit outputs).
///
/// # Errors
/// Returns an error when directory creation fails.
pub fn ensure_parent(path: &Path) -> ProjectionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ProjectionError::Io {
            operation: "create output directory".to_owned(),
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}
