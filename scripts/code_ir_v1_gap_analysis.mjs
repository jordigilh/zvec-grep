// Read-only, source-backed diagnostics. A line overlap is not an identity match.
import { createHash } from "node:crypto";
import { lexicalTextForFragment } from "../dist/engine/extraction/vector-content.js";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const expectedArms = [
  "syntax-only",
  "code-ir-v1",
  "code-ir-policy-only",
  "code-ir-metadata-only",
  "code-ir-v2",
];

export function validateDiagnosticInputs({
  raw,
  normalized,
  runManifest,
  fixtureId,
  language,
  sourceDigest,
  snapshotId,
  rawSha256,
  normalizedSha256,
  fixtureManifestSha256,
  qrelsSha256,
}) {
  if (
    raw.provenance?.fixture_id !== fixtureId ||
    raw.provenance?.language !== language ||
    raw.provenance?.source_set_sha256 !== sourceDigest ||
    raw.provenance?.code_ir?.snapshot_id !== snapshotId ||
    normalized.fixture_id !== fixtureId ||
    normalized.source?.snapshot_sha256 !== sourceDigest ||
    runManifest.fixture_id !== fixtureId ||
    runManifest.language !== language ||
    runManifest.source_set_sha256 !== sourceDigest ||
    runManifest.code_ir?.snapshot_id !== snapshotId ||
    runManifest.raw_sha256 !== rawSha256 ||
    runManifest.normalized_sha256 !== normalizedSha256 ||
    runManifest.manifest_sha256 !== fixtureManifestSha256 ||
    runManifest.qrels_sha256 !== qrelsSha256 ||
    raw.provenance?.model_identity !== runManifest.model?.identity ||
    runManifest.ablation?.enabled !== true ||
    JSON.stringify(raw.arms?.map((arm) => arm.name)) !==
      JSON.stringify(expectedArms) ||
    JSON.stringify(normalized.runs?.map((arm) => arm.backend)) !==
      JSON.stringify(expectedArms)
  )
    throw new Error(
      "diagnostic input provenance, hashes or arms differ from frozen run",
    );
}

function symbolType(unit) {
  if (
    ["class", "interface", "alias", "function", "module", "value"].includes(
      unit.subtype,
    )
  )
    return unit.subtype;
  return unit.kind === "type" ? "class" : unit.kind;
}

