// Recheck current syntax IR against an independent, frozen producer artifact.
// Reads sibling spike + staged fixture bytes only; never modifies either.
// node scripts/code-ir-shadow-recheck.mjs --root ROOT --mode lsp|scip-go [--language go]
import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  extractSnapshot,
  validateSnapshot,
} from "../dist/engine/code-ir/index.js";

function option(flag) {
  const index = process.argv.indexOf(flag);
  return index < 0 ? undefined : process.argv[index + 1];
}
const root = option("--root");
const mode = option("--mode");
const spike = resolve(option("--spike") ?? "../zvec-grep-scip-spike");
const fixtures = resolve(
  option("--fixtures") ?? "../engram/benchmarks/semantic_search/fixtures",
);
if (!root || !["lsp", "scip-go"].includes(mode))
  throw new Error(
    "Usage: --root STAGED_ROOT --mode lsp|scip-go [--language go|python|rust|typescript]",
  );
const languages = option("--language")
  ? [option("--language")]
  : mode === "scip-go"
    ? ["go"]
    : ["go", "python", "rust", "typescript"];
for (const language of languages) {
  const manifest = JSON.parse(
    await readFile(
      resolve(fixtures, `${language}-workflow-discovery-v1/manifest.json`),
      "utf8",
    ),
  );
  const paths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
  const files = await Promise.all(
    paths.map(async (relative_path) => ({
      root_id: manifest.fixture_id,
      relative_path,
      language,
      bytes: await readFile(resolve(root, language, relative_path)),
    })),
  );
  const { snapshot, sources } = await extractSnapshot(
    manifest.source.repository,
    files,
  );
  validateSnapshot(snapshot, sources);
  const record = {
    snapshot,
    provenance: { staged_root: resolve(root, language) },
  };
  const process = spawnSync(
    "python3",
    [
      "-B",
      "scripts/code-ir-shadow-recheck.py",
      mode,
      resolve(root),
      language,
      spike,
      ...(globalThis.process.argv.includes("--aliases") ? ["aliases"] : []),
      ...(option("--expect-scip-sha256")
        ? [`sha256:${option("--expect-scip-sha256")}`]
        : []),
    ],
    {
      input: JSON.stringify(record),
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
      env: { ...globalThis.process.env, PYTHONDONTWRITEBYTECODE: "1" },
    },
  );
  if (process.status !== 0)
    throw new Error(
      `Shadow join rejected ${language}: ${process.stderr || process.stdout}`,
    );
  const result = JSON.parse(process.stdout);
  validateSnapshot(result.snapshot, sources);
  if (result.snapshot.snapshot_id !== snapshot.snapshot_id)
    throw new Error("Shadow join changed the syntax snapshot identity");
  if (result.alternate) {
    validateSnapshot(result.alternate.snapshot, sources);
    if (result.alternate.snapshot.snapshot_id !== snapshot.snapshot_id)
      throw new Error(
        "Source-verified alternate joins changed snapshot identity",
      );
  }
  console.log(
    JSON.stringify({
      language,
      mode,
      frontend: snapshot.frontend_versions[language],
      files: files.length,
      units: snapshot.units.length,
      syntax_facts: snapshot.facts.length,
      ...result.counts,
      producer_counts: result.producer_counts,
      producer_artifact_sha256: result.producer_artifact_sha256,
      validated_shadow_facts:
        result.snapshot.facts.length - snapshot.facts.length,
      authored_matches: result.authored_matches,
      ...(result.alternate
        ? {
            source_verified_alternate_joins: {
              matched_aliases: result.alternate.source_verified_aliases.length,
              ...result.alternate.counts,
              authored_matches: result.alternate.authored_matches,
            },
          }
        : {}),
      ...(globalThis.process.argv.includes("--details")
        ? { unmatched_focus: result.unmatched_focus }
        : {}),
    }),
  );
}
