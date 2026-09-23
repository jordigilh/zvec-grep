//! Deterministic multi-language codegraph sidecar generation.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tree_sitter::{Language, Node, Parser};

mod graph_queries;

pub use graph_queries::{
    CallGraphAssignment, CallGraphBlastRadius, CallGraphCluster, CallGraphClustering,
    CallGraphIndex, CallGraphPath,
};

pub const CODEGRAPH_SCHEMA: &str = "zvec-grep.codegraph";
pub const CODEGRAPH_VERSION: u32 = 1;
pub const CODEGRAPH_FILE: &str = "codegraph-v1.json";

#[derive(Debug, Error)]
pub enum CodeGraphError {
    #[error("codegraph I/O while {operation} {path}: {source}")]
    Io {
        operation: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("source file is not UTF-8: {path}: {source}")]
    Utf8 {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("configure Go parser: {0}")]
    Parser(String),
    #[error("unsupported codegraph source language: {0}")]
    UnsupportedLanguage(PathBuf),
    #[error("parse Go source: {0}")]
    Parse(String),
    #[error("invalid codegraph change path: {0}")]
    InvalidChangePath(PathBuf),
    #[error("base codegraph schema/version is unsupported: {schema} v{version}")]
    IncompatibleArtifact { schema: String, version: u32 },
    #[error("codegraph node was not found: {query}")]
    GraphNodeNotFound { query: String },
    #[error("codegraph node is ambiguous: {query}; candidates: {candidates:?}")]
    GraphNodeAmbiguous {
        query: String,
        candidates: Vec<String>,
    },
    #[error("Leiden clustering failed: {0}")]
    Clustering(String),
    #[error("serialize codegraph: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub type CodeGraphResult<T> = Result<T, CodeGraphError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeGraphArtifact {
    pub schema: String,
    pub version: u32,
    pub manifest_key: String,
    pub files: Vec<CodeGraphFile>,
    pub nodes: Vec<CodeGraphNode>,
    pub edges: Vec<CodeGraphEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodeGraphChange {
    Upsert(PathBuf),
    Delete(PathBuf),
}

/// Filesystem metadata used for cheap in-memory freshness checks between graph queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeGraphSourceStamp {
    byte_len: u64,
    modified_unix_nanos: Option<u128>,
    changed_unix_nanos: Option<i128>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeGraphFile {
    pub path: String,
    #[serde(default = "default_language")]
    pub language: String,
    pub sha256: String,
    pub bytes: usize,
    pub package: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceLanguage {
    Go,
    Rust,
    TypeScript,
    Tsx,
    Python,
}

impl SourceLanguage {
    fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "go" => Some(Self::Go),
            "rs" => Some(Self::Rust),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "py" => Some(Self::Python),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Python => "python",
        }
    }

