import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { extractSnapshot } from "../../dist/engine/code-ir/index.js";
import { replayMetadataInventory } from "../../scripts/replay_ir_metadata_inventory.mjs";

const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

function control() {
  const path = "example.ts";
  const source = Buffer.from(
    "/** docs */\nexport function run() {}\n/** unrelated */\n\nfunction skip() {}\n",
  );
  const manifest = {
    schema_version: 1,
    fixture_id: "authored-control",
    language: "typescript",
    source: { repository: "synthetic/authored-control" },
    units: [
      { path, symbol: "run" },
      { path, symbol: "skip" },
    ],
  };
  const manifestBytes = Buffer.from(JSON.stringify(manifest));
  const inventory = {
    schema_version: 1,
    fixture_id: manifest.fixture_id,
    language: manifest.language,
    manifest_sha256: hash(manifestBytes),
    source_set_sha256: hash(Buffer.from(JSON.stringify([[path, hash(source)]]))),
    cases: [
      {
        path,
        file_sha256: hash(source),
        name: "run",
        kind: "function",
        signature: "export function run()",
        documentation: "/** docs */",
      },
      {
        path,
        file_sha256: hash(source),
        name: "skip",
        kind: "function",
        signature: "function skip()",
        documentation: null,
      },
    ],
  };
  const readSource = async (relativePath) => {
    assert.equal(relativePath, path);
    return source;
  };
  return { manifestBytes, inventory, readSource };
}

test("authored exact metadata sites replay without deriving labels from the IR", async () => {
  const result = await replayMetadataInventory({
    ...control(),
    extractSnapshot,
  });
  assert.deepEqual(result, {
    language: "typescript",
    selected_cases: 2,
    signature_matches: 2,
    documentation_matches: 1,
    documented_absences: 1,
    unsupported_documentation: 0,
  });
});

test("inventory refuses stale manifest, source, incorrect or duplicated author labels", async () => {
  const base = control();
  await assert.rejects(
    replayMetadataInventory({
      ...base,
      manifestBytes: Buffer.from("{}"),
      extractSnapshot,
    }),
    /manifest/i,
  );
  await assert.rejects(
    replayMetadataInventory({
      ...base,
      readSource: async () => Buffer.from("changed"),
      extractSnapshot,
    }),
    /source set|source hash/i,
  );
  const incorrect = structuredClone(base.inventory);
  incorrect.cases[0].signature = "function invented()";
  await assert.rejects(
    replayMetadataInventory({ ...base, inventory: incorrect, extractSnapshot }),
    /authored.*signature/i,
  );
  const duplicate = structuredClone(base.inventory);
  duplicate.cases.push(structuredClone(duplicate.cases[0]));
  await assert.rejects(
    replayMetadataInventory({ ...base, inventory: duplicate, extractSnapshot }),
    /duplicate/i,
  );
});

test("an unsupported, source-authored doc site remains a reported gap, not a fabricated pass", async () => {
  const base = control();
  const inventory = structuredClone(base.inventory);
  inventory.cases[1].unsupported_documentation = "/** unrelated */";
  const result = await replayMetadataInventory({
    ...base,
    inventory,
    extractSnapshot,
  });
  assert.equal(result.unsupported_documentation, 1);
  assert.equal(result.documented_absences, 0);
});
