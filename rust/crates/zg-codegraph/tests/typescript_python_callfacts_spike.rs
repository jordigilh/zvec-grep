use std::{collections::BTreeMap, fs, path::Path};

use tempfile::TempDir;
use zg_codegraph::{
    CodeGraphArtifact, CodeGraphNode, PYTHON_CALLFACTS_FILE, PYTHON_CALLFACTS_SCHEMA,
    PYTHON_CALLFACTS_VERSION, PythonCallFact, PythonCallFactsArtifact, PythonCallFactsContext,
    PythonCallFactsFile, PythonCallFactsSymbol, TYPESCRIPT_CALLFACTS_FILE,
    TYPESCRIPT_CALLFACTS_SCHEMA, TYPESCRIPT_CALLFACTS_VERSION, TypeScriptCallFact,
    TypeScriptCallFactsArtifact, TypeScriptCallFactsContext, TypeScriptCallFactsFile,
    TypeScriptCallFactsSymbol, build_codegraph, refresh_codegraph,
};

#[test]
fn typescript_callfacts_replace_static_sites_and_preserve_unresolved_calls() {
    let workspace = typescript_fixture();
    let syntax = build_codegraph(workspace.path()).expect("TypeScript syntax graph");
    let helper = function_node(&syntax, "helper");
    let caller = function_node(&syntax, "caller");
    let dynamic = function_node(&syntax, "dynamic");
    let helper_call = syntax_call(&syntax, &caller.id, "helper");
    let dynamic_call = syntax_call(&syntax, &dynamic.id, "fn");
    write_typescript_sidecar(
        workspace.path(),
        &syntax,
        vec![
            TypeScriptCallFact {
                path: "src/lib.ts".to_owned(),
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
            TypeScriptCallFact {
                path: "src/lib.ts".to_owned(),
                start_byte: dynamic_call.start_byte,
                end_byte: dynamic_call.end_byte,
                start_line: dynamic_call.start_line,
                end_line: dynamic_call.end_line,
                start_column: dynamic_call.start_column,
                end_column: dynamic_call.end_column,
                caller: symbol(dynamic),
                target_name: "fn".to_owned(),
                target: None,
                possible_targets: Vec::new(),
                resolution: "function-value".to_owned(),
            },
        ],
    );

    let (_, graph) = refresh_codegraph(workspace.path()).expect("TypeScript semantic graph");
    assert!(graph.typescript_callfacts_context_sha256.is_some());
    let helper_edge = graph
        .edges
        .iter()
        .find(|edge| edge.kind == "calls" && edge.source == caller.id)
        .expect("helper call edge");
    assert_eq!(helper_edge.target.as_deref(), Some(helper.id.as_str()));
    assert!(helper_edge.resolved);
    let dynamic_edge = graph
        .edges
        .iter()
        .find(|edge| edge.kind == "calls" && edge.source == dynamic.id)
        .expect("dynamic call edge");
    assert!(!dynamic_edge.resolved);
    assert!(dynamic_edge.ambiguous_candidates.is_empty());

    let config = workspace.path().join("config/base.json");
    fs::remove_file(&config).expect("remove extended TypeScript context");
    let (_, stale) = refresh_codegraph(workspace.path()).expect("TypeScript stale fallback");
    assert!(stale.typescript_callfacts_context_sha256.is_none());

    fs::write(
        workspace
            .path()
            .join(".zvec-grep")
            .join(TYPESCRIPT_CALLFACTS_FILE),
        b"malformed TypeScript facts",
    )
    .expect("malformed TypeScript sidecar");
    let (_, fallback) = refresh_codegraph(workspace.path()).expect("TypeScript fallback graph");
    assert!(fallback.typescript_callfacts_context_sha256.is_none());
}

#[test]
fn python_callfacts_replace_static_sites_and_reject_stale_context() {
    let workspace = python_fixture();
    let syntax = build_codegraph(workspace.path()).expect("Python syntax graph");
    let helper = function_node(&syntax, "helper");
    let caller = function_node(&syntax, "caller");
    let call = syntax_call(&syntax, &caller.id, "helper");
    write_python_sidecar(
        workspace.path(),
        &syntax,
        vec![PythonCallFact {
            path: "src/lib.py".to_owned(),
            start_byte: call.start_byte,
            end_byte: call.end_byte,
            start_line: call.start_line,
            end_line: call.end_line,
            start_column: call.start_column,
            end_column: call.end_column,
            caller: python_symbol(caller),
            target_name: "helper".to_owned(),
            target: Some(python_symbol(helper)),
            possible_targets: Vec::new(),
            resolution: "static".to_owned(),
        }],
    );

    let (_, initial) = refresh_codegraph(workspace.path()).expect("Python semantic graph");
    assert!(initial.python_callfacts_context_sha256.is_some());

    let config = workspace.path().join("pyrightconfig.json");
    let mut changed = fs::read(&config).expect("pyright config");
    changed.extend_from_slice(b"\n");
    fs::write(&config, changed).expect("change Python context");
    let (_, fallback) = refresh_codegraph(workspace.path()).expect("Python syntax fallback");
    assert!(fallback.python_callfacts_context_sha256.is_none());
    assert!(
        fallback
            .edges
            .iter()
            .any(|edge| edge.kind == "calls" && edge.source == caller.id)
    );
}

fn typescript_fixture() -> TempDir {
    let workspace = tempfile::tempdir().expect("workspace");
    fs::create_dir_all(workspace.path().join("src")).expect("src");
    fs::create_dir_all(workspace.path().join("config")).expect("config");
    fs::write(
        workspace.path().join("config/base.json"),
        "{\"compilerOptions\":{\"target\":\"ES2022\"}}\n",
    )
    .expect("base TypeScript config");
    fs::write(
        workspace.path().join("tsconfig.json"),
        "{\"extends\":\"./config/base.json\",\"include\":[\"src/**/*.ts\"]}\n",
    )
    .expect("tsconfig");
    fs::write(
        workspace.path().join("src/lib.ts"),
        concat!(
            "export function helper(value: number): number { return value + 1; }\n",
            "export function caller(value: number): number { return helper(value); }\n",
            "export function dynamic(value: number): number { const fn: any = helper; return fn(value); }\n",
        ),
    )
    .expect("TypeScript source");
    workspace
}

fn python_fixture() -> TempDir {
    let workspace = tempfile::tempdir().expect("workspace");
    fs::create_dir_all(workspace.path().join("src")).expect("src");
    fs::write(
        workspace.path().join("pyrightconfig.json"),
        "{\"include\":[\"src\"],\"pythonVersion\":\"3.13\"}\n",
    )
    .expect("pyrightconfig");
    fs::write(
        workspace.path().join("src/lib.py"),
        concat!(
            "def helper(value: int) -> int:\n",
            "    return value + 1\n",
            "\n",
            "def caller(value: int) -> int:\n",
            "    return helper(value)\n",
        ),
    )
    .expect("Python source");
    workspace
}

fn write_typescript_sidecar(
    root: &Path,
    graph: &CodeGraphArtifact,
    calls: Vec<TypeScriptCallFact>,
) {
    let files = graph
        .files
        .iter()
        .filter(|file| matches!(file.language.as_str(), "typescript" | "tsx"))
        .map(|file| TypeScriptCallFactsFile {
            path: file.path.clone(),
            sha256: file.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let context_files = ["config/base.json", "tsconfig.json"]
        .into_iter()
        .map(|path| TypeScriptCallFactsFile {
            path: path.to_owned(),
            sha256: sha256(&fs::read(root.join(path)).expect("TypeScript config")),
        })
        .collect();
    let context = TypeScriptCallFactsContext {
        typescript_version: "5.9.3".to_owned(),
        node_version: "v22.0.0".to_owned(),
        project_path: "tsconfig.json".to_owned(),
        target: "ES2022".to_owned(),
        module: "CommonJS".to_owned(),
        jsx: "unknown".to_owned(),
        settings: BTreeMap::new(),
        context_files,
    };
    let artifact = TypeScriptCallFactsArtifact {
        schema: TYPESCRIPT_CALLFACTS_SCHEMA.to_owned(),
        version: TYPESCRIPT_CALLFACTS_VERSION,
        context_sha256: context.fingerprint(),
        context,
        files,
        calls,
    };
    write_json(
        root.join(".zvec-grep").join(TYPESCRIPT_CALLFACTS_FILE),
        &artifact,
    );
}

fn write_python_sidecar(root: &Path, graph: &CodeGraphArtifact, calls: Vec<PythonCallFact>) {
    let files = graph
        .files
        .iter()
        .filter(|file| file.language == "python")
        .map(|file| PythonCallFactsFile {
            path: file.path.clone(),
            sha256: file.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let context_file = PythonCallFactsFile {
        path: "pyrightconfig.json".to_owned(),
        sha256: sha256(&fs::read(root.join("pyrightconfig.json")).expect("pyright config")),
    };
    let context = PythonCallFactsContext {
        pyright_version: "1.1.414".to_owned(),
        python_version: "3.14.6".to_owned(),
        target_version: "3.13".to_owned(),
        project_path: "pyrightconfig.json".to_owned(),
        typeshed_sha256: "test-typeshed".to_owned(),
        settings: BTreeMap::new(),
        context_files: vec![context_file],
    };
    let artifact = PythonCallFactsArtifact {
        schema: PYTHON_CALLFACTS_SCHEMA.to_owned(),
        version: PYTHON_CALLFACTS_VERSION,
        context_sha256: context.fingerprint(),
        context,
        files,
        calls,
    };
    write_json(
        root.join(".zvec-grep").join(PYTHON_CALLFACTS_FILE),
        &artifact,
    );
}

fn write_json(path: std::path::PathBuf, value: &impl serde::Serialize) {
    fs::create_dir_all(path.parent().expect("sidecar directory")).expect("sidecar directory");
    fs::write(path, serde_json::to_vec(value).expect("sidecar JSON")).expect("sidecar");
}

fn function_node<'a>(graph: &'a CodeGraphArtifact, name: &str) -> &'a CodeGraphNode {
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

fn symbol(node: &CodeGraphNode) -> TypeScriptCallFactsSymbol {
    let range = node.range.as_ref().expect("definition range");
    TypeScriptCallFactsSymbol {
        path: node.path.clone().expect("definition path"),
        start_byte: range.start_byte,
        end_byte: range.end_byte,
    }
}

fn python_symbol(node: &CodeGraphNode) -> PythonCallFactsSymbol {
    let range = node.range.as_ref().expect("definition range");
    PythonCallFactsSymbol {
        path: node.path.clone().expect("definition path"),
        start_byte: range.start_byte,
        end_byte: range.end_byte,
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
