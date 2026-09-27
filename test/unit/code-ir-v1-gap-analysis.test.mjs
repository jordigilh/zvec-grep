import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import {
  inventoryBaselineVsIR,
  compareRankedQueries,
  validateDiagnosticInputs,
  summarizeLexicalDifferences,
  buildBaselineTextViews,
} from "../../scripts/code_ir_v1_gap_analysis.mjs";

const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

function data({
  text = "α\nclass Workflow:\n pass",
  baseline = "class Workflow:\n pass",
  ir = baseline,
} = {}) {
  const bytes = Buffer.from(text);
  const start = text.indexOf(baseline);
  const irStart = text.indexOf(ir);
  const file = {
    id: "baseline-file",
    relativePath: "a.py",
    contentHash: hash(bytes),
  };
  const source = (a, b) => ({
    file_id: "ir-file",
    sha256: hash(bytes),
    start_byte: Buffer.byteLength(text.slice(0, a)),
    end_byte: Buffer.byteLength(text.slice(0, b)),
    start: { line: text.slice(0, a).split("\n").length },
    end: { line: text.slice(0, b).split("\n").length },
  });
  const entity = {
    id: "syntax-id",
    content: { kind: "text", text: baseline },
    metadata: { kind: "code", symbolName: "Workflow", symbolType: "class" },
    range: {
      kind: "text",
      startOffset: start,
      endOffset: start + baseline.length,
      startLine: source(start, start).start.line,
      endLine: source(start, start + baseline.length).end.line,
    },
  };
  const unit = {
    id: "ir-id",
    name: "Workflow",
    kind: "type",
    subtype: "class",
    source: source(irStart, irStart + ir.length),
  };
  return {
    stored: [{ file, entity }],
    snapshot: {
      files: [
        {
          file_id: "ir-file",
          relative_path: "a.py",
          sha256: file.contentHash,
          byte_length: bytes.length,
        },
      ],
      units: [unit],
    },
    projection: {
      records: [
        {
          unit_id: unit.id,
          unit_source: unit.source,
          source: unit.source,
          window_index: 0,
        },
      ],
    },
    sources: new Map([[file.relativePath, bytes]]),
  };
}

test("exact matched identity converts Unicode JS offsets to original UTF-8 bytes", () => {
  const result = inventoryBaselineVsIR(data());
  assert.equal(result.summary.exact, 1);
  assert.equal(result.summary.baseline, 1);
  assert.equal(result.summary.ir_units, 1);
  assert.equal(result.entries[0].start_byte, 3);
  assert.equal(result.entries[0].category, "exact");
  assert.equal(result.entries[0].source_verified, true);
});

test("same name and source lines do not imply exact span; reports leading syntax separately", () => {
  const result = inventoryBaselineVsIR(
    data({
      text: "type Workflow struct{}",
      baseline: "Workflow struct{}",
      ir: "type Workflow struct{}",
    }),
  );
  assert.equal(result.summary.exact, 0);
  assert.equal(result.summary.range_mismatch, 1);
  assert.equal(result.entries[0].same_lines, true);
  assert.equal(result.entries[0].ir_leading_source, "type ");
});

test("ambiguous name, missing name, duplicate ID, and altered bytes fail closed", () => {
  const input = data({ text: "class Workflow:\n pass class Workflow:\n pass" });
  input.snapshot.units.push({
    ...input.snapshot.units[0],
    id: "other",
    source: { ...input.snapshot.units[0].source, start_byte: 2 },
  });
  input.projection.records.push({
    ...input.projection.records[0],
    unit_id: "other",
    unit_source: input.snapshot.units[1].source,
    source: input.snapshot.units[1].source,
  });
  // An exact match disambiguates; without one, same-named definitions are ambiguous.
  assert.equal(inventoryBaselineVsIR(input).entries[0].category, "exact");
  input.snapshot.units[0].source = {
    ...input.snapshot.units[0].source,
    start_byte: 1,
  };
  input.projection.records[0].unit_source = input.snapshot.units[0].source;
  input.projection.records[0].source = input.snapshot.units[0].source;
  assert.equal(inventoryBaselineVsIR(input).entries[0].category, "ambiguous");

  const unknown = data();
  unknown.stored[0].entity.metadata.symbolName = null;
  assert.equal(inventoryBaselineVsIR(unknown).entries[0].category, "unnamed");
  const duplicate = data();
  duplicate.stored.push(duplicate.stored[0]);
  assert.throws(() => inventoryBaselineVsIR(duplicate), /duplicate.*ID/i);
  const changed = data();
  changed.sources.set("a.py", Buffer.from("tampered"));
  assert.throws(
    () => inventoryBaselineVsIR(changed),
    /source.*hash|hash.*source/i,
  );
  const falseContent = data();
  falseContent.stored[0].entity.content.text = "not source text";
  assert.throws(() => inventoryBaselineVsIR(falseContent), /source.*text/i);
});

