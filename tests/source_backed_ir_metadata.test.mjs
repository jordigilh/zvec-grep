import assert from "node:assert/strict";
import { test } from "node:test";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { augmentSourceProjection, collectSourceBackedMetadata } from "../scripts/source_backed_ir_metadata.mjs";

const dist = resolve(dirname(fileURLToPath(import.meta.url)), "../../zvec-grep-code-ir-design/dist/engine");
const [{ extractSnapshot }, { CodeExtractor }] = await Promise.all([
  import(pathToFileURL(resolve(dist, "code-ir/index.js"))),
  import(pathToFileURL(resolve(dist, "extraction/code/extractor.js"))),
]);

async function irFor(text) {
  const files = [{
    root_id: "synthetic",
    relative_path: "selection/policy.go",
    language: "go",
    bytes: Buffer.from(text),
  }];
  const { snapshot } = await extractSnapshot("synthetic/policy", files);
  return { snapshot, files };
}

function recordFor(unit, files) {
  const text = Buffer.from(files[0].bytes).subarray(unit.source.start_byte, unit.source.end_byte).toString("utf8");
  return {
    unit_id: unit.id,
    unit_source: unit.source,
    source: unit.source,
    lexical_text: `symbol: function ${unit.name}\n${text}`,
    vector_input: `symbol: function ${unit.name}\n${text}`,
  };
}

test("Go declaration docs and signature are bound to verified original UTF-8/CRLF source", async () => {
  const source = [
    "package selection",
    "// 🚀 IsAllowed requires discovery membership.",
    "// Fail closed when membership is absent.",
    "func IsAllowed(workflowID string) bool { return workflowID != \"\" }",
    "",
  ].join("\r\n");
  const { snapshot, files } = await irFor(source);
  const unit = snapshot.units.find((candidate) => candidate.name === "IsAllowed");
  assert.ok(unit);
  const { byUnit, stats } = await collectSourceBackedMetadata(snapshot, files, CodeExtractor);
  const evidence = byUnit.get(unit.id);
  assert.match(evidence.signature, /^func IsAllowed\(/u);
  assert.match(evidence.doc, /discovery membership/u);
  assert.equal(stats.signature_units, 1);
  assert.equal(stats.doc_units, 1);
  assert.equal(evidence.doc_source.sha256, snapshot.files[0].sha256);
  assert.match(files[0].bytes.subarray(evidence.doc_source.start_byte, evidence.doc_source.end_byte).toString("utf8"), /🚀 IsAllowed/u);

  const raw = recordFor(unit, files);
  const enriched = augmentSourceProjection(raw, unit, files[0].bytes, evidence, { signature: true, doc: true });
  assert.match(enriched.lexical_text, /signature: func IsAllowed/u);
  assert.match(enriched.vector_input, /doc: 🚀 IsAllowed/u);
  assert.ok(enriched.lexical_text.endsWith(files[0].bytes.subarray(unit.source.start_byte, unit.source.end_byte).toString("utf8")));
  assert.equal(raw.lexical_text.includes("doc:"), false);
  const lexical = augmentSourceProjection(raw, unit, files[0].bytes, evidence, { signature: true, doc: true, route: "fts" });
  const vector = augmentSourceProjection(raw, unit, files[0].bytes, evidence, { signature: true, doc: true, route: "vector" });
  assert.equal(lexical.vector_input, raw.vector_input);
  assert.equal(vector.lexical_text, raw.lexical_text);
  assert.notEqual(lexical.lexical_text, vector.lexical_text);
  assert.throws(() => augmentSourceProjection(raw, unit, files[0].bytes, evidence, { signature: true, route: "unsupported" }), /unsupported source metadata route/u);
});

test("blank line prevents borrowing a previous comment as declaration documentation", async () => {
  const { snapshot, files } = await irFor("package selection\n// Old comment for another declaration.\n\nfunc IsAllowed() bool { return true }\n");
  const unit = snapshot.units.find((candidate) => candidate.name === "IsAllowed");
  const { byUnit } = await collectSourceBackedMetadata(snapshot, files, CodeExtractor);
  assert.ok(byUnit.get(unit.id)?.signature);
  assert.equal(byUnit.get(unit.id)?.doc, undefined);
});

test("same-name methods on different Go receivers keep distinct source-backed metadata", async () => {
  const { snapshot, files } = await irFor([
    "package selection",
    "type Catalog struct{}",
    "type State struct{}",
    "func (c *Catalog) Contains(id string) bool { return id != \"\" }",
    "func (s *State) Contains(id string) bool { return id == \"\" }",
    "",
  ].join("\n"));
  const methods = snapshot.units.filter((unit) => unit.name === "Contains");
  assert.equal(methods.length, 2);
  const { byUnit, stats } = await collectSourceBackedMetadata(snapshot, files, CodeExtractor);
  assert.equal(stats.ambiguous_units, 0);
  for (const unit of methods) {
    const receiver = unit.qualified_name.split("::")[0];
    assert.match(byUnit.get(unit.id).signature, new RegExp(`^func \\([cs] \\*${receiver}\\) Contains`, "u"));
    assert.equal(byUnit.get(unit.id).signature_source.sha256, snapshot.files[0].sha256);
  }
  assert.notEqual(byUnit.get(methods[0].id).signature, byUnit.get(methods[1].id).signature);
});

test("changed source bytes or projection source ref cannot enter a metadata arm", async () => {
  const { snapshot, files } = await irFor("package selection\nfunc IsAllowed() bool { return true }\n");
  const unit = snapshot.units.find((candidate) => candidate.name === "IsAllowed");
  const { byUnit } = await collectSourceBackedMetadata(snapshot, files, CodeExtractor);
  await assert.rejects(
    collectSourceBackedMetadata(snapshot, [{ ...files[0], bytes: Buffer.from(files[0].bytes.toString().replace("true", "false")) }], CodeExtractor),
    /stale\/missing source/u,
  );
  assert.throws(() => augmentSourceProjection({ ...recordFor(unit, files), lexical_text: "forged" }, unit, files[0].bytes, byUnit.get(unit.id), { signature: true }), /source window changed/u);
});