    fn parser_language(self) -> Language {
        match self {
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }
}

fn default_language() -> String {
    "go".to_owned()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeGraphNode {
    pub id: String,
    pub kind: String,
    pub path: Option<String>,
    pub name: String,
    pub qualified_name: Option<String>,
    pub range: Option<CodeGraphRange>,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeGraphEdge {
    pub kind: String,
    pub source: String,
    pub target: Option<String>,
    pub target_name: Option<String>,
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguous_candidates: Vec<String>,
    pub range: Option<CodeGraphRange>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeGraphRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub start_column: usize,
    pub end_column: usize,
}

#[derive(Clone, Debug)]
struct ParsedFile {
    file: CodeGraphFile,
    file_node_id: String,
    definitions: Vec<Definition>,
    imports: Vec<String>,
    calls: Vec<CallSite>,
}

#[derive(Clone, Debug)]
struct Definition {
    node: CodeGraphNode,
    start_byte: usize,
    end_byte: usize,
    simple_name: String,
    qualified_name: String,
}

struct LanguageCollector<'source> {
    source: &'source [u8],
    language: SourceLanguage,
    path: &'source str,
    scopes: Vec<(String, bool)>,
    definitions: Vec<Definition>,
    imports: BTreeSet<String>,
    calls: Vec<CallSite>,
}

impl<'source> LanguageCollector<'source> {
    fn new(source: &'source [u8], language: SourceLanguage, path: &'source str) -> Self {
        Self {
            source,
            language,
            path,
            scopes: Vec::new(),
            definitions: Vec::new(),
            imports: BTreeSet::new(),
            calls: Vec::new(),
        }
    }

    fn collect(&mut self, node: Node<'_>) {
        if let Some(definition) =
            language_definition(node, self.source, self.language, self.path, &self.scopes)
        {
            self.definitions.push(definition);
        }
        if let Some(import) = language_import(node, self.source, self.language) {
            self.imports.insert(import);
        }
        if let Some(call) = language_call_site(node, self.source, self.language) {
            self.calls.push(call);
        }

        let scope = language_scope(node, self.source, self.language);
        if let Some(scope) = &scope {
            self.scopes.push(scope.clone());
        }
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        for child in children {
            self.collect(child);
        }
        if scope.is_some() {
            self.scopes.pop();
        }
    }
}

#[derive(Clone, Debug)]
struct CallSite {
    start_byte: usize,
    end_byte: usize,
    target_name: String,
    qualified_target: Option<String>,
    range: CodeGraphRange,
}

/// Builds a complete multi-language graph for supported source files below `root`.
///
/// # Errors
///
/// Returns an error if the root cannot be scanned, a supported source file cannot be
/// read or parsed, or the resulting artifact cannot be represented.
pub fn build_codegraph(root: &Path) -> CodeGraphResult<CodeGraphArtifact> {
    let root = resolve_root(root)?;
    let mut paths = Vec::new();
    collect_code_files(&root, &mut paths)?;
    paths.sort();
    let parsed = paths
        .iter()
        .map(|path| parse_file(&root, path))
        .collect::<CodeGraphResult<Vec<_>>>()?;
    Ok(build_artifact(&parsed))
}

/// Builds a Go-only graph for existing callers that need the original scope.
///
/// # Errors
///
/// Returns an error if the root cannot be scanned or a Go source file cannot be
/// read or parsed.
pub fn build_go_codegraph(root: &Path) -> CodeGraphResult<CodeGraphArtifact> {
    let root = resolve_root(root)?;
    let mut paths = Vec::new();
    collect_code_files(&root, &mut paths)?;
    paths.retain(|path| SourceLanguage::from_path(path) == Some(SourceLanguage::Go));
    paths.sort();
    let parsed = paths
        .iter()
        .map(|path| parse_file(&root, path))
        .collect::<CodeGraphResult<Vec<_>>>()?;
    Ok(build_artifact(&parsed))
}

/// Applies changed, deleted, and renamed files to a previous graph snapshot.
/// Only upserted files are parsed; existing call edges are re-resolved against
/// the resulting definition set so deletions and renames cannot leave stale targets.
///
/// # Errors
///
/// Returns an error if the base artifact has an unsupported schema/version, a
/// changed path is invalid or escapes the root, or a source file cannot be read
/// or parsed.
pub fn update_codegraph(
    base: &CodeGraphArtifact,
    root: &Path,
    changes: &[CodeGraphChange],
) -> CodeGraphResult<CodeGraphArtifact> {
    if base.schema != CODEGRAPH_SCHEMA || base.version != CODEGRAPH_VERSION {
        return Err(CodeGraphError::IncompatibleArtifact {
            schema: base.schema.clone(),
            version: base.version,
        });
    }
    let root = resolve_root(root)?;
    let mut changed_paths = BTreeSet::new();
    let mut upsert_paths = BTreeSet::new();
    for change in changes {
        let (path, upsert) = match change {
            CodeGraphChange::Upsert(path) => (path, true),
            CodeGraphChange::Delete(path) => (path, false),
        };
        let path = normalize_change_path(path)?;
        changed_paths.insert(path.clone());
        if upsert {
            upsert_paths.insert(path);
        } else {
            upsert_paths.remove(&path);
        }
    }

    let mut parsed = Vec::with_capacity(upsert_paths.len());
    for relative in upsert_paths {
        let path = root.join(&relative);
        let canonical = path
            .canonicalize()
            .map_err(|error| io_failure("resolve changed source", &path, error))?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err(CodeGraphError::InvalidChangePath(PathBuf::from(relative)));
        }
        parsed.push(parse_file(&root, &canonical)?);
    }
    let changed_artifact = build_artifact(&parsed);
    Ok(merge_codegraph_delta(
        base,
        &changed_paths,
        changed_artifact,
    ))
}

/// Applies a source-file delta to a previous graph snapshot.
///
/// Kept as a compatibility alias for early Go-sidecar callers.
///
/// # Errors
///
/// Returns an error if the artifact is incompatible, a changed path escapes
/// the root, or a changed source file cannot be read or parsed.
pub fn update_go_codegraph(
    base: &CodeGraphArtifact,
    root: &Path,
    changes: &[CodeGraphChange],
) -> CodeGraphResult<CodeGraphArtifact> {
    update_codegraph(base, root, changes)
}

fn merge_codegraph_delta(
    base: &CodeGraphArtifact,
    changed_paths: &BTreeSet<String>,
    changed_artifact: CodeGraphArtifact,
) -> CodeGraphArtifact {
    let mut files = base
        .files
        .iter()
        .filter(|file| !changed_paths.contains(&file.path))
        .cloned()
        .chain(changed_artifact.files)
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut nodes = BTreeMap::new();
    for node in base
        .nodes
        .iter()
        .filter(|node| {
            node.path
                .as_ref()
                .is_none_or(|path| !changed_paths.contains(path))
        })
        .chain(changed_artifact.nodes.iter())
    {
        nodes.insert(node.id.clone(), node.clone());
    }
    let mut nodes = nodes.into_values().collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let valid_node_ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    let changed_source_ids = base
        .nodes
        .iter()
        .filter(|node| {
            node.path
                .as_ref()
                .is_some_and(|path| changed_paths.contains(path))
        })
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();

    let mut edges = base
        .edges
        .iter()
        .filter(|edge| {
            valid_node_ids.contains(edge.source.as_str())
                && !changed_source_ids.contains(&edge.source)
                && (edge.kind == "calls"
                    || edge
                        .target
                        .as_deref()
                        .is_none_or(|target| valid_node_ids.contains(target)))
        })
        .cloned()
        .chain(changed_artifact.edges)
        .collect::<Vec<_>>();
    resolve_call_edges(&mut edges, &nodes);

    let imported_packages = edges
        .iter()
        .filter(|edge| edge.kind == "imports")
        .filter_map(|edge| edge.target.as_deref())
        .collect::<BTreeSet<_>>();
    nodes.retain(|node| node.kind != "package" || imported_packages.contains(node.id.as_str()));
    let valid_node_ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    edges.retain(|edge| {
        valid_node_ids.contains(edge.source.as_str())
            && edge
                .target
                .as_deref()
                .is_none_or(|target| valid_node_ids.contains(target))
    });
    sort_graph(&mut nodes, &mut edges);

    CodeGraphArtifact {
        schema: CODEGRAPH_SCHEMA.to_owned(),
        version: CODEGRAPH_VERSION,
        manifest_key: manifest_key(&files),
        files,
        nodes,
        edges,
    }
}

fn resolve_root(root: &Path) -> CodeGraphResult<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|error| io_failure("resolve codegraph root", root, error))?;
    if !root.is_dir() {
        return Err(CodeGraphError::Io {
            operation: "open codegraph root".to_owned(),
            path: root,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "codegraph root is not a directory",
            ),
        });
    }
    Ok(root)
}

fn normalize_change_path(path: &Path) -> CodeGraphResult<String> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
        || SourceLanguage::from_path(path).is_none()
    {
        return Err(CodeGraphError::InvalidChangePath(path.to_path_buf()));
    }
    let normalized = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        return Err(CodeGraphError::InvalidChangePath(path.to_path_buf()));
    }
    Ok(normalized)
}

fn build_artifact(parsed: &[ParsedFile]) -> CodeGraphArtifact {
    let files = parsed
        .iter()
        .map(|file| file.file.clone())
        .collect::<Vec<_>>();
    let manifest_key = manifest_key(&files);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut package_nodes = BTreeMap::new();

    for file in parsed {
        nodes.push(CodeGraphNode {
            id: file.file_node_id.clone(),
            kind: "file".to_owned(),
            path: Some(file.file.path.clone()),
            name: file.file.path.clone(),
            qualified_name: None,
            range: None,
            signature: None,
        });
        for definition in &file.definitions {
            nodes.push(definition.node.clone());
            edges.push(CodeGraphEdge {
                kind: "defines".to_owned(),
                source: file.file_node_id.clone(),
                target: Some(definition.node.id.clone()),
                target_name: Some(definition.qualified_name.clone()),
                resolved: true,
                ambiguous_candidates: Vec::new(),
                range: definition.node.range.clone(),
            });
        }
        for import in &file.imports {
            let package_id = package_node_id(import);
            package_nodes
                .entry(import.clone())
                .or_insert_with(|| CodeGraphNode {
                    id: package_id.clone(),
                    kind: "package".to_owned(),
                    path: None,
                    name: import.clone(),
                    qualified_name: Some(import.clone()),
                    range: None,
                    signature: None,
                });
            edges.push(CodeGraphEdge {
                kind: "imports".to_owned(),
                source: file.file_node_id.clone(),
                target: Some(package_id),
                target_name: Some(import.clone()),
                resolved: true,
                ambiguous_candidates: Vec::new(),
                range: None,
            });
        }
    }
    nodes.extend(package_nodes.into_values());

    for file in parsed {
        for call in &file.calls {
            let owner = file
                .definitions
                .iter()
                .filter(|definition| {
                    definition.start_byte <= call.start_byte && call.end_byte <= definition.end_byte
                })
                .min_by_key(|definition| definition.end_byte - definition.start_byte)
                .map_or_else(
                    || file.file_node_id.clone(),
                    |definition| definition.node.id.clone(),
                );
            edges.push(CodeGraphEdge {
                kind: "calls".to_owned(),
                source: owner,
                target: None,
                target_name: Some(
                    call.qualified_target
                        .clone()
                        .unwrap_or_else(|| call.target_name.clone()),
                ),
                resolved: false,
                ambiguous_candidates: Vec::new(),
                range: Some(call.range.clone()),
            });
        }
    }

    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    resolve_call_edges(&mut edges, &nodes);
    sort_graph(&mut nodes, &mut edges);

    CodeGraphArtifact {
        schema: CODEGRAPH_SCHEMA.to_owned(),
        version: CODEGRAPH_VERSION,
        manifest_key,
        files,
        nodes,
        edges,
    }
}

