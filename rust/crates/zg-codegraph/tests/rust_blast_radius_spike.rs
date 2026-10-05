use std::{collections::BTreeMap, fs, path::Path};

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zg_codegraph::{
    CODEGRAPH_PUBLICATION_FILE, CallGraphIndex, CodeGraphArtifact, CodeGraphNode,
    RUST_CALLFACTS_FILE, RUST_CALLFACTS_SCHEMA, RUST_CALLFACTS_VERSION, RustCallFact,
    RustCallFactsArtifact, RustCallFactsContext, RustCallFactsFile, RustCallFactsSymbol,
    build_codegraph, read_codegraph_publication, refresh_codegraph, validate_codegraph_publication,
};

#[test]
fn rust_callfacts_replace_supported_sites_and_keep_possible_dispatch_separate() {
    let workspace = fixture();
    let syntax = build_codegraph(workspace.path()).expect("syntax graph");
    let helper = node(&syntax, "helper");
    let caller = node(&syntax, "caller");
    let trait_work = syntax
        .nodes
        .iter()
        .find(|node| node.kind == "method" && node.qualified_name.as_deref() == Some("Worker.work"))
        .expect("trait method node");
    let dyn_caller = node(&syntax, "dyn_caller");

    let helper_call = syntax_call(&syntax, &caller.id, "helper");
    let trait_call = syntax_call(&syntax, &dyn_caller.id, "work");
    write_sidecar(
        workspace.path(),
        &syntax,
        vec![
            RustCallFact {
                path: "src/lib.rs".to_owned(),
                start_byte: helper_call.start_byte,
                end_byte: helper_call.end_byte,
                start_line: helper_call.start_line,
                end_line: helper_call.end_line,
                start_column: helper_call.start_column,
                end_column: helper_call.end_column,
                caller: symbol(caller),
                target_name: "helper".to_owned(),
                target: Some(symbol(helper)),
                possible_targets: Vec::new(),
                resolution: "static".to_owned(),
            },
            RustCallFact {
                path: "src/lib.rs".to_owned(),
                start_byte: trait_call.start_byte,
                end_byte: trait_call.end_byte,
                start_line: trait_call.start_line,
                end_line: trait_call.end_line,
                start_column: trait_call.start_column,
                end_column: trait_call.end_column,
                caller: symbol(dyn_caller),
                target_name: "work".to_owned(),
                target: None,
                possible_targets: vec![symbol(trait_work)],
                resolution: "trait-dispatch".to_owned(),
            },
        ],
    );

    let semantic = build_codegraph(workspace.path()).expect("attested Rust graph");
    assert!(semantic.rust_callfacts_context_sha256.is_some());
    let semantic_calls = semantic
        .edges
        .iter()
        .filter(|edge| edge.kind == "calls")
        .collect::<Vec<_>>();
    assert!(semantic_calls.iter().any(|edge| {
        edge.source == caller.id
            && edge.target.as_deref() == Some(helper.id.as_str())
            && edge.resolved
    }));
    assert!(semantic_calls.iter().any(|edge| {
        edge.source == dyn_caller.id
            && edge.target.is_none()
            && !edge.resolved
            && edge.ambiguous_candidates == [trait_work.id.clone()]
    }));

    let index = CallGraphIndex::new(&semantic);
    assert_eq!(
        index
            .blast_radius("helper", Some(1))
            .expect("helper callers")
            .callers_by_depth,
        [vec!["src/lib.rs::caller".to_owned()]]
    );
    assert_eq!(
        index
            .blast_radius("Worker.work", Some(1))
            .expect("trait method callers")
            .possible_callers_by_depth,
        [vec!["src/lib.rs::dyn_caller".to_owned()]]
    );
}

