import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { extractSnapshot } from "../../dist/engine/code-ir/index.js";
import {
  citeControlledRun,
  verifyControlledTruth,
  verifyControlledSnapshot,
  groundBaseline,
  scoreControlledRun,
} from "../../scripts/code_ir_controlled_suite.mjs";

const root = new URL("../fixtures/code-ir-controlled-v1/", import.meta.url);
async function lane(language) {
  const directory = new URL(`${language}/`, root);
  const truth = JSON.parse(await readFile(new URL("truth.json", directory)));
  const sources = new Map(
    await Promise.all(
      Object.keys(truth.files).map(async (path) => [
        path,
        await readFile(new URL(path, directory)),
      ]),
    ),
  );
  const checked = verifyControlledTruth(truth, sources);
  const { snapshot } = await extractSnapshot(
    truth.fixture_id,
    [...sources].map(([relative_path, bytes]) => ({
      root_id: truth.fixture_id,
      relative_path,
      language,
      bytes,
    })),
  );
  return { truth, sources, checked, snapshot };
}

test("four independent source-pinned lanes have disjoint exact site and task truth", async () => {
  for (const language of ["go", "python", "rust", "typescript"]) {
    const { truth, checked } = await lane(language);
    assert.equal(truth.queries.length, 5);
    assert.deepEqual(
      truth.queries.map((q) => q.split),
      ["calibration", "calibration", "holdout", "holdout", "holdout"],
    );
    assert.equal(checked.size, truth.sites.length);
    assert.notEqual(
      truth.sites.find((site) => site.id === "guard").path,
      truth.sites.find((site) => site.id === "decoy").path,
    );
  }
});

test("stale bytes, fabricated ranges, duplicate anchors, and invalid relation references fail closed", async () => {
  const { truth, sources } = await lane("go");
  const altered = new Map(sources);
  altered.set("core.go", Buffer.from("stale"));
  assert.throws(() => verifyControlledTruth(truth, altered), /hash|source/i);
  const badRange = structuredClone(truth);
  badRange.sites[0].start_byte++;
  assert.throws(() => verifyControlledTruth(badRange, sources), /byte|anchor/i);
  const duplicate = structuredClone(truth);
  duplicate.sites.push({ ...duplicate.sites[0], id: "other" });
  assert.throws(
    () => verifyControlledTruth(duplicate, sources),
    /duplicate|overlap/i,
  );
  const bogus = structuredClone(truth);
  bogus.relations[0].object = "missing";
  assert.throws(
    () => verifyControlledTruth(bogus, sources),
    /unknown|missing/i,
  );
  const falseLink = structuredClone(truth);
  falseLink.relations[1].object = "guard";
  assert.throws(
    () => verifyControlledTruth(falseLink, sources),
    /unresolved|object/i,
  );
  const unknownQuery = structuredClone(truth);
  unknownQuery.queries[0].answer = "missing";
  assert.throws(
    () => verifyControlledTruth(unknownQuery, sources),
    /unknown|missing/i,
  );
});

test("four-language IR evidence resolves containment and preserves unresolved calls without a guessed callee", async () => {
  for (const language of ["go", "python", "rust", "typescript"]) {
    const { truth, sources, snapshot } = await lane(language);
    const audit = verifyControlledSnapshot(truth, sources, snapshot);
    assert.deepEqual(audit.missing_sites, [], language);
    assert.deepEqual(audit.missing_relations, [], language);
    assert.deepEqual(audit.false_links, [], language);
    for (const relation of truth.relations) {
      const fact = audit.facts.get(relation.id);
      assert.equal(fact.kind, relation.kind);
      assert.equal(fact.status, relation.status);
      assert.equal(fact.object_id === undefined, relation.object === null);
    }
  }
});

test("a forged type-resolved call is a false link, never a follow-up target", async () => {
  const { truth, sources, snapshot } = await lane("python");
  const injected = structuredClone(snapshot);
  const fact = injected.facts.find(
    (row) =>
      row.kind === "calls" &&
      row.target_spelling === "vault.guard" &&
      row.site.start_byte ===
        truth.sites.find((s) => s.id === "dispatch-call").start_byte,
  );
  const guard = injected.units.find(
    (unit) => unit.kind === "method" && unit.name === "guard",
  );
  assert.ok(fact && guard);
  fact.status = "type_resolved";
  fact.object_id = guard.id;
  fact.provenance.resolver = "untrusted-test";
  fact.provenance.resolver_version = "0";
  const audit = verifyControlledSnapshot(truth, sources, injected);
  assert.ok(audit.false_links.includes("dispatch-guard"));
  assert.equal(audit.facts.has("dispatch-guard"), false);
});

