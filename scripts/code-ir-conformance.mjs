// Read-only comparison against the independently frozen Engram manifests.
// Run after npm run build: node scripts/code-ir-conformance.mjs [fixture-root].
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  extractSnapshot,
  validateSnapshot,
} from "../dist/engine/code-ir/index.js";

const root = resolve(
  process.argv[2] ?? "../engram/benchmarks/semantic_search/fixtures",
);
for (const [language, directory] of [
  ["go", "go-workflow-discovery-v1"],
  ["python", "python-workflow-discovery-v1"],
  ["rust", "rust-workflow-discovery-v1"],
  ["typescript", "typescript-workflow-discovery-v1"],
]) {
  const manifest = JSON.parse(
    await readFile(resolve(root, directory, "manifest.json"), "utf8"),
  );
  const paths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
  const files = await Promise.all(
    paths.map(async (relative_path) => ({
      root_id: directory,
      relative_path,
      language,
      bytes: await readFile(resolve(root, directory, relative_path)),
    })),
  );
  const { snapshot, sources } = await extractSnapshot(
    manifest.source.repository,
    files,
  );
  validateSnapshot(snapshot, sources);
  const matched = [],
    missing = [],
    ambiguous = [],
    lineSpanMatches = [],
    lineSpanMismatches = [];
  for (const manifestUnit of manifest.units) {
    const file = snapshot.files.find(
      (f) => f.relative_path === manifestUnit.path,
    );
    // A symbol/name match is only a diagnostic: no source span or kind is
    // available in the Go manifest; never count it as exact-span coverage.
    const name = manifestUnit.symbol.split(".").at(-1);
    const moduleCandidates = snapshot.units.filter(
      (unit) => unit.source.file_id === file.file_id && unit.kind === "module",
    );
    const candidates = snapshot.units.filter(
      (unit) =>
        unit.source.file_id === file.file_id &&
        (manifestUnit.symbol === "module-declarations"
          ? moduleCandidates.length > 0
            ? moduleCandidates.some((module) => module.id === unit.id)
            : unit.kind === "file"
          : unit.name === name),
    );
    const record = {
      unit_id: manifestUnit.unit_id,
      path: manifestUnit.path,
      symbol: manifestUnit.symbol,
      candidates: candidates.map((unit) => ({
        id: unit.id,
        kind: unit.kind,
        start_byte: unit.source.start_byte,
        end_byte: unit.source.end_byte,
      })),
    };
    if (manifestUnit.start_line && manifestUnit.end_line) {
      const exactLineSpan = candidates.filter(
        (unit) =>
          unit.source.start.line === manifestUnit.start_line &&
          unit.source.end.line === manifestUnit.end_line,
      );
      (exactLineSpan.length > 0 ? lineSpanMatches : lineSpanMismatches).push({
        unit_id: manifestUnit.unit_id,
        expected: [manifestUnit.start_line, manifestUnit.end_line],
        candidates: candidates.map((unit) => [
          unit.source.start.line,
          unit.source.end.line,
          unit.kind,
        ]),
      });
    }
    if (!candidates.length) missing.push(record);
    else if (candidates.length > 1) ambiguous.push(record);
    else matched.push(record);
  }
  const statuses = Object.fromEntries(
    [
      "observed",
      "name_candidate",
      "ambiguous",
      "type_resolved",
      "unresolved",
    ].map((status) => [
      status,
      snapshot.facts.filter((f) => f.kind === "calls" && f.status === status)
        .length,
    ]),
  );
  const result = {
    language,
    frontend: snapshot.frontend_versions[language],
    fixture: directory,
    manifest_units: manifest.units.length,
    exact_span_kind_matches: null,
    exact_span_kind_reason:
      "independent byte-span/kind inventory not adjudicated; provisional symbol matches are not exact coverage",
    provisional_unique_name_matches: matched.length,
    manifest_line_span_matches: manifest.units.some((unit) => unit.start_line)
      ? lineSpanMatches.length
      : null,
    manifest_line_span_total: manifest.units.some((unit) => unit.start_line)
      ? lineSpanMatches.length + lineSpanMismatches.length
      : null,
    manifest_line_span_mismatches: lineSpanMismatches,
    ambiguous_name_matches: ambiguous,
    missing,
    emitted_unit_round_trips: snapshot.units.length,
    emitted_fact_round_trips: snapshot.facts.length,
    extraction_statuses: Object.fromEntries(
      ["complete", "partial", "opaque", "failed"].map((status) => [
        status,
        snapshot.files.filter((f) => f.extraction.status === status).length,
      ]),
    ),
    call_statuses: statuses,
    manually_labeled_call_sites: null,
    rust_grade_zero_module_units:
      language === "rust"
        ? manifest.units
            .filter((unit) => unit.symbol === "module-declarations")
            .map((unit) => {
              const file = snapshot.files.find(
                (candidate) => candidate.relative_path === unit.path,
              );
              const modules = snapshot.units.filter(
                (candidate) =>
                  candidate.source.file_id === file?.file_id &&
                  candidate.kind === "module",
              );
              return {
                unit_id: unit.unit_id,
                module_units: modules.length,
                representation:
                  modules.length > 0
                    ? `individual source-backed module units (${modules.length}); grouped benchmark inventory needs a many-to-one mapping`
                    : "file source unit; module declaration unit missing",
              };
            })
        : undefined,
  };
  console.log(JSON.stringify(result, null, 2));
}
