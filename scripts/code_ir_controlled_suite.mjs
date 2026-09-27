// Independent, byte-pinned task truth for a new controlled suite; no generated IR
// or search result is used to create the oracle. Evaluation is opt-in only.
import { createHash } from "node:crypto";
import { validateSnapshot } from "../dist/engine/code-ir/index.js";

const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const safePath = (path) =>
  typeof path === "string" &&
  path.length > 0 &&
  !path.startsWith("/") &&
  !path.includes("\\") &&
  !path.split("/").some((part) => part === ".." || part === "." || !part);

function uniqueIds(rows, label) {
  if (!Array.isArray(rows)) throw new Error(`${label} must be an array`);
  const indexed = new Map();
  for (const row of rows) {
    if (typeof row.id !== "string" || !row.id || indexed.has(row.id))
      throw new Error(`duplicate or missing ${label} id`);
    indexed.set(row.id, row);
  }
  return indexed;
}

export function verifyControlledTruth(truth, sources) {
  if (
    truth.schema_version !== 1 ||
    !["go", "python", "rust", "typescript"].includes(truth.language) ||
    truth.fixture_id !== `controlled-${truth.language}-v1` ||
    !truth.files ||
    Object.keys(truth.files).length !== 2 ||
    sources.size !== 2 ||
    !Object.keys(truth.files).every(safePath)
  )
    throw new Error("invalid controlled source truth/fixture");
  for (const [path, expected] of Object.entries(truth.files)) {
    const bytes = sources.get(path);
    if (
      !Buffer.isBuffer(bytes) ||
      !/^[a-f0-9]{64}$/.test(expected) ||
      hash(bytes) !== expected
    )
      throw new Error(`source hash mismatch: ${path}`);
  }
  if ([...sources.keys()].some((path) => !Object.hasOwn(truth.files, path)))
    throw new Error("unexpected source path");
  const sourceSet = createHash("sha256");
  for (const path of [...sources.keys()].sort())
    sourceSet.update(path).update("\0").update(sources.get(path)).update("\0");
  if (sourceSet.digest("hex") !== truth.source_set_sha256)
    throw new Error("source set digest differs from pinned truth");
  const sites = uniqueIds(truth.sites, "site");
  if (!sites.size) throw new Error("missing source sites");
  const used = new Set();
  for (const site of sites.values()) {
    const bytes = sources.get(site.path);
    if (
      !bytes ||
      !["type", "value", "method", "function", "call"].includes(site.kind) ||
      typeof site.anchor !== "string" ||
      !site.anchor ||
      (site.kind !== "call" &&
        (site.name === undefined
          ? !(
              truth.language === "rust" &&
              site.kind === "type" &&
              site.anchor.startsWith("impl ")
            )
          : typeof site.name !== "string" ||
            !site.anchor.includes(site.name))) ||
      (site.kind === "call" && site.name !== undefined)
    )
      throw new Error(`invalid source site: ${site.id}`);
    const anchor = Buffer.from(site.anchor, "utf8");
    if (
      !Number.isSafeInteger(site.start_byte) ||
      !Number.isSafeInteger(site.end_byte) ||
      site.start_byte < 0 ||
      site.end_byte > bytes.length ||
      site.end_byte - site.start_byte !== anchor.length ||
      !bytes.subarray(site.start_byte, site.end_byte).equals(anchor) ||
      bytes.indexOf(anchor) !== site.start_byte ||
      bytes.indexOf(anchor, site.start_byte + 1) !== -1
    )
      throw new Error(
        `source byte anchor missing, duplicated or stale: ${site.id}`,
      );
    const key = `${site.path}\0${site.start_byte}\0${site.end_byte}`;
    if (used.has(key)) throw new Error(`duplicate anchored site: ${site.id}`);
    used.add(key);
  }
  const relations = uniqueIds(truth.relations, "relation");
  for (const relation of relations.values()) {
    const subject = sites.get(relation.subject);
    const object = sites.get(relation.object);
    const call = sites.get(relation.site);
    if (!subject || subject.kind === "call")
      throw new Error(`unknown relation subject: ${relation.id}`);
    if (relation.kind === "contains") {
      if (
        relation.status !== "observed" ||
        !object ||
        object.kind === "call" ||
        relation.site !== undefined ||
        subject.path !== object.path ||
        subject.start_byte >= object.start_byte
      )
        throw new Error(`unknown or invalid contains object: ${relation.id}`);
    } else if (relation.kind === "calls") {
      if (
        relation.status !== "unresolved" ||
        relation.object !== null ||
        !call ||
        call.kind !== "call" ||
        call.path !== subject.path ||
        call.start_byte <= subject.start_byte
      )
        throw new Error(
          `unresolved call cannot have an object or unknown site: ${relation.id}`,
        );
    } else throw new Error(`unsupported relation kind: ${relation.id}`);
  }
  const queries = uniqueIds(truth.queries, "query");
  if (
    queries.size !== 5 ||
    truth.queries.filter((q) => q.split === "calibration").length !== 2 ||
    truth.queries.filter((q) => q.split === "holdout").length !== 3
  )
    throw new Error("invalid controlled task split");
  for (const query of queries.values()) {
    const answer = sites.get(query.answer);
    const relation = query.relation && relations.get(query.relation);
    if (
      !answer ||
      answer.kind === "call" ||
      typeof query.query !== "string" ||
      !query.query ||
      (query.relation !== undefined && !relation) ||
      (relation &&
        relation.subject !== answer.id &&
        relation.object !== answer.id)
    )
      throw new Error(`unknown or invalid query answer/relation: ${query.id}`);
  }
  return sites;
}