#[test]
fn stale_rust_callfacts_fall_back_to_syntax_edges() {
    let workspace = fixture();
    let syntax = build_codegraph(workspace.path()).expect("syntax graph");
    let helper = node(&syntax, "helper");
    let caller = node(&syntax, "caller");
    let call = syntax_call(&syntax, &caller.id, "helper");
    write_sidecar(
        workspace.path(),
        &syntax,
        vec![RustCallFact {
            path: "src/lib.rs".to_owned(),
            start_byte: call.start_byte,
            end_byte: call.end_byte,
            start_line: call.start_line,
            end_line: call.end_line,
            start_column: call.start_column,
            end_column: call.end_column,
            caller: symbol(caller),
            target_name: "helper".to_owned(),
            target: Some(symbol(helper)),
            possible_targets: Vec::new(),
            resolution: "static".to_owned(),
        }],
    );
    let (_, initial) = refresh_codegraph(workspace.path()).expect("semantic graph");
    assert!(initial.rust_callfacts_context_sha256.is_some());

    let mut changed_source = fs::read(workspace.path().join("src/lib.rs")).expect("source");
    changed_source.extend_from_slice(b"\n");
    fs::write(workspace.path().join("src/lib.rs"), changed_source).expect("change source");
    let (_, fallback) = refresh_codegraph(workspace.path()).expect("syntax fallback graph");
    assert!(fallback.rust_callfacts_context_sha256.is_none());
    let call = fallback
        .edges
        .iter()
        .find(|edge| edge.kind == "calls" && edge.source == caller.id)
        .expect("syntax call");
    assert!(call.resolved);
}

#[test]
fn rust_callfacts_refresh_revalidates_context_and_malformed_sidecars() {
    let workspace = fixture();
    let syntax = build_codegraph(workspace.path()).expect("syntax graph");
    let helper = node(&syntax, "helper");
    let caller = node(&syntax, "caller");
    let call = syntax_call(&syntax, &caller.id, "helper");
    write_sidecar(
        workspace.path(),
        &syntax,
        vec![RustCallFact {
            path: "src/lib.rs".to_owned(),
            start_byte: call.start_byte,
            end_byte: call.end_byte,
            start_line: call.start_line,
            end_line: call.end_line,
            start_column: call.start_column,
            end_column: call.end_column,
            caller: symbol(caller),
            target_name: "helper".to_owned(),
            target: Some(symbol(helper)),
            possible_targets: Vec::new(),
            resolution: "static".to_owned(),
        }],
    );

    let (_, initial) = refresh_codegraph(workspace.path()).expect("initial semantic refresh");
    let initial_context = initial
        .rust_callfacts_context_sha256
        .clone()
        .expect("Rust context fingerprint");
    let publication = read_codegraph_publication(
        &workspace
            .path()
            .join(".zvec-grep")
            .join(CODEGRAPH_PUBLICATION_FILE),
    )
    .expect("semantic publication");
    assert_eq!(publication.sidecars.len(), 1);
    assert_eq!(publication.sidecars[0].language, "rust");
    assert_eq!(
        validate_codegraph_publication(workspace.path()).expect("validate semantic publication"),
        initial
    );
    let sidecar_path = rust_callfacts_path(workspace.path());
    let sidecar = fs::read(&sidecar_path).expect("read Rust sidecar");
    let mut changed_sidecar = sidecar.clone();
    changed_sidecar.extend_from_slice(b"\n");
    fs::write(&sidecar_path, changed_sidecar).expect("change Rust sidecar");
    assert!(validate_codegraph_publication(workspace.path()).is_err());
    fs::write(&sidecar_path, sidecar).expect("restore Rust sidecar");
    let module_path = workspace.path().join("Cargo.toml");
    let module = fs::read(&module_path).expect("read Cargo.toml");
    let mut changed_module = module.clone();
    changed_module.extend_from_slice(b"\n# context changed after generation\n");
    fs::write(&module_path, changed_module).expect("change Cargo.toml");

    let (_, context_fallback) =
        refresh_codegraph(workspace.path()).expect("changed context should fall back");
    assert!(context_fallback.rust_callfacts_context_sha256.is_none());
    assert!(
        context_fallback
            .edges
            .iter()
            .any(|edge| edge.kind == "calls" && edge.source == caller.id && edge.resolved)
    );

    fs::write(&module_path, module).expect("restore Cargo.toml");
    let (_, restored) = refresh_codegraph(workspace.path()).expect("restore context");
    assert_eq!(
        restored.rust_callfacts_context_sha256.as_deref(),
        Some(initial_context.as_str())
    );

    fs::write(rust_callfacts_path(workspace.path()), b"not valid JSON")
        .expect("corrupt optional sidecar");
    let (_, malformed) = refresh_codegraph(workspace.path()).expect("malformed sidecar fallback");
    assert!(malformed.rust_callfacts_context_sha256.is_none());
    assert!(
        malformed
            .edges
            .iter()
            .any(|edge| edge.kind == "calls" && edge.source == caller.id && edge.resolved)
    );
}