export function inventoryBaselineVsIR({
  stored,
  snapshot,
  projection,
  sources,
}) {
  const fileByPath = new Map(
    snapshot.files.map((file) => [file.relative_path, file]),
  );
  const unitById = new Map(snapshot.units.map((unit) => [unit.id, unit]));
  if (
    fileByPath.size !== snapshot.files.length ||
    unitById.size !== snapshot.units.length
  )
    throw new Error("duplicate IR file path or unit ID");

  const selectedIds = new Set(
    projection.records.map((record) => record.unit_id),
  );
  for (const record of projection.records) {
    const unit = unitById.get(record.unit_id);
    if (
      !unit ||
      JSON.stringify(record.unit_source) !== JSON.stringify(unit.source)
    )
      throw new Error(
        "projection record does not reference a selected IR unit source",
      );
  }
  const selected = [...selectedIds].map((id) => unitById.get(id));
  const unitsByPathName = new Map();
  for (const unit of selected) {
    const file = snapshot.files.find(
      (item) => item.file_id === unit.source.file_id,
    );
    if (!file || !unit.name)
      throw new Error("selected IR unit lacks a file or name");
    const key = `${file.relative_path}\0${unit.name}`;
    unitsByPathName.set(key, [...(unitsByPathName.get(key) ?? []), unit]);
  }

  const sourceTextByPath = new Map();
  for (const file of snapshot.files) {
    const bytes = sources.get(file.relative_path);
    if (
      !bytes ||
      sha256(bytes) !== file.sha256 ||
      bytes.length !== file.byte_length
    )
      throw new Error(
        `source hash/length differs from IR: ${file.relative_path}`,
      );
    sourceTextByPath.set(
      file.relative_path,
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    );
  }

  const entries = [];
  const seenIds = new Set();
  for (const { file, entity } of stored) {
    if (seenIds.has(entity.id))
      throw new Error(`duplicate baseline entity ID: ${entity.id}`);
    seenIds.add(entity.id);
    const irFile = fileByPath.get(file.relativePath);
    const bytes = sources.get(file.relativePath);
    if (!irFile || !bytes || file.contentHash !== irFile.sha256)
      throw new Error(
        `baseline source hash differs from IR: ${file.relativePath}`,
      );
    const text = sourceTextByPath.get(file.relativePath);
    const range = entity.range;
    if (
      range.kind !== "text" ||
      !Number.isInteger(range.startOffset) ||
      !Number.isInteger(range.endOffset) ||
      range.startOffset < 0 ||
      range.endOffset < range.startOffset ||
      range.endOffset > text.length ||
      entity.content.kind !== "text" ||
      entity.content.text !== text.slice(range.startOffset, range.endOffset)
    )
      throw new Error(`baseline entity is not exact source text: ${entity.id}`);
    const startByte = Buffer.byteLength(text.slice(0, range.startOffset));
    const endByte = Buffer.byteLength(text.slice(0, range.endOffset));
    const name = entity.metadata?.symbolName ?? null;
    const candidates = name
      ? (unitsByPathName.get(`${file.relativePath}\0${name}`) ?? [])
      : [];
    const matches = candidates.filter(
      (unit) =>
        unit.source.start_byte === startByte &&
        unit.source.end_byte === endByte,
    );
    const kindMatches = matches.filter(
      (unit) => entity.metadata?.symbolType === symbolType(unit),
    );
    const chosen =
      kindMatches.length === 1
        ? kindMatches[0]
        : candidates.length === 1
          ? candidates[0]
          : null;
    const category = !name
      ? "unnamed"
      : kindMatches.length === 1
        ? "exact"
        : matches.length
          ? "kind_mismatch"
          : candidates.length > 1
            ? "ambiguous"
            : candidates.length === 1
              ? "range_mismatch"
              : "missing_ir_unit";
    const prefix =
      chosen &&
      chosen.source.start_byte < startByte &&
      chosen.source.end_byte === endByte
        ? bytes.subarray(chosen.source.start_byte, startByte).toString("utf8")
        : null;
    entries.push({
      baseline_id: entity.id,
      path: file.relativePath,
      name,
      baseline_kind: entity.metadata?.symbolType ?? null,
      category,
      source_verified: true,
      start_byte: startByte,
      end_byte: endByte,
      baseline_lines: [range.startLine, range.endLine],
      ir_unit_id: chosen?.id ?? null,
      ir_kind: chosen ? symbolType(chosen) : null,
      ir_bytes: chosen
        ? [chosen.source.start_byte, chosen.source.end_byte]
        : null,
      same_lines: chosen
        ? chosen.source.start.line === range.startLine &&
          chosen.source.end.line === range.endLine
        : false,
      ir_leading_source:
        prefix && !prefix.includes("\n") && prefix.length <= 80 ? prefix : null,
      candidate_unit_ids:
        category === "ambiguous" ? candidates.map((unit) => unit.id) : [],
    });
  }

  const matchedIds = new Set(
    entries.map((entry) => entry.ir_unit_id).filter(Boolean),
  );
  const summary = {
    baseline: entries.length,
    ir_units: selected.length,
    exact: entries.filter((entry) => entry.category === "exact").length,
    range_mismatch: entries.filter(
      (entry) => entry.category === "range_mismatch",
    ).length,
    kind_mismatch: entries.filter((entry) => entry.category === "kind_mismatch")
      .length,
    ambiguous: entries.filter((entry) => entry.category === "ambiguous").length,
    unnamed: entries.filter((entry) => entry.category === "unnamed").length,
    missing_ir_unit: entries.filter(
      (entry) => entry.category === "missing_ir_unit",
    ).length,
    unmatched_ir_units: selected.length - matchedIds.size,
    same_name_and_lines: entries.filter((entry) => entry.same_lines).length,
  };
  return {
    summary,
    entries,
    selected_unit_ids: [...selectedIds],
    snapshot_unit_ids: [...unitById.keys()],
  };
}

