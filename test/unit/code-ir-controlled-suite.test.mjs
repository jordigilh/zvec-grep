import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { extractSnapshot } from "../../dist/engine/code-ir/index.js";
import {
  citeControlledRun,
  verifyControlledTruth,
  verifyControlledSnapshot,
  groundBaseline,
  judgeBaseline,
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
    entity("method", "guard", "function", guard.start_byte, 114),
    entity("enclosing", "Vault", "class", 0, 114),
    entity("same-name", "guard", "function", 0, 114),
  ];
  const result = groundBaseline({ truth, sources, stored, snapshot, audit });
  assert.equal(result.get("method")?.unit_id, audit.units.get("guard").id);
  assert.equal(result.get("enclosing")?.unit_id, audit.units.get("vault").id);
  const judged = judgeBaseline({ truth, sources, stored, grounded: result });
  assert.equal(judged.get("method")?.site_id, "guard");
  assert.equal(judged.get("enclosing")?.site_id, "vault");
  assert.notEqual(judged.get("same-name")?.site_id, "guard");
  const unlabeled = groundBaseline({
    truth,
    sources,
    stored,
    snapshot,
    audit: { units: new Map() },
  });
  assert.equal(unlabeled.get("method")?.unit_id, audit.units.get("guard").id);
  assert.equal(unlabeled.get("method")?.site_id, undefined);
  const ambiguous = structuredClone(snapshot);
  ambiguous.units.push({
    ...audit.units.get("guard"),
    id: "other-guard",
    source: { ...audit.units.get("guard").source, start_byte: 1 },
  });
  const collisions = groundBaseline({
    truth,
    sources,
    stored,
    snapshot: ambiguous,
    audit,
  });
  assert.equal(collisions.get("method")?.unit_id, null);
  assert.equal(collisions.get("method")?.reason, "ambiguous");
  assert.equal(
    judgeBaseline({ truth, sources, stored, grounded: collisions }).get(
      "method",
    )?.site_id,
    "guard",
  );
  const duplicate = [
    ...stored,
    entity("duplicate", "guard", "function", guard.start_byte, 114),
  ];
  assert.throws(
    () =>
      groundBaseline({ truth, sources, stored: duplicate, snapshot, audit }),
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
  const ungrounded = new Map(baseline);
  ungrounded.set("type", {
    site_id: "vault",
    unit_id: null,
    reason: "ambiguous",
  });
  const ungroundedReport = scoreControlledRun({
    truth,
    raw,
    audit,
    baseline: ungrounded,
  });
  assert.equal(ungroundedReport.queries[1].discovery.syntax_rank, 1);
  assert.equal(ungroundedReport.queries[1].follow_up.status, "not_grounded");
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

test("v2 truth accepts a third pinned file and exactly seven predeclared tasks without consulting IR", async () => {
  const { truth: original, sources: originalSources } = await lane("go");
  const truth = structuredClone(original);
  const sources = new Map(originalSources);
  truth.schema_version = 2;
  truth.fixture_id = "controlled-go-v2";
  const source = Buffer.from(
    "package demo\n" +
      Array.from(
        { length: 12 },
        (_, n) => `func lure${n}() bool { return false }\n`,
      ).join(""),
  );
  sources.set("lures.go", source);
  truth.files["lures.go"] = createHash("sha256").update(source).digest("hex");
  const digest = createHash("sha256");
  for (const [path, bytes] of [...sources].sort(([a], [b]) =>
    a.localeCompare(b),
  ))
    digest.update(path).update("\0").update(bytes).update("\0");
  truth.source_set_sha256 = digest.digest("hex");
  for (let i = 0; i < 12; i++) {
    const anchor = `func lure${i}()`;
    const start = source.indexOf(Buffer.from(anchor));
    truth.sites.push({
      id: `lure${i}`,
      path: "lures.go",
      kind: "function",
      name: `lure${i}`,
      anchor,
      start_byte: start,
      end_byte: start + Buffer.byteLength(anchor),
    });
  }
  truth.queries.push(
    {
      id: "extra-one",
      split: "holdout",
      query: "Where is lure 11?",
      answer: "lure11",
    },
    {
      id: "extra-two",
      split: "holdout",
      query: "Where is lure 3?",
      answer: "lure3",
    },
  );
  assert.equal(
    verifyControlledTruth(truth, sources).size,
    original.sites.length + 12,
  );
  const short = structuredClone(truth);
  short.queries.pop();
  assert.throws(() => verifyControlledTruth(short, sources), /split|task/i);
  const stale = new Map(sources);
  stale.set("lures.go", Buffer.from("changed"));
  assert.throws(() => verifyControlledTruth(truth, stale), /source hash/i);
});

test("small cutoffs and reciprocal ranks expose losses even when @10 is saturated", async () => {
  const { truth, snapshot } = await lane("go");
  const units = new Map(
    truth.sites
      .filter((site) => site.kind !== "call")
      .map((site) => [site.id, { id: site.id }]),
  );
  const audit = {
    units,
    facts: new Map(),
    missing_sites: [],
    missing_relations: [],
    false_links: [],
  };
  const baseline = new Map(
    [...units].map(([site]) => [site, { site_id: site, unit_id: site }]),
  );
  const candidates = ["vault", "token", "guard", "dispatch", "decoy", "proxy"];
  const arm = (name) => ({
    name,
    queries: truth.queries.map((query) => ({
      id: query.id,
      query: query.query,
      hits: [
        query.answer,
        ...candidates.filter((candidate) => candidate !== query.answer),
      ]
        .filter((_, i) => name === "syntax-only" || i > 0)
        .map((entity_id, index) => ({ entity_id, rank: index + 1 })),
    })),
  });
  const syntax = arm("syntax-only");
  const ir = arm("code-ir-v2");
  for (const row of ir.queries) {
    row.hits = ["vault", "token", "guard", "dispatch", "decoy", "proxy"].filter(
      (id) => id !== truth.queries.find((q) => q.id === row.id).answer,
    );
    row.hits.push(truth.queries.find((q) => q.id === row.id).answer);
    row.hits = row.hits.map((entity_id, i) => ({ entity_id, rank: i + 1 }));
  }
  const raw = {
    provenance: {
      fixture_id: truth.fixture_id,
      language: truth.language,
      source_set_sha256: truth.source_set_sha256,
      code_ir: { snapshot_id: snapshot.snapshot_id },
    },
    arms: [syntax, ir],
  };
  const report = scoreControlledRun({ truth, raw, audit, baseline });
  assert.equal(report.aggregate.calibration.syntax_exact_at_1, 2);
  assert.equal(report.aggregate.calibration.ir_exact_at_1, 0);
  assert.equal(report.aggregate.calibration.syntax_mrr, 1);
  assert.equal(report.aggregate.calibration.ir_mrr, 1 / 6);
  assert.equal(report.aggregate.calibration.syntax_exact_at_10, 2);
  assert.equal(report.aggregate.calibration.ir_exact_at_10, 2);
});
