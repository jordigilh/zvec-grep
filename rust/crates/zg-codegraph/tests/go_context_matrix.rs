use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;
use tempfile::TempDir;
use zg_codegraph::{
    CallGraphIndex, GO_CALLFACTS_FILE, GO_CALLFACTS_SCHEMA, GO_CALLFACTS_VERSION, GoCallFact,
    GoCallFactsArtifact, GoCallFactsContext, GoCallFactsFile, refresh_codegraph,
};

#[derive(Debug, Deserialize)]
struct ContextMatrixTruth {
    source_sha256: BTreeMap<String, String>,
    profiles: BTreeMap<String, ProfileTruth>,
    generic_target: String,
}

#[derive(Debug, Deserialize)]
struct ProfileTruth {
    goos: String,
    goflags: String,
    platform_target: String,
    tagged_target: String,
}

#[test]
fn blast_radius_truth_is_bound_to_platform_and_tag_context() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-context-matrix");
    let truth: ContextMatrixTruth =
        serde_json::from_slice(&fs::read(fixture.join("truth.json")).expect("matrix truth"))
            .expect("decode matrix truth");
    let mut fingerprints = Vec::new();

    for profile_name in ["linux-default", "windows-special"] {
        let profile = truth
            .profiles
            .get(profile_name)
            .unwrap_or_else(|| panic!("missing truth profile {profile_name}"));
        let workspace = copy_fixture(&fixture, &truth);
        let facts = sidecar(&workspace, &truth, profile);
        let fingerprint = facts.context_sha256.clone();
        write_sidecar(workspace.path(), &facts);

        let (_, artifact) = refresh_codegraph(workspace.path()).expect("refresh context graph");
        assert_eq!(
            artifact.go_callfacts_context_sha256.as_deref(),
            Some(fingerprint.as_str())
        );
        let index = CallGraphIndex::new(&artifact);
        let platform_query = graph_query_name(&profile.platform_target);
        assert_callers(
            &index,
            &platform_query,
            &[
                "matrix/caller.go::platformCaller",
                if profile.goos == "linux" {
                    "matrix/platform_linux.go::platformOnlyCaller"
                } else {
                    "matrix/platform_windows.go::platformOnlyCaller"
                },
            ],
            &fingerprint,
        );
        let tagged_query = graph_query_name(&profile.tagged_target);
        assert_callers(
            &index,
            &tagged_query,
            &[
                "matrix/caller.go::taggedCaller",
                if profile.goflags.is_empty() {
                    "matrix/tag_default.go::tagOnlyCaller"
                } else {
                    "matrix/tag_special.go::tagOnlyCaller"
                },
            ],
            &fingerprint,
        );
        let generic_query = graph_query_name(&truth.generic_target);
        assert_callers(
            &index,
            &generic_query,
            &["matrix/caller.go::genericCaller"],
            &fingerprint,
        );

        let inactive_platform = if profile.goos == "linux" {
            "matrix/platform_windows.go::matrix.platformTarget"
        } else {
            "matrix/platform_linux.go::matrix.platformTarget"
        };
        assert_callers(
            &index,
            &graph_query_name(inactive_platform),
            &[],
            &fingerprint,
        );
        let inactive_tag = if profile.goflags.is_empty() {
            "matrix/tag_special.go::matrix.taggedTarget"
        } else {
            "matrix/tag_default.go::matrix.taggedTarget"
        };
        assert_callers(&index, &graph_query_name(inactive_tag), &[], &fingerprint);
        fingerprints.push(fingerprint);
    }

    assert_ne!(fingerprints[0], fingerprints[1]);
}

fn graph_query_name(symbol: &str) -> String {
    let (path, qualified_name) = symbol
        .split_once("::")
        .unwrap_or_else(|| panic!("missing symbol path in {symbol}"));
    let name = qualified_name
        .rsplit('.')
        .next()
        .expect("qualified symbol name");
    format!("{path}::{name}")
}

fn assert_callers(index: &CallGraphIndex, target: &str, expected: &[&str], fingerprint: &str) {
    let result = index
        .blast_radius(target, Some(1))
        .unwrap_or_else(|error| panic!("blast radius for {target}: {error}"));
    let expected: Vec<Vec<String>> = if expected.is_empty() {
        Vec::new()
    } else {
        vec![expected.iter().map(|caller| (*caller).to_owned()).collect()]
    };
    assert_eq!(result.callers_by_depth, expected, "callers for {target}");
    assert!(result.possible_callers_by_depth.is_empty());
    assert_eq!(
        result.go_callfacts_context_sha256.as_deref(),
        Some(fingerprint)
    );
}

fn copy_fixture(fixture: &Path, truth: &ContextMatrixTruth) -> TempDir {
    let workspace = tempfile::tempdir().expect("workspace");
    for relative in truth.source_sha256.keys() {
        let source = fixture.join(relative);
        let target = workspace.path().join(relative);
        fs::create_dir_all(target.parent().expect("fixture parent")).expect("fixture directory");
        fs::copy(source, target).expect("copy pinned fixture input");
    }
    workspace
}