test("missing evidence is reported rather than supplied by same-name decoys", async () => {
  const { truth, sources, snapshot } = await lane("go");
  const missing = structuredClone(snapshot);
  const call = truth.sites.find((site) => site.id === "dispatch-call");
  missing.facts = missing.facts.filter(
    (fact) =>
      !(
        fact.kind === "calls" &&
        fact.site.start_byte === call.start_byte &&
        fact.site.sha256 === truth.files[call.path]
      ),
  );
  const audit = verifyControlledSnapshot(truth, sources, missing);
  assert.ok(audit.missing_relations.includes("dispatch-guard"));
  assert.equal(audit.facts.has("dispatch-guard"), false);
});

test("grounding rejects enclosing type, decoy, and non-unique same-name matches", async () => {
  const { truth, sources, snapshot } = await lane("python");
  const audit = verifyControlledSnapshot(truth, sources, snapshot);
  const text = sources.get("core.py").toString("utf8");
  const file = { relativePath: "core.py", contentHash: truth.files["core.py"] };
  const entity = (id, name, type, start, end) => ({
    file,
    entity: {
      id,
      range: { kind: "text", startOffset: start, endOffset: end },
      content: { kind: "text", text: text.slice(start, end) },
      metadata: { symbolName: name, symbolType: type },
    },
  });
  const guard = truth.sites.find((site) => site.id === "guard");
  const stored = [
    entity("method", "guard", "function", guard.start_byte, 112),
    entity("enclosing", "Vault", "class", 0, 112),
    entity("same-name", "guard", "function", 0, 112),
  ];
  const result = groundBaseline({ truth, sources, stored, audit });
  assert.equal(result.get("method")?.site_id, "guard");
  assert.equal(result.get("enclosing")?.site_id, "vault");
  assert.notEqual(result.get("same-name")?.site_id, "guard");
  const duplicate = [
    ...stored,
    entity("duplicate", "guard", "function", guard.start_byte, 112),
  ];
  assert.throws(
    () => groundBaseline({ truth, sources, stored: duplicate, audit }),
    /ambiguous|duplicate/i,
  );
});

test("scorer uses entity IDs and exact truth sites, not line overlaps or query order", async () => {
  const { truth, sources, snapshot } = await lane("python");
  const audit = verifyControlledSnapshot(truth, sources, snapshot);
  const baseline = new Map([
    ["answer", { site_id: "guard", unit_id: audit.units.get("guard").id }],
    ["type", { site_id: "vault", unit_id: audit.units.get("vault").id }],
  ]);
  const raw = {
    provenance: {
      fixture_id: truth.fixture_id,
      language: truth.language,
      source_set_sha256: truth.source_set_sha256,
      code_ir: { snapshot_id: snapshot.snapshot_id },
    },
    arms: [
      {
        name: "syntax-only",
        queries: truth.queries.map((q) => ({
          id: q.id,
          query: q.query,
          hits: [{ entity_id: q.id === "check" ? "answer" : "type", rank: 1 }],
        })),
      },
      {
        name: "code-ir-v2",
        queries: truth.queries.map((q) => ({
          id: q.id,
          query: q.query,
          hits: [{ entity_id: audit.units.get("vault").id, rank: 1 }],
        })),
      },
    ],
  };
  const report = scoreControlledRun({ truth, raw, audit, baseline });
  assert.equal(report.queries[0].discovery.syntax_rank, 1);
  assert.equal(report.queries[0].discovery.ir_rank, null);
  assert.equal(report.queries[1].follow_up.status, "observed");
  citeControlledRun(report, sources, snapshot, audit);
  const citation = report.queries[1].follow_up.citation;
  assert.equal(citation.path, "core.py");
  assert.equal(
    citation.text,
    sources
      .get(citation.path)
      .subarray(citation.start_byte, citation.end_byte)
      .toString("utf8"),
  );
  raw.arms[0].queries.reverse();
  assert.throws(
    () => scoreControlledRun({ truth, raw, audit, baseline }),
    /order|query/i,
  );
  raw.arms[0].queries.reverse();
  raw.arms[0].queries[0].hits[0].entity_id = "missing";
  assert.throws(
    () => scoreControlledRun({ truth, raw, audit, baseline }),
    /unknown.*id/i,
  );
});
