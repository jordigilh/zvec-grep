// Standalone opt-in IR sidecar: does not change the existing zg index or search.
// node scripts/code-ir-snapshot.mjs --mode shadow|projection --manifest PATH --output DIRECTORY
//   [--projection-policy source-metadata-entity-v1]
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { publishIR, readPublishedIR } from "../dist/engine/code-ir/sidecar.js";

function option(name) {
  const i = process.argv.indexOf(name);
  return i < 0 ? undefined : process.argv[i + 1];
}
const mode = option("--mode");
const policy = option("--projection-policy");
const manifestPath = option("--manifest");
const directory = option("--output");
if (
  process.argv.includes("--projection-policy") &&
  (mode !== "projection" || policy !== "source-metadata-entity-v1")
)
  throw new Error(
    "--projection-policy source-metadata-entity-v1 requires projection mode",
  );
if (mode === "off") {
  console.log(JSON.stringify({ mode: "off", sidecar_written: false }));
} else {
  if (!["shadow", "projection"].includes(mode) || !manifestPath || !directory)
    throw new Error(
      "Usage: node scripts/code-ir-snapshot.mjs --mode off|shadow|projection [--manifest PATH --output DIRECTORY] [--projection-policy source-metadata-entity-v1]",
    );
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const root = resolve(manifestPath, "..");
  const paths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
  const files = await Promise.all(
    paths.map(async (relative_path) => ({
      root_id: manifest.fixture_id,
      relative_path,
      language: manifest.language,
      bytes: await readFile(resolve(root, relative_path)),
    })),
  );
  const active = await publishIR(
    directory,
    manifest.source.repository,
    files,
    mode,
    null,
    policy ?? "all-units-v1",
  );
  const pinned = await readPublishedIR(directory);
  if (active.ir_snapshot_id !== pinned.snapshot.snapshot_id)
    throw new Error("publication failed");
  console.log(
    JSON.stringify(
      {
        ...active,
        files: pinned.snapshot.files.length,
        units: pinned.snapshot.units.length,
        facts: pinned.snapshot.facts.length,
        projected_records: pinned.projection?.records.length ?? 0,
      },
      null,
      2,
    ),
  );
}