fn sidecar(
    workspace: &TempDir,
    truth: &ContextMatrixTruth,
    profile: &ProfileTruth,
) -> GoCallFactsArtifact {
    let mut files = truth
        .source_sha256
        .iter()
        .filter(|(path, _)| {
            Path::new(path)
                .extension()
                .is_some_and(|extension| extension == "go")
        })
        .map(|(path, sha256)| GoCallFactsFile {
            path: path.clone(),
            sha256: sha256.clone(),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let context_files = truth
        .source_sha256
        .iter()
        .filter(|(path, _)| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == "go.mod")
        })
        .map(|(path, sha256)| GoCallFactsFile {
            path: path.clone(),
            sha256: sha256.clone(),
        })
        .collect();
    let context = GoCallFactsContext {
        go_version: "go1.26.0".to_owned(),
        go_mod: "go.mod".to_owned(),
        go_work: "off".to_owned(),
        settings: context_settings(profile),
        context_files,
    };
    let calls = matrix_call_facts(workspace, truth, profile);
    let context_sha256 = context.fingerprint();
    GoCallFactsArtifact {
        schema: GO_CALLFACTS_SCHEMA.to_owned(),
        version: GO_CALLFACTS_VERSION,
        context,
        context_sha256,
        files,
        calls,
    }
}

fn context_settings(profile: &ProfileTruth) -> BTreeMap<String, String> {
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
            "GOOS" => profile.goos.as_str(),
            "GOARCH" => "amd64",
            "CGO_ENABLED" => "0",
            "GOFLAGS" => profile.goflags.as_str(),
            "GOTOOLCHAIN" => "auto",
            _ => "",
        };
        (name.to_owned(), value.to_owned())
    })
    .collect()
}

fn matrix_call_facts(
    workspace: &TempDir,
    truth: &ContextMatrixTruth,
    profile: &ProfileTruth,
) -> Vec<GoCallFact> {
    let mut facts = vec![
        fact(
            workspace,
            "matrix/caller.go",
            "matrix/caller.go::matrix.platformCaller",
            "platformTarget()",
            "platformTarget",
            Some(&profile.platform_target),
            "static",
        ),
        fact(
            workspace,
            "matrix/caller.go",
            "matrix/caller.go::matrix.taggedCaller",
            "taggedTarget()",
            "taggedTarget",
            Some(&profile.tagged_target),
            "static",
        ),
        fact(
            workspace,
            "matrix/caller.go",
            "matrix/caller.go::matrix.genericCaller",
            "identity[int](1)",
            "identity[int]",
            Some(&truth.generic_target),
            "static",
        ),
        fact(
            workspace,
            "matrix/caller.go",
            "matrix/caller.go::matrix.externalCaller",
            "fmt.Println(\"external\")",
            "fmt.Println",
            None,
            "external",
        ),
    ];
    for (path, active) in [
        ("matrix/platform_linux.go", profile.goos == "linux"),
        ("matrix/platform_windows.go", profile.goos == "windows"),
    ] {
        let target = if active {
            Some(format!("{path}::matrix.platformTarget"))
        } else {
            None
        };
        facts.push(fact(
            workspace,
            path,
            &format!("{path}::matrix.platformOnlyCaller"),
            "platformTarget()",
            "platformTarget",
            target.as_deref(),
            if active { "static" } else { "unresolved" },
        ));
    }
    for (path, active) in [
        ("matrix/tag_default.go", profile.goflags.is_empty()),
        ("matrix/tag_special.go", !profile.goflags.is_empty()),
    ] {
        let target = if active {
            Some(format!("{path}::matrix.taggedTarget"))
        } else {
            None
        };
        facts.push(fact(
            workspace,
            path,
            &format!("{path}::matrix.tagOnlyCaller"),
            "taggedTarget()",
            "taggedTarget",
            target.as_deref(),
            if active { "static" } else { "unresolved" },
        ));
    }
    facts
}

fn fact(
    workspace: &TempDir,
    path: &str,
    caller: &str,
    expression: &str,
    target_name: &str,
    target: Option<&str>,
    resolution: &str,
) -> GoCallFact {
    let source = fs::read_to_string(workspace.path().join(path)).expect("read call source");
    let start_byte = source.rfind(expression).expect("locate call expression");
    let end_byte = start_byte + expression.len();
    let (start_line, start_column) = source_position(&source, start_byte);
    let (end_line, end_column) = source_position(&source, end_byte);
    GoCallFact {
        path: path.to_owned(),
        start_byte,
        end_byte,
        start_line,
        end_line,
        start_column,
        end_column,
        caller: caller.to_owned(),
        target_name: target_name.to_owned(),
        target: target.map(str::to_owned),
        possible_targets: Vec::new(),
        resolution: resolution.to_owned(),
    }
}

fn source_position(source: &str, byte_offset: usize) -> (usize, usize) {
    let prefix = &source.as_bytes()[..byte_offset];
    let line = prefix.split(|byte| *byte == b'\n').count();
    let column = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(prefix.len(), |newline| prefix.len() - newline - 1);
    (line, column)
}

fn write_sidecar(root: &Path, artifact: &GoCallFactsArtifact) {
    let path = root.join(".zvec-grep").join(GO_CALLFACTS_FILE);
    fs::create_dir_all(path.parent().expect("sidecar parent")).expect("create sidecar dir");
    fs::write(path, serde_json::to_vec(artifact).expect("encode sidecar")).expect("write sidecar");
}
