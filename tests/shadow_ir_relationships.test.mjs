import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rename, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { openShadowRelationships, sourceSetDigest } from "../scripts/shadow_ir_relationships.mjs";

const modulePath = resolve(dirname(fileURLToPath(import.meta.url)),
  "../../zvec-grep-code-ir-design/dist/engine/code-ir/index.js");
const implementationFile = resolve(dirname(modulePath), "../extraction/code/ir.js");
const { extractSnapshot, sourceRef, validateSnapshot } = await import(pathToFileURL(modulePath).href);
const hash = (data) => createHash("sha256").update(data).digest("hex");

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "shadow-ir-relationships-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const target = Buffer.from("package p\nfunc Target() {}\n");
  const caller = Buffer.from("package p\nfunc Caller() { Target() }\n");
  const files = [["target.go", target], ["caller.go", caller]];
  for (const [path, content] of files) await writeFile(join(root, path), content);
  const config = Buffer.from("module example.com/synthetic\n\ngo 1.26.0\n");
  await writeFile(join(root, "go.mod"), config);
  const manifestPath = join(root, "manifest.json");
  await writeFile(manifestPath, JSON.stringify({ fixture_id: "synthetic", language: "go",
    units: files.map(([path]) => ({ path })) }));
  const { snapshot, sources } = await extractSnapshot("synthetic/go", files.map(([relative_path, bytes]) =>
    ({ root_id: "synthetic", language: "go", relative_path, bytes })));
  const targetUnit = snapshot.units.find((unit) => unit.name === "Target"),
    callerUnit = snapshot.units.find((unit) => unit.name === "Caller");
  assert.ok(targetUnit && callerUnit);
  const callerFile = snapshot.files.find((file) => file.relative_path === "caller.go");
  const offset = caller.lastIndexOf(Buffer.from("Target()"));
  const fact = { id: hash("relation-1"), kind: "references", subject_id: callerUnit.id,
    object_id: targetUnit.id, site: sourceRef(callerFile, caller, offset, offset + 6),
    status: "type_resolved", target_spelling: "Target",
    provenance: { frontend: "scip-shadow-v1", resolver: "go", resolver_version: "scip-go-spike",
      method: "strict source-mapped join" } };
  snapshot.facts.push(fact);
  validateSnapshot(snapshot, sources);
  const indexPath = join(root, "index.scip");
  await writeFile(indexPath, "pinned fake producer artifact");
  const shadowPath = join(root, "shadow.json");
  await writeFile(shadowPath, JSON.stringify({ snapshot, producer_artifact_sha256: hash(await readFile(indexPath)) }));
  const opts = {
    root, manifestPath, shadowPath, indexPath, configRelativePath: "go.mod",
    expectedShadowSha256: hash(await readFile(shadowPath)),
    expectedSourceSetSha256: sourceSetDigest(files), expectedConfigSha256: hash(config),
    modulePath, expectedModuleSha256: hash(await readFile(modulePath)),
    expectedImplementationSha256: hash(await readFile(implementationFile)),
  };
  return { root, opts, files, targetUnit, callerUnit, fact, offset, snapshot, shadowPath, indexPath };
}

test("selected unit ID resolves pinned incoming/outgoing reference evidence, not calls", async (t) => {
  const { opts, targetUnit, callerUnit, offset } = await fixture(t);
  const view = await openShadowRelationships(opts);
  const result = view.queryUnit({ unitId: targetUnit.id });
  assert.equal(result.status, "validated_shadow_candidate");
  assert.equal(result.incoming_total, 1);
  assert.equal(result.outgoing_total, 0);
  assert.equal(result.incoming[0].kind, "references");
  assert.equal(result.incoming[0].site.source, "Target");
  assert.equal(result.incoming[0].site.start_byte, offset);
  assert.equal(result.incoming[0].subject.id, callerUnit.id);
  assert.equal(view.queryUnit({ path: "caller.go", symbol: "Caller" }).outgoing_total, 1);
  assert.equal(view.querySite({ path: "caller.go", startByte: offset }).binding.target.id, targetUnit.id);
  assert.equal(view.querySite({ path: "caller.go", startByte: 0 }).binding, null);
  assert.throws(() => view.querySite({ path: "caller.go", startByte: offset, endByte: offset + 5.5 }), /invalid reference site end/u);
  assert.throws(() => view.queryUnit({ unitId: targetUnit.id, limit: 501 }), /limit must be 1\.\.500/u);
  assert.throws(() => view.queryUnit({ unitId: "missing" }), /unknown Code IR unit/u);
});

test("an unchanged caller cannot use a stale target, config, index or shadow artifact", async (t) => {
  const { opts, root, files, indexPath, shadowPath } = await fixture(t);
  const callerBefore = hash(files.find(([path]) => path === "caller.go")[1]);
  await writeFile(join(root, "target.go"), "package p\nfunc RenamedTarget() {}\n");
  await assert.rejects(openShadowRelationships(opts), /stale selected source snapshot/u);
  assert.equal(hash(await readFile(join(root, "caller.go"))), callerBefore);
  await writeFile(join(root, "target.go"), files.find(([path]) => path === "target.go")[1]);
  await writeFile(join(root, "go.mod"), "module another.example\n");
  await assert.rejects(openShadowRelationships(opts), /stale producer configuration/u);
  await writeFile(join(root, "go.mod"), "module example.com/synthetic\n\ngo 1.26.0\n");
  await writeFile(indexPath, "another index");
  await assert.rejects(openShadowRelationships(opts), /SCIP index differs/u);
  await writeFile(indexPath, "pinned fake producer artifact");
  const binary = join(root, "producer");
  await writeFile(binary, "custom producer");
  await assert.rejects(openShadowRelationships({ ...opts, producerBinaryPath: binary,
    expectedBinarySha256: hash("different producer") }), /producer binary digest mismatch/u);
  await writeFile(shadowPath, `${await readFile(shadowPath, "utf8")} `);
  await assert.rejects(openShadowRelationships(opts), /shadow artifact digest mismatch/u);
  await assert.rejects(openShadowRelationships({ ...opts, expectedShadowSha256: hash(await readFile(shadowPath)),
    expectedModuleSha256: hash("wrong barrel") }), /Code IR module changed/u);
  await rename(join(root, "target.go"), join(root, "renamed.go"));
  await assert.rejects(openShadowRelationships({ ...opts, expectedShadowSha256: hash(await readFile(shadowPath)) }), /ENOENT/u);
  await symlink(join(root, "renamed.go"), join(root, "target.go"));
  await assert.rejects(openShadowRelationships({ ...opts, expectedShadowSha256: hash(await readFile(shadowPath)) }), /selected source escaped attested root/u);
});

test("multiple local target bindings at a site abstain instead of choosing a name match", async (t) => {
  const { opts, snapshot, shadowPath, callerUnit, fact, offset } = await fixture(t);
  snapshot.facts.push({ ...fact, id: hash("relation-2"), object_id: callerUnit.id });
  await writeFile(shadowPath, JSON.stringify({ snapshot, producer_artifact_sha256: hash(await readFile(opts.indexPath)) }));
  const view = await openShadowRelationships({ ...opts, expectedShadowSha256: hash(await readFile(shadowPath)) });
  assert.throws(() => view.querySite({ path: "caller.go", startByte: offset }), /ambiguous relationship/u);
  assert.throws(() => view.queryUnit({ path: "../other.go", symbol: "Target" }), /unsafe selected source path/u);
});
