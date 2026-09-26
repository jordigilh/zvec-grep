#!/usr/bin/env node
// Read-only analysis of already-scored frozen runs, indexed syntax entities,
// and the published/read-validated v2 IR. Never adjusts an index or qrels.
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { isAbsolute, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  compareRankedQueries,
  inventoryBaselineVsIR,
  summarizeLexicalDifferences,
  validateDiagnosticInputs,
} from "./code_ir_v1_gap_analysis.mjs";

function option(name) {
  const index = process.argv.indexOf(name);
  const value = index < 0 ? undefined : process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`missing ${name}`);
  return resolve(value);
}

const runtimeRoot = resolve(import.meta.dirname, "..");
const fixture = option("--fixture");
const resultsDir = option("--results-dir");
const baselineIndex = option("--baseline-index");
const sidecarRoot = option("--sidecar-root");
const outputDir = option("--output-dir");
const fixtureCheckout = resolve(fixture, "../../../..");
function inside(path, root) {
  const child = relative(root, path);
  return (
    child === "" ||
    (child !== ".." &&
      !child.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) &&
      !isAbsolute(child))
  );
}
if (
  existsSync(outputDir) ||
  [
    runtimeRoot,
    fixtureCheckout,
    fixture,
    baselineIndex,
    sidecarRoot,
    resultsDir,
  ].some((root) => inside(outputDir, root))
)
  throw new Error(
    "diagnostic output must be new and outside all source and input directories",
  );

const [{ createWorkspaceIndexStorage }, { publishIR, readPublishedIR }] =
  await Promise.all([
    import(pathToFileURL(join(runtimeRoot, "dist/engine/storage/index.js"))),
    import(pathToFileURL(join(runtimeRoot, "dist/engine/code-ir/sidecar.js"))),
  ]);
const hash = (data) => createHash("sha256").update(data).digest("hex");
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const [fixtureManifest, qrels, raw, normalized, runManifest] =
  await Promise.all([
    readJson(join(fixture, "manifest.json")),
    readJson(join(fixture, "qrels.json")),
    readJson(join(resultsDir, "raw-runs.json")),
    readJson(join(resultsDir, "normalized-runs.json")),
    readJson(join(resultsDir, "run-manifest.json")),
  ]);
const language = fixtureManifest.language;
if (
  !["go", "python", "rust", "typescript"].includes(language) ||
  fixtureManifest.fixture_id !== `${language}-workflow-discovery-v1` ||
  qrels.fixture_id !== fixtureManifest.fixture_id
)
  throw new Error("not a supported frozen fixture");
const paths = [
  ...new Set(fixtureManifest.units.map((unit) => unit.path)),
].sort();
const sources = new Map();
const digest = createHash("sha256");
for (const path of paths) {
  const bytes = await readFile(join(fixture, path));
  sources.set(path, bytes);
  digest.update(path).update("\0").update(bytes).update("\0");
}
const sourceDigest = digest.digest("hex");
if (
  sourceDigest !== qrels.source.snapshot_sha256 ||
  paths.length !== qrels.source.files
)
  throw new Error("frozen fixture source differs from qrels");
const {
  snapshot,
  projection,
  manifest: sidecarManifest,
} = await readPublishedIR(sidecarRoot);
if (
  sidecarManifest.ir_snapshot_id !== snapshot.snapshot_id ||
  sidecarManifest.projection_version !== 2 ||
  projection.policy !== "source-metadata-entity-v1" ||
  sidecarManifest.projection_model_identity !== raw.provenance.model_identity ||
  snapshot.files.length !== paths.length ||
  projection.records.length !== raw.provenance.code_ir.projection_records
)
  throw new Error("published v2 sidecar differs from scored IR snapshot");
const rawSha256 = hash(await readFile(join(resultsDir, "raw-runs.json")));
const normalizedSha256 = hash(
  await readFile(join(resultsDir, "normalized-runs.json")),
);
validateDiagnosticInputs({
  raw,
  normalized,
  runManifest,
  fixtureId: fixtureManifest.fixture_id,
  language,
  sourceDigest,
  snapshotId: snapshot.snapshot_id,
  rawSha256,
  normalizedSha256,
  fixtureManifestSha256: hash(await readFile(join(fixture, "manifest.json"))),
  qrelsSha256: hash(await readFile(join(fixture, "qrels.json"))),
});