fn sort_graph(nodes: &mut Vec<CodeGraphNode>, edges: &mut [CodeGraphEdge]) {
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    nodes.dedup_by(|left, right| left.id == right.id);
    edges.sort_by(|left, right| {
        (
            left.source.as_str(),
            left.kind.as_str(),
            left.target.as_deref().unwrap_or_default(),
            left.target_name.as_deref().unwrap_or_default(),
            left.range.as_ref().map_or(0, |range| range.start_byte),
            left.range.as_ref().map_or(0, |range| range.end_byte),
        )
            .cmp(&(
                right.source.as_str(),
                right.kind.as_str(),
                right.target.as_deref().unwrap_or_default(),
                right.target_name.as_deref().unwrap_or_default(),
                right.range.as_ref().map_or(0, |range| range.start_byte),
                right.range.as_ref().map_or(0, |range| range.end_byte),
            ))
    });
}

fn resolve_call_edges(edges: &mut [CodeGraphEdge], nodes: &[CodeGraphNode]) {
    let definitions = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "function" | "method"))
        .filter_map(|node| {
            let qualified_name = node.qualified_name.clone()?;
            Some(Definition {
                node: node.clone(),
                start_byte: 0,
                end_byte: 0,
                simple_name: node.name.clone(),
                qualified_name,
            })
        })
        .collect::<Vec<_>>();
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut by_name: HashMap<&str, Vec<&Definition>> = HashMap::new();
    for definition in &definitions {
        by_name
            .entry(definition.simple_name.as_str())
            .or_default()
            .push(definition);
    }

    for edge in edges.iter_mut().filter(|edge| edge.kind == "calls") {
        let Some(target_name) = edge.target_name.as_deref() else {
            edge.target = None;
            edge.resolved = false;
            continue;
        };
        let simple_name = target_name
            .rsplit("::")
            .next()
            .unwrap_or(target_name)
            .rsplit('.')
            .next()
            .unwrap_or(target_name);
        let call = CallSite {
            start_byte: edge.range.as_ref().map_or(0, |range| range.start_byte),
            end_byte: edge.range.as_ref().map_or(0, |range| range.end_byte),
            target_name: simple_name.to_owned(),
            qualified_target: target_name.contains('.').then(|| target_name.to_owned()),
            range: edge.range.clone().unwrap_or(CodeGraphRange {
                start_byte: 0,
                end_byte: 0,
                start_line: 0,
                end_line: 0,
                start_column: 0,
                end_column: 0,
            }),
        };
        let caller_path = nodes_by_id
            .get(edge.source.as_str())
            .and_then(|node| node.path.as_deref());
        let (target, ambiguous_candidates) = resolve_call(&call, caller_path, &by_name);
        edge.target = target.map(|definition| definition.node.id.clone());
        edge.resolved = edge.target.is_some();
        edge.ambiguous_candidates = ambiguous_candidates;
    }
}

/// Writes a graph beside the semantic index and returns the artifact and path.
///
/// # Errors
///
/// Returns an error if scanning, parsing, serialization, or writing fails.
pub fn write_codegraph(
    root: &Path,
    output: Option<&Path>,
) -> CodeGraphResult<(PathBuf, CodeGraphArtifact)> {
    let artifact = build_codegraph(root)?;
    write_artifact(root, output, artifact)
}

/// Writes a Go-only graph for existing callers that need the original scope.
///
/// # Errors
///
/// Returns an error if scanning, parsing, serialization, or writing fails.
pub fn write_go_codegraph(
    root: &Path,
    output: Option<&Path>,
) -> CodeGraphResult<(PathBuf, CodeGraphArtifact)> {
    let artifact = build_go_codegraph(root)?;
    write_artifact(root, output, artifact)
}

/// Applies a file delta to a base artifact and writes the updated snapshot.
///
/// # Errors
///
/// Returns an error if applying the update, serialization, or writing fails.
pub fn write_go_codegraph_update(
    base: &CodeGraphArtifact,
    root: &Path,
    output: Option<&Path>,
    changes: &[CodeGraphChange],
) -> CodeGraphResult<(PathBuf, CodeGraphArtifact)> {
    let artifact = update_go_codegraph(base, root, changes)?;
    write_artifact(root, output, artifact)
}

/// Applies a source-file delta to a base artifact and writes the updated snapshot.
///
/// # Errors
///
/// Returns an error if applying the update, serialization, or writing fails.
pub fn write_codegraph_update(
    base: &CodeGraphArtifact,
    root: &Path,
    output: Option<&Path>,
    changes: &[CodeGraphChange],
) -> CodeGraphResult<(PathBuf, CodeGraphArtifact)> {
    let artifact = update_codegraph(base, root, changes)?;
    write_artifact(root, output, artifact)
}

/// Refreshes the persisted graph snapshot against all supported source files.
/// Existing artifacts are updated only for added, changed, or deleted files.
///
/// # Errors
///
/// Returns an error if the workspace cannot be scanned, a changed source cannot
/// be parsed, or the refreshed artifact cannot be persisted.
pub fn refresh_codegraph(root: &Path) -> CodeGraphResult<(PathBuf, CodeGraphArtifact)> {
    let root = resolve_root(root)?;
    let artifact_path = root.join(".zvec-grep").join(CODEGRAPH_FILE);
    let base = match fs::read(&artifact_path) {
        Ok(bytes) => serde_json::from_slice::<CodeGraphArtifact>(&bytes)
            .ok()
            .filter(|artifact| {
                artifact.schema == CODEGRAPH_SCHEMA && artifact.version == CODEGRAPH_VERSION
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(io_failure("read existing codegraph", &artifact_path, error));
        }
    };

    let mut paths = Vec::new();
    collect_code_files(&root, &mut paths)?;
    paths.sort();
    let mut current = BTreeMap::new();
    for path in paths {
        let bytes = fs::read(&path).map_err(|error| io_failure("hash source", &path, error))?;
        current.insert(relative_path(&root, &path), sha256_hex(&bytes));
    }

    let artifact = if let Some(base) = base {
        let previous = base
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.sha256.as_str()))
            .collect::<HashMap<_, _>>();
        let changes = current
            .iter()
            .filter(|(path, hash)| previous.get(path.as_str()) != Some(&hash.as_str()))
            .map(|(path, _)| CodeGraphChange::Upsert(PathBuf::from(path)))
            .chain(
                base.files
                    .iter()
                    .filter(|file| !current.contains_key(&file.path))
                    .map(|file| CodeGraphChange::Delete(PathBuf::from(&file.path))),
            )
            .collect::<Vec<_>>();
        if changes.is_empty() {
            return Ok((artifact_path, base));
        }
        update_codegraph(&base, &root, &changes)?
    } else {
        build_codegraph(&root)?
    };
    write_artifact(&root, Some(&artifact_path), artifact)
}

