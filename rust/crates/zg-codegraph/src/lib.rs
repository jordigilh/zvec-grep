//! Deterministic multi-language codegraph sidecar generation.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tree_sitter::{Language, Node, Parser};

mod graph_queries;

pub use graph_queries::{
    CallGraphAssignment, CallGraphBlastRadius, CallGraphCluster, CallGraphClustering,
    CallGraphIndex, CallGraphPath, CodeGraphCapabilities, CodeGraphDirection, CodeGraphExplanation,
    CodeGraphNeighbor, CodeGraphNeighbors, CodeGraphNodeResult, CodeGraphQueryMetadata,
    CodeGraphRelationCapability, CodeGraphRelationKind, CodeGraphRelationPath,
    CodeGraphRelationSupport,
};

pub const CODEGRAPH_SCHEMA: &str = "zvec-grep.codegraph";
pub const CODEGRAPH_VERSION: u32 = 2;
pub const CODEGRAPH_FILE: &str = "codegraph-v2.json";
pub const CODEGRAPH_RELATION_SCHEMA: &str = "zvec-grep.codegraph.relations";
pub const CODEGRAPH_RELATION_VERSION: u32 = 1;
/// Generation of the relation extractor serialized into codegraph artifacts.
///
/// This is intentionally separate from the artifact schema version so future
/// relation-extractor changes can require regeneration without changing the
/// relation vocabulary itself.
pub const CODEGRAPH_RELATION_GENERATION: u32 = 2;
pub const GO_CALLFACTS_SCHEMA: &str = "zvec-grep.go-callfacts";
pub const GO_CALLFACTS_VERSION: u32 = 2;
pub const GO_CALLFACTS_FILE: &str = "go-callfacts-v2.json";
pub const RUST_CALLFACTS_SCHEMA: &str = "zvec-grep.rust-callfacts";
pub const RUST_CALLFACTS_VERSION: u32 = 1;
pub const RUST_CALLFACTS_FILE: &str = "rust-callfacts-v1.json";
pub const TYPESCRIPT_CALLFACTS_SCHEMA: &str = "zvec-grep.typescript-callfacts";
pub const TYPESCRIPT_CALLFACTS_VERSION: u32 = 1;
pub const TYPESCRIPT_CALLFACTS_FILE: &str = "typescript-callfacts-v1.json";
pub const PYTHON_CALLFACTS_SCHEMA: &str = "zvec-grep.python-callfacts";
pub const PYTHON_CALLFACTS_VERSION: u32 = 1;
pub const PYTHON_CALLFACTS_FILE: &str = "python-callfacts-v1.json";

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
    #[error("invalid Go call-facts artifact {path}: {reason}")]
    GoCallFacts { path: PathBuf, reason: String },
    #[error("invalid Rust call-facts artifact {path}: {reason}")]
    RustCallFacts { path: PathBuf, reason: String },
    #[error("invalid TypeScript call-facts artifact {path}: {reason}")]
    TypeScriptCallFacts { path: PathBuf, reason: String },
    #[error("invalid Python call-facts artifact {path}: {reason}")]
    PythonCallFacts { path: PathBuf, reason: String },
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
    /// Relation-extraction generation that produced this snapshot.
    pub relation_generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript_callfacts_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_callfacts_context_sha256: Option<String>,
    pub files: Vec<CodeGraphFile>,
    pub nodes: Vec<CodeGraphNode>,
    pub edges: Vec<CodeGraphEdge>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoCallFactsArtifact {
    pub schema: String,
    pub version: u32,
    pub context: GoCallFactsContext,
    pub context_sha256: String,
    pub files: Vec<GoCallFactsFile>,
    pub calls: Vec<GoCallFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoCallFactsContext {
    pub go_version: String,
    pub go_mod: String,
    pub go_work: String,
    pub settings: BTreeMap<String, String>,
    pub context_files: Vec<GoCallFactsFile>,
}

impl GoCallFactsContext {
    /// Returns the deterministic fingerprint for this recorded Go analysis context.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        go_callfacts_context_sha256(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoCallFactsFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoCallFact {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub caller: String,
    pub target_name: String,
    pub target: Option<String>,
    pub possible_targets: Vec<String>,
    pub resolution: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustCallFactsArtifact {
    pub schema: String,
    pub version: u32,
    pub context: RustCallFactsContext,
    pub context_sha256: String,
    pub files: Vec<RustCallFactsFile>,
    pub calls: Vec<RustCallFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustCallFactsContext {
    pub rustc_version: String,
    pub rustc_commit: String,
    pub host: String,
    pub target: String,
    pub edition: String,
    pub manifest_path: String,
    pub lockfile_path: Option<String>,
    pub toolchain_path: Option<String>,
    pub settings: BTreeMap<String, String>,
    pub context_files: Vec<RustCallFactsFile>,
}

impl RustCallFactsContext {
    /// Returns the deterministic fingerprint for this recorded Rust analysis context.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        rust_callfacts_context_sha256(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustCallFactsFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustCallFactsSymbol {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypeScriptCallFactsArtifact {
    pub schema: String,
    pub version: u32,
    pub context: TypeScriptCallFactsContext,
    pub context_sha256: String,
    pub files: Vec<TypeScriptCallFactsFile>,
    pub calls: Vec<TypeScriptCallFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypeScriptCallFactsContext {
    pub typescript_version: String,
    pub node_version: String,
    pub project_path: String,
    pub target: String,
    pub module: String,
    pub jsx: String,
    pub settings: BTreeMap<String, String>,
    pub context_files: Vec<TypeScriptCallFactsFile>,
}

impl TypeScriptCallFactsContext {
    /// Returns the deterministic fingerprint for the recorded TypeScript context.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        typescript_callfacts_context_sha256(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypeScriptCallFactsFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypeScriptCallFactsSymbol {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypeScriptCallFact {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub caller: TypeScriptCallFactsSymbol,
    pub target_name: String,
    pub target: Option<TypeScriptCallFactsSymbol>,
    pub possible_targets: Vec<TypeScriptCallFactsSymbol>,
    pub resolution: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PythonCallFactsArtifact {
    pub schema: String,
    pub version: u32,
    pub context: PythonCallFactsContext,
    pub context_sha256: String,
    pub files: Vec<PythonCallFactsFile>,
    pub calls: Vec<PythonCallFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PythonCallFactsContext {
    pub pyright_version: String,
    pub python_version: String,
    pub target_version: String,
    pub project_path: String,
    pub typeshed_sha256: String,
    pub settings: BTreeMap<String, String>,
    pub context_files: Vec<PythonCallFactsFile>,
}

impl PythonCallFactsContext {
    /// Returns the deterministic fingerprint for the recorded Python context.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        python_callfacts_context_sha256(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PythonCallFactsFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PythonCallFactsSymbol {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PythonCallFact {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub caller: PythonCallFactsSymbol,
    pub target_name: String,
    pub target: Option<PythonCallFactsSymbol>,
    pub possible_targets: Vec<PythonCallFactsSymbol>,
    pub resolution: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustCallFact {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub caller: RustCallFactsSymbol,
    pub target_name: String,
    pub target: Option<RustCallFactsSymbol>,
    pub possible_targets: Vec<RustCallFactsSymbol>,
    pub resolution: String,
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
    content_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphNode {
    pub id: String,
    pub kind: String,
    pub path: Option<String>,
    pub name: String,
    pub qualified_name: Option<String>,
    pub range: Option<CodeGraphRange>,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CodeGraphEdge {
    pub kind: String,
    pub source: String,
    pub target: Option<String>,
    pub target_name: Option<String>,
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguous_candidates: Vec<String>,
    /// Producer or syntax certainty label. It is optional for edge kinds that
    /// do not carry a certainty classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    pub range: Option<CodeGraphRange>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
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
    relations: Vec<StructuralRelation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackageManifest {
    path: String,
    sha256: String,
    ecosystem: String,
    name: Option<String>,
    dependencies: Vec<String>,
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
    relations: Vec<StructuralRelation>,
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
            relations: Vec::new(),
        }
    }

    fn collect(&mut self, node: Node<'_>) {
        let definition =
            language_definition(node, self.source, self.language, self.path, &self.scopes);
        if let Some(definition) = definition.as_ref() {
            self.definitions.push(definition.clone());
        }
        self.relations.extend(language_relations(
            node,
            self.source,
            self.language,
            self.path,
            &self.scopes,
            definition.as_ref(),
        ));
        if let Some(import) = language_import(node, self.source, self.language) {
            self.imports.insert(import);
        }
        if let Some(call) = language_call_site(node, self.source, self.language) {
            if let Some(source_id) =
                test_owner_id(node, &self.scopes, self.language, self.path, self.source)
            {
                self.relations.push(StructuralRelation {
                    kind: "tests".to_owned(),
                    source: source_id,
                    target_name: call
                        .qualified_target
                        .clone()
                        .unwrap_or_else(|| call.target_name.clone()),
                    range: call.range.clone(),
                });
            }
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

#[derive(Clone, Debug)]
struct StructuralRelation {
    kind: String,
    source: String,
    target_name: String,
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
    let package_manifests = collect_package_manifests(&root)?;
    let mut artifact = build_artifact(&parsed, &package_manifests);
    apply_semantic_callfacts(&root, &mut artifact)?;
    Ok(artifact)
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
    let package_manifests = collect_package_manifests(&root)?;
    let package_manifests = package_manifests
        .iter()
        .filter(|manifest| manifest.ecosystem == "go")
        .cloned()
        .collect::<Vec<_>>();
    let mut artifact = build_artifact(&parsed, &package_manifests);
    apply_semantic_callfacts(&root, &mut artifact)?;
    Ok(artifact)
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
    if !has_current_relation_generation(base) {
        return build_codegraph(&root);
    }
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
    // The changed artifact contains only source-derived nodes. Manifest-derived
    // package nodes and edges are retained from the base graph; refresh performs
    // a full rebuild when a manifest input changes.
    let changed_artifact = build_artifact(&parsed, &[]);
    let mut artifact = merge_codegraph_delta(base, &changed_paths, changed_artifact);
    apply_semantic_callfacts(&root, &mut artifact)?;
    Ok(artifact)
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

    let mut base_edges = base.edges.clone();
    resolve_call_edges(&mut base_edges, &base.nodes);
    let mut edges = base_edges
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
    resolve_relation_edges(&mut edges, &nodes);

    let referenced_packages = edges
        .iter()
        .filter(|edge| matches!(edge.kind.as_str(), "imports" | "depends_on"))
        .flat_map(|edge| [Some(edge.source.as_str()), edge.target.as_deref()])
        .flatten()
        .collect::<BTreeSet<_>>();
    nodes.retain(|node| {
        node.kind != "package"
            || node.path.is_some()
            || referenced_packages.contains(node.id.as_str())
    });
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
        relation_generation: CODEGRAPH_RELATION_GENERATION,
        go_callfacts_context_sha256: None,
        rust_callfacts_context_sha256: None,
        typescript_callfacts_context_sha256: None,
        python_callfacts_context_sha256: None,
        files,
        nodes,
        edges,
    }
}

fn has_current_relation_generation(artifact: &CodeGraphArtifact) -> bool {
    artifact.relation_generation == CODEGRAPH_RELATION_GENERATION
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

fn build_artifact(
    parsed: &[ParsedFile],
    package_manifests: &[PackageManifest],
) -> CodeGraphArtifact {
    let files = parsed
        .iter()
        .map(|file| file.file.clone())
        .collect::<Vec<_>>();
    let manifest_key = package_manifest_key(&files, package_manifests);
    let (mut nodes, mut edges) = structural_graph(parsed, package_manifests);
    append_call_edges(parsed, &mut edges);

    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    resolve_call_edges(&mut edges, &nodes);
    resolve_relation_edges(&mut edges, &nodes);
    sort_graph(&mut nodes, &mut edges);

    CodeGraphArtifact {
        schema: CODEGRAPH_SCHEMA.to_owned(),
        version: CODEGRAPH_VERSION,
        manifest_key,
        relation_generation: CODEGRAPH_RELATION_GENERATION,
        go_callfacts_context_sha256: None,
        rust_callfacts_context_sha256: None,
        typescript_callfacts_context_sha256: None,
        python_callfacts_context_sha256: None,
        files,
        nodes,
        edges,
    }
}

fn structural_graph(
    parsed: &[ParsedFile],
    package_manifests: &[PackageManifest],
) -> (Vec<CodeGraphNode>, Vec<CodeGraphEdge>) {
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
                resolution: Some("structural".to_owned()),
                range: definition.node.range.clone(),
            });
        }
        for import in &file.imports {
            let package_id = package_node_id(import);
            ensure_package_node(&mut package_nodes, import, None);
            edges.push(CodeGraphEdge {
                kind: "imports".to_owned(),
                source: file.file_node_id.clone(),
                target: Some(package_id),
                target_name: Some(import.clone()),
                resolved: true,
                ambiguous_candidates: Vec::new(),
                resolution: Some("structural".to_owned()),
                range: None,
            });
        }
        for relation in &file.relations {
            edges.push(CodeGraphEdge {
                kind: relation.kind.clone(),
                source: relation.source.clone(),
                target: None,
                target_name: Some(relation.target_name.clone()),
                resolved: false,
                ambiguous_candidates: Vec::new(),
                resolution: Some("syntax".to_owned()),
                range: Some(relation.range.clone()),
            });
        }
    }

    let mut seen_dependency_edges = BTreeSet::new();
    for manifest in package_manifests {
        let Some(name) = manifest.name.as_deref() else {
            continue;
        };
        let package_id = package_node_id(name);
        ensure_package_node(&mut package_nodes, name, Some(manifest.path.as_str()));
        for dependency in &manifest.dependencies {
            if dependency.is_empty()
                || dependency == name
                || !seen_dependency_edges.insert((name.to_owned(), dependency.clone()))
            {
                continue;
            }
            let dependency_id = package_node_id(dependency);
            ensure_package_node(&mut package_nodes, dependency, None);
            edges.push(CodeGraphEdge {
                kind: "depends_on".to_owned(),
                source: package_id.clone(),
                target: Some(dependency_id),
                target_name: Some(dependency.clone()),
                resolved: true,
                ambiguous_candidates: Vec::new(),
                resolution: Some("manifest".to_owned()),
                range: None,
            });
        }
    }
    nodes.extend(package_nodes.into_values());
    (nodes, edges)
}

fn ensure_package_node(
    package_nodes: &mut BTreeMap<String, CodeGraphNode>,
    name: &str,
    path: Option<&str>,
) {
    let id = package_node_id(name);
    let node = package_nodes
        .entry(name.to_owned())
        .or_insert_with(|| CodeGraphNode {
            id,
            kind: "package".to_owned(),
            path: None,
            name: name.to_owned(),
            qualified_name: Some(name.to_owned()),
            range: None,
            signature: None,
        });
    if node.path.is_none() {
        node.path = path.map(str::to_owned);
    }
}

fn append_call_edges(parsed: &[ParsedFile], edges: &mut Vec<CodeGraphEdge>) {
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
            let owner_definition = file
                .definitions
                .iter()
                .filter(|definition| {
                    definition.start_byte <= call.start_byte && call.end_byte <= definition.end_byte
                })
                .min_by_key(|definition| definition.end_byte - definition.start_byte);
            edges.push(CodeGraphEdge {
                kind: "calls".to_owned(),
                source: owner.clone(),
                target: None,
                target_name: Some(
                    call.qualified_target
                        .clone()
                        .unwrap_or_else(|| call.target_name.clone()),
                ),
                resolved: false,
                ambiguous_candidates: Vec::new(),
                resolution: Some("syntax".to_owned()),
                range: Some(call.range.clone()),
            });
            if owner_definition.is_some_and(|definition| is_test_name(&definition.simple_name)) {
                edges.push(CodeGraphEdge {
                    kind: "tests".to_owned(),
                    source: owner,
                    target: None,
                    target_name: Some(
                        call.qualified_target
                            .clone()
                            .unwrap_or_else(|| call.target_name.clone()),
                    ),
                    resolved: false,
                    ambiguous_candidates: Vec::new(),
                    resolution: Some("syntax".to_owned()),
                    range: Some(call.range.clone()),
                });
            }
        }
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
            left.resolution.as_deref().unwrap_or_default(),
            left.range.as_ref().map_or(0, |range| range.start_byte),
            left.range.as_ref().map_or(0, |range| range.end_byte),
        )
            .cmp(&(
                right.source.as_str(),
                right.kind.as_str(),
                right.target.as_deref().unwrap_or_default(),
                right.target_name.as_deref().unwrap_or_default(),
                right.resolution.as_deref().unwrap_or_default(),
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
            edge.ambiguous_candidates.clear();
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
        edge.resolution = Some(
            if edge.resolved {
                "syntax"
            } else if edge.ambiguous_candidates.is_empty() {
                "unresolved"
            } else {
                "ambiguous"
            }
            .to_owned(),
        );
    }
}

fn resolve_relation_edges(edges: &mut [CodeGraphEdge], nodes: &[CodeGraphNode]) {
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();

    for edge in edges.iter_mut().filter(|edge| {
        matches!(
            edge.kind.as_str(),
            "inherits" | "implements" | "overrides" | "mixes_in" | "references" | "tests"
        )
    }) {
        let Some(target_name) = edge.target_name.as_deref() else {
            edge.target = None;
            edge.resolved = false;
            edge.ambiguous_candidates.clear();
            edge.resolution = Some("unresolved".to_owned());
            continue;
        };

        let normalized_name = clean_relation_target(target_name);
        let simple_name = normalized_name
            .rsplit('.')
            .next()
            .unwrap_or(normalized_name.as_str());
        let source_path = nodes_by_id
            .get(edge.source.as_str())
            .and_then(|node| node.path.as_deref());

        let mut candidates = nodes
            .iter()
            .filter(|node| relation_target_kind(edge.kind.as_str(), node.kind.as_str()))
            .filter(|node| {
                node.qualified_name.as_deref() == Some(normalized_name.as_str())
                    || (normalized_name.contains('.')
                        && node.qualified_name.as_deref().is_some_and(|qualified| {
                            qualified.ends_with(format!(".{normalized_name}").as_str())
                        }))
                    || node.name == simple_name
            })
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();

        if let Some(source_path) = source_path {
            let same_file = candidates
                .iter()
                .filter(|candidate| {
                    nodes_by_id
                        .get(candidate.as_str())
                        .and_then(|node| node.path.as_deref())
                        == Some(source_path)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !same_file.is_empty() {
                candidates = same_file;
            }
        }

        edge.target = (candidates.len() == 1).then(|| candidates[0].clone());
        edge.resolved = edge.target.is_some();
        edge.ambiguous_candidates = if edge.resolved {
            Vec::new()
        } else {
            candidates
        };
        edge.resolution = Some(
            if edge.resolved {
                "syntax"
            } else if edge.ambiguous_candidates.is_empty() {
                "unresolved"
            } else {
                "ambiguous"
            }
            .to_owned(),
        );
    }
}

fn relation_target_kind(relation: &str, node_kind: &str) -> bool {
    match relation {
        "tests" => matches!(node_kind, "function" | "method" | "class"),
        "overrides" => matches!(node_kind, "method" | "function"),
        "depends_on" => node_kind == "package",
        "inherits" | "implements" | "mixes_in" => {
            matches!(node_kind, "class" | "interface" | "type" | "alias" | "enum")
        }
        "references" => matches!(
            node_kind,
            "class" | "interface" | "type" | "alias" | "enum" | "package"
        ),
        _ => false,
    }
}

fn apply_semantic_callfacts(root: &Path, artifact: &mut CodeGraphArtifact) -> CodeGraphResult<()> {
    // Start from parser-derived names so replacing/removing a prior overlay
    // cannot leave semantic edges behind.
    restore_syntax_call_edges(artifact);
    artifact.manifest_key = package_manifest_key_for_root(root, &artifact.files)?;
    if let Err(error) = try_apply_go_callfacts(root, artifact) {
        // Semantic facts are an optional enhancement. Invalid, unsupported, or
        // internally inconsistent facts must not prevent the syntax graph from
        // remaining queryable. A Rust sidecar may still be valid when the Go
        // sidecar is not, so restore only the syntax overlay before continuing.
        match error {
            CodeGraphError::GoCallFacts { .. } => restore_syntax_call_edges(artifact),
            error => return Err(error),
        }
    }
    if let Err(error) = try_apply_rust_callfacts(root, artifact) {
        match error {
            CodeGraphError::RustCallFacts { .. } => {
                // Rust validation is completed before its edges are committed;
                // preserve any valid Go overlay already applied.
            }
            error => return Err(error),
        }
    }
    if let Err(error) = try_apply_typescript_callfacts(root, artifact) {
        match error {
            CodeGraphError::TypeScriptCallFacts { .. } => {
                // TypeScript facts are optional and independently scoped.
            }
            error => return Err(error),
        }
    }
    if let Err(error) = try_apply_python_callfacts(root, artifact) {
        match error {
            CodeGraphError::PythonCallFacts { .. } => {
                // Python facts are optional and independently scoped.
            }
            error => return Err(error),
        }
    }
    Ok(())
}

fn try_apply_go_callfacts(root: &Path, artifact: &mut CodeGraphArtifact) -> CodeGraphResult<()> {
    let Some((path, _bytes, facts)) = read_go_callfacts(root)? else {
        return Ok(());
    };
    if facts.schema != GO_CALLFACTS_SCHEMA || facts.version != GO_CALLFACTS_VERSION {
        return Err(go_callfacts_error(
            &path,
            format!(
                "unsupported schema/version: {} v{}",
                facts.schema, facts.version
            ),
        ));
    }
    if !validate_go_callfacts_context(root, &facts.context, &facts.context_sha256, &path)? {
        return Ok(());
    }
    let Some(go_paths) = validate_go_callfacts_sources(root, artifact, &facts, &path)? else {
        return Ok(());
    };
    let (symbols, nodes_by_id) = graph_callfact_indexes(artifact, &path)?;
    let (semantic_edges, covered_call_sites) = go_callfact_edges(
        &facts.calls,
        &go_paths,
        &symbols,
        &nodes_by_id,
        &artifact.edges,
        &path,
    )?;
    let node_paths = artifact
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.clone(), node.path.clone()?)))
        .collect::<HashMap<_, _>>();
    drop(nodes_by_id);
    artifact.edges.retain(|edge| {
        if edge.kind != "calls" {
            return true;
        }
        let Some(path) = node_paths.get(edge.source.as_str()) else {
            return true;
        };
        let Some(range) = edge.range.as_ref() else {
            return true;
        };
        !covered_call_sites.contains(&(
            path.clone(),
            range.start_byte,
            range.end_byte,
            edge.source.clone(),
        ))
    });
    artifact.edges.extend(semantic_edges);
    sort_graph(&mut artifact.nodes, &mut artifact.edges);

    artifact.go_callfacts_context_sha256 = Some(facts.context_sha256);
    update_callfacts_manifest(root, artifact)?;
    Ok(())
}

fn read_go_callfacts(
    root: &Path,
) -> CodeGraphResult<Option<(PathBuf, Vec<u8>, GoCallFactsArtifact)>> {
    let path = root.join(".zvec-grep").join(GO_CALLFACTS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure("read Go call-facts artifact", &path, error)),
    };
    let facts = serde_json::from_slice(&bytes)
        .map_err(|error| go_callfacts_error(&path, format!("decode JSON: {error}")))?;
    Ok(Some((path, bytes, facts)))
}

fn read_rust_callfacts(
    root: &Path,
) -> CodeGraphResult<Option<(PathBuf, Vec<u8>, RustCallFactsArtifact)>> {
    let path = root.join(".zvec-grep").join(RUST_CALLFACTS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure("read Rust call-facts artifact", &path, error)),
    };
    let facts = serde_json::from_slice(&bytes)
        .map_err(|error| rust_callfacts_error(&path, format!("decode JSON: {error}")))?;
    Ok(Some((path, bytes, facts)))
}

fn try_apply_rust_callfacts(root: &Path, artifact: &mut CodeGraphArtifact) -> CodeGraphResult<()> {
    let Some((path, _bytes, facts)) = read_rust_callfacts(root)? else {
        return Ok(());
    };
    if facts.schema != RUST_CALLFACTS_SCHEMA || facts.version != RUST_CALLFACTS_VERSION {
        return Err(rust_callfacts_error(
            &path,
            format!(
                "unsupported schema/version: {} v{}",
                facts.schema, facts.version
            ),
        ));
    }
    if !validate_rust_callfacts_context(root, &facts.context, &facts.context_sha256, &path)? {
        return Ok(());
    }
    let Some(rust_paths) = validate_rust_callfacts_sources(root, artifact, &facts, &path)? else {
        return Ok(());
    };
    let (semantic_edges, covered_call_sites) = rust_callfact_edges(
        &facts.calls,
        &rust_paths,
        &artifact.edges,
        &artifact.nodes,
        &path,
    )?;
    let node_paths = artifact
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.clone(), node.path.clone()?)))
        .collect::<HashMap<_, _>>();
    artifact.edges.retain(|edge| {
        if edge.kind != "calls" {
            return true;
        }
        let Some(path) = node_paths.get(edge.source.as_str()) else {
            return true;
        };
        if !rust_paths.contains(path) {
            return true;
        }
        let Some(range) = edge.range.as_ref() else {
            return true;
        };
        !covered_call_sites.contains(&(
            path.clone(),
            range.start_byte,
            range.end_byte,
            edge.source.clone(),
        ))
    });
    artifact.edges.extend(semantic_edges);
    sort_graph(&mut artifact.nodes, &mut artifact.edges);
    artifact.rust_callfacts_context_sha256 = Some(facts.context_sha256);
    update_callfacts_manifest(root, artifact)?;
    Ok(())
}

