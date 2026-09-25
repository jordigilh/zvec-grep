use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use zg_code_ir::{InputFile, Snapshot, extract};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 2 {
        return Err("usage: conformance FIXTURE_ROOT".into());
    }
    let root = PathBuf::from(&args[1]);
    let mut reports = Vec::new();
    for (language, fixture) in [
        ("go", "go-workflow-discovery-v1"),
        ("python", "python-workflow-discovery-v1"),
        ("rust", "rust-workflow-discovery-v1"),
        ("typescript", "typescript-workflow-discovery-v1"),
    ] {
        reports.push(run_fixture(&root, language, fixture)?);
    }
    println!("{}", serde_json::to_string_pretty(&reports)?);
    Ok(())
}

fn run_fixture(
    root: &Path,
    language: &str,
    fixture: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let fixture_root = root.join(fixture);
    let manifest: Value = serde_json::from_slice(&fs::read(fixture_root.join("manifest.json"))?)?;
    let units = manifest["units"]
        .as_array()
        .ok_or("manifest units array missing")?;
    let repository_id = manifest["source"]["repository"]
        .as_str()
        .ok_or("repository missing")?;
    let mut paths = BTreeSet::new();
    for unit in units {
        paths.insert(unit["path"].as_str().ok_or("unit path missing")?.to_owned());
    }
    let files: Vec<_> = paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(fixture_root.join(&path))?;
            Ok((path, bytes))
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    let inputs: Vec<_> = files
        .iter()
        .map(|(path, bytes)| InputFile {
            root_id: fixture,
            relative_path: path,
            language,
            bytes,
        })
        .collect();
    let (snapshot, _) = extract(repository_id, &inputs).map_err(std::io::Error::other)?;
    report(language, fixture, units, &snapshot)
}

#[allow(clippy::too_many_lines)]
fn report(
    language: &str,
    fixture: &str,
    manifest_units: &[Value],
    snapshot: &Snapshot,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut unique_name_matches = 0;
    let mut ambiguous = Vec::new();
    let mut missing = Vec::new();
    let mut line_span_matches = 0;
    let mut line_span_total = 0;
    let mut line_span_mismatches = Vec::new();
    let mut rust_modules = Vec::new();
    for expected in manifest_units {
        let path = expected["path"].as_str().ok_or("path missing")?;
        let file = snapshot
            .files
            .iter()
            .find(|file| file.relative_path == path)
            .ok_or("IR file missing")?;
        let symbol = expected["symbol"].as_str().ok_or("symbol missing")?;
        let candidates: Vec<_> = if symbol == "module-declarations" {
            let modules: Vec<_> = snapshot
                .units
                .iter()
                .filter(|unit| unit.source.file_id == file.file_id && unit.kind == "module")
                .collect();
            rust_modules
                .push(json!({"unit_id": expected["unit_id"], "module_units": modules.len()}));
            modules
        } else {
            let name = symbol.rsplit('.').next().unwrap_or(symbol);
            snapshot
                .units
                .iter()
                .filter(|unit| {
                    unit.source.file_id == file.file_id && unit.name.as_deref() == Some(name)
                })
                .collect()
        };
        if candidates.is_empty() {
            missing.push(expected["unit_id"].clone());
        } else if candidates.len() == 1 {
            unique_name_matches += 1;
        } else {
            ambiguous
                .push(json!({"unit_id": expected["unit_id"], "candidate_count": candidates.len()}));
        }
        if let (Some(start_line), Some(end_line)) = (
            expected["start_line"].as_u64(),
            expected["end_line"].as_u64(),
        ) {
            line_span_total += 1;
            let matched = candidates.iter().any(|unit| {
                unit.source.start.line as u64 == start_line
                    && unit.source.end.line as u64 == end_line
            });
            if matched {
                line_span_matches += 1;
            } else {
                line_span_mismatches.push(json!({"unit_id": expected["unit_id"], "expected": [start_line, end_line],
                    "candidate_lines": candidates.iter().map(|unit| [unit.source.start.line, unit.source.end.line]).collect::<Vec<_>>() }));
            }
        }
    }
    let calls = snapshot.facts.iter().filter(|fact| fact.kind == "calls");
    let call_count = calls.clone().count();
    let call_statuses: BTreeMap<_, _> = [
        "observed",
        "name_candidate",
        "ambiguous",
        "type_resolved",
        "unresolved",
    ]
    .into_iter()
    .map(|status| {
        (
            status,
            snapshot
                .facts
                .iter()
                .filter(|fact| fact.kind == "calls" && fact.status == status)
                .count(),
        )
    })
    .collect();
    let statuses: BTreeMap<_, _> = ["complete", "partial", "opaque", "failed"]
        .into_iter()
        .map(|status| {
            (
                status,
                snapshot
                    .files
                    .iter()
                    .filter(|file| file.extraction.status == status)
                    .count(),
            )
        })
        .collect();
    Ok(json!({
        "language": language,
        "frontend": snapshot.frontend_versions.get(language),
        "fixture": fixture,
        "manifest_units": manifest_units.len(),
        "provisional_unique_name_matches": unique_name_matches,
        "ambiguous": ambiguous,
        "missing": missing,
        "manifest_line_span_matches": if line_span_total > 0 { Some(line_span_matches) } else { None::<usize> },
        "manifest_line_span_total": if line_span_total > 0 { Some(line_span_total) } else { None::<usize> },
        "line_span_mismatches": line_span_mismatches,
        "exact_byte_span_kind_matches": null,
        "exact_byte_span_kind_reason": "frozen manifest does not adjudicate exact byte columns and kind; name/line matches are provisional",
        "emitted_units": snapshot.units.len(),
        "emitted_facts": snapshot.facts.len(),
        "file_extraction_statuses": statuses,
        "call_sites": call_count,
        "call_statuses": call_statuses,
        "manually_labeled_call_sites": null,
        "rust_grouped_module_units": if language == "rust" { Some(rust_modules) } else { None::<Vec<Value>> }
    }))
}
