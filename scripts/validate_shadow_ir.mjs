// Validate the spike's enriched shadow snapshots with the actual Code IR v1
// runtime validator from the sibling worktree, without writing to that worktree.
// node scripts/validate_shadow_ir.mjs ROOT [IR_MODULE] [languages...]
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(process.argv[2]);
const artifactPrefix = process.env.IR_ARTIFACT_PREFIX ?? "shadow-ir";
const moduleFile = resolve(process.argv[3] ??
  "/Users/jgil/go/src/github.com/jordigilh/zvec-grep-code-ir-design/dist/engine/code-ir/index.js");
const { validateSnapshot } = await import(pathToFileURL(moduleFile).href);
const moduleSha = createHash("sha256").update(await readFile(moduleFile)).digest("hex");
const implementationFile = resolve(dirname(moduleFile), "../extraction/code/ir.js");
const implementationSha = createHash("sha256").update(await readFile(implementationFile)).digest("hex");
for (const language of process.argv.slice(4).length ? process.argv.slice(4) : ["typescript", "python", "rust", "go"]) {
  const artifact = JSON.parse(await readFile(resolve(root, `${artifactPrefix}-${language}.json`), "utf8"));
  const syntax = JSON.parse(await readFile(resolve(root, `syntax-ir-${language}.json`), "utf8"));
  if (syntax.provenance.compiled_ir_module_sha256 !== moduleSha)
    throw new Error("syntax frontend changed since snapshot extraction");
  if (syntax.provenance.compiled_ir_implementation_sha256 &&
      syntax.provenance.compiled_ir_implementation_sha256 !== implementationSha)
    throw new Error("compiled IR extraction implementation changed since snapshot extraction");
  const sources = new Map();
  for (const file of artifact.snapshot.files)
    sources.set(file.file_id, await readFile(resolve(root, language, file.relative_path)));
  validateSnapshot(artifact.snapshot, sources);
  if (artifact.snapshot.snapshot_id !== syntax.snapshot.snapshot_id)
    throw new Error("shadow join changed canonical syntax snapshot identity");
  const expectedFrontend = artifactPrefix === "shadow-ir" ? "lsp-shadow-v1" : "scip-shadow-v1";
  const newFacts = artifact.snapshot.facts.filter(f => f.provenance.frontend === expectedFrontend);
  console.log(JSON.stringify({language, schema_and_runtime_valid: true,
    snapshot_id: artifact.snapshot.snapshot_id, module_sha256: moduleSha,
    implementation_sha256: implementationSha,
    syntax_units: artifact.snapshot.units.length, new_references: newFacts.length,
    all_facts: artifact.snapshot.facts.length}));
}
