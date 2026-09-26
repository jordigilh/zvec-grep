//! Opt-in source-backed Code IR v1. The canonical schema is schemas/code-ir-v1.schema.json.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tree_sitter::{Node, Parser};

const FRONTEND: &str = "tree-sitter-rust-ir-v1.2";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column_byte: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRef {
    pub file_id: String,
    pub sha256: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Extraction {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub frontend_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct File {
    pub file_id: String,
    pub root_id: String,
    pub relative_path: String,
    pub language: String,
    pub sha256: String,
    pub byte_length: usize,
    pub extraction: Extraction,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Origin {
    pub language: String,
    pub frontend: String,
    pub syntax_kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnchoredText {
    pub text: String,
    pub source: SourceRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Extension {
    pub language: String,
    pub data: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unit {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<AnchoredText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<AnchoredText>,
    pub source: SourceRef,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extension>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub frontend: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolver_version: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_spelling: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_ids: Option<Vec<String>>,
    pub site: SourceRef,
    pub status: String,
    pub provenance: Provenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extension>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: String,
    pub schema_version: usize,
    pub repository_id: String,
    pub root_set: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub snapshot_id: String,
    pub frontend_versions: BTreeMap<String, String>,
    pub files: Vec<File>,
    pub units: Vec<Unit>,
    pub facts: Vec<Fact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishedManifest {
    pub ir_snapshot_id: String,
    pub mode: String,
    pub projection_version: Option<usize>,
    pub source_selection: Vec<String>,
    pub source_hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct LoadedSnapshot {
    pub manifest: PublishedManifest,
    pub snapshot: Snapshot,
    pub sources: BTreeMap<String, Vec<u8>>,
}

/// Persist one complete, opt-in shadow snapshot and atomically publish its manifest last.
///
/// # Errors
/// Returns an error for stale source bytes, invalid IR, or failed filesystem operations.
pub fn publish_shadow(
    directory: &Path,
    snapshot: &Snapshot,
    sources: &BTreeMap<String, Vec<u8>>,
) -> Result<PublishedManifest, String> {
    validate(snapshot, sources)?;
    let generation = directory.join("generations").join(&snapshot.snapshot_id);
    let source_directory = generation.join("sources");
    fs::create_dir_all(&source_directory).map_err(|error| error.to_string())?;
    let snapshot_json = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
    write_immutable(&generation.join("snapshot.json"), &snapshot_json)?;
    for (file_id, bytes) in sources {
        write_immutable(&source_directory.join(format!("{file_id}.bin")), bytes)?;
    }
    let manifest = PublishedManifest {
        ir_snapshot_id: snapshot.snapshot_id.clone(),
        mode: "shadow".into(),
        projection_version: None,
        source_selection: snapshot.root_set.clone(),
        source_hashes: snapshot
            .files
            .iter()
            .map(|file| (file.file_id.clone(), file.sha256.clone()))
            .collect(),
    };
    let encoded = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    write_atomic(&directory.join("active.json"), &encoded)?;
    Ok(manifest)
}

/// Read and validate a pinned shadow generation. The active manifest is read exactly once.
///
/// # Errors
/// Returns an error when the manifest, generation, or any original source byte is stale.
pub fn read_published_shadow(directory: &Path) -> Result<LoadedSnapshot, String> {
    let manifest: PublishedManifest = serde_json::from_slice(
        &fs::read(directory.join("active.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if manifest.mode != "shadow" || manifest.projection_version.is_some() {
        return Err("unsupported published IR mode".into());
    }
    let generation = directory.join("generations").join(&manifest.ir_snapshot_id);
    let snapshot: Snapshot = serde_json::from_slice(
        &fs::read(generation.join("snapshot.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if snapshot.snapshot_id != manifest.ir_snapshot_id {
        return Err("published snapshot ID mismatch".into());
    }
    let mut sources = BTreeMap::new();
    for file in &snapshot.files {
        let path = generation
            .join("sources")
            .join(format!("{}.bin", file.file_id));
        sources.insert(
            file.file_id.clone(),
            fs::read(path).map_err(|error| error.to_string())?,
        );
    }
    validate(&snapshot, &sources)?;
    if snapshot
        .files
        .iter()
        .any(|file| manifest.source_hashes.get(&file.file_id) != Some(&file.sha256))
    {
        return Err("published source manifest mismatch".into());
    }
    Ok(LoadedSnapshot {
        manifest,
        snapshot,
        sources,
    })
}

fn write_immutable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Ok(existing) = fs::read(path) {
        return if existing == bytes {
            Ok(())
        } else {
            Err("immutable IR generation collision".into())
        };
    }
    write_atomic(path, bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("IR output path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let mut temporary = PathBuf::from(path);
    temporary.set_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut output, "{byte:02x}").expect("write to string");
    }
    output
}
// Callers pass temporary serde_json::json! values; own them at this boundary.
#[allow(clippy::needless_pass_by_value)]
fn identity(parts: serde_json::Value) -> String {
    digest(parts.to_string().as_bytes())
}
fn line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        bytes
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1)),
    );
    starts
}
fn position(starts: &[usize], offset: usize) -> Position {
    let line_index = starts
        .partition_point(|start| *start <= offset)
        .saturating_sub(1);
    Position {
        line: line_index + 1,
        column_byte: offset - starts[line_index],
    }
}
fn source_ref(
    file: &File,
    bytes: &[u8],
    lines: &[usize],
    start: usize,
    end: usize,
) -> Result<SourceRef, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid UTF-8 source")?;
    if start >= end
        || end > bytes.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return Err("invalid source byte range".into());
    }
    Ok(SourceRef {
        file_id: file.file_id.clone(),
        sha256: file.sha256.clone(),
        start_byte: start,
        end_byte: end,
        start: position(lines, start),
        end: position(lines, end),
    })
}
#[must_use]
pub fn snapshot_identity(snapshot: &Snapshot) -> String {
    let mut roots = snapshot.root_set.clone();
    roots.sort();
    let versions: Vec<_> = snapshot
        .frontend_versions
        .iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect();
    let mut files: Vec<_> = snapshot
        .files
        .iter()
        .map(|f| format!("{}\0{}\0{}", f.root_id, f.relative_path, f.sha256))
        .collect();
    files.sort();
    identity(serde_json::json!([
        "zvec-grep.code-ir",
        1,
        snapshot.repository_id,
        roots,
        versions,
        files
    ]))
}
/// Validate the serialized snapshot and every source reference against original bytes.
///
/// # Errors
/// Rejects stale bytes, invalid spans/IDs, missing endpoints or unsupported versions.
#[allow(clippy::too_many_lines)]
pub fn validate(snapshot: &Snapshot, sources: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    if snapshot.schema != "zvec-grep.code-ir" || snapshot.schema_version != 1 {
        return Err("unknown IR schema/version".into());
    }
    if snapshot.repository_id.is_empty()
        || snapshot.root_set.iter().collect::<BTreeSet<_>>().len() != snapshot.root_set.len()
    {
        return Err("invalid repository/root identity".into());
    }
    let mut files = BTreeMap::new();
    for file in &snapshot.files {
        let path_components: Vec<_> = file.relative_path.split('/').collect();
        if file.root_id.is_empty()
            || file.relative_path.is_empty()
            || file.relative_path.starts_with('/')
            || file.relative_path.contains('\\')
            || path_components
                .iter()
                .any(|part| part.is_empty() || *part == "." || *part == "..")
            || !snapshot.root_set.contains(&file.root_id)
            || file.file_id
                != identity(serde_json::json!([
                    snapshot.repository_id,
                    file.root_id,
                    file.relative_path
                ]))
            || files.insert(&file.file_id, file).is_some()
        {
            return Err("invalid or duplicate file ID".into());
        }
        if !["complete", "partial", "opaque", "failed"].contains(&file.extraction.status.as_str()) {
            return Err("invalid extraction status".into());
        }
        if snapshot.frontend_versions.get(&file.language) != Some(&file.extraction.frontend_version)
            || file.extraction.frontend_version.is_empty()
        {
            return Err("frontend version mismatch".into());
        }
        let bytes = sources.get(&file.file_id).ok_or("missing source")?;
        if digest(bytes) != file.sha256 || bytes.len() != file.byte_length {
            return Err("stale source".into());
        }
    }
    if sources.len() != files.len() || sources.keys().any(|file_id| !files.contains_key(file_id)) {
        return Err("source set does not match snapshot file set".into());
    }
    if snapshot.snapshot_id != snapshot_identity(snapshot) {
        return Err("snapshot mismatch".into());
    }
    let line_maps: BTreeMap<_, _> = sources
        .iter()
        .map(|(file_id, bytes)| (file_id.as_str(), line_starts(bytes)))
        .collect();
    let check = |r: &SourceRef| -> Result<(), String> {
        let file = files.get(&r.file_id).ok_or("missing source file")?;
        let bytes = sources.get(&r.file_id).ok_or("missing source bytes")?;
        let lines = line_maps
            .get(r.file_id.as_str())
            .ok_or("missing source line map")?;
        if r.sha256 != file.sha256
            || serde_json::to_value(source_ref(file, bytes, lines, r.start_byte, r.end_byte)?)
                .map_err(|e| e.to_string())?
                != serde_json::to_value(r).map_err(|e| e.to_string())?
        {
            return Err("stale or invalid source map".into());
        }
        Ok(())
    };
    let mut units = BTreeMap::new();
    for unit in &snapshot.units {
        if ![
            "file", "module", "type", "function", "method", "value", "opaque",
        ]
        .contains(&unit.kind.as_str())
            || units.insert(&unit.id, unit).is_some()
            || unit.id.is_empty()
        {
            return Err("invalid or duplicate unit".into());
        }
        check(&unit.source)?;
        if unit.origin.frontend.is_empty() || unit.origin.syntax_kind.is_empty() {
            return Err("unit origin is required".into());
        }
        if unit.kind == "file"
            && (unit.source.start_byte != 0
                || unit.source.end_byte != files[&unit.source.file_id].byte_length
                || unit.parent_id.is_some())
        {
            return Err("file unit must cover the complete source".into());
        }
        if files[&unit.source.file_id].language != unit.origin.language
            || unit
                .extensions
                .as_ref()
                .is_some_and(|e| e.language != unit.origin.language)
        {
            return Err("unit language mismatch".into());
        }
        for anchored in [&unit.signature, &unit.documentation].into_iter().flatten() {
            check(&anchored.source)?;
            if anchored.source.file_id != unit.source.file_id
                || sources[&anchored.source.file_id]
                    [anchored.source.start_byte..anchored.source.end_byte]
                    != *anchored.text.as_bytes()
            {
                return Err("synthetic anchored text".into());
            }
        }
    }
    for unit in &snapshot.units {
        if let Some(parent_id) = &unit.parent_id {
            let parent = units.get(parent_id).ok_or("missing parent")?;
            if parent.id == unit.id
                || parent.source.file_id != unit.source.file_id
                || parent.source.start_byte > unit.source.start_byte
                || parent.source.end_byte < unit.source.end_byte
            {
                return Err("invalid parent span".into());
            }
        }
        if unit
            .scope_id
            .as_ref()
            .is_some_and(|id| !units.contains_key(id))
        {
            return Err("missing scope".into());
        }
        if unit
            .scope_id
            .as_ref()
            .is_some_and(|id| units[id].source.file_id != unit.source.file_id)
        {
            return Err("scope crosses files".into());
        }
    }
    for file in &snapshot.files {
        let file_unit_count = snapshot
            .units
            .iter()
            .filter(|unit| unit.kind == "file" && unit.source.file_id == file.file_id)
            .count();
        if (file.byte_length > 0 && file.extraction.status != "failed" && file_unit_count != 1)
            || (file.byte_length == 0 && file_unit_count != 0)
        {
            return Err("file source-unit coverage mismatch".into());
        }
    }
    let mut facts = BTreeSet::new();
    for fact in &snapshot.facts {
        if !facts.insert(&fact.id)
            || ![
                "contains",
                "calls",
                "references",
                "reads",
                "writes",
                "returns",
                "throws",
                "implements",
                "tests",
                "guards",
                "imports",
            ]
            .contains(&fact.kind.as_str())
            || ![
                "observed",
                "name_candidate",
                "ambiguous",
                "type_resolved",
                "unresolved",
            ]
            .contains(&fact.status.as_str())
        {
            return Err("invalid or duplicate fact".into());
        }
        check(&fact.site)?;
        let subject = units.get(&fact.subject_id).ok_or("missing fact subject")?;
        let object = fact.object_id.as_ref().and_then(|id| units.get(id));
        if fact
            .object_id
            .as_ref()
            .is_some_and(|id| !units.contains_key(id))
            || fact
                .candidate_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| !units.contains_key(id)))
            || fact
                .candidate_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().collect::<BTreeSet<_>>().len() != ids.len())
        {
            return Err("missing fact endpoint".into());
        }
        if fact.site.file_id != subject.source.file_id
            || fact.site.start_byte < subject.source.start_byte
            || fact.site.end_byte > subject.source.end_byte
            || (fact.kind == "contains"
                && object.is_none_or(|unit| {
                    unit.source.file_id != subject.source.file_id
                        || unit.source.start_byte < subject.source.start_byte
                        || unit.source.end_byte > subject.source.end_byte
                        || unit.source.start_byte != fact.site.start_byte
                        || unit.source.end_byte != fact.site.end_byte
                }))
        {
            return Err("fact site is outside subject/source containment".into());
        }
        if fact
            .extensions
            .as_ref()
            .is_some_and(|e| e.language != files[&fact.site.file_id].language)
        {
            return Err("fact language mismatch".into());
        }
        if fact.provenance.frontend.is_empty() || fact.provenance.method.is_empty() {
            return Err("fact provenance is required".into());
        }
        let candidates = fact.candidate_ids.as_ref().map_or(0, Vec::len);
        if (fact.kind == "contains" && (fact.status != "observed" || fact.object_id.is_none()))
            || (fact.kind != "contains"
                && fact.status != "type_resolved"
                && fact.object_id.is_some())
            || (fact.status == "type_resolved" && fact.object_id.is_none())
            || (fact.status == "name_candidate" && candidates != 1)
            || (fact.status == "ambiguous" && candidates < 2)
            || (["name_candidate", "ambiguous"].contains(&fact.status.as_str())
                && fact.target_spelling.as_deref().is_none_or(str::is_empty))
            || (fact.status == "type_resolved"
                && (fact.provenance.resolver.is_none()
                    || fact.provenance.resolver_version.is_none()))
            || (["unresolved", "observed"].contains(&fact.status.as_str()) && candidates != 0)
        {
            return Err("invalid relationship status".into());
        }
    }
    Ok(())
}

pub struct InputFile<'a> {
    pub root_id: &'a str,
    pub relative_path: &'a str,
    pub language: &'a str,
    pub bytes: &'a [u8],
}

/// Extract source-backed observations from the original bytes; no name resolver is run.
///
/// # Errors
/// Rejects parser failures or any generated snapshot that fails validation.
#[allow(clippy::too_many_lines)]
pub fn extract(
    repository_id: &str,
    input: &[InputFile<'_>],
) -> Result<(Snapshot, BTreeMap<String, Vec<u8>>), String> {
    let mut snapshot = Snapshot {
        schema: "zvec-grep.code-ir".into(),
        schema_version: 1,
        repository_id: repository_id.into(),
        root_set: input
            .iter()
            .map(|f| f.root_id.to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        revision: None,
        snapshot_id: String::new(),
        frontend_versions: BTreeMap::new(),
        files: Vec::new(),
        units: Vec::new(),
        facts: Vec::new(),
    };
    let mut sources = BTreeMap::new();
    let mut sorted: Vec<_> = input.iter().collect();
    sorted.sort_by_key(|f| (f.root_id, f.relative_path));
    for item in sorted {
        let mut file = File {
            file_id: identity(serde_json::json!([
                repository_id,
                item.root_id,
                item.relative_path
            ])),
            root_id: item.root_id.into(),
            relative_path: item.relative_path.into(),
            language: item.language.into(),
            sha256: digest(item.bytes),
            byte_length: item.bytes.len(),
            extraction: Extraction {
                status: "opaque".into(),
                reason: None,
                frontend_version: FRONTEND.into(),
            },
        };
        sources.insert(file.file_id.clone(), item.bytes.to_vec());
        snapshot
            .frontend_versions
            .insert(file.language.clone(), FRONTEND.into());
        if item.bytes.is_empty() {
            file.extraction.reason = Some("empty file".into());
            snapshot.files.push(file);
            continue;
        }
        if std::str::from_utf8(item.bytes).is_err() {
            file.extraction.status = "failed".into();
            file.extraction.reason = Some("invalid UTF-8".into());
            snapshot.files.push(file);
            continue;
        }
        let lines = line_starts(item.bytes);
        let file_unit = make_unit(
            &file,
            item.bytes,
            &lines,
            "file",
            0,
            item.bytes.len(),
            0,
            "source_file",
            None,
        )?;
        snapshot.units.push(file_unit.clone());
        let grammar = match item.language {
            "go" => Some(tree_sitter_go::LANGUAGE.into()),
            "python" => Some(tree_sitter_python::LANGUAGE.into()),
            "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
            "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            _ => None,
        };
        if let Some(grammar) = grammar {
            let mut parser = Parser::new();
            parser.set_language(&grammar).map_err(|e| e.to_string())?;
            let tree = parser.parse(item.bytes, None).ok_or("parser unavailable")?;
            let mut ordinals = BTreeMap::new();
            let mut error_ranges = Vec::new();
            collect_error_ranges(tree.root_node(), &mut error_ranges);
            walk(
                tree.root_node(),
                &file,
                item.bytes,
                &lines,
                &file_unit,
                &[],
                false,
                &mut snapshot,
                &mut ordinals,
            )?;
            if tree.root_node().has_error() {
                file.extraction.status = "partial".into();
                file.extraction.reason = Some("parser error/missing regions".into());
                error_ranges.sort_unstable();
                error_ranges.dedup();
                for (start, end) in error_ranges {
                    if start >= end {
                        continue;
                    }
                    let key = format!("opaque:{start}:{end}");
                    let ordinal_entry = ordinals.entry(key).or_default();
                    let ordinal = *ordinal_entry;
                    *ordinal_entry += 1;
                    let mut opaque = make_unit(
                        &file, item.bytes, &lines, "opaque", start, end, ordinal, "ERROR", None,
                    )?;
                    opaque.parent_id = Some(file_unit.id.clone());
                    add_fact(
                        &mut snapshot,
                        "contains",
                        &file_unit,
                        &opaque.source,
                        Some(&opaque),
                        None,
                        &mut ordinals,
                    );
                    snapshot.units.push(opaque);
                }
            } else {
                file.extraction.status = "complete".into();
            }
        } else {
            file.extraction.reason = Some("unsupported grammar".into());
        }
        snapshot.files.push(file);
    }
    snapshot.snapshot_id = snapshot_identity(&snapshot);
    validate(&snapshot, &sources)?;
    Ok((snapshot, sources))
}
#[allow(clippy::too_many_arguments)]
fn make_unit(
    file: &File,
    bytes: &[u8],
    lines: &[usize],
    kind: &str,
    start: usize,
    end: usize,
    ordinal: usize,
    syntax: &str,
    name: Option<String>,
) -> Result<Unit, String> {
    Ok(Unit {
        id: identity(serde_json::json!([
            file.file_id,
            file.sha256,
            kind,
            start,
            end,
            ordinal
        ])),
        kind: kind.into(),
        subtype: Some(syntax.into()),
        name,
        qualified_name: None,
        parent_id: None,
        scope_id: None,
        signature: None,
        documentation: None,
        source: source_ref(file, bytes, lines, start, end)?,
        origin: Origin {
            language: file.language.clone(),
            frontend: FRONTEND.into(),
            syntax_kind: syntax.into(),
        },
        extensions: None,
    })
}
fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn collect_error_ranges(node: Node<'_>, out: &mut Vec<(usize, usize)>) {
    if node.is_error() || node.is_missing() {
        if node.start_byte() < node.end_byte() {
            out.push((node.start_byte(), node.end_byte()));
        }
        return;
    }
    let previous_len = out.len();
    for child in named_children(node) {
        if child.has_error() {
            collect_error_ranges(child, out);
        }
    }
    // Some grammars express recovery only as a zero-width missing node. In that
    // case, use the smallest enclosing erroneous syntax node as opaque coverage.
    if out.len() == previous_len && node.has_error() && node.start_byte() < node.end_byte() {
        out.push((node.start_byte(), node.end_byte()));
    }
}
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn walk(
    node: Node<'_>,
    file: &File,
    bytes: &[u8],
    lines: &[usize],
    parent: &Unit,
    breadcrumb: &[String],
    uncertain: bool,
    snapshot: &mut Snapshot,
    ordinals: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    for child in named_children(node) {
        if child.is_error() || child.is_missing() {
            continue;
        }
        let has_error = child.has_error();
        let uncertain_region = uncertain || has_error;
        let syntax = child.kind();
        let python_inner = if syntax == "decorated_definition" {
            named_children(child)
                .into_iter()
                .find(|node| ["class_definition", "function_definition"].contains(&node.kind()))
        } else {
            None
        };
        let semantic_syntax = python_inner.map_or(syntax, |inner| inner.kind());
        let wrapped_python_child = parent.origin.syntax_kind == "decorated_definition"
            && ["class_definition", "function_definition"].contains(&syntax)
            && child.child_by_field_name("name").and_then(|name| {
                std::str::from_utf8(&bytes[name.start_byte()..name.end_byte()]).ok()
            }) == parent.name.as_deref();
        let local_variable =
            syntax == "variable_declarator" && !["file", "module"].contains(&parent.kind.as_str());
        let kind = if uncertain_region || wrapped_python_child || local_variable {
            None
        } else {
            match semantic_syntax {
                "function_declaration"
                | "function_definition"
                | "function_item"
                | "function_signature" => Some(if parent.kind == "type" && file.language != "go" {
                    "method"
                } else {
                    "function"
                }),
                "method_declaration"
                | "function_signature_item"
                | "method_spec"
                | "method_elem"
                | "method_definition"
                | "method_signature"
                | "abstract_method_signature" => Some("method"),
                "decorated_definition"
                | "class_definition"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_spec"
                | "type_alias"
                | "type_alias_declaration"
                | "struct_item"
                | "trait_item"
                | "enum_item"
                | "impl_item"
                | "type_item"
                | "union_item" => Some("type"),
                "mod_item" | "module" | "internal_module" => Some("module"),
                "const_item"
                | "static_item"
                | "variable_declarator"
                | "field_definition"
                | "public_field_definition"
                | "property_signature" => Some("value"),
                _ => None,
            }
        };
        let mut owner = parent.clone();
        if let Some(kind) = kind {
            let name_node = python_inner
                .and_then(|inner| inner.child_by_field_name("name"))
                .or_else(|| child.child_by_field_name("name"))
                .or_else(|| child.child_by_field_name("declarator"))
                .or_else(|| {
                    (syntax == "impl_item")
                        .then(|| child.child_by_field_name("type"))
                        .flatten()
                });
            let mut name = name_node
                .and_then(|n| std::str::from_utf8(&bytes[n.start_byte()..n.end_byte()]).ok())
                .map(str::to_owned);
            let receiver = (syntax == "method_declaration")
                .then(|| child.child_by_field_name("receiver"))
                .flatten()
                .and_then(|node| {
                    std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()]).ok()
                })
                .and_then(|text| {
                    text.rsplit_once(' ').map(|(_, ty)| {
                        ty.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                            .to_owned()
                    })
                });
            let receiver = receiver.filter(|value| !value.is_empty());
            let key = format!("{kind}:{}:{}", child.start_byte(), child.end_byte());
            let ordinal = *ordinals.entry(key.clone()).or_default();
            *ordinals.get_mut(&key).ok_or("ordinal missing")? += 1;
            let mut unit = make_unit(
                file,
                bytes,
                lines,
                kind,
                child.start_byte(),
                child.end_byte(),
                ordinal,
                syntax,
                name.clone(),
            )?;
            unit.subtype = Some(semantic_syntax.into());
            unit.parent_id = Some(parent.id.clone());
            unit.scope_id = Some(parent.id.clone());
            if let Some(receiver_type) = receiver.as_ref() {
                name = name.map(|method| format!("{receiver_type}::{method}"));
            }
            let qualified_parts =
                if let Some(full_name) = name.as_ref().filter(|name| name.contains("::")) {
                    full_name.split("::").map(str::to_owned).collect::<Vec<_>>()
                } else {
                    breadcrumb
                        .iter()
                        .cloned()
                        .chain(name.clone())
                        .collect::<Vec<_>>()
                };
            if !qualified_parts.is_empty() {
                unit.qualified_name = Some(qualified_parts.join("::"));
            }
            if receiver.is_some() || syntax == "decorated_definition" || syntax == "impl_item" {
                let mut data = serde_json::Map::new();
                if let Some(receiver_type) = receiver {
                    data.insert("receiver".into(), receiver_type.into());
                }
                if syntax == "decorated_definition" {
                    data.insert("decorated_definition".into(), true.into());
                }
                if syntax == "impl_item" {
                    data.insert("impl".into(), true.into());
                }
                unit.extensions = Some(Extension {
                    language: file.language.clone(),
                    data: data.into(),
                });
            }
            add_fact(
                snapshot,
                "contains",
                parent,
                &unit.source,
                Some(&unit),
                None,
                ordinals,
            );
            snapshot.units.push(unit.clone());
            owner = unit;
        }
        if !uncertain_region
            && ["call_expression", "call"].contains(&syntax)
            && let Some(target) = child
                .child_by_field_name("function")
                .or_else(|| named_children(child).first().copied())
        {
            let spelling =
                std::str::from_utf8(&bytes[target.start_byte()..target.end_byte()]).unwrap_or("");
            if !spelling.is_empty() && spelling.len() < 180 {
                let site = source_ref(file, bytes, lines, child.start_byte(), child.end_byte())?;
                add_fact(
                    snapshot,
                    "calls",
                    parent,
                    &site,
                    None,
                    Some(spelling),
                    ordinals,
                );
            }
        }
        let next_breadcrumb = if owner.id == parent.id {
            breadcrumb.to_vec()
        } else {
            owner.qualified_name.as_ref().map_or_else(
                || breadcrumb.to_vec(),
                |name| name.split("::").map(str::to_owned).collect(),
            )
        };
        walk(
            child,
            file,
            bytes,
            lines,
            &owner,
            &next_breadcrumb,
            uncertain_region,
            snapshot,
            ordinals,
        )?;
    }
    Ok(())
}
fn add_fact(
    snapshot: &mut Snapshot,
    kind: &str,
    subject: &Unit,
    site: &SourceRef,
    object: Option<&Unit>,
    spelling: Option<&str>,
    ordinals: &mut BTreeMap<String, usize>,
) {
    let key = serde_json::json!([
        subject.id,
        kind,
        site.start_byte,
        site.end_byte,
        object.map(|u| &u.id),
        spelling
    ])
    .to_string();
    let ordinal = *ordinals.entry(key.clone()).or_default();
    *ordinals.get_mut(&key).expect("ordinal inserted") += 1;
    snapshot.facts.push(Fact {
        id: identity(serde_json::json!([key, ordinal])),
        kind: kind.into(),
        subject_id: subject.id.clone(),
        object_id: object.map(|u| u.id.clone()),
        target_spelling: spelling.map(str::to_owned),
        candidate_ids: None,
        site: site.clone(),
        status: if object.is_some() {
            "observed"
        } else {
            "unresolved"
        }
        .into(),
        provenance: Provenance {
            frontend: FRONTEND.into(),
            method: "syntax".into(),
            resolver: None,
            resolver_version: None,
        },
        extensions: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_languages_and_stale_source() {
        for (language, text) in [
            ("go", "package demo\nfunc add() { save() }\n"),
            ("python", "def add():\n    save()\n"),
            ("rust", "fn add() { save(); }\n"),
            ("typescript", "// é 🚀\nfunction add() { save(); }\n"),
        ] {
            let (snapshot, mut sources) = extract(
                "demo",
                &[InputFile {
                    root_id: "main",
                    relative_path: "add",
                    language,
                    bytes: text.as_bytes(),
                }],
            )
            .expect("four-language extraction succeeds");
            assert!(
                snapshot
                    .units
                    .iter()
                    .any(|u| u.name.as_deref() == Some("add")),
                "{language}"
            );
            assert!(
                snapshot
                    .facts
                    .iter()
                    .any(|f| f.kind == "calls" && f.target_spelling.as_deref() == Some("save")),
                "{language}"
            );
            let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
            let decoded: Snapshot = serde_json::from_str(&json).expect("deserialize snapshot");
            validate(&decoded, &sources).expect("round-trip validation");
            sources.values_mut().next().expect("one fixture source")[0] ^= 1;
            assert!(validate(&snapshot, &sources).is_err());
        }
        let text = "// é 🚀\r\nfn after_emoji() { save(); }\r\n";
        let (snapshot, _) = extract(
            "demo",
            &[InputFile {
                root_id: "main",
                relative_path: "unicode.rs",
                language: "rust",
                bytes: text.as_bytes(),
            }],
        )
        .expect("CRLF Unicode source parses");
        let function = snapshot
            .units
            .iter()
            .find(|unit| unit.name.as_deref() == Some("after_emoji"))
            .expect("function unit");
        assert_eq!(function.source.start_byte, "// é 🚀\r\n".len());
        assert_eq!(
            function.source.start,
            Position {
                line: 2,
                column_byte: 0
            }
        );
    }

    #[test]
    fn syntax_errors_are_partial_and_keep_opaque_source_coverage() {
        let text = "fn good() { call(); }\nfn broken( { uncertain(); }\n";
        let (snapshot, _) = extract(
            "demo",
            &[InputFile {
                root_id: "main",
                relative_path: "bad.rs",
                language: "rust",
                bytes: text.as_bytes(),
            }],
        )
        .expect("partial parse is a valid snapshot");
        assert_eq!(snapshot.files[0].extraction.status, "partial");
        assert!(
            snapshot
                .units
                .iter()
                .any(|unit| unit.name.as_deref() == Some("good"))
        );
        assert!(snapshot.units.iter().any(|unit| unit.kind == "opaque"));
        assert_eq!(
            snapshot
                .facts
                .iter()
                .filter(
                    |fact| fact.kind == "calls" && fact.target_spelling.as_deref() == Some("call")
                )
                .count(),
            1
        );
        assert!(!snapshot.facts.iter().any(
            |fact| fact.kind == "calls" && fact.target_spelling.as_deref() == Some("uncertain")
        ));
    }

    #[test]
    fn validates_complete_typescript_production_examples_across_runtime() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../test/fixtures/code-ir-v1/examples.json"
        ))
        .expect("conformance fixture JSON");
        for example in fixture["examples"].as_array().expect("examples array") {
            let snapshot: Snapshot = serde_json::from_value(example["snapshot"].clone())
                .expect("production snapshot decodes");
            let source = example["source"]
                .as_str()
                .expect("source literal")
                .as_bytes()
                .to_vec();
            let file = snapshot.files.first().expect("one file per fixture");
            let sources = BTreeMap::from([(file.file_id.clone(), source)]);
            validate(&snapshot, &sources).expect("Rust validates TypeScript serialization");
        }
    }

    #[test]
    fn shadow_manifest_publishes_full_snapshot_atomically_and_restarts() {
        let directory = tempfile::tempdir().expect("temporary sidecar directory");
        let source = "fn add() { save(); }\n";
        let (snapshot, sources) = extract(
            "demo",
            &[InputFile {
                root_id: "main",
                relative_path: "add.rs",
                language: "rust",
                bytes: source.as_bytes(),
            }],
        )
        .expect("valid source snapshot");
        let first = publish_shadow(directory.path(), &snapshot, &sources)
            .expect("publish shadow generation");
        let loaded =
            read_published_shadow(directory.path()).expect("restart reads published generation");
        assert_eq!(first.ir_snapshot_id, loaded.manifest.ir_snapshot_id);
        assert_eq!(loaded.snapshot.snapshot_id, snapshot.snapshot_id);
        assert_eq!(loaded.sources, sources);
        assert_eq!(loaded.manifest.mode, "shadow");
        assert_eq!(loaded.manifest.projection_version, None);
        assert_eq!(
            publish_shadow(directory.path(), &snapshot, &sources)
                .expect("idempotent publish")
                .ir_snapshot_id,
            snapshot.snapshot_id
        );
    }

    #[test]
    fn rust_runtime_preserves_receiver_decorator_impl_and_overload_units() {
        let cases = [
            (
                "go",
                "package demo\ntype Service struct{}\nfunc (s *Service) Run() { save() }\n",
                "Run",
                "method",
            ),
            (
                "python",
                "@register\nclass Service:\n    @cached\n    def fetch(self):\n        def inner():\n            save()\n        return inner()\n    def plain(self):\n        save()\n",
                "fetch",
                "method",
            ),
            (
                "rust",
                "mod api { pub struct Service; impl Service { fn run(&self) { save(); } } }\n",
                "run",
                "method",
            ),
            (
                "typescript",
                "class Service { run() {} }\nfunction add(): number;\nfunction add(value: number): number;\nfunction add(value = 0) { return value; }\n",
                "add",
                "function",
            ),
        ];
        for (language, text, name, kind) in cases {
            let (snapshot, sources) = extract(
                "demo",
                &[InputFile {
                    root_id: "main",
                    relative_path: "fixture",
                    language,
                    bytes: text.as_bytes(),
                }],
            )
            .expect("language fixture parses");
            assert_eq!(
                snapshot.files[0].extraction.status, "complete",
                "{language}"
            );
            assert!(
                snapshot
                    .units
                    .iter()
                    .any(|unit| unit.name.as_deref() == Some(name) && unit.kind == kind),
                "{language}: {name}"
            );
            if language == "python" {
                assert!(
                    snapshot
                        .units
                        .iter()
                        .any(|unit| unit.name.as_deref() == Some("plain") && unit.kind == "method")
                );
            }
            if language == "typescript" {
                assert_eq!(
                    snapshot
                        .units
                        .iter()
                        .filter(|unit| unit.name.as_deref() == Some("add"))
                        .count(),
                    3
                );
            }
            validate(&snapshot, &sources).expect("language IR validates");
        }
    }
}