fn try_apply_typescript_callfacts(
    root: &Path,
    artifact: &mut CodeGraphArtifact,
) -> CodeGraphResult<()> {
    let Some((path, _bytes, facts)) = read_typescript_callfacts(root)? else {
        return Ok(());
    };
    if facts.schema != TYPESCRIPT_CALLFACTS_SCHEMA || facts.version != TYPESCRIPT_CALLFACTS_VERSION
    {
        return Err(typescript_callfacts_error(
            &path,
            format!(
                "unsupported schema/version: {} v{}",
                facts.schema, facts.version
            ),
        ));
    }
    if !validate_typescript_callfacts_context(root, &facts.context, &facts.context_sha256, &path)? {
        return Ok(());
    }
    let Some(source_paths) = validate_typescript_callfacts_sources(root, artifact, &facts, &path)?
    else {
        return Ok(());
    };
    let external_facts = facts.calls.iter().map(Into::into).collect::<Vec<_>>();
    let (semantic_edges, covered_call_sites) = external_callfact_edges(
        &external_facts,
        &source_paths,
        &artifact.edges,
        &artifact.nodes,
        &path,
        "TypeScript",
    )?;
    let node_paths = artifact
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.clone(), node.path.clone()?)))
        .collect::<HashMap<_, _>>();
    artifact.edges.retain(|edge| {
        if edge.kind != "calls" {
            return true;
        }
        let Some(path) = node_paths.get(edge.source.as_str()) else {
            return true;
        };
        if !source_paths.contains(path) {
            return true;
        }
        let Some(range) = edge.range.as_ref() else {
            return true;
        };
        !covered_call_sites.contains(&(
            path.clone(),
            range.start_byte,
            range.end_byte,
            edge.source.clone(),
        ))
    });
    artifact.edges.extend(semantic_edges);
    sort_graph(&mut artifact.nodes, &mut artifact.edges);
    artifact.typescript_callfacts_context_sha256 = Some(facts.context_sha256);
    update_callfacts_manifest(root, artifact)?;
    Ok(())
}

fn try_apply_python_callfacts(
    root: &Path,
    artifact: &mut CodeGraphArtifact,
) -> CodeGraphResult<()> {
    let Some((path, _bytes, facts)) = read_python_callfacts(root)? else {
        return Ok(());
    };
    if facts.schema != PYTHON_CALLFACTS_SCHEMA || facts.version != PYTHON_CALLFACTS_VERSION {
        return Err(python_callfacts_error(
            &path,
            format!(
                "unsupported schema/version: {} v{}",
                facts.schema, facts.version
            ),
        ));
    }
    if !validate_python_callfacts_context(root, &facts.context, &facts.context_sha256, &path)? {
        return Ok(());
    }
    let Some(source_paths) = validate_python_callfacts_sources(root, artifact, &facts, &path)?
    else {
        return Ok(());
    };
    let external_facts = facts.calls.iter().map(Into::into).collect::<Vec<_>>();
    let (semantic_edges, covered_call_sites) = external_callfact_edges(
        &external_facts,
        &source_paths,
        &artifact.edges,
        &artifact.nodes,
        &path,
        "Python",
    )?;
    let node_paths = artifact
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.clone(), node.path.clone()?)))
        .collect::<HashMap<_, _>>();
    artifact.edges.retain(|edge| {
        if edge.kind != "calls" {
            return true;
        }
        let Some(path) = node_paths.get(edge.source.as_str()) else {
            return true;
        };
        if !source_paths.contains(path) {
            return true;
        }
        let Some(range) = edge.range.as_ref() else {
            return true;
        };
        !covered_call_sites.contains(&(
            path.clone(),
            range.start_byte,
            range.end_byte,
            edge.source.clone(),
        ))
    });
    artifact.edges.extend(semantic_edges);
    sort_graph(&mut artifact.nodes, &mut artifact.edges);
    artifact.python_callfacts_context_sha256 = Some(facts.context_sha256);
    update_callfacts_manifest(root, artifact)?;
    Ok(())
}

fn read_typescript_callfacts(
    root: &Path,
) -> CodeGraphResult<Option<(PathBuf, Vec<u8>, TypeScriptCallFactsArtifact)>> {
    let path = root.join(".zvec-grep").join(TYPESCRIPT_CALLFACTS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_failure(
                "read TypeScript call-facts artifact",
                &path,
                error,
            ));
        }
    };
    let facts = serde_json::from_slice(&bytes)
        .map_err(|error| typescript_callfacts_error(&path, format!("decode JSON: {error}")))?;
    Ok(Some((path, bytes, facts)))
}

fn read_python_callfacts(
    root: &Path,
) -> CodeGraphResult<Option<(PathBuf, Vec<u8>, PythonCallFactsArtifact)>> {
    let path = root.join(".zvec-grep").join(PYTHON_CALLFACTS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_failure("read Python call-facts artifact", &path, error));
        }
    };
    let facts = serde_json::from_slice(&bytes)
        .map_err(|error| python_callfacts_error(&path, format!("decode JSON: {error}")))?;
    Ok(Some((path, bytes, facts)))
}

fn validate_typescript_callfacts_sources(
    root: &Path,
    artifact: &CodeGraphArtifact,
    facts: &TypeScriptCallFactsArtifact,
    facts_path: &Path,
) -> CodeGraphResult<Option<BTreeSet<String>>> {
    let expected = artifact
        .files
        .iter()
        .filter(|file| matches!(file.language.as_str(), "typescript" | "tsx"))
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeMap::new();
    for file in &facts.files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| typescript_callfacts_error(facts_path, reason))?;
        if actual
            .insert(file.path.as_str(), file.sha256.as_str())
            .is_some()
        {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("duplicate source file: {}", file.path),
            ));
        }
    }
    if expected != actual {
        return Ok(None);
    }
    for (relative_path, expected_digest) in &expected {
        let path = root.join(relative_path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(io_failure(
                    "verify TypeScript call-facts source",
                    &path,
                    error,
                ));
            }
        };
        if sha256_hex(&bytes) != *expected_digest {
            return Ok(None);
        }
    }
    Ok(Some(
        expected.keys().map(|path| (*path).to_owned()).collect(),
    ))
}

fn validate_python_callfacts_sources(
    root: &Path,
    artifact: &CodeGraphArtifact,
    facts: &PythonCallFactsArtifact,
    facts_path: &Path,
) -> CodeGraphResult<Option<BTreeSet<String>>> {
    let expected = artifact
        .files
        .iter()
        .filter(|file| file.language == "python")
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeMap::new();
    for file in &facts.files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| python_callfacts_error(facts_path, reason))?;
        if actual
            .insert(file.path.as_str(), file.sha256.as_str())
            .is_some()
        {
            return Err(python_callfacts_error(
                facts_path,
                format!("duplicate source file: {}", file.path),
            ));
        }
    }
    if expected != actual {
        return Ok(None);
    }
    for (relative_path, expected_digest) in &expected {
        let path = root.join(relative_path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(io_failure("verify Python call-facts source", &path, error));
            }
        };
        if sha256_hex(&bytes) != *expected_digest {
            return Ok(None);
        }
    }
    Ok(Some(
        expected.keys().map(|path| (*path).to_owned()).collect(),
    ))
}

fn validate_typescript_callfacts_context(
    root: &Path,
    context: &TypeScriptCallFactsContext,
    expected_fingerprint: &str,
    facts_path: &Path,
) -> CodeGraphResult<bool> {
    if context.typescript_version.is_empty()
        || context.node_version.is_empty()
        || context.target.is_empty()
        || context.module.is_empty()
        || context.jsx.is_empty()
    {
        return Err(typescript_callfacts_error(
            facts_path,
            "analysis context is missing TypeScript, Node, or compiler-option attestation"
                .to_owned(),
        ));
    }
    if typescript_callfacts_context_sha256(context) != expected_fingerprint {
        return Err(typescript_callfacts_error(
            facts_path,
            "analysis context fingerprint does not match its contents".to_owned(),
        ));
    }
    if !context.project_path.is_empty() {
        validate_relative_source_path(&context.project_path)
            .map_err(|reason| typescript_callfacts_error(facts_path, reason))?;
        if !context
            .context_files
            .iter()
            .any(|file| file.path == context.project_path)
        {
            return Err(typescript_callfacts_error(
                facts_path,
                "selected TypeScript project is absent from context_files".to_owned(),
            ));
        }
    }
    validate_typescript_context_file_list(context, facts_path)?;
    if !validate_typescript_context_file_digests(root, context)? {
        return Ok(false);
    }
    validate_typescript_context_extends(root, context, facts_path)?;
    let current_files = collect_typescript_context_files(root)?;
    Ok(current_files == context.context_files)
}

