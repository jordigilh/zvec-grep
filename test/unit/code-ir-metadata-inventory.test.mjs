import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
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
    source_set_sha256: hash(
      Buffer.from(JSON.stringify([[path, hash(source)]])),
    ),
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

function authoredControl(language, path, text, cases) {
  const source = Buffer.from(text);
  const manifest = {
    schema_version: 1,
    fixture_id: `authored-${language}`,
    language,
    source: { repository: `synthetic/authored-${language}` },
    units: cases.map(({ name }) => ({ path, symbol: name })),
  };
  const manifestBytes = Buffer.from(JSON.stringify(manifest));
  return {
    manifestBytes,
    inventory: {
      schema_version: 1,
      fixture_id: manifest.fixture_id,
      language,
      manifest_sha256: hash(manifestBytes),
      source_set_sha256: hash(
        Buffer.from(JSON.stringify([[path, hash(source)]])),
      ),
      cases: cases.map((entry) => ({
        path,
        file_sha256: hash(source),
        ...entry,
      })),
    },
    readSource: async () => source,
    extractSnapshot,
  };
}

test("authored exact metadata sites replay without deriving labels from the IR", async () => {
  const result = await replayMetadataInventory({
    ...control(),
    extractSnapshot,
  });
  assert.deepEqual(result, {
    language: "typescript",
    selected_cases: 2,
    matched_units: 2,
    missing_units: 0,
    signature_expected: 2,
    signature_matches: 2,
    signature_absences: 0,
    unsupported_signatures: 0,
    documentation_expected: 1,
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
  const unlabelledGap = structuredClone(base.inventory);
  unlabelledGap.cases[0].signature = null;
  await assert.rejects(
    replayMetadataInventory({
      ...base,
      inventory: unlabelledGap,
      extractSnapshot,
    }),
    /missing.*signature label/i,
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

test("bodyless multiline header is independently labelled but counted as an unsupported signature", async () => {
  const result = await replayMetadataInventory(
    authoredControl(
      "typescript",
      "overload.ts",
      "function run(\n  x: number,\n): number;\n",
      [
        {
          name: "run",
          kind: "function",
          signature: null,
          unsupported_signature: "function run(\n  x: number,\n): number;",
          documentation: null,
        },
      ],
    ),
  );
  assert.equal(result.signature_expected, 1);
  assert.equal(result.signature_matches, 0);
  assert.equal(result.unsupported_signatures, 1);
  assert.equal(result.documented_absences, 1);
});

test("non-declaration values remain IR units without acquiring an invented signature", async () => {
  const result = await replayMetadataInventory(
    authoredControl(
      "go",
      "value.go",
      "package demo\nvar Result = compute()\n",
      [{ name: "Result", kind: "value", signature: null, documentation: null }],
    ),
  );
  assert.equal(result.signature_matches, 0);
  assert.equal(result.signature_absences, 1);
});

test("negative metadata still requires an attested IR unit source", async () => {
  const input = authoredControl(
    "go",
    "value.go",
    "package demo\nvar Result = compute()\n",
    [{ name: "Result", kind: "value", signature: null, documentation: null }],
  );
  await assert.rejects(
    replayMetadataInventory({
      ...input,
      extractSnapshot: async (...args) => {
        const result = await extractSnapshot(...args);
        result.snapshot.units.find(
          (unit) => unit.name === "Result",
        ).source.sha256 = "0".repeat(64);
        return result;
      },
    }),
    /stale IR unit/i,
  );
});

test("source-authored header distinguishes a Rust struct from its same-named impl unit", async () => {
  const result = await replayMetadataInventory(
    authoredControl(
      "rust",
      "gadget.rs",
      "pub struct Gadget {}\nimpl Gadget { pub fn run(&self) {} }\n",
      [
        {
          name: "Gadget",
          kind: "type",
          signature: "pub struct Gadget",
          documentation: null,
        },
      ],
    ),
  );
  assert.equal(result.signature_matches, 1);
});

test("a source-labelled site without a fine-grained IR unit remains a measured gap", async () => {
  const result = await replayMetadataInventory(
    authoredControl("rust", "field.rs", "pub struct Gadget { id: i32 }\n", [
      {
        name: "id",
        kind: "value",
        unit_anchor: "id: i32",
        signature: null,
        documentation: null,
      },
    ]),
  );
  assert.equal(result.selected_cases, 1);
  assert.equal(result.matched_units, 0);
  assert.equal(result.missing_units, 1);
  assert.equal(result.signature_absences, 0);
  assert.equal(result.documented_absences, 0);
});

test("authored multiline declarations and adjacent docs replay across four languages", async () => {
  const cases = [
    [
      "go",
      "package demo\r\n// guía 🚀\r\nfunc run(\r\n  value int,\r\n) int {\r\n  return value\r\n}\r\n",
      "func run(\r\n  value int,\r\n) int",
      "// guía 🚀",
    ],
    [
      "python",
      "# guía 🚀\n@register\ndef run(\n    value: int,\n) -> int:\n    return value\n",
      "def run(\n    value: int,\n) -> int:",
      "# guía 🚀",
    ],
    [
      "rust",
      "/// guía 🚀\n#[inline]\npub fn run(\n    value: i32,\n) -> i32 {\n    value\n}\n",
      "pub fn run(\n    value: i32,\n) -> i32",
      "/// guía 🚀",
    ],
    [
      "typescript",
      "/** guía 🚀 */\nexport function run(\n  value: number,\n): number {\n  return value;\n}\n",
      "export function run(\n  value: number,\n): number",
      "/** guía 🚀 */",
    ],
  ];
  for (const [language, text, signature, documentation] of cases) {
    const result = await replayMetadataInventory(
      authoredControl(language, `multi.${language}`, text, [
        { name: "run", kind: "function", signature, documentation },
      ]),
    );
    assert.equal(result.signature_matches, 1, language);
    assert.equal(result.documentation_matches, 1, language);
  }
});

const fixtureRoot = resolve(
  process.env.CODE_IR_FROZEN_FIXTURES ??
    "../engram/benchmarks/semantic_search/fixtures",
);
const labels = JSON.parse(
  await readFile(
    new URL("../fixtures/code-ir-metadata-inventory-v1.json", import.meta.url),
  ),
);

test("source-authored inventory names exactly four frozen lanes and seven selected sites each", () => {
  assert.equal(labels.schema_version, 1);
  assert.deepEqual(
    labels.lanes.map((lane) => lane.language),
    ["go", "python", "rust", "typescript"],
  );
  for (const lane of labels.lanes) {
    assert.equal(lane.fixture_id, `${lane.language}-workflow-discovery-v1`);
    assert.equal(lane.cases.length, 7);
  }
});

test(
  "frozen read-only source labels replay per language when fixture checkout exists",
  {
    skip:
      !process.env.CODE_IR_FROZEN_FIXTURES && !existsSync(fixtureRoot)
        ? "set CODE_IR_FROZEN_FIXTURES to replay the external read-only corpus"
        : false,
  },
  async () => {
    for (const lane of labels.lanes) {
      const root = resolve(fixtureRoot, lane.fixture_id);
      const result = await replayMetadataInventory({
        inventory: lane,
        manifestBytes: await readFile(resolve(root, "manifest.json")),
        readSource: (path) => readFile(resolve(root, path)),
        extractSnapshot,
      });
      assert.equal(result.selected_cases, 7, lane.language);
      assert.equal(
        result.matched_units + result.missing_units,
        7,
        lane.language,
      );
      assert.equal(result.signature_expected, 6, lane.language);
      assert.equal(result.signature_matches, 6, lane.language);
      assert.equal(result.signature_absences, lane.language === "rust" ? 0 : 1);
      assert.equal(result.missing_units, lane.language === "rust" ? 1 : 0);
      assert.equal(
        result.documentation_matches,
        lane.language === "go" ? 4 : 0,
      );
      assert.equal(
        result.unsupported_documentation,
        lane.language === "python" ? 1 : 0,
      );
    }
  },
);