fn fixture() -> TempDir {
    let workspace = tempfile::tempdir().expect("workspace");
    fs::create_dir_all(workspace.path().join("src")).expect("src directory");
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("Cargo.toml");
    fs::write(
        workspace.path().join("src/lib.rs"),
        "pub trait Worker { fn work(&self, value: i32) -> i32; }\n\
pub struct Impl;\n\
impl Worker for Impl { fn work(&self, value: i32) -> i32 { value + 1 } }\n\
pub fn helper(value: i32) -> i32 { value + 2 }\n\
pub fn caller(value: i32) -> i32 { helper(value) }\n\
pub fn dyn_caller(value: i32) -> i32 { let worker: &dyn Worker = &Impl; worker.work(value) }\n",
    )
    .expect("Rust source");
    workspace
}

fn write_sidecar(root: &Path, graph: &CodeGraphArtifact, calls: Vec<RustCallFact>) {
    let files = graph
        .files
        .iter()
        .filter(|file| file.language == "rust")
        .map(|file| RustCallFactsFile {
            path: file.path.clone(),
            sha256: file.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let context_file = RustCallFactsFile {
        path: "Cargo.toml".to_owned(),
        sha256: sha256(&fs::read(root.join("Cargo.toml")).expect("Cargo.toml")),
    };
    let context = RustCallFactsContext {
        rustc_version: "rustc 1.98.0 (test)".to_owned(),
        rustc_commit: "test-commit".to_owned(),
        host: "aarch64-apple-darwin".to_owned(),
        target: "aarch64-apple-darwin".to_owned(),
        edition: "2024".to_owned(),
        manifest_path: "Cargo.toml".to_owned(),
        lockfile_path: None,
        toolchain_path: None,
        settings: BTreeMap::new(),
        context_files: vec![context_file],
    };
    let artifact = RustCallFactsArtifact {
        schema: RUST_CALLFACTS_SCHEMA.to_owned(),
        version: RUST_CALLFACTS_VERSION,
        context_sha256: context.fingerprint(),
        context,
        files,
        calls,
    };
    let path = root.join(".zvec-grep").join(RUST_CALLFACTS_FILE);
    fs::create_dir_all(path.parent().expect("sidecar directory")).expect("sidecar directory");
    fs::write(path, serde_json::to_vec(&artifact).expect("sidecar JSON")).expect("sidecar");
}

fn rust_callfacts_path(root: &Path) -> std::path::PathBuf {
    root.join(".zvec-grep").join(RUST_CALLFACTS_FILE)
}

fn node<'a>(graph: &'a CodeGraphArtifact, name: &str) -> &'a CodeGraphNode {
    graph
        .nodes
        .iter()
        .find(|node| node.kind == "function" && node.name == name)
        .unwrap_or_else(|| panic!("missing function {name}: {:#?}", graph.nodes))
}

fn syntax_call<'a>(
    graph: &'a CodeGraphArtifact,
    source: &str,
    target_name: &str,
) -> &'a zg_codegraph::CodeGraphRange {
    graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == "calls"
                && edge.source == source
                && edge
                    .target_name
                    .as_deref()
                    .is_some_and(|name| name.contains(target_name))
        })
        .and_then(|edge| edge.range.as_ref())
        .unwrap_or_else(|| panic!("missing {target_name} call from {source}"))
}

fn symbol(node: &CodeGraphNode) -> RustCallFactsSymbol {
    let range = node.range.as_ref().expect("definition range");
    RustCallFactsSymbol {
        path: node.path.clone().expect("definition path"),
        start_byte: range.start_byte,
        end_byte: range.end_byte,
    }
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}