test("rank comparison rejects unknown entity ID and misordered query IDs", () => {
  const inventory = inventoryBaselineVsIR(data());
  const qrels = {
    queries: [
      { id: "q1", judgments: [{ unit_id: "a.py::Workflow", grade: 2 }] },
      { id: "q2", judgments: [] },
    ],
  };
  const normalized = {
    runs: [
      {
        backend: "syntax-only",
        queries: [
          {
            id: "q1",
            results: [{ unit_id: "a.py::Workflow", backend_rank: 1 }],
          },
          { id: "q2", results: [] },
        ],
      },
      {
        backend: "code-ir-v1",
        queries: [
          { id: "q1", results: [] },
          { id: "q2", results: [] },
        ],
      },
    ],
  };
  const raw = {
    arms: [
      {
        name: "syntax-only",
        queries: [
          {
            id: "q1",
            hits: [{ entity_id: "syntax-id", rank: 1, trace: { recall: [] } }],
          },
          { id: "q2", hits: [] },
        ],
      },
      {
        name: "code-ir-v1",
        queries: [
          {
            id: "q1",
            hits: [{ entity_id: "ir-id", rank: 1, trace: { recall: [] } }],
          },
          { id: "q2", hits: [] },
        ],
      },
    ],
  };
  const result = compareRankedQueries({ raw, normalized, qrels, inventory });
  assert.equal(result.queries[0].positive_units[0].ranks["syntax-only"], 1);
  assert.equal(result.queries[0].positive_units[0].ranks["code-ir-v1"], null);
  assert.equal(
    result.queries[0].raw_hits["syntax-only"][0].inventory_category,
    "exact",
  );
  raw.arms[0].queries[0].hits[0].entity_id = "missing";
  assert.throws(
    () => compareRankedQueries({ raw, normalized, qrels, inventory }),
    /unknown.*entity/i,
  );
  raw.arms[0].queries[0].hits[0].entity_id = "syntax-id";
  normalized.runs[1].queries.reverse();
  assert.throws(
    () => compareRankedQueries({ raw, normalized, qrels, inventory }),
    /query.*order/i,
  );
});

test("diagnostic rejects altered run, fixture, and source provenance", () => {
  const raw = {
    provenance: {
      fixture_id: "go-workflow-discovery-v1",
      language: "go",
      source_set_sha256: "source",
      model_identity: "model",
      code_ir: { snapshot_id: "ir" },
    },
    arms: [
      "syntax-only",
      "code-ir-v1",
      "code-ir-policy-only",
      "code-ir-metadata-only",
      "code-ir-v2",
    ].map((name) => ({ name })),
  };
  const normalized = {
    fixture_id: raw.provenance.fixture_id,
    source: { snapshot_sha256: "source" },
    runs: raw.arms.map((arm) => ({ backend: arm.name })),
  };
  const runManifest = {
    fixture_id: raw.provenance.fixture_id,
    language: "go",
    source_set_sha256: "source",
    raw_sha256: "raw",
    normalized_sha256: "normalized",
    manifest_sha256: "fixture manifest",
    qrels_sha256: "fixture qrels",
    model: { identity: "model" },
    code_ir: { snapshot_id: "ir" },
    ablation: { enabled: true },
  };
  const inputs = {
    raw,
    normalized,
    runManifest,
    fixtureId: raw.provenance.fixture_id,
    language: "go",
    sourceDigest: "source",
    snapshotId: "ir",
    rawSha256: "raw",
    normalizedSha256: "normalized",
    fixtureManifestSha256: "fixture manifest",
    qrelsSha256: "fixture qrels",
  };
  validateDiagnosticInputs(inputs);
  for (const changed of [
    { ...inputs, rawSha256: "altered" },
    { ...inputs, sourceDigest: "altered" },
    { ...inputs, fixtureId: "other" },
    { ...inputs, qrelsSha256: "altered" },
    {
      ...inputs,
      raw: {
        ...raw,
        provenance: { ...raw.provenance, model_identity: "other" },
      },
    },
    { ...inputs, raw: { ...raw, arms: raw.arms.slice(1) } },
  ])
    assert.throws(
      () => validateDiagnosticInputs(changed),
      /provenance|hash|arm/i,
    );
});