fn write_artifact(
    root: &Path,
    output: Option<&Path>,
    artifact: CodeGraphArtifact,
) -> CodeGraphResult<(PathBuf, CodeGraphArtifact)> {
    let output = output.map_or_else(
        || root.join(".zvec-grep").join(CODEGRAPH_FILE),
        Path::to_path_buf,
    );
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| io_failure("create codegraph output directory", parent, error))?;
    }
    let encoded = serde_json::to_vec(&artifact)?;
    fs::write(&output, encoded)
        .map_err(|error| io_failure("write codegraph artifact", &output, error))?;
    Ok((output, artifact))
}

fn collect_code_files(root: &Path, output: &mut Vec<PathBuf>) -> CodeGraphResult<()> {
    let entries =
        fs::read_dir(root).map_err(|error| io_failure("scan codegraph root", root, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| io_failure("read codegraph directory entry", root, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect codegraph path", &path, error))?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if name == ".git" || name == ".zvec-grep" || name == "node_modules" {
                continue;
            }
            collect_code_files(&path, output)?;
        } else if file_type.is_file() && SourceLanguage::from_path(&path).is_some() {
            output.push(path);
        }
    }
    Ok(())
}

/// Collects inexpensive change stamps for supported source files under `root`.
/// Callers may compare these with a previous snapshot before hashing or parsing
/// file contents.
///
/// # Errors
///
/// Returns an error if the root or any source directory/file cannot be read.
pub fn codegraph_source_stamps(
    root: &Path,
) -> CodeGraphResult<BTreeMap<String, CodeGraphSourceStamp>> {
    let root = resolve_root(root)?;
    let mut paths = Vec::new();
    collect_code_files(&root, &mut paths)?;
    let mut stamps = BTreeMap::new();
    for path in paths {
        let metadata =
            fs::metadata(&path).map_err(|error| io_failure("stat source", &path, error))?;
        let modified_unix_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());
        #[cfg(unix)]
        let changed_unix_nanos =
            Some(i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()));
        #[cfg(not(unix))]
        let changed_unix_nanos = None;

        stamps.insert(
            relative_path(&root, &path),
            CodeGraphSourceStamp {
                byte_len: metadata.len(),
                modified_unix_nanos,
                changed_unix_nanos,
            },
        );
    }
    Ok(stamps)
}

fn parse_file(root: &Path, path: &Path) -> CodeGraphResult<ParsedFile> {
    let language = SourceLanguage::from_path(path)
        .ok_or_else(|| CodeGraphError::UnsupportedLanguage(path.to_path_buf()))?;
    let bytes = fs::read(path).map_err(|error| io_failure("read source", path, error))?;
    let text = String::from_utf8(bytes.clone()).map_err(|source| CodeGraphError::Utf8 {
        path: path.to_path_buf(),
        source,
    })?;
    let relative_path = relative_path(root, path);
    let source_hash = sha256_hex(&bytes);
    let mut parser = Parser::new();
    let grammar = language.parser_language();
    parser
        .set_language(&grammar)
        .map_err(|error| CodeGraphError::Parser(error.to_string()))?;
    let tree = parser
        .parse(&text, None)
        .ok_or_else(|| CodeGraphError::Parse(path.display().to_string()))?;
    let package = if language == SourceLanguage::Go {
        package_name(tree.root_node(), text.as_bytes())
    } else {
        Some(module_name(&relative_path))
    };
    let file_node_id = file_node_id(&relative_path);
    let mut definitions = Vec::new();
    let mut imports = BTreeSet::new();
    let mut calls = Vec::new();
    if language == SourceLanguage::Go {
        collect_file_data(
            tree.root_node(),
            text.as_bytes(),
            package.as_deref().unwrap_or("_"),
            &relative_path,
            &mut definitions,
            &mut imports,
            &mut calls,
        );
    } else {
        let mut collector = LanguageCollector::new(text.as_bytes(), language, &relative_path);
        collector.collect(tree.root_node());
        definitions = collector.definitions;
        imports = collector.imports;
        calls = collector.calls;
    }
    Ok(ParsedFile {
        file: CodeGraphFile {
            path: relative_path,
            language: language.name().to_owned(),
            sha256: source_hash,
            bytes: bytes.len(),
            package,
        },
        file_node_id,
        definitions,
        imports: imports.into_iter().collect(),
        calls,
    })
}