export function compareRankedQueries({ raw, normalized, qrels, inventory }) {
  const ids = qrels.queries.map((query) => query.id);
  if (
    !ids.length ||
    new Set(ids).size !== ids.length ||
    raw.arms.length !== normalized.runs.length ||
    raw.arms.some(
      (arm, index) =>
        arm.name !== normalized.runs[index].backend ||
        JSON.stringify(arm.queries.map((q) => q.id)) !== JSON.stringify(ids) ||
        JSON.stringify(normalized.runs[index].queries.map((q) => q.id)) !==
          JSON.stringify(ids),
    )
  )
    throw new Error("query IDs/order or arm names differ from qrels");

  const byBaselineId = new Map(
    inventory.entries.map((entry) => [entry.baseline_id, entry]),
  );
  const allIds = new Set(inventory.snapshot_unit_ids);
  const selectedIds = new Set(inventory.selected_unit_ids);
  const byIrId = new Map(
    inventory.entries
      .filter((entry) => entry.ir_unit_id)
      .map((entry) => [entry.ir_unit_id, entry]),
  );
  const queries = qrels.queries.map((query, index) => {
    const rawHits = {};
    const positiveUnits = query.judgments
      .filter((judge) => judge.grade > 0)
      .map((judge) => ({
        unit_id: judge.unit_id,
        grade: judge.grade,
        ranks: {},
      }));
    for (const [armIndex, arm] of raw.arms.entries()) {
      const run = normalized.runs[armIndex];
      const ranked = run.queries[index].results;
      for (const unit of positiveUnits)
        unit.ranks[arm.name] =
          ranked.find((row) => row.unit_id === unit.unit_id)?.backend_rank ??
          null;
      rawHits[arm.name] = arm.queries[index].hits.map((hit, hitIndex) => {
        if (hit.rank !== hitIndex + 1)
          throw new Error(`non-contiguous rank for ${query.id}`);
        const isSyntax = arm.name === "syntax-only";
        const item = isSyntax
          ? byBaselineId.get(hit.entity_id)
          : byIrId.get(hit.entity_id);
        if (
          (!isSyntax && !allIds.has(hit.entity_id)) ||
          (isSyntax && !item) ||
          (arm.name === "code-ir-v2" && !selectedIds.has(hit.entity_id))
        )
          throw new Error(
            `unknown indexed entity ID for ${query.id}: ${hit.entity_id}`,
          );
        return {
          rank: hit.rank,
          entity_id: hit.entity_id,
          inventory_category: isSyntax
            ? item.category
            : (item?.category ?? "v1-only-or-unmatched"),
          path: hit.path,
          lines: [hit.start_line, hit.end_line],
          routes: Object.fromEntries(
            (hit.trace?.recall ?? []).map((route) => [
              route.path,
              route.rank ?? null,
            ]),
          ),
          fusion_score: hit.trace?.fusion?.score ?? null,
        };
      });
    }
    return { id: query.id, positive_units: positiveUnits, raw_hits: rawHits };
  });
  return { queries };
}

