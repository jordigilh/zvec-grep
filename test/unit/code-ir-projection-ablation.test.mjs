import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import {
  publishIR,
  readPublishedIR,
} from "../../dist/engine/code-ir/sidecar.js";
import { buildProjectionAblations } from "../../scripts/code_ir_projection_ablation.mjs";

const source = {
  file_id: "f",
  sha256: "0".repeat(64),
  start_byte: 1,
  end_byte: 4,
};
const windows = (id, text) => ({
  unit_id: id,
  group_id: id,
  window_index: 0,
  source: { ...source },
  unit_source: { ...source },
  lexical_text: text,
  vector_input: text,
});
const snapshot = {
  snapshot_id: "1".repeat(64),
  units: [
    { id: "file", kind: "file", origin: { language: "go" } },
    { id: "answer", kind: "function", origin: { language: "go" } },
    {
      id: "field",
      kind: "value",
      subtype: "struct_field",
      origin: { language: "go" },
    },
  ],
};
const v1 = {
  version: 1,
  ir_snapshot_id: snapshot.snapshot_id,
  model_identity: "model",
  records: [
    windows("file", "whole file"),
    windows("answer", "original"),
    windows("field", "field"),
  ],
};
const v2 = {
  version: 2,
  policy: "source-metadata-entity-v1",
  ir_snapshot_id: snapshot.snapshot_id,
  model_identity: "model",
  records: [windows("answer", "signature: source-backed\noriginal")],
};

test("four factorial arms change only unit selection and attested text", () => {
  const before = structuredClone({ snapshot, v1, v2 });
  const arms = buildProjectionAblations(snapshot, v1, v2);
  assert.deepEqual(
    arms.map((arm) => arm.name),
    [
      "code-ir-v1",
      "code-ir-policy-only",
      "code-ir-metadata-only",
      "code-ir-v2",
    ],
  );
  assert.deepEqual(
    arms.map((arm) => arm.records.map((record) => record.unit_id)),
    [["answer", "field"], ["answer"], ["answer", "field"], ["answer"]],
  );
  assert.equal(arms[0].records[0], v1.records[1]);
  assert.equal(arms[1].records[0], v1.records[1]);
  assert.equal(arms[2].records[0], v2.records[0]);
  assert.equal(arms[2].records[1], v1.records[2]);
  assert.equal(arms[3].records[0], v2.records[0]);
  assert.deepEqual({ snapshot, v1, v2 }, before);
});

test("ablation refuses ref, version, model and policy mismatches rather than silently confounding text", () => {
  const failures = [
    [v1, { ...v2, model_identity: "wrong" }],
    [
      { ...v1, model_identity: undefined },
      { ...v2, model_identity: undefined },
    ],
    [{ ...v1, version: 2 }, v2],
    [v1, { ...v2, ir_snapshot_id: "2".repeat(64) }],
    [v1, { ...v2, policy: "other" }],
    [
      v1,
      {
        ...v2,
        records: [{ ...v2.records[0], source: { ...source, start_byte: 2 } }],
      },
    ],
    [v1, { ...v2, records: [] }],
    [{ ...v1, records: v1.records.filter((r) => r.unit_id !== "answer") }, v2],
    [v1, { ...v2, records: [...v2.records, v2.records[0]] }],
    [v1, { ...v2, records: [{ ...v2.records[0], window_index: 1 }] }],
    [v1, { ...v2, records: [{ ...v2.records[0], group_id: "different" }] }],
    [
      v1,
      {
        ...v2,
        records: [
          { ...v2.records[0], unit_source: { ...source, end_byte: 3 } },
        ],
      },
    ],
    [v1, { ...v2, records: [windows("field", "wrong selection")] }],
  ];
  for (const [left, right] of failures) {
    assert.throws(() => buildProjectionAblations(snapshot, left, right));
  }
});

test("policy-only ablation keeps Python class attributes in IR but out of standalone hits", () => {
  const python = {
    ...snapshot,
    units: [
      { id: "file", kind: "file", origin: { language: "python" } },
      { id: "answer", kind: "function", origin: { language: "python" } },
      {
        id: "field",
        kind: "value",
        subtype: "class_attribute",
        origin: { language: "python" },
      },
    ],
  };
  const arms = buildProjectionAblations(python, v1, v2);
  assert.deepEqual(
    arms[1].records.map((r) => r.unit_id),
    ["answer"],
  );
  assert.deepEqual(
    arms[2].records.map((r) => r.unit_id),
    ["answer", "field"],
  );
});

const frozen = resolve(
  process.env.CODE_IR_FROZEN_FIXTURES ??
    "../engram/benchmarks/semantic_search/fixtures",
);
test(
  "read-only frozen v1/v2 sidecars share every eligible source window",
  {
    skip:
      !process.env.CODE_IR_FROZEN_FIXTURES && !existsSync(frozen)
        ? "set CODE_IR_FROZEN_FIXTURES for optional external fixture replay"
        : false,
  },
  async () => {
    for (const [language, v1Count, v2Count] of [
      ["go", 58, 38],
      ["python", 45, 33],
      ["rust", 49, 49],
      ["typescript", 38, 38],
    ]) {
      const fixture = join(frozen, `${language}-workflow-discovery-v1`);
      const manifest = JSON.parse(
        await readFile(join(fixture, "manifest.json")),
      );
      const files = await Promise.all(
        [...new Set(manifest.units.map((unit) => unit.path))]
          .sort()
          .map(async (path) => ({
            root_id: manifest.fixture_id,
            relative_path: path,
            language,
            bytes: await readFile(join(fixture, path)),
          })),
      );
      const directory = await mkdtemp(join(tmpdir(), "zg-ir-factorial-"));
      try {
        await publishIR(
          directory,
          manifest.source.repository,
          files,
          "projection",
          "model",
        );
        const first = await readPublishedIR(directory);
        await publishIR(
          directory,
          manifest.source.repository,
          files,
          "projection",
          "model",
          "source-metadata-entity-v1",
        );
        const second = await readPublishedIR(directory);
        assert.equal(
          first.snapshot.snapshot_id,
          second.snapshot.snapshot_id,
          language,
        );
        assert.deepEqual(first.snapshot.units, second.snapshot.units, language);
        assert.deepEqual(first.snapshot.facts, second.snapshot.facts, language);
        const arms = buildProjectionAblations(
          second.snapshot,
          first.projection,
          second.projection,
        );
        assert.deepEqual(
          arms.map((arm) => arm.records.length),
          [v1Count, v2Count, v1Count, v2Count],
          language,
        );
      } finally {
        await rm(directory, { recursive: true, force: true });
      }
    }
  },
);