fn collect_file_data(
    node: Node<'_>,
    source: &[u8],
    package: &str,
    path: &str,
    definitions: &mut Vec<Definition>,
    imports: &mut BTreeSet<String>,
    calls: &mut Vec<CallSite>,
) {
    match node.kind() {
        "function_declaration" => {
            if let Some(definition) = definition(node, source, package, path, "function") {
                definitions.push(definition);
            }
        }
        "method_declaration" => {
            if let Some(definition) = definition(node, source, package, path, "method") {
                definitions.push(definition);
            }
        }
        "type_spec" => {
            if is_package_level_spec(node)
                && let Some(definition) = definition(node, source, package, path, "type")
            {
                definitions.push(definition);
            }
        }
        "var_spec" => {
            if is_package_level_spec(node)
                && let Some(definition) = definition(node, source, package, path, "variable")
            {
                definitions.push(definition);
            }
        }
        "const_spec" => {
            if is_package_level_spec(node)
                && let Some(definition) = definition(node, source, package, path, "constant")
            {
                definitions.push(definition);
            }
        }
        "import_spec" => {
            if let Some(import) = import_path(node, source) {
                imports.insert(import);
            }
        }
        "call_expression" => {
            if let Some(call) = call_site(node, source) {
                calls.push(call);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_file_data(child, source, package, path, definitions, imports, calls);
    }
}

fn module_name(path: &str) -> String {
    Path::new(path)
        .with_extension("")
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

fn language_definition(
    node: Node<'_>,
    source: &[u8],
    language: SourceLanguage,
    path: &str,
    scopes: &[(String, bool)],
) -> Option<Definition> {
    let (kind, name) = match (language, node.kind()) {
        (SourceLanguage::Rust, "function_item") => (
            if scopes.iter().rev().any(|(_, class_like)| *class_like) {
                "method"
            } else {
                "function"
            },
            node.child_by_field_name("name")?,
        ),
        (SourceLanguage::Rust, "struct_item" | "enum_item") => {
            ("type", node.child_by_field_name("name")?)
        }
        (SourceLanguage::Rust, "trait_item") => ("interface", node.child_by_field_name("name")?),
        (SourceLanguage::Rust, "type_item") => ("alias", node.child_by_field_name("name")?),
        (SourceLanguage::Rust, "const_item") => ("constant", node.child_by_field_name("name")?),
        (SourceLanguage::Rust, "static_item") => ("variable", node.child_by_field_name("name")?),
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "function_declaration") => {
            ("function", node.child_by_field_name("name")?)
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "method_definition") => {
            ("method", node.child_by_field_name("name")?)
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "class_declaration")
        | (SourceLanguage::Python, "class_definition") => {
            ("class", node.child_by_field_name("name")?)
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "interface_declaration") => {
            ("interface", node.child_by_field_name("name")?)
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "enum_declaration") => {
            ("enum", node.child_by_field_name("name")?)
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "type_alias_declaration") => {
            ("alias", node.child_by_field_name("name")?)
        }
        (SourceLanguage::Python, "function_definition") => (
            if scopes.iter().rev().any(|(_, class_like)| *class_like) {
                "method"
            } else {
                "function"
            },
            node.child_by_field_name("name")?,
        ),
        _ => return None,
    };
    let name = node_text(name, source)
        .trim_matches(['\'', '"', '`'])
        .to_owned();
    if name.is_empty() || name == "_" {
        return None;
    }
    let qualified_name = scopes
        .iter()
        .map(|(scope, _)| scope.as_str())
        .chain(std::iter::once(name.as_str()))
        .collect::<Vec<_>>()
        .join(".");
    let range = graph_range(node);
    let signature = node
        .child_by_field_name("body")
        .and_then(|body| source.get(node.start_byte()..body.start_byte()))
        .and_then(|signature| std::str::from_utf8(signature).ok())
        .map(str::trim)
        .filter(|signature| !signature.is_empty())
        .map(str::to_owned);
    Some(Definition {
        node: CodeGraphNode {
            id: symbol_node_id(kind, path, &qualified_name),
            kind: kind.to_owned(),
            path: Some(path.to_owned()),
            name: name.clone(),
            qualified_name: Some(qualified_name.clone()),
            range: Some(range),
            signature,
        },
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        simple_name: name,
        qualified_name,
    })
}

fn language_scope(
    node: Node<'_>,
    source: &[u8],
    language: SourceLanguage,
) -> Option<(String, bool)> {
    let (field, class_like) = match (language, node.kind()) {
        (SourceLanguage::Rust, "impl_item") => ("type", true),
        (SourceLanguage::Rust, "trait_item")
        | (SourceLanguage::TypeScript | SourceLanguage::Tsx, "class_declaration")
        | (SourceLanguage::Python, "class_definition") => ("name", true),
        (SourceLanguage::Rust, "mod_item" | "function_item")
        | (
            SourceLanguage::TypeScript | SourceLanguage::Tsx,
            "function_declaration" | "method_definition",
        )
        | (SourceLanguage::Python, "function_definition") => ("name", false),
        _ => return None,
    };
    let name = node.child_by_field_name(field).map(|name| {
        node_text(name, source)
            .trim_matches(['\'', '"', '`'])
            .to_owned()
    })?;
    (!name.is_empty()).then_some((name, class_like))
}

fn language_import(node: Node<'_>, source: &[u8], language: SourceLanguage) -> Option<String> {
    let text = match (language, node.kind()) {
        (SourceLanguage::Rust, "use_declaration") => {
            node_text(node, source).trim().strip_prefix("use")?.trim()
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "import_statement") => {
            node.child_by_field_name("source").map_or_else(
                || node_text(node, source),
                |source_node| node_text(source_node, source),
            )
        }
        (SourceLanguage::Python, "import_from_statement") => {
            node.child_by_field_name("module").map_or_else(
                || node_text(node, source),
                |module| node_text(module, source),
            )
        }
        (SourceLanguage::Python, "import_statement") => node_text(node, source),
        _ => return None,
    };
    let import = text
        .trim()
        .trim_end_matches(';')
        .trim_matches(['\'', '"', '`'])
        .trim();
    (!import.is_empty()).then(|| import.to_owned())
}

fn language_call_site(node: Node<'_>, source: &[u8], language: SourceLanguage) -> Option<CallSite> {
    let target = match (language, node.kind()) {
        (SourceLanguage::Rust, "method_call_expression") => {
            let receiver = node.child_by_field_name("receiver")?;
            let name = node.child_by_field_name("name")?;
            format!(
                "{}.{}",
                node_text(receiver, source),
                node_text(name, source)
            )
        }
        (
            SourceLanguage::Rust | SourceLanguage::TypeScript | SourceLanguage::Tsx,
            "call_expression",
        )
        | (SourceLanguage::Python, "call") => {
            let function = node.child_by_field_name("function")?;
            node_text(function, source).to_owned()
        }
        _ => return None,
    };
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let simple_name = target
        .rsplit("::")
        .next()
        .unwrap_or(target)
        .rsplit('.')
        .next()
        .unwrap_or(target)
        .split('<')
        .next()
        .unwrap_or(target)
        .trim()
        .to_owned();
    if simple_name.is_empty() {
        return None;
    }
    Some(CallSite {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        target_name: simple_name.clone(),
        qualified_target: (target != simple_name).then(|| target.to_owned()),
        range: graph_range(node),
    })
}

fn is_package_level_spec(node: Node<'_>) -> bool {
    node.parent()
        .and_then(|declaration| declaration.parent())
        .is_some_and(|parent| parent.kind() == "source_file")
}

fn definition(
    node: Node<'_>,
    source: &[u8],
    package: &str,
    path: &str,
    kind: &str,
) -> Option<Definition> {
    let name = node
        .child_by_field_name("name")
        .map(|name| node_text(name, source).to_owned())
        .filter(|name| !name.is_empty() && name != "_")?;
    let qualified_name = match kind {
        "method" => {
            let receiver = node
                .child_by_field_name("receiver")
                .and_then(|receiver| receiver_type(receiver, source))?;
            format!("{package}.{receiver}.{name}")
        }
        _ => format!("{package}.{name}"),
    };
    let range = graph_range(node);
    let signature = node
        .child_by_field_name("body")
        .and_then(|body| source.get(node.start_byte()..body.start_byte()))
        .and_then(|signature| std::str::from_utf8(signature).ok())
        .map(str::trim)
        .filter(|signature| !signature.is_empty())
        .map(str::to_owned);
    let id = symbol_node_id(kind, path, &qualified_name);
    Some(Definition {
        node: CodeGraphNode {
            id,
            kind: kind.to_owned(),
            path: Some(path.to_owned()),
            name: name.clone(),
            qualified_name: Some(qualified_name.clone()),
            range: Some(range),
            signature,
        },
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        simple_name: name,
        qualified_name,
    })
}

fn call_site(node: Node<'_>, source: &[u8]) -> Option<CallSite> {
    let function = node.child_by_field_name("function")?;
    let target_name = call_symbol_name(function, source)?;
    if target_name.is_empty() {
        return None;
    }
    let qualified_target =
        qualified_call_name(function, source).filter(|qualified| qualified != &target_name);
    Some(CallSite {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        target_name,
        qualified_target,
        range: graph_range(node),
    })
}

fn call_symbol_name(function: Node<'_>, source: &[u8]) -> Option<String> {
    match function.kind() {
        "identifier" => Some(node_text(function, source).to_owned()),
        "selector_expression" => function
            .child_by_field_name("field")
            .map(|field| node_text(field, source).to_owned()),
        "index_expression" => function
            .child_by_field_name("operand")
            .and_then(|operand| call_symbol_name(operand, source)),
        "parenthesized_expression" => named_children(function)
            .into_iter()
            .next()
            .and_then(|expression| call_symbol_name(expression, source)),
        _ => None,
    }
}

fn qualified_call_name(function: Node<'_>, source: &[u8]) -> Option<String> {
    match function.kind() {
        "identifier" => Some(node_text(function, source).to_owned()),
        "selector_expression" => {
            let operand = function.child_by_field_name("operand")?;
            let field = function.child_by_field_name("field")?;
            Some(format!(
                "{}.{}",
                qualified_call_name(operand, source)?,
                node_text(field, source)
            ))
        }
        "index_expression" => function
            .child_by_field_name("operand")
            .and_then(|operand| qualified_call_name(operand, source)),
        "parenthesized_expression" => named_children(function)
            .into_iter()
            .next()
            .and_then(|expression| qualified_call_name(expression, source)),
        _ => None,
    }
}

fn import_path(node: Node<'_>, source: &[u8]) -> Option<String> {
    let path = node.child_by_field_name("path").or_else(|| {
        named_children(node).into_iter().find(|child| {
            matches!(
                child.kind(),
                "interpreted_string_literal" | "raw_string_literal"
            )
        })
    })?;
    let value = node_text(path, source).trim();
    if value.len() >= 2 && (value.starts_with('"') || value.starts_with('`')) {
        Some(value[1..value.len() - 1].to_owned())
    } else {
        None
    }
}

fn package_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "package_clause" {
        return node
            .child_by_field_name("name")
            .map(|name| node_text(name, source).to_owned())
            .or_else(|| {
                named_children(node)
                    .into_iter()
                    .find(|child| child.kind() == "package_identifier")
                    .map(|name| node_text(name, source).to_owned())
            });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(package) = package_name(child, source) {
            return Some(package);
        }
    }
    None
}

fn receiver_type(node: Node<'_>, source: &[u8]) -> Option<String> {
    let receiver = node
        .child_by_field_name("type")
        .map_or_else(|| node_text(node, source), |node| node_text(node, source));
    let receiver = receiver
        .trim()
        .trim_start_matches('*')
        .split('[')
        .next()
        .unwrap_or(receiver)
        .trim();
    let receiver = receiver.rsplit('.').next().unwrap_or(receiver);
    (!receiver.is_empty()).then(|| receiver.to_owned())
}

fn resolve_call<'a>(
    call: &CallSite,
    caller_path: Option<&str>,
    by_name: &HashMap<&str, Vec<&'a Definition>>,
) -> (Option<&'a Definition>, Vec<String>) {
    let Some(candidates) = by_name.get(call.target_name.as_str()) else {
        return (None, Vec::new());
    };
    let same_file_candidates = caller_path.map_or_else(Vec::new, |path| {
        candidates
            .iter()
            .copied()
            .filter(|definition| definition.node.path.as_deref() == Some(path))
            .collect::<Vec<_>>()
    });
    let candidates = if same_file_candidates.is_empty() {
        candidates
    } else {
        &same_file_candidates
    };
    match candidates.as_slice() {
        [candidate] => (Some(*candidate), Vec::new()),
        [] => (None, Vec::new()),
        ambiguous => {
            let mut candidate_ids = ambiguous
                .iter()
                .map(|candidate| candidate.node.id.clone())
                .collect::<Vec<_>>();
            candidate_ids.sort();
            (None, candidate_ids)
        }
    }
}

