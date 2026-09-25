// Read-only use of the sibling design worktree's actual compiled Code IR v1.
// Writes only to the isolated staged ROOT: node scripts/export_syntax_ir.mjs ROOT
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
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
const implementationFile = resolve(dirname(moduleFile), "../extraction/code/ir.js");
const { extractSnapshot, validateSnapshot, FRONTEND_VERSION } = await import(pathToFileURL(moduleFile).href);
const moduleSha256 = createHash("sha256").update(await readFile(moduleFile)).digest("hex");
const implSha256 = createHash("sha256").update(await readFile(implementationFile)).digest("hex");
for (const language of process.argv.slice(5).length
  ? process.argv.slice(5)
  : ["typescript", "python", "rust", "go"]) {
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
  if (snapshot.frontend_versions[language] !== FRONTEND_VERSION ||
      createHash("sha256").update(await readFile(implementationFile)).digest("hex") !== implSha256)
    throw new Error("compiled IR implementation changed during extraction");
  const artifact = resolve(root, `syntax-ir-${language}.json`);
  const data = JSON.stringify({ snapshot, provenance: {
    staged_root: resolve(root, language),
    compiled_ir_module: moduleFile,
    compiled_ir_module_sha256: moduleSha256,
    compiled_ir_implementation: implementationFile,
    compiled_ir_implementation_sha256: implSha256,
  } }) + "\n";
  await writeFile(artifact, data, { flag: "wx" });
  console.log(JSON.stringify({ language, snapshot_id: snapshot.snapshot_id,
    frontend: snapshot.frontend_versions[language], files: snapshot.files.length,
    units: snapshot.units.length, facts: snapshot.facts.length,
    artifact, artifact_sha256: createHash("sha256").update(data).digest("hex"),
    module_sha256: moduleSha256, implementation_sha256: implSha256 }));
}