test("text audit compares source-identical units without misattributing range differences", () => {
  const exact = data();
  exact.stored[0].entity.metadata = {
    ...exact.stored[0].entity.metadata,
    scope: null,
    signature: "class Workflow:",
    modifiers: [],
    doc: null,
  };
  const inventory = inventoryBaselineVsIR(exact);
  const v1 = {
    records: [
      {
        ...exact.projection.records[0],
        lexical_text: "symbol: class Workflow\nclass Workflow:\n pass",
      },
    ],
  };
  const v2 = {
    records: [
      {
        ...exact.projection.records[0],
        lexical_text:
          "symbol: class Workflow\nsignature: class Workflow:\nclass Workflow:\n pass",
      },
    ],
  };
  const audit = summarizeLexicalDifferences({
    inventory,
    stored: exact.stored,
    v1,
    v2,
  });
  assert.equal(audit.summary.comparable, 1);
  assert.equal(audit.summary.baseline_equals_v1, 0);
  assert.equal(audit.summary.baseline_equals_v2, 0);
  assert.equal(audit.entries[0].baseline_signature, true);
  assert.equal(audit.entries[0].v1_signature, false);
  assert.equal(audit.entries[0].v2_signature, true);
  const shifted = data({
    text: "type Workflow struct{}",
    baseline: "Workflow struct{}",
    ir: "type Workflow struct{}",
  });
  assert.equal(
    summarizeLexicalDifferences({
      inventory: inventoryBaselineVsIR(shifted),
      stored: shifted.stored,
      v1: shifted.projection,
      v2: shifted.projection,
    }).summary.comparable,
    0,
  );
});

test("text swap uses identical indexed units/windows and isolates FTS from vector", () => {
  const fixture = data();
  const inventory = inventoryBaselineVsIR(fixture);
  const original = {
    ...fixture.projection.records[0],
    lexical_text: "v1 fts",
    vector_input: "v1 vector",
  };
  const before = structuredClone(original);
  const prepared = new Map([
    [
      "syntax-id",
      { lexical_text: "syntax fts", vector_input: "syntax vector" },
    ],
  ]);
  const views = buildBaselineTextViews({
    inventory,
    records: [original],
    prepared,
  });
  assert.deepEqual(
    views.map((view) => view.name),
    [
      "code-ir-v1-baseline-fts",
      "code-ir-v1-baseline-vector",
      "code-ir-v1-baseline-both",
    ],
  );
  assert.deepEqual(
    views.map((view) => [
      view.records[0].lexical_text,
      view.records[0].vector_input,
    ]),
    [
      ["syntax fts", "v1 vector"],
      ["v1 fts", "syntax vector"],
      ["syntax fts", "syntax vector"],
    ],
  );
  for (const view of views) {
    assert.deepEqual(view.records[0].source, original.source);
    assert.deepEqual(view.records[0].unit_source, original.unit_source);
    assert.equal(view.records[0].unit_id, original.unit_id);
  }
  assert.deepEqual(original, before);
  assert.throws(
    () =>
      buildBaselineTextViews({
        inventory,
        records: [original],
        prepared: new Map(),
      }),
    /missing.*text/i,
  );
  assert.throws(
    () =>
      buildBaselineTextViews({
        inventory,
        records: [
          { ...original, source: { ...original.source, start_byte: 0 } },
        ],
        prepared,
      }),
    /source.*range/i,
  );
  assert.throws(
    () =>
      buildBaselineTextViews({
        inventory,
        records: [original, original],
        prepared,
      }),
    /duplicate/i,
  );
  const shifted = data({
    text: "type Workflow struct{}",
    baseline: "Workflow struct{}",
    ir: "type Workflow struct{}",
  });
  assert.throws(
    () =>
      buildBaselineTextViews({
        inventory: inventoryBaselineVsIR(shifted),
        records: shifted.projection.records,
        prepared,
      }),
    /exact.*unit/i,
  );
});
