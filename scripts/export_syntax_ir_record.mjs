// Emit fresh, validated Code IR records from a staged fixture root to stdout.
// This avoids overwriting the versioned snapshots stored beside SCIP artifacts.
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(process.argv[2]);
const moduleFile = resolve(
  process.argv[3] ??
    "/Users/jgil/go/src/github.com/jordigilh/zvec-grep-code-ir-design/dist/engine/code-ir/index.js",
);
const fixtures = resolve(
  process.argv[4] ??
    "/Users/jgil/go/src/github.com/jordigilh/engram/benchmarks/semantic_search/fixtures",
);
const languages = process.argv.slice(5).length
  ? process.argv.slice(5)
  : ["go", "python", "rust", "typescript"];
const implementation = resolve(dirname(moduleFile), "../extraction/code/ir.js");
const { extractSnapshot, validateSnapshot } = await import(pathToFileURL(moduleFile).href);
const moduleHash = createHash("sha256").update(await readFile(moduleFile)).digest("hex");
const implementationHash = createHash("sha256")
  .update(await readFile(implementation))
  .digest("hex");
const snapshots = {};
for (const language of languages) {
  const manifest = JSON.parse(
    await readFile(resolve(fixtures, `${language}-workflow-discovery-v1`, "manifest.json"), "utf8"),
  );
  const paths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
  const files = await Promise.all(paths.map(async (relative_path) => ({
    root_id: manifest.fixture_id,
    relative_path,
    language,
    bytes: await readFile(resolve(root, language, relative_path)),
  })));
  const { snapshot, sources } = await extractSnapshot(manifest.source.repository, files);
  validateSnapshot(snapshot, sources);
  if (snapshot.frontend_versions[language] !== "web-tree-sitter-ir-v1.5")
    throw new Error(`${language} frontend is not v1.5`);
  snapshots[language] = {
    snapshot,
    provenance: {
      staged_root: resolve(root, language),
      compiled_ir_module: moduleFile,
      compiled_ir_module_sha256: moduleHash,
      compiled_ir_implementation: implementation,
      compiled_ir_implementation_sha256: implementationHash,
    },
  };
}
process.stdout.write(JSON.stringify({ schema: "code-ir-records-v1", snapshots }));