function inside(ref, site) {
  return ref.start_byte <= site.start_byte && ref.end_byte >= site.end_byte;
}

export function verifyControlledSnapshot(truth, sources, snapshot) {
  const sites = verifyControlledTruth(truth, sources);
  const byPath = new Map(
    snapshot.files.map((file) => [file.relative_path, file]),
  );
  if (byPath.size !== snapshot.files.length || byPath.size !== sources.size)
    throw new Error("snapshot source selection differs from truth");
  const byFileId = new Map();
  for (const [path, file] of byPath) {
    if (truth.files[path] !== file.sha256 || file.language !== truth.language)
      throw new Error(`snapshot source hash/language mismatch: ${path}`);
    byFileId.set(file.file_id, sources.get(path));
  }
  validateSnapshot(snapshot, byFileId);
  const units = new Map();
  const missing_sites = [];
  for (const site of sites.values()) {
    if (site.kind === "call") continue;
    const file = byPath.get(site.path);
    const matches = snapshot.units.filter(
      (unit) =>
        unit.source.file_id === file.file_id &&
        unit.kind === site.kind &&
        (site.name === undefined || unit.name === site.name) &&
        inside(unit.source, site),
    );
    if (matches.length === 1) units.set(site.id, matches[0]);
    else
      missing_sites.push({
        site: site.id,
        reason: matches.length ? "ambiguous" : "missing",
      });
  }
  const facts = new Map();
  const missing_relations = [];
  const false_links = [];
  for (const relation of truth.relations) {
    const subject = units.get(relation.subject);
    const object = units.get(relation.object);
    const call = sites.get(relation.site);
    if (!subject || (relation.kind === "contains" && !object)) {
      missing_relations.push(relation.id);
      continue;
    }
    const matches = snapshot.facts.filter(
      (fact) =>
        fact.subject_id === subject.id &&
        fact.kind === relation.kind &&
        (relation.kind === "contains"
          ? fact.object_id === object.id &&
            inside(fact.site, sites.get(relation.object))
          : fact.site.file_id === byPath.get(call.path).file_id &&
            inside(fact.site, call)),
    );
    if (matches.length !== 1) {
      missing_relations.push(relation.id);
      if (matches.length > 1) false_links.push(relation.id);
      continue;
    }
    const fact = matches[0];
    if (
      fact.status !== relation.status ||
      (relation.kind === "calls" &&
        (fact.object_id !== undefined || fact.candidate_ids?.length))
    ) {
      false_links.push(relation.id);
      continue;
    }
    facts.set(relation.id, fact);
  }
  return { units, facts, missing_sites, missing_relations, false_links };
}

const baselineKinds = {
  type: new Set(["class", "struct", "interface", "alias", "type"]),
  method: new Set(["method", "function"]),
  function: new Set(["function"]),
  value: new Set(["field", "value", "property"]),
};