fn validate_python_callfacts_context(
    root: &Path,
    context: &PythonCallFactsContext,
    expected_fingerprint: &str,
    facts_path: &Path,
) -> CodeGraphResult<bool> {
    if context.pyright_version.is_empty()
        || context.python_version.is_empty()
        || context.target_version.is_empty()
        || context.typeshed_sha256.is_empty()
        || context.typeshed_sha256 == "unknown"
    {
        return Err(python_callfacts_error(
            facts_path,
            "analysis context is missing Pyright, Python, target, or typeshed attestation"
                .to_owned(),
        ));
    }
    if python_callfacts_context_sha256(context) != expected_fingerprint {
        return Err(python_callfacts_error(
            facts_path,
            "analysis context fingerprint does not match its contents".to_owned(),
        ));
    }
    if !context.project_path.is_empty() {
        validate_relative_source_path(&context.project_path)
            .map_err(|reason| python_callfacts_error(facts_path, reason))?;
        if !context
            .context_files
            .iter()
            .any(|file| file.path == context.project_path)
        {
            return Err(python_callfacts_error(
                facts_path,
                "selected Python project is absent from context_files".to_owned(),
            ));
        }
    }
    validate_python_context_file_list(context, facts_path)?;
    if !validate_python_context_file_digests(root, context)? {
        return Ok(false);
    }
    let current_files = collect_python_context_files(root)?;
    Ok(current_files == context.context_files)
}

fn validate_typescript_context_file_list(
    context: &TypeScriptCallFactsContext,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    let mut seen = BTreeSet::new();
    for file in &context.context_files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| typescript_callfacts_error(facts_path, reason))?;
        if !is_typescript_context_file_path(Path::new(&file.path)) {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("unsupported TypeScript context input: {}", file.path),
            ));
        }
        if !seen.insert(file.path.as_str()) {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("duplicate TypeScript context input: {}", file.path),
            ));
        }
    }
    Ok(())
}

fn validate_python_context_file_list(
    context: &PythonCallFactsContext,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    let mut seen = BTreeSet::new();
    for file in &context.context_files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| python_callfacts_error(facts_path, reason))?;
        if !is_python_context_input_path(Path::new(&file.path)) {
            return Err(python_callfacts_error(
                facts_path,
                format!("unsupported Python context input: {}", file.path),
            ));
        }
        if !seen.insert(file.path.as_str()) {
            return Err(python_callfacts_error(
                facts_path,
                format!("duplicate Python context input: {}", file.path),
            ));
        }
    }
    Ok(())
}

fn validate_typescript_context_file_digests(
    root: &Path,
    context: &TypeScriptCallFactsContext,
) -> CodeGraphResult<bool> {
    for file in &context.context_files {
        let path = root.join(&file.path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_failure("verify TypeScript context input", &path, error)),
        };
        if sha256_hex(&bytes) != file.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_python_context_file_digests(
    root: &Path,
    context: &PythonCallFactsContext,
) -> CodeGraphResult<bool> {
    for file in &context.context_files {
        let path = root.join(&file.path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_failure("verify Python context input", &path, error)),
        };
        if sha256_hex(&bytes) != file.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_typescript_context_extends(
    root: &Path,
    context: &TypeScriptCallFactsContext,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    let context_paths = context
        .context_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut pending = context
        .context_files
        .iter()
        .filter(|file| is_typescript_context_input_path(Path::new(&file.path)))
        .map(|file| root.join(&file.path))
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    while let Some(config_path) = pending.pop() {
        if !seen.insert(config_path.clone()) {
            continue;
        }
        let Ok(bytes) = fs::read(&config_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(extends) = value.get("extends").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !extends.starts_with('.') {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("external TypeScript config extends is not attested: {extends}"),
            ));
        }
        let Some(parent) = config_path.parent() else {
            continue;
        };
        let base = parent.join(extends);
        let candidates = [
            base.clone(),
            base.with_extension("json"),
            base.join("tsconfig.json"),
        ];
        let Some(extended) = candidates
            .iter()
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| candidate.canonicalize().ok())
        else {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("TypeScript config extends file does not exist: {extends}"),
            ));
        };
        if !extended.starts_with(root) {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("TypeScript config extends outside root: {extends}"),
            ));
        }
        let relative = relative_path(root, &extended);
        if !context_paths.contains(relative.as_str()) {
            return Err(typescript_callfacts_error(
                facts_path,
                format!("extended TypeScript config is absent from context_files: {relative}"),
            ));
        }
        pending.push(extended);
    }
    Ok(())
}

fn validate_rust_callfacts_sources(
    root: &Path,
    artifact: &CodeGraphArtifact,
    facts: &RustCallFactsArtifact,
    facts_path: &Path,
) -> CodeGraphResult<Option<BTreeSet<String>>> {
    let expected = artifact
        .files
        .iter()
        .filter(|file| file.language == "rust")
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeMap::new();
    for file in &facts.files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
        if actual
            .insert(file.path.as_str(), file.sha256.as_str())
            .is_some()
        {
            return Err(rust_callfacts_error(
                facts_path,
                format!("duplicate source file: {}", file.path),
            ));
        }
    }
    if expected != actual {
        return Ok(None);
    }
    for (relative_path, expected_digest) in &expected {
        let path = root.join(relative_path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_failure("verify Rust call-facts source", &path, error)),
        };
        if sha256_hex(&bytes) != *expected_digest {
            return Ok(None);
        }
    }
    Ok(Some(
        expected.keys().map(|path| (*path).to_owned()).collect(),
    ))
}

fn validate_rust_callfacts_context(
    root: &Path,
    context: &RustCallFactsContext,
    expected_fingerprint: &str,
    facts_path: &Path,
) -> CodeGraphResult<bool> {
    if context.rustc_version.is_empty()
        || context.rustc_commit.is_empty()
        || context.host.is_empty()
        || context.target.is_empty()
        || context.edition.is_empty()
    {
        return Err(rust_callfacts_error(
            facts_path,
            "analysis context is missing compiler, host, target, or edition attestation".to_owned(),
        ));
    }
    if rust_callfacts_context_sha256(context) != expected_fingerprint {
        return Err(rust_callfacts_error(
            facts_path,
            "analysis context fingerprint does not match its contents".to_owned(),
        ));
    }
    validate_relative_source_path(&context.manifest_path)
        .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
    if Path::new(&context.manifest_path)
        .file_name()
        .is_none_or(|name| name != "Cargo.toml")
    {
        return Err(rust_callfacts_error(
            facts_path,
            "analysis context manifest_path must name Cargo.toml".to_owned(),
        ));
    }
    if let Some(path) = &context.lockfile_path {
        validate_relative_source_path(path)
            .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
        if Path::new(path)
            .file_name()
            .is_none_or(|name| name != "Cargo.lock")
        {
            return Err(rust_callfacts_error(
                facts_path,
                "analysis context lockfile_path must name Cargo.lock".to_owned(),
            ));
        }
    }
    if let Some(path) = &context.toolchain_path {
        validate_relative_source_path(path)
            .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
        if !matches!(
            Path::new(path).file_name().and_then(|name| name.to_str()),
            Some("rust-toolchain" | "rust-toolchain.toml")
        ) {
            return Err(rust_callfacts_error(
                facts_path,
                "analysis context toolchain_path must name rust-toolchain.toml or rust-toolchain"
                    .to_owned(),
            ));
        }
    }

    let mut seen = BTreeSet::new();
    for file in &context.context_files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
        if !seen.insert(file.path.as_str()) {
            return Err(rust_callfacts_error(
                facts_path,
                format!("duplicate Rust context input: {}", file.path),
            ));
        }
        let path = root.join(&file.path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_failure("verify Rust context input", &path, error)),
        };
        if sha256_hex(&bytes) != file.sha256 {
            return Ok(false);
        }
    }
    if !seen.contains(context.manifest_path.as_str())
        || context
            .lockfile_path
            .as_deref()
            .is_some_and(|path| !seen.contains(path))
        || context
            .toolchain_path
            .as_deref()
            .is_some_and(|path| !seen.contains(path))
    {
        return Err(rust_callfacts_error(
            facts_path,
            "selected Rust context input is absent from context_files".to_owned(),
        ));
    }
    Ok(collect_rust_context_files(root)? == context.context_files)
}