export function summarizeLexicalDifferences({ inventory, stored, v1, v2 }) {
  const firstWindows = (view) => {
    const records = new Map();
    for (const record of view.records) {
      if (record.window_index !== 0) continue;
      if (records.has(record.unit_id))
        throw new Error(`duplicate first window for ${record.unit_id}`);
      records.set(record.unit_id, record);
    }
    return records;
  };
  const v1Records = firstWindows(v1);
  const v2Records = firstWindows(v2);
  const byId = new Map(stored.map(({ entity }) => [entity.id, entity]));
  const entries = inventory.entries.map((entry) => {
    const baseline = byId.get(entry.baseline_id);
    const one = v1Records.get(entry.ir_unit_id);
    const two = v2Records.get(entry.ir_unit_id);
    if (
      !["exact", "kind_mismatch"].includes(entry.category) ||
      !one ||
      !two ||
      [one, two].some(
        (record) =>
          record.source.start_byte !== entry.start_byte ||
          record.source.end_byte !== entry.end_byte ||
          record.unit_source.start_byte !== entry.start_byte ||
          record.unit_source.end_byte !== entry.end_byte,
      )
    )
      return {
        baseline_id: entry.baseline_id,
        ir_unit_id: entry.ir_unit_id,
        status: "not_byte_comparable",
      };

    const syntax = lexicalTextForFragment(baseline);
    const lines = (text) =>
      text.slice(0, text.length - baseline.content.text.length).split("\n");
    const contains = (prefix, name) =>
      prefix.some((line) => line.startsWith(`${name}:`));
    if (
      ![syntax, one.lexical_text, two.lexical_text].every((text) =>
        text.endsWith(baseline.content.text),
      )
    )
      throw new Error(
        `source text differs for byte-matched lexical unit ${entry.baseline_id}`,
      );
    const [syntaxPrefix, onePrefix, twoPrefix] = [
      syntax,
      one.lexical_text,
      two.lexical_text,
    ].map(lines);
    return {
      baseline_id: entry.baseline_id,
      ir_unit_id: entry.ir_unit_id,
      status: "comparable",
      baseline_equals_v1: syntax === one.lexical_text,
      baseline_equals_v2: syntax === two.lexical_text,
      baseline_signature: contains(syntaxPrefix, "signature"),
      v1_signature: contains(onePrefix, "signature"),
      v2_signature: contains(twoPrefix, "signature"),
      baseline_doc: contains(syntaxPrefix, "doc"),
      v1_doc: contains(onePrefix, "doc"),
      v2_doc: contains(twoPrefix, "doc"),
      v1_empty_scope: onePrefix.includes("scope: "),
      v2_empty_scope: twoPrefix.includes("scope: "),
      lexical_prefix_sha256: [syntaxPrefix, onePrefix, twoPrefix].map(
        (prefix) => sha256(prefix.join("\n")),
      ),
    };
  });
  const comparable = entries.filter((entry) => entry.status === "comparable");
  const summary = {
    comparable: comparable.length,
    not_byte_comparable: entries.length - comparable.length,
    baseline_equals_v1: comparable.filter((entry) => entry.baseline_equals_v1)
      .length,
    baseline_equals_v2: comparable.filter((entry) => entry.baseline_equals_v2)
      .length,
    baseline_signature: comparable.filter((entry) => entry.baseline_signature)
      .length,
    v1_signature: comparable.filter((entry) => entry.v1_signature).length,
    v2_signature: comparable.filter((entry) => entry.v2_signature).length,
    baseline_doc: comparable.filter((entry) => entry.baseline_doc).length,
    v1_doc: comparable.filter((entry) => entry.v1_doc).length,
    v2_doc: comparable.filter((entry) => entry.v2_doc).length,
    v1_empty_scope: comparable.filter((entry) => entry.v1_empty_scope).length,
    v2_empty_scope: comparable.filter((entry) => entry.v2_empty_scope).length,
  };
  return { summary, entries };
}

export function buildBaselineTextViews({ inventory, records, prepared }) {
  if (new Set(records.map((record) => record.unit_id)).size !== records.length)
    throw new Error("duplicate record in text-only ablation");
  if (
    inventory.summary.baseline !== inventory.summary.ir_units ||
    inventory.summary.unmatched_ir_units !== 0 ||
    inventory.entries.length !== records.length ||
    inventory.entries.some(
      (entry) => !["exact", "kind_mismatch"].includes(entry.category),
    )
  )
    throw new Error(
      "text-only ablation requires exact matched units throughout the index",
    );
  const byUnit = new Map(
    inventory.entries.map((entry) => [entry.ir_unit_id, entry]),
  );
  if (byUnit.size !== records.length)
    throw new Error("duplicate record or matched unit in text-only ablation");
  const sourceText = records.map((record) => {
    const entry = byUnit.get(record.unit_id);
    if (
      !entry ||
      [record.source, record.unit_source].some(
        (source) =>
          source.start_byte !== entry.start_byte ||
          source.end_byte !== entry.end_byte,
      )
    )
      throw new Error(
        "IR record source range differs from matched syntax entity",
      );
    const text = prepared.get(entry.baseline_id);
    if (
      !text ||
      typeof text.lexical_text !== "string" ||
      typeof text.vector_input !== "string"
    )
      throw new Error(
        `missing verified baseline text for ${entry.baseline_id}`,
      );
    return text;
  });
  const makeView = (name, lexical, vector) => ({
    name,
    records: records.map((record, index) => ({
      ...record,
      lexical_text: lexical
        ? sourceText[index].lexical_text
        : record.lexical_text,
      vector_input: vector
        ? sourceText[index].vector_input
        : record.vector_input,
    })),
  });
  return [
    makeView("code-ir-v1-baseline-fts", true, false),
    makeView("code-ir-v1-baseline-vector", false, true),
    makeView("code-ir-v1-baseline-both", true, true),
  ];
}