export function groundBaseline({ truth, sources, stored, snapshot, audit }) {
  verifyControlledTruth(truth, sources);
  const grounded = new Map();
  const occupied = new Set();
  const irFiles = new Map(
    snapshot.files.map((file) => [file.relative_path, file]),
  );
  const siteByUnit = new Map(
    [...audit.units].map(([site, unit]) => [unit.id, site]),
  );
  for (const { file, entity } of stored) {
    if (grounded.has(entity.id))
      throw new Error("duplicate baseline entity id");
    const bytes = sources.get(file.relativePath);
    const irFile = irFiles.get(file.relativePath);
    if (
      !bytes ||
      file.contentHash !== truth.files[file.relativePath] ||
      irFile?.sha256 !== file.contentHash
    )
      throw new Error("baseline source hash differs from pinned truth");
    const text = bytes.toString("utf8");
    const { startOffset, endOffset } = entity.range ?? {};
    if (
      entity.range?.kind !== "text" ||
      !Number.isSafeInteger(startOffset) ||
      !Number.isSafeInteger(endOffset) ||
      startOffset < 0 ||
      endOffset <= startOffset ||
      endOffset > text.length ||
      entity.content?.kind !== "text" ||
      entity.content.text !== text.slice(startOffset, endOffset)
    )
      throw new Error(`baseline entity is not exact source: ${entity.id}`);
    const start = Buffer.byteLength(text.slice(0, startOffset));
    const end = Buffer.byteLength(text.slice(0, endOffset));
    // No oracle site or answer label participates in this mapping. Requiring
    // a shared end boundary permits an IR declaration's attested leading doc,
    // Go `type ` or TS `export ` without treating an enclosing class/impl as
    // a method. Anything beyond this narrow policy abstains.
    const matches = snapshot.units.filter(
      (unit) =>
        unit.source.file_id === irFile.file_id &&
        unit.name &&
        unit.name === entity.metadata?.symbolName &&
        baselineKinds[unit.kind]?.has(entity.metadata?.symbolType) &&
        unit.source.start_byte <= start &&
        unit.source.end_byte === end,
    );
    if (matches.length > 1) throw new Error("ambiguous baseline-to-IR unit");
    const unit = matches[0];
    if (unit && occupied.has(unit.id))
      throw new Error(`ambiguous duplicate baseline grounding: ${unit.id}`);
    if (unit) occupied.add(unit.id);
    grounded.set(entity.id, {
      site_id: unit ? (siteByUnit.get(unit.id) ?? null) : null,
      unit_id: unit?.id ?? null,
    });
  }
  return grounded;
}