fn manifest_key(files: &[CodeGraphFile]) -> String {
    let mut input = format!("{CODEGRAPH_SCHEMA}\0{CODEGRAPH_VERSION}\0").into_bytes();
    for file in files {
        input.extend_from_slice(file.path.as_bytes());
        input.push(0);
        input.extend_from_slice(file.sha256.as_bytes());
        input.push(0);
    }
    sha256_hex(&input)
}

fn file_node_id(path: &str) -> String {
    format!("file:{}", sha256_hex(path.as_bytes()))
}

fn package_node_id(path: &str) -> String {
    format!("package:{}", sha256_hex(path.as_bytes()))
}

fn symbol_node_id(kind: &str, path: &str, qualified_name: &str) -> String {
    let key = format!("{kind}\0{path}\0{qualified_name}");
    format!("symbol:{}", sha256_hex(key.as_bytes()))
}

fn graph_range(node: Node<'_>) -> CodeGraphRange {
    let start = node.start_position();
    let end = node.end_position();
    CodeGraphRange {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: start.row + 1,
        end_line: end.row + 1,
        start_column: start.column,
        end_column: end.column,
    }
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn node_text<'source>(node: Node<'_>, source: &'source [u8]) -> &'source str {
    node.utf8_text(source).unwrap_or_default()
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("codegraph file belongs to root")
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn io_failure(operation: &str, path: &Path, source: std::io::Error) -> CodeGraphError {
    CodeGraphError::Io {
        operation: operation.to_owned(),
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::Path};

    use tempfile::tempdir;

    use super::{CodeGraphChange, build_go_codegraph, update_go_codegraph, write_go_codegraph};

    fn node_id(artifact: &super::CodeGraphArtifact, kind: &str, name: &str) -> String {
        artifact
            .nodes
            .iter()
            .find(|node| node.kind == kind && node.name == name)
            .map_or_else(
                || panic!("missing {kind} node {name}"),
                |node| node.id.clone(),
            )
    }

    #[test]
    fn builds_manifest_keyed_definitions_imports_and_calls() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\nimport \"fmt\"\n\nfunc helper() {}\nfunc main() { helper(); fmt.Println(\"ok\") }\n",
        )
        .expect("Go source");

        let artifact = build_go_codegraph(directory.path()).expect("graph");
        assert_eq!(artifact.version, 1);
        assert_eq!(artifact.files.len(), 1);
        assert!(
            artifact
                .nodes
                .iter()
                .any(|node| node.kind == "function" && node.name == "helper")
        );
        assert!(
            artifact
                .nodes
                .iter()
                .any(|node| node.kind == "package" && node.name == "fmt")
        );
        assert!(artifact.edges.iter().any(|edge| {
            edge.kind == "calls" && edge.target_name.as_deref() == Some("helper") && edge.resolved
        }));
        assert!(artifact.edges.iter().any(|edge| {
            edge.kind == "calls"
                && edge.target_name.as_deref() == Some("fmt.Println")
                && !edge.resolved
        }));
    }

    #[test]
    fn builds_definitions_imports_and_calls_for_rust_typescript_and_python() {
        let cases = [
            (
                "lib.rs",
                "use std::fmt;\nstruct Service;\nimpl Service { fn call(&self) {} }\nfn helper() {}\nfn run(service: &Service) { helper(); service.call(); }\n",
                "rust",
            ),
            (
                "src.ts",
                "import { external } from './helper';\nclass Service { call() {} }\nfunction helper() {}\nfunction run(service: Service) { helper(); service.call(); }\n",
                "typescript",
            ),
            (
                "src.tsx",
                "import { external } from './helper';\nclass Service { call() {} }\nfunction helper() {}\nfunction run(service: Service) { helper(); service.call(); return <main />; }\n",
                "tsx",
            ),
            (
                "src.py",
                "from helpers import external\nclass Service:\n    def call(self):\n        pass\ndef helper():\n    pass\ndef run():\n    helper()\n    Service().call()\n",
                "python",
            ),
        ];

        for (filename, source, expected_language) in cases {
            let directory = tempdir().expect("workspace");
            fs::write(directory.path().join(filename), source).expect("source file");
            let artifact = super::build_codegraph(directory.path()).expect("graph");

            assert_eq!(artifact.files.len(), 1);
            assert_eq!(artifact.files[0].language, expected_language);
            assert!(
                artifact
                    .nodes
                    .iter()
                    .any(|node| { node.kind == "function" && node.name == "run" })
            );
            assert!(
                artifact
                    .nodes
                    .iter()
                    .any(|node| { node.kind == "function" && node.name == "helper" })
            );
            assert!(artifact.edges.iter().any(|edge| edge.kind == "imports"));
            assert!(artifact.edges.iter().any(|edge| {
                edge.kind == "calls"
                    && edge.target_name.as_deref() == Some("helper")
                    && edge.resolved
            }));
            assert!(
                artifact.edges.iter().any(|edge| {
                    edge.kind == "calls"
                        && edge
                            .target_name
                            .as_deref()
                            .is_some_and(|name| name.rsplit(['.', ':']).next() == Some("call"))
                        && edge.resolved
                }),
                "{expected_language}: {:#?}",
                artifact.edges
            );
        }
    }

    #[test]
    fn generic_delta_updates_supported_non_go_sources() {
        let directory = tempdir().expect("workspace");
        let source = directory.path().join("module.py");
        fs::write(&source, "def old_function():\n    pass\n").expect("Python source");
        let base = super::build_codegraph(directory.path()).expect("base graph");

        fs::write(&source, "def new_function():\n    pass\n").expect("updated source");
        let updated = super::update_codegraph(
            &base,
            directory.path(),
            &[CodeGraphChange::Upsert("module.py".into())],
        )
        .expect("updated graph");

        assert!(updated.nodes.iter().any(|node| node.name == "new_function"));
        assert!(!updated.nodes.iter().any(|node| node.name == "old_function"));
        assert_eq!(
            updated,
            super::build_codegraph(directory.path()).expect("full graph")
        );
    }

    #[test]
    fn source_stamps_detect_modified_added_and_deleted_files() {
        let directory = tempdir().expect("workspace");
        let source = directory.path().join("module.py");
        fs::write(&source, "def old_name():\n    pass\n").expect("Python source");
        let original = super::codegraph_source_stamps(directory.path()).expect("initial stamps");
        assert_eq!(original.len(), 1);

        fs::write(&source, "def renamed_function():\n    pass\n").expect("updated source");
        let modified = super::codegraph_source_stamps(directory.path()).expect("updated stamps");
        assert_ne!(original, modified);

        fs::write(directory.path().join("added.rs"), "fn added() {}\n").expect("Rust source");
        let added = super::codegraph_source_stamps(directory.path()).expect("added stamps");
        assert_eq!(added.len(), 2);
        fs::remove_file(source).expect("delete Python source");
        let deleted = super::codegraph_source_stamps(directory.path()).expect("deleted stamps");
        assert_eq!(deleted.len(), 1);
        assert!(deleted.contains_key("added.rs"));
    }

    #[test]
    fn refresh_detects_added_modified_and_deleted_files_and_persists_snapshot() {
        let directory = tempdir().expect("workspace");
        let source = directory.path().join("module.py");
        let removed = directory.path().join("removed.rs");
        fs::write(&source, "def old_function():\n    pass\n").expect("Python source");
        fs::write(&removed, "fn removed_function() {}\n").expect("Rust source");
        let (_, base) = super::refresh_codegraph(directory.path()).expect("initial refresh");

        fs::write(&source, "def new_function():\n    pass\n").expect("updated source");
        fs::write(directory.path().join("added.ts"), "function added() {}\n")
            .expect("new TypeScript source");
        fs::remove_file(removed).expect("delete Rust source");
        let (path, refreshed) =
            super::refresh_codegraph(directory.path()).expect("incremental refresh");

        assert_eq!(
            path,
            directory
                .path()
                .canonicalize()
                .expect("canonical workspace")
                .join(".zvec-grep/codegraph-v1.json")
        );
        assert_ne!(base.manifest_key, refreshed.manifest_key);
        assert!(
            refreshed
                .nodes
                .iter()
                .any(|node| node.name == "new_function")
        );
        assert!(refreshed.nodes.iter().any(|node| node.name == "added"));
        assert!(
            !refreshed
                .nodes
                .iter()
                .any(|node| node.name == "old_function")
        );
        assert!(
            !refreshed
                .nodes
                .iter()
                .any(|node| node.name == "removed_function")
        );
        assert_eq!(
            refreshed,
            super::build_codegraph(directory.path()).expect("full graph")
        );
        assert_eq!(
            super::refresh_codegraph(directory.path())
                .expect("unchanged refresh")
                .1,
            refreshed
        );
    }

    #[test]
    fn same_file_candidates_win_over_duplicate_names_in_other_files() {
        let directory = tempdir().expect("workspace");
        fs::create_dir_all(directory.path().join("first")).expect("first package");
        fs::create_dir_all(directory.path().join("second")).expect("second package");
        fs::write(
            directory.path().join("first/first.go"),
            "package first\n\nfunc helper() {}\nfunc caller() { helper() }\n",
        )
        .expect("first Go source");
        fs::write(
            directory.path().join("second/second.go"),
            "package second\n\nfunc helper() {}\n",
        )
        .expect("second Go source");

        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let caller = node_id(&artifact, "function", "caller");
        let local_helper = artifact
            .nodes
            .iter()
            .find(|node| {
                node.kind == "function"
                    && node.path.as_deref() == Some("first/first.go")
                    && node.name == "helper"
            })
            .expect("same-file helper");
        let call = artifact
            .edges
            .iter()
            .find(|edge| edge.kind == "calls" && edge.source == caller)
            .expect("call edge");

        assert_eq!(call.target.as_deref(), Some(local_helper.id.as_str()));
        assert!(call.resolved);
        assert!(call.ambiguous_candidates.is_empty());
    }

    #[test]
    fn duplicate_qualified_names_are_reported_as_ambiguous_not_guessed() {
        let directory = tempdir().expect("workspace");
        fs::create_dir_all(directory.path().join("one")).expect("first package");
        fs::create_dir_all(directory.path().join("two")).expect("second package");
        fs::create_dir_all(directory.path().join("caller")).expect("caller package");
        fs::write(
            directory.path().join("one/config.go"),
            "package config\n\nfunc Load() {}\n",
        )
        .expect("first config source");
        fs::write(
            directory.path().join("two/config.go"),
            "package config\n\nfunc Load() {}\n",
        )
        .expect("second config source");
        fs::write(
            directory.path().join("caller/caller.go"),
            "package caller\n\nfunc caller() { config.Load() }\n",
        )
        .expect("caller source");

        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let caller = node_id(&artifact, "function", "caller");
        let call = artifact
            .edges
            .iter()
            .find(|edge| edge.kind == "calls" && edge.source == caller)
            .expect("call edge");

        assert_eq!(call.target_name.as_deref(), Some("config.Load"));
        assert!(!call.resolved);
        assert!(call.target.is_none());
        assert_eq!(call.ambiguous_candidates.len(), 2);
        assert!(
            call.ambiguous_candidates
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
    }

    #[test]
    fn chained_and_indexed_method_calls_keep_the_terminal_method_name() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\ntype command struct{}\nfunc (command) Result() {}\ntype client struct{}\nfunc (client) Add() command { return command{} }\ntype record struct{}\nfunc (record) Convert() {}\nfunc caller(c client, records []record) { c.Add().Result(); records[0].Convert() }\n",
        )
        .expect("Go source");

        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let caller = node_id(&artifact, "function", "caller");
        let calls = artifact
            .edges
            .iter()
            .filter(|edge| edge.kind == "calls" && edge.source == caller)
            .collect::<Vec<_>>();

        assert_eq!(
            calls
                .iter()
                .map(|edge| {
                    edge.target_name
                        .as_deref()
                        .unwrap_or_default()
                        .rsplit('.')
                        .next()
                        .unwrap_or_default()
                })
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["Add", "Convert", "Result"])
        );
        assert!(calls.iter().all(|edge| edge.resolved));
    }

    #[test]
    fn logical_symbol_ids_survive_line_shifts() {
        let directory = tempdir().expect("workspace");
        let source = directory.path().join("main.go");
        fs::write(&source, "package main\n\nfunc helper() {}\n").expect("Go source");
        let first = build_go_codegraph(directory.path()).expect("first graph");
        let first_id = node_id(&first, "function", "helper");
        fs::write(&source, "package main\n\n\n\nfunc helper() {}\n").expect("updated Go source");
        let second = build_go_codegraph(directory.path()).expect("second graph");
        assert_eq!(first_id, node_id(&second, "function", "helper"));
        assert_ne!(first.manifest_key, second.manifest_key);
    }

    #[test]
    fn graph_symbols_use_unique_ids_for_package_scope_declarations() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\nvar shared int\nvar _ = 1\nvar _ = 2\nconst _ = 3\n\nfunc first() {\n var local int\n const localConstant = 1\n type localType struct{}\n { var local int; _ = local }\n _ = local\n _ = localConstant\n}\n\nfunc second() { var local int; _ = local }\n",
        )
        .expect("Go source");

        let artifact = build_go_codegraph(directory.path()).expect("graph");
        let variables = artifact
            .nodes
            .iter()
            .filter(|node| node.kind == "variable")
            .map(|node| node.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(variables, BTreeSet::from(["shared"]));
        assert!(!artifact.nodes.iter().any(|node| {
            matches!(node.kind.as_str(), "constant" | "type")
                && matches!(node.name.as_str(), "localConstant" | "localType")
        }));
        assert!(!artifact.nodes.iter().any(|node| node.name == "_"));
        let ids = artifact
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), artifact.nodes.len());
    }

    #[test]
    fn writes_the_default_sidecar_path() {
        let directory = tempdir().expect("workspace");
        fs::write(directory.path().join("main.go"), "package main\n").expect("Go source");
        let (path, artifact) = write_go_codegraph(directory.path(), None).expect("write graph");
        assert_eq!(path, directory.path().join(".zvec-grep/codegraph-v1.json"));
        assert!(Path::new(&path).is_file());
        assert!(!artifact.manifest_key.is_empty());
        assert!(!fs::read(&path).expect("encoded artifact").contains(&b'\n'));
    }

    #[test]
    fn deleting_a_file_removes_its_definitions_and_invalidates_incoming_calls() {
        let directory = tempdir().expect("workspace");
        let helper = directory.path().join("helper.go");
        fs::write(&helper, "package main\n\nfunc helper() {}\n").expect("helper source");
        fs::write(
            directory.path().join("caller.go"),
            "package main\n\nfunc caller() { helper(helper()) }\n",
        )
        .expect("caller source");
        let base = build_go_codegraph(directory.path()).expect("base graph");
        let caller_id = node_id(&base, "function", "caller");

        fs::remove_file(helper).expect("delete helper source");
        let updated = update_go_codegraph(
            &base,
            directory.path(),
            &[CodeGraphChange::Delete("helper.go".into())],
        )
        .expect("updated graph");

        assert!(!updated.files.iter().any(|file| file.path == "helper.go"));
        assert!(!updated.nodes.iter().any(|node| node.name == "helper"));
        assert!(updated.edges.iter().any(|edge| {
            edge.kind == "calls"
                && edge.source == caller_id
                && edge.target_name.as_deref() == Some("helper")
                && edge.target.is_none()
                && !edge.resolved
        }));
        assert_ne!(base.manifest_key, updated.manifest_key);
        assert_eq!(
            updated,
            build_go_codegraph(directory.path()).expect("full graph")
        );
    }

    #[test]
    fn rename_replaces_logical_ids_and_retargets_incoming_calls() {
        let directory = tempdir().expect("workspace");
        let old_path = directory.path().join("old.go");
        let new_path = directory.path().join("new.go");
        fs::write(&old_path, "package main\n\nfunc helper() {}\n").expect("helper source");
        fs::write(
            directory.path().join("caller.go"),
            "package main\n\nfunc caller() { helper() }\n",
        )
        .expect("caller source");
        let base = build_go_codegraph(directory.path()).expect("base graph");
        let old_id = node_id(&base, "function", "helper");

        fs::rename(old_path, &new_path).expect("rename helper source");
        let updated = update_go_codegraph(
            &base,
            directory.path(),
            &[
                CodeGraphChange::Delete("old.go".into()),
                CodeGraphChange::Upsert("new.go".into()),
            ],
        )
        .expect("updated graph");
        let new_id = node_id(&updated, "function", "helper");

        assert_ne!(old_id, new_id, "logical IDs include the source path");
        assert!(!updated.files.iter().any(|file| file.path == "old.go"));
        assert!(updated.files.iter().any(|file| file.path == "new.go"));
        assert!(updated.edges.iter().any(|edge| {
            edge.kind == "calls" && edge.target.as_deref() == Some(new_id.as_str()) && edge.resolved
        }));
        assert!(
            !updated
                .edges
                .iter()
                .any(|edge| edge.target.as_deref() == Some(old_id.as_str()))
        );
        assert_eq!(
            updated,
            build_go_codegraph(directory.path()).expect("full graph")
        );
    }
}