fn rust_callfact_edges(
    facts: &[RustCallFact],
    rust_paths: &BTreeSet<String>,
    graph_edges: &[CodeGraphEdge],
    nodes: &[CodeGraphNode],
    facts_path: &Path,
) -> CodeGraphResult<(Vec<CodeGraphEdge>, BTreeSet<CallSiteKey>)> {
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let syntax_sites = graph_edges
        .iter()
        .filter(|edge| edge.kind == "calls")
        .filter_map(|edge| {
            let caller = nodes_by_id.get(edge.source.as_str())?;
            let path = caller.path.as_deref()?;
            if !rust_paths.contains(path) {
                return None;
            }
            let range = edge.range.as_ref()?;
            Some((
                (
                    path.to_owned(),
                    range.start_byte,
                    range.end_byte,
                    edge.source.clone(),
                ),
                range.clone(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut fact_sites = BTreeSet::new();
    let mut covered_sites = BTreeSet::new();
    let mut edges = Vec::with_capacity(facts.len());
    for fact in facts {
        let (edge, site) = rust_callfact_edge(
            fact,
            rust_paths,
            nodes,
            &nodes_by_id,
            &syntax_sites,
            facts_path,
        )?;
        if !fact_sites.insert((fact.path.as_str(), fact.start_byte, fact.end_byte)) {
            return Err(rust_callfacts_error(
                facts_path,
                format!(
                    "duplicate call-site fact: {}:{}-{}",
                    fact.path, fact.start_byte, fact.end_byte
                ),
            ));
        }
        covered_sites.insert(site);
        edges.push(edge);
    }
    Ok((edges, covered_sites))
}

fn rust_callfact_edge(
    fact: &RustCallFact,
    rust_paths: &BTreeSet<String>,
    nodes: &[CodeGraphNode],
    nodes_by_id: &HashMap<&str, &CodeGraphNode>,
    syntax_sites: &BTreeMap<CallSiteKey, CodeGraphRange>,
    facts_path: &Path,
) -> CodeGraphResult<(CodeGraphEdge, CallSiteKey)> {
    validate_relative_source_path(&fact.path)
        .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
    if !rust_paths.contains(&fact.path)
        || fact.start_byte >= fact.end_byte
        || fact.start_line == 0
        || fact.end_line < fact.start_line
        || fact.target_name.is_empty()
    {
        return Err(rust_callfacts_error(
            facts_path,
            format!(
                "invalid call-site range or non-Rust source: {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let caller_id = rust_symbol_id(&fact.caller, rust_paths, nodes, facts_path)?;
    let Some(caller) = nodes_by_id.get(caller_id.as_str()) else {
        return Err(rust_callfacts_error(
            facts_path,
            format!("missing caller node for {}", fact.path),
        ));
    };
    if caller.path.as_deref() != Some(fact.path.as_str())
        || !caller.range.as_ref().is_some_and(|range| {
            range.start_byte <= fact.start_byte && fact.end_byte <= range.end_byte
        })
        || fact.caller.start_byte > fact.start_byte
        || fact.end_byte > fact.caller.end_byte
    {
        return Err(rust_callfacts_error(
            facts_path,
            format!(
                "caller/range does not match source node at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let site = (
        fact.path.clone(),
        fact.start_byte,
        fact.end_byte,
        caller_id.clone(),
    );
    let Some(syntax_range) = syntax_sites.get(&site) else {
        return Err(rust_callfacts_error(
            facts_path,
            format!(
                "call-site range does not match parsed Rust syntax at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    };
    let fact_range = CodeGraphRange {
        start_byte: fact.start_byte,
        end_byte: fact.end_byte,
        start_line: fact.start_line,
        end_line: fact.end_line,
        start_column: fact.start_column,
        end_column: fact.end_column,
    };
    if syntax_range != &fact_range {
        return Err(rust_callfacts_error(
            facts_path,
            format!(
                "call-site coordinates do not match parsed syntax at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let (target, candidates, resolved) = rust_fact_target(fact, rust_paths, nodes, facts_path)?;
    Ok((
        CodeGraphEdge {
            kind: "calls".to_owned(),
            source: caller_id,
            target,
            target_name: Some(fact.target_name.clone()),
            resolved,
            ambiguous_candidates: candidates,
            resolution: Some(fact.resolution.clone()),
            range: Some(fact_range),
        },
        site,
    ))
}

fn rust_symbol_id(
    symbol: &RustCallFactsSymbol,
    rust_paths: &BTreeSet<String>,
    nodes: &[CodeGraphNode],
    facts_path: &Path,
) -> CodeGraphResult<String> {
    validate_relative_source_path(&symbol.path)
        .map_err(|reason| rust_callfacts_error(facts_path, reason))?;
    if !rust_paths.contains(&symbol.path) || symbol.start_byte >= symbol.end_byte {
        return Err(rust_callfacts_error(
            facts_path,
            format!("invalid Rust symbol range: {}", symbol.path),
        ));
    }
    let mut candidates = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "function" | "method"))
        .filter(|node| node.path.as_deref() == Some(symbol.path.as_str()))
        .filter(|node| {
            node.range.as_ref().is_some_and(|range| {
                range.start_byte < symbol.end_byte && symbol.start_byte < range.end_byte
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|node| {
        node.range
            .as_ref()
            .map_or(usize::MAX, |range| range.end_byte - range.start_byte)
    });
    match candidates.as_slice() {
        [node, ..] => Ok(node.id.clone()),
        [] => Err(rust_callfacts_error(
            facts_path,
            format!("Rust symbol is absent from graph snapshot: {}", symbol.path),
        )),
    }
}

fn rust_fact_target(
    fact: &RustCallFact,
    rust_paths: &BTreeSet<String>,
    nodes: &[CodeGraphNode],
    facts_path: &Path,
) -> CodeGraphResult<(Option<String>, Vec<String>, bool)> {
    let location = format!("{}:{}", fact.path, fact.start_line);
    match fact.resolution.as_str() {
        "static" => {
            let target = fact.target.as_ref().ok_or_else(|| {
                rust_callfacts_error(
                    facts_path,
                    format!("static Rust call has no target at {location}"),
                )
            })?;
            if !fact.possible_targets.is_empty() {
                return Err(rust_callfacts_error(
                    facts_path,
                    format!("static Rust call has possible targets at {location}"),
                ));
            }
            Ok((
                Some(rust_symbol_id(target, rust_paths, nodes, facts_path)?),
                Vec::new(),
                true,
            ))
        }
        "possible" | "trait-dispatch" | "ambiguous" => {
            if fact.target.is_some() {
                return Err(rust_callfacts_error(
                    facts_path,
                    format!("uncertain Rust call claims a definite target at {location}"),
                ));
            }
            let mut candidates = fact
                .possible_targets
                .iter()
                .map(|target| rust_symbol_id(target, rust_paths, nodes, facts_path))
                .collect::<CodeGraphResult<Vec<_>>>()?;
            candidates.sort();
            candidates.dedup();
            Ok((None, candidates, false))
        }
        "external" | "function-value" | "unresolved" => {
            if fact.target.is_some() || !fact.possible_targets.is_empty() {
                return Err(rust_callfacts_error(
                    facts_path,
                    format!(
                        "{} Rust call has an invalid target at {location}",
                        fact.resolution
                    ),
                ));
            }
            Ok((None, Vec::new(), false))
        }
        resolution => Err(rust_callfacts_error(
            facts_path,
            format!("unsupported Rust call resolution `{resolution}` at {location}"),
        )),
    }
}

#[derive(Clone, Debug)]
struct ExternalCallFactsSymbol {
    path: String,
    start_byte: usize,
    end_byte: usize,
}

#[derive(Clone, Debug)]
struct ExternalCallFact {
    path: String,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    start_column: usize,
    end_column: usize,
    caller: ExternalCallFactsSymbol,
    target_name: String,
    target: Option<ExternalCallFactsSymbol>,
    possible_targets: Vec<ExternalCallFactsSymbol>,
    resolution: String,
}

impl From<&TypeScriptCallFactsSymbol> for ExternalCallFactsSymbol {
    fn from(symbol: &TypeScriptCallFactsSymbol) -> Self {
        Self {
            path: symbol.path.clone(),
            start_byte: symbol.start_byte,
            end_byte: symbol.end_byte,
        }
    }
}

impl From<&PythonCallFactsSymbol> for ExternalCallFactsSymbol {
    fn from(symbol: &PythonCallFactsSymbol) -> Self {
        Self {
            path: symbol.path.clone(),
            start_byte: symbol.start_byte,
            end_byte: symbol.end_byte,
        }
    }
}

impl From<&TypeScriptCallFact> for ExternalCallFact {
    fn from(fact: &TypeScriptCallFact) -> Self {
        Self {
            path: fact.path.clone(),
            start_byte: fact.start_byte,
            end_byte: fact.end_byte,
            start_line: fact.start_line,
            end_line: fact.end_line,
            start_column: fact.start_column,
            end_column: fact.end_column,
            caller: (&fact.caller).into(),
            target_name: fact.target_name.clone(),
            target: fact.target.as_ref().map(Into::into),
            possible_targets: fact.possible_targets.iter().map(Into::into).collect(),
            resolution: fact.resolution.clone(),
        }
    }
}

impl From<&PythonCallFact> for ExternalCallFact {
    fn from(fact: &PythonCallFact) -> Self {
        Self {
            path: fact.path.clone(),
            start_byte: fact.start_byte,
            end_byte: fact.end_byte,
            start_line: fact.start_line,
            end_line: fact.end_line,
            start_column: fact.start_column,
            end_column: fact.end_column,
            caller: (&fact.caller).into(),
            target_name: fact.target_name.clone(),
            target: fact.target.as_ref().map(Into::into),
            possible_targets: fact.possible_targets.iter().map(Into::into).collect(),
            resolution: fact.resolution.clone(),
        }
    }
}

fn external_callfact_edges(
    facts: &[ExternalCallFact],
    source_paths: &BTreeSet<String>,
    graph_edges: &[CodeGraphEdge],
    nodes: &[CodeGraphNode],
    facts_path: &Path,
    language: &str,
) -> CodeGraphResult<(Vec<CodeGraphEdge>, BTreeSet<CallSiteKey>)> {
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let syntax_sites = graph_edges
        .iter()
        .filter(|edge| edge.kind == "calls")
        .filter_map(|edge| {
            let caller = nodes_by_id.get(edge.source.as_str())?;
            let path = caller.path.as_deref()?;
            if !source_paths.contains(path) {
                return None;
            }
            let range = edge.range.as_ref()?;
            Some((
                (
                    path.to_owned(),
                    range.start_byte,
                    range.end_byte,
                    edge.source.clone(),
                ),
                range.clone(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut fact_sites = BTreeSet::new();
    let mut covered_sites = BTreeSet::new();
    let mut edges = Vec::with_capacity(facts.len());
    for fact in facts {
        let (edge, site) = external_callfact_edge(
            fact,
            source_paths,
            nodes,
            &nodes_by_id,
            &syntax_sites,
            facts_path,
            language,
        )?;
        if !fact_sites.insert((fact.path.as_str(), fact.start_byte, fact.end_byte)) {
            return Err(external_callfacts_error(
                facts_path,
                language,
                format!(
                    "duplicate call-site fact: {}:{}-{}",
                    fact.path, fact.start_byte, fact.end_byte
                ),
            ));
        }
        covered_sites.insert(site);
        edges.push(edge);
    }
    Ok((edges, covered_sites))
}

fn external_callfact_edge(
    fact: &ExternalCallFact,
    source_paths: &BTreeSet<String>,
    nodes: &[CodeGraphNode],
    nodes_by_id: &HashMap<&str, &CodeGraphNode>,
    syntax_sites: &BTreeMap<CallSiteKey, CodeGraphRange>,
    facts_path: &Path,
    language: &str,
) -> CodeGraphResult<(CodeGraphEdge, CallSiteKey)> {
    validate_relative_source_path(&fact.path)
        .map_err(|reason| external_callfacts_error(facts_path, language, reason))?;
    if !source_paths.contains(&fact.path)
        || fact.start_byte >= fact.end_byte
        || fact.start_line == 0
        || fact.end_line < fact.start_line
        || fact.target_name.is_empty()
    {
        return Err(external_callfacts_error(
            facts_path,
            language,
            format!(
                "invalid call-site range or target name: {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let caller_id = external_symbol_id(&fact.caller, source_paths, nodes, facts_path, language)?;
    let Some(caller) = nodes_by_id.get(caller_id.as_str()) else {
        return Err(external_callfacts_error(
            facts_path,
            language,
            format!("missing caller node for {}", fact.path),
        ));
    };
    if caller.path.as_deref() != Some(fact.path.as_str())
        || !caller.range.as_ref().is_some_and(|range| {
            range.start_byte <= fact.start_byte && fact.end_byte <= range.end_byte
        })
        || fact.caller.start_byte > fact.start_byte
        || fact.end_byte > fact.caller.end_byte
    {
        return Err(external_callfacts_error(
            facts_path,
            language,
            format!(
                "caller/range does not match source node at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let site = (
        fact.path.clone(),
        fact.start_byte,
        fact.end_byte,
        caller_id.clone(),
    );
    let Some(syntax_range) = syntax_sites.get(&site) else {
        return Err(external_callfacts_error(
            facts_path,
            language,
            format!(
                "call-site range does not match parsed syntax at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    };
    let fact_range = CodeGraphRange {
        start_byte: fact.start_byte,
        end_byte: fact.end_byte,
        start_line: fact.start_line,
        end_line: fact.end_line,
        start_column: fact.start_column,
        end_column: fact.end_column,
    };
    if syntax_range != &fact_range {
        return Err(external_callfacts_error(
            facts_path,
            language,
            format!(
                "call-site coordinates do not match parsed syntax at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let (target, candidates, resolved) =
        external_fact_target(fact, source_paths, nodes, facts_path, language)?;
    Ok((
        CodeGraphEdge {
            kind: "calls".to_owned(),
            source: caller_id,
            target,
            target_name: Some(fact.target_name.clone()),
            resolved,
            ambiguous_candidates: candidates,
            resolution: Some(fact.resolution.clone()),
            range: Some(fact_range),
        },
        site,
    ))
}

fn external_symbol_id(
    symbol: &ExternalCallFactsSymbol,
    source_paths: &BTreeSet<String>,
    nodes: &[CodeGraphNode],
    facts_path: &Path,
    language: &str,
) -> CodeGraphResult<String> {
    validate_relative_source_path(&symbol.path)
        .map_err(|reason| external_callfacts_error(facts_path, language, reason))?;
    if !source_paths.contains(&symbol.path) || symbol.start_byte >= symbol.end_byte {
        return Err(external_callfacts_error(
            facts_path,
            language,
            format!("invalid symbol range: {}", symbol.path),
        ));
    }
    let mut candidates = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "function" | "method"))
        .filter(|node| node.path.as_deref() == Some(symbol.path.as_str()))
        .filter(|node| {
            node.range.as_ref().is_some_and(|range| {
                range.start_byte < symbol.end_byte && symbol.start_byte < range.end_byte
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|node| {
        node.range
            .as_ref()
            .map_or(usize::MAX, |range| range.end_byte - range.start_byte)
    });
    match candidates.as_slice() {
        [node, ..] => Ok(node.id.clone()),
        [] => Err(external_callfacts_error(
            facts_path,
            language,
            format!("symbol is absent from graph snapshot: {}", symbol.path),
        )),
    }
}

fn external_fact_target(
    fact: &ExternalCallFact,
    source_paths: &BTreeSet<String>,
    nodes: &[CodeGraphNode],
    facts_path: &Path,
    language: &str,
) -> CodeGraphResult<(Option<String>, Vec<String>, bool)> {
    let location = format!("{}:{}", fact.path, fact.start_line);
    match fact.resolution.as_str() {
        "static" => {
            let target = fact.target.as_ref().ok_or_else(|| {
                external_callfacts_error(
                    facts_path,
                    language,
                    format!("static call has no target at {location}"),
                )
            })?;
            if !fact.possible_targets.is_empty() {
                return Err(external_callfacts_error(
                    facts_path,
                    language,
                    format!("static call has possible targets at {location}"),
                ));
            }
            Ok((
                Some(external_symbol_id(
                    target,
                    source_paths,
                    nodes,
                    facts_path,
                    language,
                )?),
                Vec::new(),
                true,
            ))
        }
        "possible" | "ambiguous" | "interface-dispatch" | "trait-dispatch" => {
            if fact.target.is_some() {
                return Err(external_callfacts_error(
                    facts_path,
                    language,
                    format!("uncertain call claims a definite target at {location}"),
                ));
            }
            let mut candidates = fact
                .possible_targets
                .iter()
                .map(|target| external_symbol_id(target, source_paths, nodes, facts_path, language))
                .collect::<CodeGraphResult<Vec<_>>>()?;
            candidates.sort();
            candidates.dedup();
            Ok((None, candidates, false))
        }
        "external" | "function-value" | "unresolved" => {
            if fact.target.is_some() || !fact.possible_targets.is_empty() {
                return Err(external_callfacts_error(
                    facts_path,
                    language,
                    format!(
                        "{} call has an invalid target at {location}",
                        fact.resolution
                    ),
                ));
            }
            Ok((None, Vec::new(), false))
        }
        resolution => Err(external_callfacts_error(
            facts_path,
            language,
            format!("unsupported call resolution `{resolution}` at {location}"),
        )),
    }
}

fn validate_go_callfacts_sources(
    root: &Path,
    artifact: &CodeGraphArtifact,
    facts: &GoCallFactsArtifact,
    facts_path: &Path,
) -> CodeGraphResult<Option<BTreeSet<String>>> {
    let expected = artifact
        .files
        .iter()
        .filter(|file| file.language == "go")
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeMap::new();
    for file in &facts.files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| go_callfacts_error(facts_path, reason))?;
        if actual
            .insert(file.path.as_str(), file.sha256.as_str())
            .is_some()
        {
            return Err(go_callfacts_error(
                facts_path,
                format!("duplicate source file: {}", file.path),
            ));
        }
    }
    if expected != actual {
        return Ok(None);
    }
    for (relative_path, expected_digest) in &expected {
        let path = root.join(relative_path);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_failure("verify Go call-facts source", &path, error)),
        };
        if sha256_hex(&bytes) != *expected_digest {
            return Ok(None);
        }
    }
    Ok(Some(
        expected.keys().map(|path| (*path).to_owned()).collect(),
    ))
}

fn validate_go_callfacts_context(
    root: &Path,
    context: &GoCallFactsContext,
    expected_fingerprint: &str,
    facts_path: &Path,
) -> CodeGraphResult<bool> {
    validate_context_settings(context, facts_path)?;
    validate_context_selections(context, facts_path)?;
    validate_context_file_list(context, facts_path)?;
    if go_callfacts_context_sha256(context) != expected_fingerprint {
        return Err(go_callfacts_error(
            facts_path,
            "analysis context fingerprint does not match its contents".to_owned(),
        ));
    }

    let current_files = collect_go_context_files(root)?;
    Ok(current_files == context.context_files)
}

fn validate_context_settings(
    context: &GoCallFactsContext,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    if context.go_version.is_empty()
        || context.settings.get("GOOS").is_none_or(String::is_empty)
        || context.settings.get("GOARCH").is_none_or(String::is_empty)
        || context
            .settings
            .get("CGO_ENABLED")
            .is_none_or(String::is_empty)
    {
        return Err(go_callfacts_error(
            facts_path,
            "analysis context is missing its Go version or target settings".to_owned(),
        ));
    }
    let expected_settings = [
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
    .collect::<BTreeSet<_>>();
    if context
        .settings
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected_settings
    {
        return Err(go_callfacts_error(
            facts_path,
            "analysis context has an unsupported Go setting set".to_owned(),
        ));
    }
    Ok(())
}

fn validate_context_selections(
    context: &GoCallFactsContext,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    if context.go_mod.is_empty() {
        return Err(go_callfacts_error(
            facts_path,
            "analysis context must select an in-root go.mod".to_owned(),
        ));
    }
    validate_context_selected_file(&context.go_mod, "go.mod", facts_path)?;
    if !context.go_work.is_empty() && context.go_work != "off" {
        validate_context_selected_file(&context.go_work, "go.work", facts_path)?;
    }
    Ok(())
}

fn validate_context_selected_file(
    path: &str,
    expected_name: &str,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    validate_relative_source_path(path).map_err(|reason| go_callfacts_error(facts_path, reason))?;
    if Path::new(path)
        .file_name()
        .is_none_or(|name| name != expected_name)
    {
        return Err(go_callfacts_error(
            facts_path,
            format!("active context input must name a {expected_name} file"),
        ));
    }
    Ok(())
}

fn validate_context_file_list(
    context: &GoCallFactsContext,
    facts_path: &Path,
) -> CodeGraphResult<()> {
    let mut seen = BTreeSet::new();
    for file in &context.context_files {
        validate_relative_source_path(&file.path)
            .map_err(|reason| go_callfacts_error(facts_path, reason))?;
        if !is_go_context_input_path(Path::new(&file.path)) {
            return Err(go_callfacts_error(
                facts_path,
                format!("unsupported Go context input: {}", file.path),
            ));
        }
        if !seen.insert(file.path.as_str()) {
            return Err(go_callfacts_error(
                facts_path,
                format!("duplicate Go context input: {}", file.path),
            ));
        }
    }
    if !seen.contains(context.go_mod.as_str()) {
        return Err(go_callfacts_error(
            facts_path,
            "active go.mod is absent from analysis context inputs".to_owned(),
        ));
    }
    if !context.go_work.is_empty()
        && context.go_work != "off"
        && !seen.contains(context.go_work.as_str())
    {
        return Err(go_callfacts_error(
            facts_path,
            "active go.work is absent from analysis context inputs".to_owned(),
        ));
    }
    Ok(())
}

fn go_callfacts_context_sha256(context: &GoCallFactsContext) -> String {
    let mut hasher = Sha256::new();
    hash_context_part(&mut hasher, "zvec-grep.go-callfacts-context-v1");
    hash_context_part(&mut hasher, &context.go_version);
    hash_context_part(&mut hasher, &context.go_mod);
    hash_context_part(&mut hasher, &context.go_work);
    for (name, value) in &context.settings {
        hash_context_part(&mut hasher, name);
        hash_context_part(&mut hasher, value);
    }
    let mut context_files = context.context_files.clone();
    context_files.sort_by(|left, right| left.path.cmp(&right.path));
    for file in &context_files {
        hash_context_part(&mut hasher, &file.path);
        hash_context_part(&mut hasher, &file.sha256);
    }
    hex::encode(hasher.finalize())
}

fn hash_context_part(hasher: &mut Sha256, value: &str) {
    hasher.update(value.as_bytes());
    hasher.update([0]);
}

fn rust_callfacts_context_sha256(context: &RustCallFactsContext) -> String {
    let mut hasher = Sha256::new();
    hash_context_part(&mut hasher, "zvec-grep.rust-callfacts-context-v1");
    hash_context_part(&mut hasher, &context.rustc_version);
    hash_context_part(&mut hasher, &context.rustc_commit);
    hash_context_part(&mut hasher, &context.host);
    hash_context_part(&mut hasher, &context.target);
    hash_context_part(&mut hasher, &context.edition);
    hash_context_part(&mut hasher, &context.manifest_path);
    hash_context_part(
        &mut hasher,
        context.lockfile_path.as_deref().unwrap_or_default(),
    );
    hash_context_part(
        &mut hasher,
        context.toolchain_path.as_deref().unwrap_or_default(),
    );
    for (name, value) in &context.settings {
        hash_context_part(&mut hasher, name);
        hash_context_part(&mut hasher, value);
    }
    let mut context_files = context.context_files.clone();
    context_files.sort_by(|left, right| left.path.cmp(&right.path));
    for file in &context_files {
        hash_context_part(&mut hasher, &file.path);
        hash_context_part(&mut hasher, &file.sha256);
    }
    hex::encode(hasher.finalize())
}

fn typescript_callfacts_context_sha256(context: &TypeScriptCallFactsContext) -> String {
    let mut hasher = Sha256::new();
    hash_context_part(&mut hasher, "zvec-grep.typescript-callfacts-context-v1");
    hash_context_part(&mut hasher, &context.typescript_version);
    hash_context_part(&mut hasher, &context.node_version);
    hash_context_part(&mut hasher, &context.project_path);
    hash_context_part(&mut hasher, &context.target);
    hash_context_part(&mut hasher, &context.module);
    hash_context_part(&mut hasher, &context.jsx);
    for (name, value) in &context.settings {
        hash_context_part(&mut hasher, name);
        hash_context_part(&mut hasher, value);
    }
    let mut context_files = context.context_files.clone();
    context_files.sort_by(|left, right| left.path.cmp(&right.path));
    for file in &context_files {
        hash_context_part(&mut hasher, &file.path);
        hash_context_part(&mut hasher, &file.sha256);
    }
    hex::encode(hasher.finalize())
}

fn python_callfacts_context_sha256(context: &PythonCallFactsContext) -> String {
    let mut hasher = Sha256::new();
    hash_context_part(&mut hasher, "zvec-grep.python-callfacts-context-v1");
    hash_context_part(&mut hasher, &context.pyright_version);
    hash_context_part(&mut hasher, &context.python_version);
    hash_context_part(&mut hasher, &context.target_version);
    hash_context_part(&mut hasher, &context.project_path);
    hash_context_part(&mut hasher, &context.typeshed_sha256);
    for (name, value) in &context.settings {
        hash_context_part(&mut hasher, name);
        hash_context_part(&mut hasher, value);
    }
    let mut context_files = context.context_files.clone();
    context_files.sort_by(|left, right| left.path.cmp(&right.path));
    for file in &context_files {
        hash_context_part(&mut hasher, &file.path);
        hash_context_part(&mut hasher, &file.sha256);
    }
    hex::encode(hasher.finalize())
}

type GoSymbolIndex = HashMap<String, Vec<String>>;
type GraphNodeIndex<'a> = HashMap<&'a str, &'a CodeGraphNode>;
type CallSiteKey = (String, usize, usize, String);

fn graph_callfact_indexes<'a>(
    artifact: &'a CodeGraphArtifact,
    facts_path: &Path,
) -> CodeGraphResult<(GoSymbolIndex, GraphNodeIndex<'a>)> {
    let mut symbols: HashMap<String, Vec<String>> = HashMap::new();
    let mut nodes_by_id = HashMap::new();
    for node in &artifact.nodes {
        nodes_by_id.insert(node.id.as_str(), node);
        if matches!(node.kind.as_str(), "function" | "method")
            && let (Some(source_path), Some(qualified_name)) =
                (node.path.as_deref(), node.qualified_name.as_deref())
        {
            symbols
                .entry(format!("{source_path}::{qualified_name}"))
                .or_default()
                .push(node.id.clone());
        }
    }
    for (symbol, ids) in &symbols {
        if ids.len() != 1 {
            return Err(go_callfacts_error(
                facts_path,
                format!("symbol identity is not unique in graph snapshot: {symbol}"),
            ));
        }
    }
    Ok((symbols, nodes_by_id))
}

fn go_callfact_edges(
    facts: &[GoCallFact],
    go_paths: &BTreeSet<String>,
    symbols: &GoSymbolIndex,
    nodes_by_id: &GraphNodeIndex<'_>,
    graph_edges: &[CodeGraphEdge],
    facts_path: &Path,
) -> CodeGraphResult<(Vec<CodeGraphEdge>, BTreeSet<CallSiteKey>)> {
    let syntax_sites = graph_edges
        .iter()
        .filter(|edge| edge.kind == "calls")
        .filter_map(|edge| {
            let caller = nodes_by_id.get(edge.source.as_str())?;
            let path = caller.path.as_deref()?;
            let range = edge.range.as_ref()?;
            Some((
                (
                    path.to_owned(),
                    range.start_byte,
                    range.end_byte,
                    edge.source.clone(),
                ),
                range.clone(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut fact_sites = BTreeSet::new();
    let mut covered_sites = BTreeSet::new();
    let mut edges = Vec::with_capacity(facts.len());
    for fact in facts {
        let (edge, site) = go_callfact_edge(
            fact,
            go_paths,
            symbols,
            nodes_by_id,
            &syntax_sites,
            facts_path,
        )?;
        if !fact_sites.insert((fact.path.as_str(), fact.start_byte, fact.end_byte)) {
            return Err(go_callfacts_error(
                facts_path,
                format!(
                    "duplicate call-site fact: {}:{}-{}",
                    fact.path, fact.start_byte, fact.end_byte
                ),
            ));
        }
        covered_sites.insert(site);
        edges.push(edge);
    }
    Ok((edges, covered_sites))
}

fn go_callfact_edge(
    fact: &GoCallFact,
    go_paths: &BTreeSet<String>,
    symbols: &GoSymbolIndex,
    nodes_by_id: &GraphNodeIndex<'_>,
    syntax_sites: &BTreeMap<CallSiteKey, CodeGraphRange>,
    facts_path: &Path,
) -> CodeGraphResult<(CodeGraphEdge, CallSiteKey)> {
    validate_relative_source_path(&fact.path)
        .map_err(|reason| go_callfacts_error(facts_path, reason))?;
    if !go_paths.contains(&fact.path)
        || fact.start_byte >= fact.end_byte
        || fact.start_line == 0
        || fact.end_line < fact.start_line
    {
        return Err(go_callfacts_error(
            facts_path,
            format!(
                "invalid call-site range or non-Go source: {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let caller_id = unique_symbol_id(symbols, &fact.caller).map_err(|reason| {
        go_callfacts_error(
            facts_path,
            format!("{reason} at {}:{}", fact.path, fact.start_line),
        )
    })?;
    let Some(caller) = nodes_by_id.get(caller_id.as_str()) else {
        return Err(go_callfacts_error(
            facts_path,
            format!("missing caller node: {}", fact.caller),
        ));
    };
    if caller.path.as_deref() != Some(fact.path.as_str())
        || !caller.range.as_ref().is_some_and(|range| {
            range.start_byte <= fact.start_byte && fact.end_byte <= range.end_byte
        })
    {
        return Err(go_callfacts_error(
            facts_path,
            format!(
                "caller/range does not match source node at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let site = (
        fact.path.clone(),
        fact.start_byte,
        fact.end_byte,
        caller_id.clone(),
    );
    let Some(syntax_range) = syntax_sites.get(&site) else {
        return Err(go_callfacts_error(
            facts_path,
            format!(
                "call-site range does not match parsed syntax at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    };
    let fact_range = CodeGraphRange {
        start_byte: fact.start_byte,
        end_byte: fact.end_byte,
        start_line: fact.start_line,
        end_line: fact.end_line,
        start_column: fact.start_column,
        end_column: fact.end_column,
    };
    if syntax_range != &fact_range {
        return Err(go_callfacts_error(
            facts_path,
            format!(
                "call-site coordinates do not match parsed syntax at {}:{}",
                fact.path, fact.start_line
            ),
        ));
    }
    let (target, candidates, resolved) = go_fact_target(fact, go_paths, symbols, facts_path)?;
    Ok((
        CodeGraphEdge {
            kind: "calls".to_owned(),
            source: caller_id,
            target,
            target_name: Some(fact.target_name.clone()),
            resolved,
            ambiguous_candidates: candidates,
            resolution: Some(fact.resolution.clone()),
            range: Some(CodeGraphRange {
                start_byte: fact.start_byte,
                end_byte: fact.end_byte,
                start_line: fact.start_line,
                end_line: fact.end_line,
                start_column: fact.start_column,
                end_column: fact.end_column,
            }),
        },
        site,
    ))
}

fn go_fact_target(
    fact: &GoCallFact,
    go_paths: &BTreeSet<String>,
    symbols: &GoSymbolIndex,
    facts_path: &Path,
) -> CodeGraphResult<(Option<String>, Vec<String>, bool)> {
    let location = format!("{}:{}", fact.path, fact.start_line);
    match fact.resolution.as_str() {
        "static" => {
            let target = fact.target.as_deref().ok_or_else(|| {
                go_callfacts_error(
                    facts_path,
                    format!("static call has no target at {location}"),
                )
            })?;
            if !fact.possible_targets.is_empty() {
                return Err(go_callfacts_error(
                    facts_path,
                    format!("static call has possible targets at {location}"),
                ));
            }
            Ok((
                Some(
                    unique_go_symbol_id(symbols, go_paths, target).map_err(|reason| {
                        go_callfacts_error(facts_path, format!("{reason} at {location}"))
                    })?,
                ),
                Vec::new(),
                true,
            ))
        }
        "interface-dispatch" => {
            if fact.target.is_some() {
                return Err(go_callfacts_error(
                    facts_path,
                    format!("interface call claims a definite target at {location}"),
                ));
            }
            let mut candidates = fact
                .possible_targets
                .iter()
                .map(|candidate| {
                    unique_go_symbol_id(symbols, go_paths, candidate).map_err(|reason| {
                        go_callfacts_error(facts_path, format!("{reason} at {location}"))
                    })
                })
                .collect::<CodeGraphResult<Vec<_>>>()?;
            candidates.sort();
            candidates.dedup();
            Ok((None, candidates, false))
        }
        "external" | "function-value" | "unresolved" => {
            if fact.target.is_some() || !fact.possible_targets.is_empty() {
                return Err(go_callfacts_error(
                    facts_path,
                    format!(
                        "{} call has an invalid target at {location}",
                        fact.resolution
                    ),
                ));
            }
            Ok((None, Vec::new(), false))
        }
        resolution => Err(go_callfacts_error(
            facts_path,
            format!("unsupported call resolution `{resolution}` at {location}"),
        )),
    }
}

fn unique_go_symbol_id(
    symbols: &GoSymbolIndex,
    go_paths: &BTreeSet<String>,
    key: &str,
) -> Result<String, String> {
    let Some((path, _)) = key.rsplit_once("::") else {
        return Err(format!("invalid Go symbol identity: {key}"));
    };
    if !go_paths.contains(path) {
        return Err(format!("Go symbol is outside the source snapshot: {key}"));
    }
    unique_symbol_id(symbols, key)
}

fn go_callfacts_error(path: &Path, reason: String) -> CodeGraphError {
    CodeGraphError::GoCallFacts {
        path: path.to_path_buf(),
        reason,
    }
}

fn rust_callfacts_error(path: &Path, reason: String) -> CodeGraphError {
    CodeGraphError::RustCallFacts {
        path: path.to_path_buf(),
        reason,
    }
}

fn external_callfacts_error(path: &Path, language: &str, reason: String) -> CodeGraphError {
    match language {
        "TypeScript" => typescript_callfacts_error(path, reason),
        "Python" => python_callfacts_error(path, reason),
        _ => CodeGraphError::Io {
            operation: format!("validate {language} call-facts artifact"),
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, reason),
        },
    }
}

fn typescript_callfacts_error(path: &Path, reason: String) -> CodeGraphError {
    CodeGraphError::TypeScriptCallFacts {
        path: path.to_path_buf(),
        reason,
    }
}

fn python_callfacts_error(path: &Path, reason: String) -> CodeGraphError {
    CodeGraphError::PythonCallFacts {
        path: path.to_path_buf(),
        reason,
    }
}

fn restore_syntax_call_edges(artifact: &mut CodeGraphArtifact) {
    resolve_call_edges(&mut artifact.edges, &artifact.nodes);
    artifact.go_callfacts_context_sha256 = None;
    artifact.rust_callfacts_context_sha256 = None;
    artifact.typescript_callfacts_context_sha256 = None;
    artifact.python_callfacts_context_sha256 = None;
}

fn manifest_key_with_callfacts(
    base_manifest: String,
    go_digest: Option<&str>,
    rust_digest: Option<&str>,
    typescript_digest: Option<&str>,
    python_digest: Option<&str>,
) -> String {
    let mut manifest = base_manifest.into_bytes();
    if let Some(digest) = go_digest {
        manifest.extend_from_slice(b"\0go-callfacts\0");
        manifest.extend_from_slice(digest.as_bytes());
    }
    if let Some(digest) = rust_digest {
        manifest.extend_from_slice(b"\0rust-callfacts\0");
        manifest.extend_from_slice(digest.as_bytes());
    }
    if let Some(digest) = typescript_digest {
        manifest.extend_from_slice(b"\0typescript-callfacts\0");
        manifest.extend_from_slice(digest.as_bytes());
    }
    if let Some(digest) = python_digest {
        manifest.extend_from_slice(b"\0python-callfacts\0");
        manifest.extend_from_slice(digest.as_bytes());
    }
    if go_digest.is_none()
        && rust_digest.is_none()
        && typescript_digest.is_none()
        && python_digest.is_none()
    {
        return String::from_utf8(manifest).expect("base manifest is UTF-8");
    }
    sha256_hex(&manifest)
}

fn update_callfacts_manifest(root: &Path, artifact: &mut CodeGraphArtifact) -> CodeGraphResult<()> {
    let go_digest = if artifact.go_callfacts_context_sha256.is_some() {
        Some(read_callfacts_digest(root, GO_CALLFACTS_FILE, "Go")?)
    } else {
        None
    };
    let rust_digest = if artifact.rust_callfacts_context_sha256.is_some() {
        Some(read_callfacts_digest(root, RUST_CALLFACTS_FILE, "Rust")?)
    } else {
        None
    };
    let typescript_digest = if artifact.typescript_callfacts_context_sha256.is_some() {
        Some(read_callfacts_digest(
            root,
            TYPESCRIPT_CALLFACTS_FILE,
            "TypeScript",
        )?)
    } else {
        None
    };
    let python_digest = if artifact.python_callfacts_context_sha256.is_some() {
        Some(read_callfacts_digest(
            root,
            PYTHON_CALLFACTS_FILE,
            "Python",
        )?)
    } else {
        None
    };
    let base_manifest = package_manifest_key_for_root(root, &artifact.files)?;
    artifact.manifest_key = manifest_key_with_callfacts(
        base_manifest,
        go_digest.as_deref(),
        rust_digest.as_deref(),
        typescript_digest.as_deref(),
        python_digest.as_deref(),
    );
    Ok(())
}

fn package_manifest_key_for_root(root: &Path, files: &[CodeGraphFile]) -> CodeGraphResult<String> {
    Ok(package_manifest_key(
        files,
        &collect_package_manifests(root)?,
    ))
}

fn read_callfacts_digest(root: &Path, filename: &str, language: &str) -> CodeGraphResult<String> {
    let path = root.join(".zvec-grep").join(filename);
    let bytes = fs::read(&path).map_err(|error| {
        io_failure(
            &format!("read {language} call-facts artifact"),
            &path,
            error,
        )
    })?;
    Ok(sha256_hex(&bytes))
}

fn validate_relative_source_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.to_string_lossy().contains('\\')
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::CurDir
            )
        })
    {
        return Err(format!("invalid relative source path: {}", path.display()));
    }
    Ok(())
}

fn unique_symbol_id(symbols: &HashMap<String, Vec<String>>, key: &str) -> Result<String, String> {
    match symbols.get(key).map(Vec::as_slice) {
        Some([id]) => Ok(id.clone()),
        Some(_) => Err(format!("symbol identity is ambiguous: {key}")),
        None => Err(format!("symbol is absent from graph snapshot: {key}")),
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

fn current_callfacts_digest(
    root: &Path,
    filename: &str,
    language: &str,
) -> CodeGraphResult<Option<String>> {
    let path = root.join(".zvec-grep").join(filename);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(
            &format!("read {language} call-facts artifact"),
            &path,
            error,
        )),
    }
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
                artifact.schema == CODEGRAPH_SCHEMA
                    && artifact.version == CODEGRAPH_VERSION
                    && has_current_relation_generation(artifact)
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
    let package_manifests = collect_package_manifests(&root)?;
    let current_go_callfacts_digest = current_callfacts_digest(&root, GO_CALLFACTS_FILE, "Go")?;
    let current_rust_callfacts_digest =
        current_callfacts_digest(&root, RUST_CALLFACTS_FILE, "Rust")?;
    let current_typescript_callfacts_digest =
        current_callfacts_digest(&root, TYPESCRIPT_CALLFACTS_FILE, "TypeScript")?;
    let current_python_callfacts_digest =
        current_callfacts_digest(&root, PYTHON_CALLFACTS_FILE, "Python")?;

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
        let expected_manifest = manifest_key_with_callfacts(
            package_manifest_key(&base.files, &package_manifests),
            current_go_callfacts_digest.as_deref(),
            current_rust_callfacts_digest.as_deref(),
            current_typescript_callfacts_digest.as_deref(),
            current_python_callfacts_digest.as_deref(),
        );
        // Package manifests and call-facts sidecars are not graph files, but
        // both can change the graph. Their digests are part of the expected
        // manifest so a context-only change cannot return a stale snapshot.
        let has_callfacts = current_go_callfacts_digest.is_some()
            || current_rust_callfacts_digest.is_some()
            || current_typescript_callfacts_digest.is_some()
            || current_python_callfacts_digest.is_some();
        if changes.is_empty() && !has_callfacts && base.manifest_key == expected_manifest {
            return Ok((artifact_path, base));
        }
        if base.manifest_key != expected_manifest {
            build_codegraph(&root)?
        } else if changes.is_empty() {
            let mut refreshed = base;
            apply_semantic_callfacts(&root, &mut refreshed)?;
            refreshed
        } else {
            update_codegraph(&base, &root, &changes)?
        }
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

const MAX_PACKAGE_MANIFEST_BYTES: usize = 2_000_000;

fn collect_package_manifests(root: &Path) -> CodeGraphResult<Vec<PackageManifest>> {
    let mut paths = Vec::new();
    collect_package_manifest_paths(root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path)
                .map_err(|error| io_failure("read package manifest", &path, error))?;
            let ecosystem = package_manifest_ecosystem(&path)
                .expect("package manifest path was filtered before parsing")
                .to_owned();
            let (name, dependencies) = if bytes.len() <= MAX_PACKAGE_MANIFEST_BYTES {
                parse_package_manifest(&ecosystem, &bytes)
            } else {
                (None, Vec::new())
            };
            Ok(PackageManifest {
                path: relative_path(root, &path),
                sha256: sha256_hex(&bytes),
                ecosystem,
                name,
                dependencies,
            })
        })
        .collect()
}

fn collect_package_manifest_paths(root: &Path, output: &mut Vec<PathBuf>) -> CodeGraphResult<()> {
    let entries =
        fs::read_dir(root).map_err(|error| io_failure("scan package manifests", root, error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| io_failure("read package manifest directory entry", root, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect package manifest path", &path, error))?;
        if file_type.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(
                    ".git"
                        | ".zvec-grep"
                        | "node_modules"
                        | "target"
                        | "dist"
                        | ".venv"
                        | "venv"
                        | "__pycache__"
                )
            ) {
                continue;
            }
            collect_package_manifest_paths(&path, output)?;
        } else if file_type.is_file() && package_manifest_ecosystem(&path).is_some() {
            output.push(path);
        }
    }
    Ok(())
}

fn package_manifest_ecosystem(path: &Path) -> Option<&'static str> {
    match path.file_name()?.to_str()?.to_ascii_lowercase().as_str() {
        "go.mod" => Some("go"),
        "cargo.toml" => Some("rust"),
        "pyproject.toml" => Some("python"),
        "package.json" => Some("typescript"),
        _ => None,
    }
}

fn parse_package_manifest(ecosystem: &str, bytes: &[u8]) -> (Option<String>, Vec<String>) {
    let (name, dependencies) = match ecosystem {
        "go" => match std::str::from_utf8(bytes) {
            Ok(text) => parse_go_manifest(text),
            Err(_) => return (None, Vec::new()),
        },
        "rust" => match std::str::from_utf8(bytes) {
            Ok(text) => parse_cargo_manifest(text),
            Err(_) => return (None, Vec::new()),
        },
        "python" => match std::str::from_utf8(bytes) {
            Ok(text) => parse_pyproject_manifest(text),
            Err(_) => return (None, Vec::new()),
        },
        "typescript" => parse_package_json_manifest(bytes),
        _ => (None, Vec::new()),
    };
    let dependencies = dependencies
        .into_iter()
        .filter(|dependency| !dependency.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (name.filter(|name| !name.is_empty()), dependencies)
}

fn parse_go_manifest(text: &str) -> (Option<String>, Vec<String>) {
    let mut name = None;
    let mut dependencies = Vec::new();
    let mut in_require_block = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if name.is_none()
            && let Some(module) = line.strip_prefix("module")
            && module.chars().next().is_some_and(char::is_whitespace)
            && let Some(module) = module.split_whitespace().next()
        {
            name = Some(module.to_owned());
            continue;
        }
        if in_require_block {
            if line == ")" {
                in_require_block = false;
            } else if let Some(dependency) = go_require_name(line) {
                dependencies.push(dependency);
            }
            continue;
        }
        if let Some(requirement) = line.strip_prefix("require") {
            let requirement = requirement.trim_start();
            if requirement.starts_with('(') {
                in_require_block = true;
            } else if let Some(dependency) = go_require_name(requirement) {
                dependencies.push(dependency);
            }
        }
    }
    if in_require_block {
        return (None, Vec::new());
    }
    (name, dependencies)
}

fn go_require_name(line: &str) -> Option<String> {
    let mut fields = line.split_whitespace();
    let dependency = fields.next()?;
    let version = fields.next()?;
    version.starts_with('v').then(|| dependency.to_owned())
}

fn parse_cargo_manifest(text: &str) -> (Option<String>, Vec<String>) {
    let mut name = None;
    let mut dependencies = Vec::new();
    let Some(assignments) = toml_assignments(text) else {
        return (None, Vec::new());
    };
    for (section, key, value) in assignments {
        if section == "package" && key == "name" {
            name = toml_string_value(&value);
        }
        let dependency_section = section == "dependencies"
            || (section.starts_with("target.") && section.ends_with(".dependencies"));
        if dependency_section {
            dependencies.push(key);
        }
    }
    (name, dependencies)
}

fn parse_pyproject_manifest(text: &str) -> (Option<String>, Vec<String>) {
    let mut name = None;
    let mut dependencies = Vec::new();
    let Some(assignments) = toml_assignments(text) else {
        return (None, Vec::new());
    };
    for (section, key, value) in assignments {
        match (section.as_str(), key.as_str()) {
            ("project", "name") => name = toml_string_value(&value),
            ("project", "dependencies") => {
                let Some(specifications) = toml_array_strings(&value) else {
                    return (None, Vec::new());
                };
                dependencies.extend(specifications.into_iter().map(|spec| pep508_name(&spec)));
            }
            ("tool.poetry", "name") if name.is_none() => {
                name = toml_string_value(&value);
            }
            ("tool.poetry.dependencies", "python") => {}
            ("tool.poetry.dependencies", _) => dependencies.push(key),
            _ => {}
        }
    }
    (name, dependencies)
}

fn parse_package_json_manifest(bytes: &[u8]) -> (Option<String>, Vec<String>) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return (None, Vec::new());
    };
    let Some(object) = value.as_object() else {
        return (None, Vec::new());
    };
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let dependencies = object
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|dependencies| dependencies.keys().cloned())
        .collect();
    (name, dependencies)
}

fn toml_assignments(text: &str) -> Option<Vec<(String, String, String)>> {
    let mut section = String::new();
    let mut assignments = Vec::new();
    let mut pending: Option<(String, String, String, usize)> = None;

    for raw_line in text.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some((pending_section, pending_key, pending_value, depth)) = pending.as_mut() {
            pending_value.push(' ');
            pending_value.push_str(line);
            *depth = toml_container_balance(pending_value)?;
            if *depth == 0 {
                assignments.push((
                    pending_section.clone(),
                    pending_key.clone(),
                    pending_value.clone(),
                ));
                pending = None;
            }
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            if header.starts_with('[') {
                // Array-of-table headers such as [[bin]] are not dependency
                // tables. Clear the previous section so following keys cannot
                // be misclassified as dependencies.
                section.clear();
            } else {
                header.trim().clone_into(&mut section);
            }
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let key = toml_key(key);
        let value = value.trim().to_owned();
        if key.is_empty() || value.is_empty() {
            return None;
        }
        let depth = toml_container_balance(&value)?;
        if depth > 0 {
            pending = Some((section.clone(), key, value, depth));
        } else {
            assignments.push((section.clone(), key, value));
        }
    }
    if pending.is_some() {
        return None;
    }
    Some(assignments)
}

fn strip_toml_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, character) {
            (Some('"'), '\\') => escaped = true,
            (Some(current), character) if current == character => quote = None,
            (None, '"' | '\'') => quote = Some(character),
            (None, '#') => return &line[..index],
            _ => {}
        }
    }
    line
}

fn toml_container_balance(value: &str) -> Option<usize> {
    let mut containers = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut unicode_digits = 0;
    let mut unicode_value = 0_u32;
    for character in value.chars() {
        if let Some(current_quote) = quote {
            if current_quote == '"' {
                if unicode_digits > 0 {
                    let digit = character.to_digit(16)?;
                    unicode_value = unicode_value.checked_mul(16)?.checked_add(digit)?;
                    unicode_digits -= 1;
                    if unicode_digits == 0 && char::from_u32(unicode_value).is_none() {
                        return None;
                    }
                } else if escaped {
                    if !matches!(character, 'b' | 't' | 'n' | 'f' | 'r' | '"' | '\\') {
                        if character == 'u' {
                            unicode_digits = 4;
                            unicode_value = 0;
                        } else if character == 'U' {
                            unicode_digits = 8;
                            unicode_value = 0;
                        } else {
                            return None;
                        }
                    }
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == current_quote {
                    quote = None;
                }
            } else if character == current_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '[' | '{' => containers.push(character),
            ']' | '}' => {
                let expected = if character == ']' { '[' } else { '{' };
                if containers.pop() != Some(expected) {
                    return None;
                }
            }
            _ => {}
        }
    }
    (quote.is_none() && !escaped && unicode_digits == 0).then_some(containers.len())
}

fn toml_key(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn toml_string_value(value: &str) -> Option<String> {
    let value = value.trim();
    let quote = value.chars().next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let mut escaped = false;
    let end = value
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| {
            if quote == '"' && escaped {
                escaped = false;
                return None;
            }
            if quote == '"' && character == '\\' {
                escaped = true;
                return None;
            }
            (character == quote).then_some(index)
        })?;
    if !value[end + 1..].trim().is_empty() {
        return None;
    }
    if quote == '"' {
        let encoded = &value[..=end];
        return serde_json::from_str(encoded)
            .ok()
            .or_else(|| Some(value[1..end].to_owned()));
    }
    Some(value[1..end].to_owned())
}

fn toml_array_strings(value: &str) -> Option<Vec<String>> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('[') || !value.ends_with(']') {
        return None;
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value[1..value.len() - 1].chars() {
        if let Some(current_quote) = quote {
            current.push(character);
            if escaped {
                escaped = false;
            } else if current_quote == '"' && character == '\\' {
                escaped = true;
            } else if character == current_quote {
                quote = None;
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
            current.push(character);
        } else if character == ',' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    parts.push(current);
    if parts.len() == 1 && parts[0].trim().is_empty() {
        return Some(Vec::new());
    }

    let mut strings = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        let part = part.trim();
        if part.is_empty() {
            if index + 1 == parts.len() {
                continue;
            }
            return None;
        }
        strings.push(toml_string_value(part)?);
    }
    Some(strings)
}

fn pep508_name(spec: &str) -> String {
    let end = spec
        .char_indices()
        .find(|(_, character)| {
            matches!(
                character,
                ' ' | '\t' | '<' | '>' | '=' | '!' | '~' | ';' | '[' | '('
            )
        })
        .map_or(spec.len(), |(index, _)| index);
    spec[..end].to_owned()
}

fn collect_go_context_files(root: &Path) -> CodeGraphResult<Vec<GoCallFactsFile>> {
    let mut paths = Vec::new();
    collect_go_context_paths(root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path)
                .map_err(|error| io_failure("read Go context input", &path, error))?;
            Ok(GoCallFactsFile {
                path: relative_path(root, &path),
                sha256: sha256_hex(&bytes),
            })
        })
        .collect()
}

fn collect_go_context_paths(root: &Path, output: &mut Vec<PathBuf>) -> CodeGraphResult<()> {
    let entries =
        fs::read_dir(root).map_err(|error| io_failure("scan Go context inputs", root, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| io_failure("read Go context directory entry", root, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect Go context input", &path, error))?;
        if file_type.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | ".zvec-grep" | "node_modules")
            ) {
                continue;
            }
            collect_go_context_paths(&path, output)?;
        } else if file_type.is_file() && is_go_context_input_path(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_go_context_input_path(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("go.mod" | "go.sum" | "go.work" | "go.work.sum") => true,
        Some("modules.txt") => path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "vendor"),
        _ => false,
    }
}

fn collect_rust_context_files(root: &Path) -> CodeGraphResult<Vec<RustCallFactsFile>> {
    let mut paths = Vec::new();
    collect_rust_context_paths(root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path)
                .map_err(|error| io_failure("read Rust context input", &path, error))?;
            Ok(RustCallFactsFile {
                path: relative_path(root, &path),
                sha256: sha256_hex(&bytes),
            })
        })
        .collect()
}

fn collect_rust_context_paths(root: &Path, output: &mut Vec<PathBuf>) -> CodeGraphResult<()> {
    let entries =
        fs::read_dir(root).map_err(|error| io_failure("scan Rust context inputs", root, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| io_failure("read Rust context directory entry", root, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect Rust context input", &path, error))?;
        if file_type.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | ".zvec-grep" | "node_modules" | "target")
            ) {
                continue;
            }
            collect_rust_context_paths(&path, output)?;
        } else if file_type.is_file() && is_rust_context_input_path(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_rust_context_input_path(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml") => true,
        Some("config" | "config.toml") => path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".cargo"),
        _ => false,
    }
}

fn collect_typescript_context_files(root: &Path) -> CodeGraphResult<Vec<TypeScriptCallFactsFile>> {
    let mut paths = Vec::new();
    collect_typescript_context_paths(root, &mut paths)?;
    collect_typescript_extended_context_paths(root, &mut paths);
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path)
                .map_err(|error| io_failure("read TypeScript context input", &path, error))?;
            Ok(TypeScriptCallFactsFile {
                path: relative_path(root, &path),
                sha256: sha256_hex(&bytes),
            })
        })
        .collect()
}

fn collect_typescript_extended_context_paths(root: &Path, paths: &mut Vec<PathBuf>) {
    let mut config_paths = paths
        .iter()
        .filter(|path| is_typescript_context_input_path(path))
        .cloned()
        .collect::<Vec<_>>();
    let mut index = 0;
    while index < config_paths.len() {
        let config_path = config_paths[index].clone();
        index += 1;
        let Ok(bytes) = fs::read(&config_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(extends) = value.get("extends").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !extends.starts_with('.') {
            continue;
        }
        let Some(parent) = config_path.parent() else {
            continue;
        };
        let base = parent.join(extends);
        let candidates = [
            base.clone(),
            base.with_extension("json"),
            base.join("tsconfig.json"),
        ];
        let Some(extended) = candidates
            .iter()
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| candidate.canonicalize().ok())
        else {
            continue;
        };
        if !extended.starts_with(root) || paths.contains(&extended) {
            continue;
        }
        paths.push(extended);
        config_paths.push(paths.last().expect("extended config path").clone());
    }
}

fn collect_typescript_context_paths(root: &Path, output: &mut Vec<PathBuf>) -> CodeGraphResult<()> {
    let entries = fs::read_dir(root)
        .map_err(|error| io_failure("scan TypeScript context inputs", root, error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| io_failure("read TypeScript context directory entry", root, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect TypeScript context input", &path, error))?;
        if file_type.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | ".zvec-grep" | "node_modules" | "target" | "dist")
            ) {
                continue;
            }
            collect_typescript_context_paths(&path, output)?;
        } else if file_type.is_file() && is_typescript_context_input_path(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_typescript_context_input_path(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(
            "package.json"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "tsconfig.json",
        ) => true,
        Some(name)
            if name.starts_with("tsconfig.")
                && Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension == "json") =>
        {
            true
        }
        _ => false,
    }
}

fn is_typescript_context_file_path(path: &Path) -> bool {
    is_typescript_context_input_path(path)
        || path
            .extension()
            .is_some_and(|extension| extension == "json")
}

fn collect_python_context_files(root: &Path) -> CodeGraphResult<Vec<PythonCallFactsFile>> {
    let mut paths = Vec::new();
    collect_python_context_paths(root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path)
                .map_err(|error| io_failure("read Python context input", &path, error))?;
            Ok(PythonCallFactsFile {
                path: relative_path(root, &path),
                sha256: sha256_hex(&bytes),
            })
        })
        .collect()
}

fn collect_python_context_paths(root: &Path, output: &mut Vec<PathBuf>) -> CodeGraphResult<()> {
    let entries = fs::read_dir(root)
        .map_err(|error| io_failure("scan Python context inputs", root, error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| io_failure("read Python context directory entry", root, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect Python context input", &path, error))?;
        if file_type.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(
                    ".git"
                        | ".zvec-grep"
                        | "node_modules"
                        | "target"
                        | ".venv"
                        | "venv"
                        | "__pycache__"
                )
            ) {
                continue;
            }
            collect_python_context_paths(&path, output)?;
        } else if file_type.is_file() && is_python_context_input_path(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_python_context_input_path(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(
            "pyrightconfig.json" | "pyproject.toml" | "setup.cfg" | "setup.py" | "Pipfile"
            | "Pipfile.lock" | "poetry.lock" | "uv.lock" | ".python-version",
        ) => true,
        Some(name)
            if name.starts_with("requirements")
                && Path::new(name).extension() == Some("txt".as_ref()) =>
        {
            true
        }
        _ => false,
    }
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
        stamps.insert(relative_path(&root, &path), source_stamp(&metadata, None));
    }
    let go_context = collect_go_context_files(&root)?
        .into_iter()
        .map(|file| (file.path, file.sha256));
    insert_context_stamps(&root, &mut stamps, go_context, "Go")?;
    let rust_context = collect_rust_context_files(&root)?
        .into_iter()
        .map(|file| (file.path, file.sha256));
    insert_context_stamps(&root, &mut stamps, rust_context, "Rust")?;
    let typescript_context = collect_typescript_context_files(&root)?
        .into_iter()
        .map(|file| (file.path, file.sha256));
    insert_context_stamps(&root, &mut stamps, typescript_context, "TypeScript")?;
    let python_context = collect_python_context_files(&root)?
        .into_iter()
        .map(|file| (file.path, file.sha256));
    insert_context_stamps(&root, &mut stamps, python_context, "Python")?;
    insert_callfacts_stamp(&root, &mut stamps, GO_CALLFACTS_FILE, "Go")?;
    insert_callfacts_stamp(&root, &mut stamps, RUST_CALLFACTS_FILE, "Rust")?;
    insert_callfacts_stamp(&root, &mut stamps, TYPESCRIPT_CALLFACTS_FILE, "TypeScript")?;
    insert_callfacts_stamp(&root, &mut stamps, PYTHON_CALLFACTS_FILE, "Python")?;
    Ok(stamps)
}

fn source_stamp(metadata: &fs::Metadata, content_sha256: Option<String>) -> CodeGraphSourceStamp {
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
    CodeGraphSourceStamp {
        byte_len: metadata.len(),
        modified_unix_nanos,
        changed_unix_nanos,
        content_sha256,
    }
}

fn insert_context_stamps<I>(
    root: &Path,
    stamps: &mut BTreeMap<String, CodeGraphSourceStamp>,
    files: I,
    language: &str,
) -> CodeGraphResult<()>
where
    I: IntoIterator<Item = (String, String)>,
{
    for (path, digest) in files {
        let full_path = root.join(&path);
        let metadata = fs::metadata(&full_path).map_err(|error| {
            io_failure(&format!("stat {language} context input"), &full_path, error)
        })?;
        stamps.insert(path, source_stamp(&metadata, Some(digest)));
    }
    Ok(())
}

fn insert_callfacts_stamp(
    root: &Path,
    stamps: &mut BTreeMap<String, CodeGraphSourceStamp>,
    filename: &str,
    language: &str,
) -> CodeGraphResult<()> {
    let path = root.join(".zvec-grep").join(filename);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(io_failure(
                &format!("stat {language} call-facts artifact"),
                &path,
                error,
            ));
        }
    };
    let bytes = fs::read(&path).map_err(|error| {
        io_failure(
            &format!("read {language} call-facts artifact for freshness"),
            &path,
            error,
        )
    })?;
    stamps.insert(
        format!(".zvec-grep/{filename}"),
        source_stamp(&metadata, Some(sha256_hex(&bytes))),
    );
    Ok(())
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
    let mut relations = Vec::new();
    if language == SourceLanguage::Go {
        collect_file_data(
            tree.root_node(),
            text.as_bytes(),
            package.as_deref().unwrap_or("_"),
            &relative_path,
            &mut definitions,
            &mut imports,
            &mut calls,
            &mut relations,
        );
    } else {
        let mut collector = LanguageCollector::new(text.as_bytes(), language, &relative_path);
        collector.collect(tree.root_node());
        definitions = collector.definitions;
        imports = collector.imports;
        calls = collector.calls;
        relations = collector.relations;
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
        relations,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_file_data(
    node: Node<'_>,
    source: &[u8],
    package: &str,
    path: &str,
    definitions: &mut Vec<Definition>,
    imports: &mut BTreeSet<String>,
    calls: &mut Vec<CallSite>,
    relations: &mut Vec<StructuralRelation>,
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
                relations.extend(go_structural_relations(node, source, &definition));
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
        "type_conversion_expression" => {
            if let Some(call) = go_generic_call_site(node, source) {
                calls.push(call);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_file_data(
            child,
            source,
            package,
            path,
            definitions,
            imports,
            calls,
            relations,
        );
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
        (SourceLanguage::Rust, "function_item" | "function_signature_item") => (
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

#[allow(clippy::too_many_lines)]
fn language_relations(
    node: Node<'_>,
    source: &[u8],
    language: SourceLanguage,
    path: &str,
    scopes: &[(String, bool)],
    definition: Option<&Definition>,
) -> Vec<StructuralRelation> {
    let mut relations = Vec::new();
    match (language, node.kind()) {
        (SourceLanguage::Rust, "impl_item") => {
            let Some(type_node) = node.child_by_field_name("type") else {
                return relations;
            };
            let source_name = clean_relation_target(node_text(type_node, source));
            if source_name.is_empty() {
                return relations;
            }
            let qualified_source_name = scopes
                .iter()
                .map(|(scope, _)| scope.as_str())
                .chain(std::iter::once(source_name.as_str()))
                .collect::<Vec<_>>()
                .join(".");
            let source_id = symbol_node_id("type", path, &qualified_source_name);
            if let Some(trait_node) = node.child_by_field_name("trait")
                && let Some(target_name) = non_empty_relation_target(node_text(trait_node, source))
            {
                relations.push(StructuralRelation {
                    kind: "implements".to_owned(),
                    source: source_id,
                    target_name,
                    range: graph_range(trait_node),
                });
            }
        }
        (SourceLanguage::Rust, "trait_item") => {
            let Some(definition) = definition else {
                return relations;
            };
            if let Some(bounds) = node.child_by_field_name("bounds") {
                for (target_name, range) in relation_targets(bounds, source) {
                    relations.push(StructuralRelation {
                        kind: "inherits".to_owned(),
                        source: definition.node.id.clone(),
                        target_name,
                        range,
                    });
                }
            }
        }
        (SourceLanguage::Rust, "struct_item") => {
            let Some(definition) = definition else {
                return relations;
            };
            if let Some(fields) = node.child_by_field_name("body") {
                for field in fields.named_children(&mut fields.walk()) {
                    let Some(type_node) = field.child_by_field_name("type") else {
                        continue;
                    };
                    if let Some(target_name) =
                        non_empty_relation_target(node_text(type_node, source))
                    {
                        relations.push(StructuralRelation {
                            kind: "references".to_owned(),
                            source: definition.node.id.clone(),
                            target_name,
                            range: graph_range(type_node),
                        });
                    }
                }
            }
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "class_declaration") => {
            let Some(definition) = definition else {
                return relations;
            };
            if let Some(heritage) = node
                .named_children(&mut node.walk())
                .find(|child| child.kind() == "class_heritage")
            {
                for clause in heritage.named_children(&mut heritage.walk()) {
                    let kind = match clause.kind() {
                        "extends_clause" => "inherits",
                        "implements_clause" => "implements",
                        _ => continue,
                    };
                    for (target_name, range) in relation_targets(clause, source) {
                        relations.push(StructuralRelation {
                            kind: kind.to_owned(),
                            source: definition.node.id.clone(),
                            target_name,
                            range,
                        });
                    }
                }
            }
            relations.extend(type_field_references(node, source, definition));
        }
        (SourceLanguage::TypeScript | SourceLanguage::Tsx, "interface_declaration") => {
            let Some(definition) = definition else {
                return relations;
            };
            for child in node.named_children(&mut node.walk()) {
                if child.kind() != "extends_type_clause" {
                    continue;
                }
                for (target_name, range) in relation_targets(child, source) {
                    relations.push(StructuralRelation {
                        kind: "inherits".to_owned(),
                        source: definition.node.id.clone(),
                        target_name,
                        range,
                    });
                }
            }
            relations.extend(type_field_references(node, source, definition));
        }
        (SourceLanguage::Python, "class_definition") => {
            let Some(definition) = definition else {
                return relations;
            };
            let Some(superclasses) = node.child_by_field_name("superclasses") else {
                return relations;
            };
            for (target_name, range) in relation_targets(superclasses, source) {
                relations.push(StructuralRelation {
                    kind: "inherits".to_owned(),
                    source: definition.node.id.clone(),
                    target_name,
                    range,
                });
            }
        }
        (SourceLanguage::Python, "function_definition") => {
            let Some(definition) = definition else {
                return relations;
            };
            relations.extend(type_field_references(node, source, definition));
        }
        _ => {}
    }
    relations
}

fn relation_targets(node: Node<'_>, source: &[u8]) -> Vec<(String, CodeGraphRange)> {
    let children = node.named_children(&mut node.walk()).collect::<Vec<_>>();
    if children.is_empty() {
        return non_empty_relation_target(node_text(node, source))
            .map(|target| vec![(target, graph_range(node))])
            .unwrap_or_default();
    }
    children
        .into_iter()
        .filter(|child| {
            !matches!(
                child.kind(),
                "type_arguments" | "type_parameters" | "type_parameters_list"
            )
        })
        .filter_map(|child| {
            non_empty_relation_target(node_text(child, source))
                .map(|target| (target, graph_range(child)))
        })
        .collect()
}

fn non_empty_relation_target(raw: &str) -> Option<String> {
    let target = clean_relation_target(raw);
    (!target.is_empty()).then_some(target)
}

fn clean_relation_target(raw: &str) -> String {
    let mut target = raw
        .trim()
        .trim_matches(['"', '\'', '`', '(', ')', ',', ';'])
        .trim_start_matches("dyn ")
        .trim_start_matches('&')
        .trim_start_matches('*')
        .trim_start_matches("mut ")
        .trim();
    if let Some((prefix, _)) = target.split_once('<') {
        target = prefix.trim();
    }
    target = target.trim_matches(['"', '\'', '`', '(', ')', ',', ';']);
    target.replace("::", ".").trim_matches('.').to_owned()
}

fn go_structural_relations(
    node: Node<'_>,
    source: &[u8],
    definition: &Definition,
) -> Vec<StructuralRelation> {
    let Some(type_node) = node.child_by_field_name("type") else {
        return Vec::new();
    };
    if type_node.kind() != "struct_type" {
        return Vec::new();
    }
    let Some(fields) = named_children(type_node)
        .into_iter()
        .find(|child| child.kind() == "field_declaration_list")
    else {
        return Vec::new();
    };
    fields
        .named_children(&mut fields.walk())
        .filter(|field| field.kind() == "field_declaration")
        .filter_map(|field| {
            let target = field.child_by_field_name("type")?;
            let target_name = non_empty_relation_target(node_text(target, source))?;
            Some(StructuralRelation {
                kind: if field.child_by_field_name("name").is_some() {
                    "references"
                } else {
                    "inherits"
                }
                .to_owned(),
                source: definition.node.id.clone(),
                target_name,
                range: graph_range(target),
            })
        })
        .collect()
}

fn type_field_references(
    node: Node<'_>,
    source: &[u8],
    definition: &Definition,
) -> Vec<StructuralRelation> {
    let mut relations = Vec::new();
    let mut pending = vec![node];
    while let Some(candidate) = pending.pop() {
        let type_node = candidate
            .child_by_field_name("type")
            .or_else(|| candidate.child_by_field_name("type_annotation"))
            .or_else(|| candidate.child_by_field_name("return_type"));
        if matches!(
            candidate.kind(),
            "public_field_definition"
                | "property_signature"
                | "required_parameter"
                | "optional_parameter"
                | "typed_parameter"
                | "function_definition"
        ) && let Some(type_node) = type_node
        {
            for (target_name, range) in relation_targets(type_node, source) {
                relations.push(StructuralRelation {
                    kind: "references".to_owned(),
                    source: definition.node.id.clone(),
                    target_name,
                    range,
                });
            }
        }
        pending.extend(named_children(candidate));
    }
    relations
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

fn test_owner_id(
    node: Node<'_>,
    scopes: &[(String, bool)],
    language: SourceLanguage,
    path: &str,
    source: &[u8],
) -> Option<String> {
    let owner = scopes
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (_, class_like))| !*class_like)?;
    if !is_test_name(&owner.1.0) && !rust_test_attribute(node, language, source) {
        return None;
    }
    let owner_index = owner.0;
    let qualified_name = scopes[..=owner_index]
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    let kind = if scopes[..owner_index]
        .iter()
        .any(|(_, class_like)| *class_like)
    {
        "method"
    } else {
        "function"
    };
    Some(symbol_node_id(kind, path, &qualified_name))
}

fn rust_test_attribute(node: Node<'_>, language: SourceLanguage, source: &[u8]) -> bool {
    if language != SourceLanguage::Rust {
        return false;
    }
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "function_item" {
            return std::str::from_utf8(&source[..parent.start_byte()])
                .is_ok_and(|prefix| prefix.trim_end().ends_with("#[test]"));
        }
        current = parent;
    }
    false
}

fn is_test_name(name: &str) -> bool {
    name == "test"
        || name.starts_with("test_")
        || name
            .strip_prefix("test")
            .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_uppercase))
        || name
            .strip_prefix("Test")
            .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_uppercase))
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

fn go_generic_call_site(node: Node<'_>, source: &[u8]) -> Option<CallSite> {
    let generic_type = node.child_by_field_name("type")?;
    if generic_type.kind() != "generic_type" {
        return None;
    }
    let function = generic_type.child_by_field_name("type")?;
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
        "identifier" | "type_identifier" => Some(node_text(function, source).to_owned()),
        "selector_expression" => function
            .child_by_field_name("field")
            .map(|field| node_text(field, source).to_owned()),
        "index_expression" => function
            .child_by_field_name("operand")
            .and_then(|operand| call_symbol_name(operand, source)),
        "type_instantiation_expression" => function
            .child_by_field_name("type")
            .and_then(|typ| call_symbol_name(typ, source)),
        "parenthesized_expression" => named_children(function)
            .into_iter()
            .next()
            .and_then(|expression| call_symbol_name(expression, source)),
        _ => None,
    }
}

fn qualified_call_name(function: Node<'_>, source: &[u8]) -> Option<String> {
    match function.kind() {
        "identifier" | "type_identifier" => Some(node_text(function, source).to_owned()),
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
        "type_instantiation_expression" => function
            .child_by_field_name("type")
            .and_then(|typ| qualified_call_name(typ, source)),
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
    let receiver = receiver.trim();
    let receiver = receiver
        .strip_prefix('(')
        .and_then(|receiver| receiver.strip_suffix(')'))
        .unwrap_or(receiver);
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

fn package_manifest_key(files: &[CodeGraphFile], manifests: &[PackageManifest]) -> String {
    let base = manifest_key(files);
    if manifests.is_empty() {
        return base;
    }
    let mut input = base.into_bytes();
    for manifest in manifests {
        input.extend_from_slice(b"\0package-manifest\0");
        input.extend_from_slice(manifest.ecosystem.as_bytes());
        input.push(0);
        input.extend_from_slice(manifest.path.as_bytes());
        input.push(0);
        input.extend_from_slice(manifest.sha256.as_bytes());
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

    use serde::Deserialize;
    use tempfile::tempdir;

    use super::{
        CODEGRAPH_RELATION_GENERATION, CodeGraphChange, build_codegraph, build_go_codegraph,
        refresh_codegraph, update_codegraph, update_go_codegraph, write_go_codegraph,
    };

    #[test]
    fn go_analysis_context_fingerprint_matches_go_producer_contract() {
        let settings = [
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
                "GOARCH" => "arm64",
                "CGO_ENABLED" => "1",
                "GOFLAGS" => "-tags=fixture",
                "GOOS" => "darwin",
                _ => "",
            };
            (name.to_owned(), value.to_owned())
        })
        .collect();
        let context = super::GoCallFactsContext {
            go_version: "go1.26.0".to_owned(),
            go_mod: "go.mod".to_owned(),
            go_work: String::new(),
            settings,
            context_files: vec![super::GoCallFactsFile {
                path: "go.mod".to_owned(),
                sha256: "abc123".to_owned(),
            }],
        };
        assert_eq!(
            context.fingerprint(),
            "5b7f099e8fbdb5261a1e04b1a2183cbecc299f34b7f13a2583f7d0e9f48e0505"
        );
    }

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
        assert_eq!(artifact.version, 2);
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
    fn emits_manifest_backed_dependency_edges_for_all_supported_ecosystems() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("main.go"),
            "package main\n\nfunc main() {}\n",
        )
        .expect("Go source");
        fs::write(
            directory.path().join("go.mod"),
            "module example.com/app\n\nrequire example.com/dep v1.2.3\n",
        )
        .expect("go.mod");

        fs::create_dir_all(directory.path().join("rust/src")).expect("Rust source directory");
        fs::write(
            directory.path().join("rust/src/lib.rs"),
            "pub fn run() {}\n",
        )
        .expect("Rust source");
        fs::write(
            directory.path().join("rust/Cargo.toml"),
            concat!(
                "[package]\nname = \"example-rs\"\nversion = \"0.1.0\"\n\n",
                "[dependencies]\nserde = \"1\"\n\n",
                "[target.'cfg(unix)'.dependencies]\ntokio = \"1\"\n",
                "\n[[bin]]\nname = \"example-rs-bin\"\npath = \"src/bin.rs\"\n",
            ),
        )
        .expect("Cargo.toml");

        fs::create_dir_all(directory.path().join("python")).expect("Python source directory");
        fs::write(
            directory.path().join("python/app.py"),
            "def run():\n    pass\n",
        )
        .expect("Python source");
        fs::write(
            directory.path().join("python/pyproject.toml"),
            concat!(
                "[project]\nname = \"example-python\"\nversion = \"0.1.0\"\n",
                "dependencies = [\"requests>=2\", \"pydantic[dotenv]==2\"]\n",
            ),
        )
        .expect("pyproject.toml");

        fs::create_dir_all(directory.path().join("web")).expect("TypeScript source directory");
        fs::write(
            directory.path().join("web/index.ts"),
            "export function run() {}\n",
        )
        .expect("TypeScript source");
        fs::write(
            directory.path().join("web/package.json"),
            concat!(
                "{\n",
                "  \"name\": \"example-web\",\n",
                "  \"dependencies\": {\"react\": \"^19\"}\n",
                "}\n",
            ),
        )
        .expect("package.json");

        let artifact = build_codegraph(directory.path()).expect("manifest graph");
        for (package, dependency, manifest_path) in [
            ("example.com/app", "example.com/dep", "go.mod"),
            ("example-rs", "serde", "rust/Cargo.toml"),
            ("example-rs", "tokio", "rust/Cargo.toml"),
            ("example-python", "requests", "python/pyproject.toml"),
            ("example-python", "pydantic", "python/pyproject.toml"),
            ("example-web", "react", "web/package.json"),
        ] {
            let source = node_id(&artifact, "package", package);
            let target = node_id(&artifact, "package", dependency);
            let edge = artifact
                .edges
                .iter()
                .find(|edge| {
                    edge.kind == "depends_on"
                        && edge.source == source
                        && edge.target.as_deref() == Some(target.as_str())
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing manifest dependency {package} -> {dependency}: {:#?}",
                        artifact.edges
                    )
                });
            assert!(edge.resolved);
            assert_eq!(edge.resolution.as_deref(), Some("manifest"));
            assert_eq!(edge.target_name.as_deref(), Some(dependency));
            assert_eq!(
                artifact
                    .nodes
                    .iter()
                    .find(|node| node.id == source)
                    .and_then(|node| node.path.as_deref()),
                Some(manifest_path)
            );
        }

        assert!(artifact.edges.iter().all(|edge| {
            edge.kind != "depends_on" || edge.resolution.as_deref() == Some("manifest")
        }));
        assert!(!artifact.edges.iter().any(|edge| {
            edge.kind == "depends_on" && edge.target_name.as_deref() == Some("name")
        }));
    }

    #[test]
    fn refresh_rebuilds_manifest_dependency_edges_when_a_manifest_changes() {
        let directory = tempdir().expect("workspace");
        fs::write(directory.path().join("main.go"), "package main\n").expect("Go source");
        let manifest = directory.path().join("go.mod");
        fs::write(
            &manifest,
            "module example.com/app\n\nrequire example.com/old v1.0.0\n",
        )
        .expect("initial go.mod");

        let (_, initial) = refresh_codegraph(directory.path()).expect("initial refresh");
        assert!(initial.edges.iter().any(|edge| {
            edge.kind == "depends_on" && edge.target_name.as_deref() == Some("example.com/old")
        }));

        fs::write(
            &manifest,
            "module example.com/app\n\nrequire example.com/new v1.0.0\n",
        )
        .expect("updated go.mod");
        let (_, refreshed) = refresh_codegraph(directory.path()).expect("manifest refresh");
        assert!(!refreshed.edges.iter().any(|edge| {
            edge.kind == "depends_on" && edge.target_name.as_deref() == Some("example.com/old")
        }));
        assert!(refreshed.edges.iter().any(|edge| {
            edge.kind == "depends_on" && edge.target_name.as_deref() == Some("example.com/new")
        }));
        assert_eq!(
            refreshed,
            build_codegraph(directory.path()).expect("full manifest graph")
        );
    }

    #[test]
    fn malformed_go_manifest_does_not_emit_dependency_facts() {
        assert_eq!(
            super::parse_package_manifest(
                "go",
                b"module example.com/app\n\nrequire (\nexample.com/dep v1.2.3\n",
            ),
            (None, Vec::new())
        );
    }

    #[test]
    fn incremental_source_refresh_preserves_manifest_package_nodes_without_dependencies() {
        let directory = tempdir().expect("workspace");
        let source = directory.path().join("main.go");
        fs::write(&source, "package main\n\nfunc main() {}\n").expect("Go source");
        fs::write(
            directory.path().join("go.mod"),
            "module example.com/app\n\ngo 1.26\n",
        )
        .expect("go.mod");

        let base = build_codegraph(directory.path()).expect("base graph");
        let package_id = node_id(&base, "package", "example.com/app");
        fs::write(
            &source,
            "package main\n\nfunc main() { println(\"updated\") }\n",
        )
        .expect("updated Go source");
        let updated = update_codegraph(
            &base,
            directory.path(),
            &[CodeGraphChange::Upsert("main.go".into())],
        )
        .expect("incremental graph");

        assert!(updated.nodes.iter().any(|node| {
            node.id == package_id
                && node.kind == "package"
                && node.path.as_deref() == Some("go.mod")
        }));
        assert_eq!(
            updated,
            build_codegraph(directory.path()).expect("full graph")
        );
    }

    #[test]
    fn malformed_toml_manifest_does_not_emit_dependency_facts() {
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"broken\"\n[dependencies]\nserde = [\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "python",
                b"[project]\nname = \"broken\"\ndependencies = [\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"broken\"\n[dependencies]\nserde = \"1\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "python",
                b"[project]\nname = \"broken\n\ndependencies = [\"requests\"]\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "python",
                b"[project]\nname = \"valid-name\"\ndependencies = [\"requests\", invalid]\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "python",
                b"[project]\nname = \"valid-name\"\ndependencies = [\"requests\" \"pydantic\"]\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"valid-name\"\n[dependencies]\nserde = \"\\q\"\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"valid-name\"\n[dependencies]\nserde = \"\\u12G4\"\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"broken\\uD800\"\n[dependencies]\nserde = \"1\"\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"valid-name\"\n[dependencies]\nserde =\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"valid-name\"\nnot valid TOML\n[dependencies]\nserde = \"1\"\n",
            ),
            (None, Vec::new())
        );
        assert_eq!(
            super::parse_package_manifest(
                "rust",
                b"[package]\nname = \"valid-name\"\n[dependencies]\nserde = \"1\"\n\xff",
            ),
            (None, Vec::new())
        );
    }

    #[test]
    fn parses_generic_function_instantiation_as_a_call_site() {
        let directory = tempdir().expect("workspace");
        let source = "package main\nfunc identity[T any](value T) T { return value }\nfunc caller() { _ = identity[int](1) }\n";
        fs::write(directory.path().join("main.go"), source).expect("Go source");

        let artifact = build_go_codegraph(directory.path()).expect("generic call graph");
        let start = source.find("identity[int](1)").expect("generic call start");
        let end = start + "identity[int](1)".len();
        assert!(
            artifact.edges.iter().any(|edge| {
                edge.kind == "calls"
                    && edge.target_name.as_deref() == Some("identity")
                    && edge.resolved
                    && edge
                        .range
                        .as_ref()
                        .is_some_and(|range| range.start_byte == start && range.end_byte == end)
            }),
            "generic call edge missing: {:#?}",
            artifact.edges
        );
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

    fn source_node_id(artifact: &super::CodeGraphArtifact, name: &str) -> String {
        artifact
            .nodes
            .iter()
            .find(|node| node.name == name && !matches!(node.kind.as_str(), "file" | "package"))
            .map_or_else(
                || panic!("missing source node {name}"),
                |node| node.id.clone(),
            )
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn emits_multifile_structural_relations_for_all_supported_languages() {
        let cases = [
            (
                "go",
                vec![
                    (
                        "types.go",
                        "package demo\n\ntype Base struct{}\ntype Child struct { Base; Value Base }\n",
                    ),
                    (
                        "test.go",
                        "package demo\n\nfunc helper() {}\nfunc TestChild() { helper() }\n",
                    ),
                ],
                vec![
                    ("inherits", "Child", "Base"),
                    ("references", "Child", "Base"),
                    ("tests", "TestChild", "helper"),
                ],
            ),
            (
                "rust",
                vec![
                    (
                        "types.rs",
                        "trait Parent {}\ntrait Child: Parent {}\nstruct Impl;\nstruct Wrapper { value: Impl }\nimpl Parent for Impl {}\n",
                    ),
                    ("tests.rs", "fn helper() {}\nfn test_impl() { helper(); }\n"),
                ],
                vec![
                    ("inherits", "Child", "Parent"),
                    ("implements", "Impl", "Parent"),
                    ("references", "Wrapper", "Impl"),
                    ("tests", "test_impl", "helper"),
                ],
            ),
            (
                "typescript",
                vec![
                    (
                        "types.ts",
                        "export class Base {}\nexport interface Contract {}\nexport class Child extends Base implements Contract { value: Base; }\n",
                    ),
                    (
                        "tests.ts",
                        "function helper() {}\nfunction testChild() { helper(); }\n",
                    ),
                ],
                vec![
                    ("inherits", "Child", "Base"),
                    ("implements", "Child", "Contract"),
                    ("references", "Child", "Base"),
                    ("tests", "testChild", "helper"),
                ],
            ),
            (
                "python",
                vec![
                    (
                        "types.py",
                        "class Base:\n    pass\n\nclass Child(Base):\n    pass\n",
                    ),
                    (
                        "tests.py",
                        "def helper():\n    pass\n\ndef typed_helper(value: Base) -> Base:\n    return value\n\ndef test_child():\n    helper()\n",
                    ),
                ],
                vec![
                    ("inherits", "Child", "Base"),
                    ("references", "typed_helper", "Base"),
                    ("tests", "test_child", "helper"),
                ],
            ),
        ];

        for (language, files, expected_relations) in cases {
            let directory = tempdir().expect("workspace");
            for (path, source) in files {
                fs::write(directory.path().join(path), source).expect("fixture source");
            }
            let artifact = build_codegraph(directory.path()).expect("graph");
            for (kind, source_name, target_name) in expected_relations {
                let edge = artifact
                    .edges
                    .iter()
                    .find(|edge| {
                        edge.kind == kind
                            && edge
                                .source
                                .as_str()
                                .eq(source_node_id(&artifact, source_name).as_str())
                            && edge.target_name.as_deref() == Some(target_name)
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "{language}: missing {kind} {source_name} -> {target_name}: {:#?}",
                            artifact.edges
                        )
                    });
                assert!(edge.resolved, "{language}: {edge:#?}");
                assert!(edge.target.is_some(), "{language}: {edge:#?}");
                assert_eq!(edge.resolution.as_deref(), Some("syntax"));
            }
        }
    }

    #[derive(Deserialize)]
    struct RelationFixture {
        schema: String,
        language: String,
        source_files: Vec<String>,
        relations: Vec<RelationExpectation>,
    }

    #[derive(Deserialize)]
    struct RelationExpectation {
        kind: String,
        source: String,
        target: String,
    }

    #[test]
    fn checked_in_relation_fixtures_match_structural_graph_output() {
        let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/codegraph-relations-20260928");
        for language in ["go", "rust", "typescript", "python"] {
            let root = fixture_root.join(language);
            let truth: RelationFixture =
                serde_json::from_slice(&fs::read(root.join("truth.json")).expect("relation truth"))
                    .expect("valid relation truth");
            assert_eq!(truth.schema, "zvec-grep.codegraph-relations-v1");
            assert_eq!(truth.language, language);
            for source_file in &truth.source_files {
                assert!(
                    root.join(source_file).is_file(),
                    "{language}: {source_file}"
                );
            }

            let artifact = build_codegraph(&root).expect("fixture graph");
            assert!(artifact.files.iter().all(|file| file.language == language));
            for expected in truth.relations {
                let source = source_node_id(&artifact, &expected.source);
                let edge = artifact
                    .edges
                    .iter()
                    .find(|edge| {
                        edge.kind == expected.kind
                            && edge.source == source
                            && edge.target_name.as_deref() == Some(expected.target.as_str())
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "{language}: missing {} {} -> {}: {:#?}",
                            expected.kind, expected.source, expected.target, artifact.edges
                        )
                    });
                assert!(edge.resolved, "{language}: {edge:#?}");
                assert!(edge.target.is_some(), "{language}: {edge:#?}");
                assert_eq!(edge.resolution.as_deref(), Some("syntax"));
            }
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
    fn source_stamps_include_go_callfacts_sidecar_for_query_cache_freshness() {
        let directory = tempdir().expect("workspace");
        fs::write(directory.path().join("main.go"), "package main\n").expect("Go source");
        let before = super::codegraph_source_stamps(directory.path()).expect("source stamps");
        let sidecar = directory
            .path()
            .join(".zvec-grep")
            .join(super::GO_CALLFACTS_FILE);
        fs::create_dir_all(sidecar.parent().expect("sidecar parent")).expect("sidecar directory");
        fs::write(&sidecar, b"first facts").expect("first sidecar");
        let after = super::codegraph_source_stamps(directory.path()).expect("sidecar stamps");
        assert_ne!(before, after);
        assert!(after.contains_key(&format!(".zvec-grep/{}", super::GO_CALLFACTS_FILE)));
        fs::write(&sidecar, b"replacement facts").expect("replacement sidecar");
        let replaced = super::codegraph_source_stamps(directory.path()).expect("updated sidecar");
        assert_ne!(after, replaced);
    }

    #[test]
    fn source_stamps_include_go_module_context_inputs_for_query_cache_freshness() {
        let directory = tempdir().expect("workspace");
        fs::write(directory.path().join("main.go"), "package main\n").expect("Go source");
        let go_mod = directory.path().join("go.mod");
        fs::write(&go_mod, "module example.com/stamps\n\ngo 1.22\n").expect("go.mod");
        let before = super::codegraph_source_stamps(directory.path()).expect("initial stamps");
        assert!(before.contains_key("go.mod"));

        fs::write(&go_mod, "module example.com/stamps\n\ngo 1.23\n").expect("update go.mod");
        let after = super::codegraph_source_stamps(directory.path()).expect("updated stamps");
        assert_ne!(before, after);
        assert!(after.contains_key("go.mod"));
    }

    #[test]
    fn source_stamps_include_rust_context_inputs_for_query_cache_freshness() {
        let directory = tempdir().expect("workspace");
        fs::write(directory.path().join("lib.rs"), "fn main() {}\n").expect("Rust source");
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = \"stamps\"\n",
        )
        .expect("Cargo.toml");
        fs::write(
            directory.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"stable\"\n",
        )
        .expect("rust-toolchain.toml");

        let before = super::codegraph_source_stamps(directory.path()).expect("initial stamps");
        assert!(before.contains_key("Cargo.toml"));
        assert!(before.contains_key("rust-toolchain.toml"));

        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = \"stamps\"\nversion = \"0.1.0\"\n",
        )
        .expect("update Cargo.toml");
        let after = super::codegraph_source_stamps(directory.path()).expect("updated stamps");
        assert_ne!(before, after);
    }

    #[test]
    fn source_stamps_include_typescript_and_python_context_inputs_and_sidecars() {
        let directory = tempdir().expect("workspace");
        fs::write(directory.path().join("module.ts"), "function main() {}\n")
            .expect("TypeScript source");
        fs::write(
            directory.path().join("module.py"),
            "def main():\n    pass\n",
        )
        .expect("Python source");
        fs::write(
            directory.path().join("tsconfig.json"),
            "{\"include\":[\"*.ts\"]}\n",
        )
        .expect("tsconfig");
        fs::write(
            directory.path().join("pyrightconfig.json"),
            "{\"include\":[\"*.py\"]}\n",
        )
        .expect("pyrightconfig");

        let before = super::codegraph_source_stamps(directory.path()).expect("initial stamps");
        assert!(before.contains_key("tsconfig.json"));
        assert!(before.contains_key("pyrightconfig.json"));

        fs::write(directory.path().join("tsconfig.json"), "{\"include\":[]}\n")
            .expect("update tsconfig");
        let after = super::codegraph_source_stamps(directory.path()).expect("updated stamps");
        assert_ne!(before, after);

        let typescript_sidecar = directory
            .path()
            .join(".zvec-grep")
            .join(super::TYPESCRIPT_CALLFACTS_FILE);
        fs::create_dir_all(typescript_sidecar.parent().expect("sidecar parent"))
            .expect("sidecar directory");
        fs::write(&typescript_sidecar, b"TypeScript facts").expect("TypeScript sidecar");
        let with_sidecar =
            super::codegraph_source_stamps(directory.path()).expect("TypeScript sidecar stamps");
        assert!(
            with_sidecar.contains_key(&format!(".zvec-grep/{}", super::TYPESCRIPT_CALLFACTS_FILE))
        );

        let python_sidecar = directory
            .path()
            .join(".zvec-grep")
            .join(super::PYTHON_CALLFACTS_FILE);
        fs::write(&python_sidecar, b"Python facts").expect("Python sidecar");
        let all_sidecars =
            super::codegraph_source_stamps(directory.path()).expect("Python sidecar stamps");
        assert!(all_sidecars.contains_key(&format!(".zvec-grep/{}", super::PYTHON_CALLFACTS_FILE)));
    }

    #[test]
    fn source_stamps_include_vendor_manifest_context_inputs() {
        let directory = tempdir().expect("workspace");
        let manifest = directory.path().join("vendor/modules.txt");
        fs::create_dir_all(manifest.parent().expect("vendor directory"))
            .expect("create vendor directory");
        fs::write(&manifest, "# example.com/dep v1.0.0\nexample.com/dep\n")
            .expect("vendor manifest");

        let before = super::codegraph_source_stamps(directory.path()).expect("initial stamps");
        assert!(before.contains_key("vendor/modules.txt"));
        fs::write(&manifest, "# example.com/dep v1.0.1\nexample.com/dep\n")
            .expect("update vendor manifest");
        let after = super::codegraph_source_stamps(directory.path()).expect("updated stamps");
        assert_ne!(before, after);
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
                .join(".zvec-grep/codegraph-v2.json")
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
    fn refresh_and_incremental_update_rebuild_stale_relation_generations() {
        let directory = tempdir().expect("workspace");
        fs::write(
            directory.path().join("types.ts"),
            "class Base {}\nclass Child extends Base {}\n",
        )
        .expect("TypeScript source");

        let (_, fresh) = refresh_codegraph(directory.path()).expect("initial refresh");
        assert_eq!(fresh.relation_generation, CODEGRAPH_RELATION_GENERATION);
        assert!(fresh.edges.iter().any(|edge| edge.kind == "inherits"));

        for marker in [0, CODEGRAPH_RELATION_GENERATION + 1] {
            let mut stale = fresh.clone();
            stale.relation_generation = marker;
            stale.edges.retain(|edge| edge.kind != "inherits");
            let artifact_path = directory.path().join(".zvec-grep/codegraph-v2.json");
            fs::write(
                &artifact_path,
                serde_json::to_vec(&stale).expect("stale artifact JSON"),
            )
            .expect("write stale artifact");

            let (_, refreshed) = refresh_codegraph(directory.path()).expect("refresh stale");
            let expected = build_codegraph(directory.path()).expect("full graph");
            assert_eq!(refreshed, expected);
            assert_eq!(refreshed.relation_generation, CODEGRAPH_RELATION_GENERATION);
            assert!(refreshed.edges.iter().any(|edge| edge.kind == "inherits"));

            let updated =
                update_codegraph(&stale, directory.path(), &[]).expect("incremental update stale");
            assert_eq!(updated, expected);
        }
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
        assert_eq!(path, directory.path().join(".zvec-grep/codegraph-v2.json"));
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