const storage = createWorkspaceIndexStorage({
  storagePath: baselineIndex,
  readOnly: true,
});
let stored, indexedFiles;
try {
  indexedFiles = storage.listFiles();
  stored = indexedFiles.flatMap((file) =>
    storage.listEntitiesByFile(file.id).map(({ entity }) => ({ file, entity })),
  );
} finally {
  storage.close();
}
if (
  indexedFiles.length !== paths.length ||
  indexedFiles.some((file) => !sources.has(file.relativePath))
)
  throw new Error("indexed syntax file inventory differs from frozen fixture");
const inventory = inventoryBaselineVsIR({
  stored,
  snapshot,
  projection,
  sources,
});
const transitions = compareRankedQueries({ raw, normalized, qrels, inventory });
await mkdir(outputDir);
const v1SidecarRoot = join(outputDir, "generated-v1-sidecar");
await publishIR(
  v1SidecarRoot,
  fixtureManifest.source.repository,
  paths.map((path) => ({
    root_id: fixtureManifest.fixture_id,
    relative_path: path,
    language,
    bytes: sources.get(path),
  })),
  "projection",
  raw.provenance.model_identity,
);
const validatedV1 = await readPublishedIR(v1SidecarRoot);
if (
  validatedV1.projection.version !== 1 ||
  validatedV1.projection.model_identity !== projection.model_identity ||
  JSON.stringify(validatedV1.snapshot) !== JSON.stringify(snapshot)
)
  throw new Error(
    "v1 sidecar readback differs from scored source/model snapshot",
  );
const lexicalAudit = summarizeLexicalDifferences({
  inventory,
  stored,
  v1: validatedV1.projection,
  v2: projection,
});

async function treeHash(dir) {
  const accumulator = createHash("sha256");
  async function walk(current) {
    for (const entry of (await readdir(current, { withFileTypes: true })).sort(
      (a, b) => a.name.localeCompare(b.name),
    )) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.isFile()) {
        accumulator
          .update(relative(dir, path).split("\\").join("/"))
          .update("\0");
        accumulator
          .update(Buffer.from(hash(await readFile(path)), "hex"))
          .update("\0");
      } else throw new Error(`index tree contains unexpected link: ${path}`);
    }
  }
  await walk(dir);
  return accumulator.digest("hex");
}

const inventoryOutput = {
  language,
  snapshot_id: snapshot.snapshot_id,
  ...inventory,
};
const transitionOutput = { language, ...transitions };
const provenance = {
  schema: "code-ir-v1-gap-diagnostic-v1",
  language,
  fixture_id: fixtureManifest.fixture_id,
  source_set_sha256: sourceDigest,
  source_manifest_sha256: hash(await readFile(join(fixture, "manifest.json"))),
  qrels_sha256: hash(await readFile(join(fixture, "qrels.json"))),
  raw_sha256: rawSha256,
  normalized_sha256: normalizedSha256,
  scorer_sha256: runManifest.scorer_sha256,
  mapping_sha256: runManifest.mapping_sha256,
  model: runManifest.model,
  baseline_index_tree_sha256: await treeHash(baselineIndex),
  v2_sidecar_tree_sha256: await treeHash(sidecarRoot),
  generated_v1_sidecar_tree_sha256: await treeHash(v1SidecarRoot),
  analysis_sha256: hash(
    await readFile(new URL("./code_ir_v1_gap_analysis.mjs", import.meta.url)),
  ),
  adapter_sha256: hash(
    await readFile(new URL("./diagnose_code_ir_v1_gap.mjs", import.meta.url)),
  ),
  ir_snapshot_id: snapshot.snapshot_id,
  qeval_engine_revision: runManifest.engine_revision,
};
for (const [name, payload] of [
  ["inventory.json", inventoryOutput],
  ["query-transitions.json", transitionOutput],
  ["lexical-audit.json", { language, ...lexicalAudit }],
]) {
  const serialized = JSON.stringify(payload, null, 2) + "\n";
  await writeFile(join(outputDir, name), serialized);
  provenance[`${name.replace(".json", "").replaceAll("-", "_")}_sha256`] =
    hash(serialized);
}
await writeFile(
  join(outputDir, "diagnostic-manifest.json"),
  JSON.stringify(provenance, null, 2) + "\n",
);
console.log(
  JSON.stringify(
    {
      output_dir: outputDir,
      summary: inventory.summary,
      lexical: lexicalAudit.summary,
      query_count: transitions.queries.length,
    },
    null,
    2,
  ),
);