export function scoreControlledRun({
  truth,
  raw,
  audit,
  baseline,
  projection,
}) {
  if (
    raw.provenance?.fixture_id !== truth.fixture_id ||
    raw.provenance?.language !== truth.language ||
    raw.provenance?.source_set_sha256 !== truth.source_set_sha256 ||
    !raw.provenance?.code_ir?.snapshot_id ||
    JSON.stringify(raw.arms?.map((arm) => arm.name)) !==
      JSON.stringify(["syntax-only", "code-ir-v2"])
  )
    throw new Error("controlled run provenance or arms differ");
  const text = truth.queries.map((query) => query.query);
  const projected = new Map();
  for (const record of projection?.records ?? []) {
    const id =
      record.window_index === 0
        ? record.unit_id
        : createHash("sha256")
            .update(`${record.unit_id}\0window:${record.window_index}`)
            .digest("hex");
    if (projected.has(id)) throw new Error("duplicate projection record id");
    projected.set(id, record.unit_id);
  }
  const irSites = new Map(
    [...audit.units].map(([site, unit]) => [unit.id, site]),
  );
  const unitFor = (id) => (projection ? projected.get(id) : id);
  const queries = truth.queries.map((query, index) => {
    const [syntax, ir] = raw.arms.map((arm) => arm.queries?.[index]);
    for (const [arm, row] of raw.arms.map((arm) => [
      arm,
      arm.queries?.[index],
    ])) {
      if (
        arm.queries.length !== truth.queries.length ||
        row?.id !== query.id ||
        row?.query !== text[index] ||
        !Array.isArray(row.hits) ||
        row.hits.length > 10
      )
        throw new Error(`query order/text differs: ${query.id}`);
      for (const [i, hit] of row.hits.entries()) {
        if (
          hit.rank !== i + 1 ||
          !hit.entity_id ||
          (arm.name === "syntax-only"
            ? !baseline.has(hit.entity_id)
            : projection
              ? !projected.has(hit.entity_id)
              : !irSites.has(hit.entity_id))
        )
          throw new Error(`unknown entity id or invalid rank: ${query.id}`);
      }
    }
    const syntaxRank =
      syntax.hits.find(
        (hit) => baseline.get(hit.entity_id).site_id === query.answer,
      )?.rank ?? null;
    const irRank =
      ir.hits.find(
        (hit) => irSites.get(unitFor(hit.entity_id)) === query.answer,
      )?.rank ?? null;
    const grounded = syntaxRank !== null;
    const relation = truth.relations.find((row) => row.id === query.relation);
    const fact =
      relation && grounded ? audit.facts.get(relation.id) : undefined;
    return {
      id: query.id,
      split: query.split,
      answer: query.answer,
      discovery: { syntax_rank: syntaxRank, ir_rank: irRank },
      follow_up: relation
        ? {
            relation: relation.id,
            status: !grounded
              ? "not_discovered"
              : fact
                ? fact.status
                : "abstained",
            ...(fact
              ? {
                  fact_id: fact.id,
                  site: {
                    file_id: fact.site.file_id,
                    start_byte: fact.site.start_byte,
                    end_byte: fact.site.end_byte,
                  },
                  object_id: fact.object_id ?? null,
                  target_spelling: fact.target_spelling ?? null,
                }
              : {}),
          }
        : null,
    };
  });
  return {
    language: truth.language,
    queries,
    aggregate: Object.fromEntries(
      ["calibration", "holdout"].map((split) => {
        const rows = queries.filter((q) => q.split === split);
        return [
          split,
          {
            tasks: rows.length,
            syntax_exact_at_10: rows.filter(
              (q) => q.discovery.syntax_rank !== null,
            ).length,
            ir_exact_at_10: rows.filter((q) => q.discovery.ir_rank !== null)
              .length,
            follow_up_observed: rows.filter(
              (q) => q.follow_up?.status === "observed",
            ).length,
            unresolved_abstentions: rows.filter(
              (q) =>
                q.follow_up?.status === "unresolved" &&
                q.follow_up.object_id === null,
            ).length,
            false_links: audit.false_links.length,
            missing_relations: audit.missing_relations.length,
          },
        ];
      }),
    ),
    audit: {
      missing_sites: audit.missing_sites,
      missing_relations: audit.missing_relations,
      false_links: audit.false_links,
    },
  };
}

// Cite immutable source bytes from a validated snapshot, never projected text,
// display names, or guessed relationship endpoints.
export function citeControlledRun(report, sources, snapshot, audit) {
  const fileById = new Map(snapshot.files.map((file) => [file.file_id, file]));
  const cite = (ref) => {
    const file = fileById.get(ref.file_id);
    const bytes = file && sources.get(file.relative_path);
    if (
      !bytes ||
      hash(bytes) !== file.sha256 ||
      ref.sha256 !== file.sha256 ||
      ref.start_byte < 0 ||
      ref.end_byte > bytes.length ||
      ref.start_byte >= ref.end_byte
    )
      throw new Error("citation source is missing or stale");
    return {
      path: file.relative_path,
      sha256: file.sha256,
      start_byte: ref.start_byte,
      end_byte: ref.end_byte,
      text: bytes.subarray(ref.start_byte, ref.end_byte).toString("utf8"),
    };
  };
  for (const query of report.queries) {
    const unit = audit.units.get(query.answer);
    query.answer_citation =
      unit &&
      (query.discovery.syntax_rank !== null || query.discovery.ir_rank !== null)
        ? cite(unit.source)
        : null;
    if (query.follow_up?.fact_id) {
      const fact = audit.facts.get(query.follow_up.relation);
      if (!fact || fact.id !== query.follow_up.fact_id)
        throw new Error("follow-up fact citation mismatch");
      query.follow_up.citation = cite(fact.site);
      if (
        fact.kind === "calls" &&
        (fact.status !== "unresolved" || fact.object_id)
      )
        throw new Error("unresolved call cannot cite a guessed target");
    }
  }
  return report;
}
