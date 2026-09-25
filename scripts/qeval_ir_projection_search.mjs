// Build and query source-only, Code IR-projection qeval arms with the current
// zvec storage/search pipeline. This is an evaluation adapter, not app wiring.
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { readFile, mkdir, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { augmentSourceProjection, collectSourceBackedMetadata } from "./source_backed_ir_metadata.mjs";
import { applyRetrievalUnitPolicy } from "./ir_retrieval_unit_policy.mjs";

function option(name, required = true) {
  const index = process.argv.indexOf(name);
  const value = index < 0 ? undefined : process.argv[index + 1];
  if (required && !value) throw new Error(`missing ${name}`);
  return value;
}

const runtimeRoot = resolve(option("--runtime-root"));
const fixtureRoot = resolve(option("--fixture"));
const baselineRoot = resolve(option("--baseline-root"));
const candidateRoot = resolve(option("--candidate-root"));
const sidecarRoot = resolve(option("--sidecar-root"));
const indexRoot = resolve(option("--index-root"));
const modelCache = resolve(option("--model-cache"));
const outputPath = resolve(option("--output"));
const modelIdentity = option("--model", false) ?? "local/potion-code-16m-v2";
const scipShadowPath = option("--scip-shadow", false);
const scipIndexPath = option("--scip-index", false);
const scipBinaryPath = option("--scip-binary", false);
const scipProducer = option("--scip-producer", false);
const expectedScipFacts = option("--expected-scip-facts", false);
const sourceParity = process.argv.includes("--source-parity");
const entityParity = process.argv.includes("--entity-parity");

const dist = join(runtimeRoot, "dist/engine");
const [{ createZvecGrep }, { createEmbeddingModel }, { publishIR, readPublishedIR },
  { validateSnapshot }, { searchWorkspaceIndex }] = await Promise.all([
  import(pathToFileURL(join(dist, "service/index.js"))),
  import(pathToFileURL(join(dist, "models/index.js"))),
  import(pathToFileURL(join(dist, "code-ir/sidecar.js"))),
  import(pathToFileURL(join(dist, "code-ir/index.js"))),
  import(pathToFileURL(join(dist, "pipeline/search/index.js"))),
]);
const requireFromRuntime = createRequire(join(runtimeRoot, "package.json"));
const zvec = requireFromRuntime("@zvec/zvec");

const manifest = JSON.parse(await readFile(join(fixtureRoot, "manifest.json"), "utf8"));
const truth = JSON.parse(await readFile(join(fixtureRoot, "truth.json"), "utf8"));
const qrels = JSON.parse(await readFile(join(fixtureRoot, "qrels.json"), "utf8"));
const language = manifest.language;
if (!["go", "python", "rust", "typescript"].includes(language))
  throw new Error(`unsupported qeval language: ${language}`);
const selectedPaths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
const files = [];
for (const relativePath of selectedPaths) {
  const baselineBytes = await readFile(join(baselineRoot, relativePath));
  const candidateBytes = await readFile(join(candidateRoot, relativePath));
  const fixtureBytes = await readFile(join(fixtureRoot, relativePath));
  if (!baselineBytes.equals(fixtureBytes) || !candidateBytes.equals(fixtureBytes))
    throw new Error(`paired qeval source differs from frozen fixture: ${relativePath}`);
  files.push({
    root_id: manifest.fixture_id,
    relative_path: relativePath,
    language,
    bytes: candidateBytes,
  });
}
const sourceDigest = digestSourceSet(files);
if (sourceDigest !== qrels.source.snapshot_sha256)
  throw new Error("qeval source digest differs from frozen qrels");

const ir = await publishIR(
  sidecarRoot,
  manifest.source.repository,
  files,
  "projection",
  modelIdentity,
);
const published = await readPublishedIR(sidecarRoot);
if (
  !ir ||
  published.manifest.ir_snapshot_id !== ir.ir_snapshot_id ||
  published.manifest.projection_model_identity !== modelIdentity ||
  published.projection?.records.length === 0
)
  throw new Error("published Code IR projection did not match the requested snapshot");

const bytesByPath = new Map(files.map((file) => [file.relative_path, Buffer.from(file.bytes)]));
const irFileById = new Map(published.snapshot.files.map((file) => [file.file_id, file]));
let joinedFacts = [];
let shadowSnapshot = null;
let shadowRecord = null;
let actualIndexSha = null;
if (scipShadowPath) {
  if (!scipIndexPath || !scipBinaryPath)
    throw new Error("SCIP qeval needs --scip-index and --scip-binary provenance");
  shadowRecord = JSON.parse(await readFile(resolve(scipShadowPath), "utf8"));
  shadowSnapshot = shadowRecord.snapshot ?? shadowRecord;
  actualIndexSha = await sha256File(resolve(scipIndexPath));
  const attestedIndexSha = shadowRecord.scip_sha256 ??
    shadowRecord.scip_artifact_sha256 ??
    shadowRecord.producer_artifact_sha256;
  if (!attestedIndexSha || attestedIndexSha !== actualIndexSha)
    throw new Error("SCIP shadow is not attested to the supplied index artifact");
  const sources = new Map(published.snapshot.files.map((file) => [
    file.file_id,
    bytesByPath.get(file.relative_path),
  ]));
  validateSnapshot(shadowSnapshot, sources);
  if (shadowSnapshot.snapshot_id !== published.snapshot.snapshot_id)
    throw new Error("SCIP shadow snapshot differs from qeval Code IR snapshot");
  if (!sameUnitSources(shadowSnapshot.units, published.snapshot.units))
    throw new Error("SCIP shadow changed syntax units or their source spans");
  const units = new Map(shadowSnapshot.units.map((unit) => [unit.id, unit]));
  joinedFacts = shadowSnapshot.facts.filter((fact) =>
    fact.kind === "references" &&
    fact.status === "type_resolved" &&
    fact.object_id &&
    units.has(fact.subject_id) &&
    units.has(fact.object_id),
  );
  if (joinedFacts.length === 0)
    throw new Error("SCIP shadow contains no strictly joined local references");
  if (expectedScipFacts && joinedFacts.length !== Number(expectedScipFacts))
    throw new Error(`SCIP strict reference fact count ${joinedFacts.length} != expected ${expectedScipFacts}`);
}

const referencesBySubject = new Map();
for (const fact of joinedFacts) {
  const target = shadowSnapshot.units.find((unit) => unit.id === fact.object_id);
  const targetName = target.qualified_name ?? target.name;
  if (!targetName) continue;
  const values = referencesBySubject.get(fact.subject_id) ?? new Set();
  values.add(targetName);
  referencesBySubject.set(fact.subject_id, values);
}

zvec.ZVecInitialize({ logLevel: zvec.ZVecLogLevel.WARN });
const model = createEmbeddingModel(modelIdentity, {
  modelCacheDir: modelCache,
  device: "cpu",
});
await model.prepare?.();
const baselineService = await createZvecGrep({
  root: baselineRoot,
  embeddingModel: model,
  embeddingModelOwnership: "borrowed",
});
const rawArms = [];
let sourceMetadataStats = null;
let sourceMetadata = null;
let entityParityStats = null;
try {
  const indexResult = await baselineService.index({
    rebuild: true,
    resetPaths: true,
    rootPaths: [{
      absolutePath: baselineRoot,
      recursive: true,
      include: manifest.source.include,
      exclude: manifest.source.exclude,
    }],
  });
  if (indexResult.filesScanned !== selectedPaths.length)
    throw new Error(`baseline scanned ${indexResult.filesScanned} files, expected ${selectedPaths.length}`);

  const baselineQueries = [];
  for (const query of truth.queries) {
    const result = await baselineService.context({
      query: query.query,
      limit: 10,
      trace: true,
      autoUpdate: false,
    });
    if (result.source !== "index") throw new Error(`baseline did not use the index for ${query.id}`);
    baselineQueries.push({ id: query.id, query: query.query, hits: result.items.map((item) => ({
      path: item.file.relativePath,
      start_line: item.range.kind === "text" ? item.range.startLine : 1,
      end_line: item.range.kind === "text" ? item.range.endLine : 1,
      rank: item.rank,
      entity_id: item.entityId,
      matched_by: item.matchedBy,
      trace: item.trace,
    })) });
  }
  rawArms.push({ name: "syntax-only", queries: baselineQueries });

  const baseRecords = searchableRecords(published.projection.records, published.snapshot.units);
  rawArms.push(await runProjectionArm({
    name: "code-ir-projection",
    records: baseRecords,
    storagePath: join(indexRoot, "code-ir"),
    model,
    snapshot: published.snapshot,
    queries: truth.queries,
    modelIdentity,
  }));

  if (sourceParity) {
    const { CodeExtractor } = await import(pathToFileURL(join(dist, "extraction/code/extractor.js")));
    sourceMetadata = await collectSourceBackedMetadata(published.snapshot, files, CodeExtractor);
    sourceMetadataStats = sourceMetadata.stats;
    const units = new Map(published.snapshot.units.map((unit) => [unit.id, unit]));
    for (const [name, fields] of [
      ["code-ir-source-signatures", { signature: true }],
      ["code-ir-source-docs", { doc: true }],
      ["code-ir-source-signature-docs", { signature: true, doc: true }],
    ]) {
      const records = baseRecords.map((record) => {
        const unit = units.get(record.unit_id);
        const file = irFileById.get(record.source.file_id);
        return augmentSourceProjection(record, unit, bytesByPath.get(file.relative_path),
          sourceMetadata.byUnit.get(record.unit_id), fields);
      });
      const changed = records.filter((record, index) => record !== baseRecords[index]).length;
      rawArms.push(await runProjectionArm({
        name,
        records,
        storagePath: join(indexRoot, name),
        model,
        snapshot: published.snapshot,
        queries: truth.queries,
        modelIdentity,
      }));
      rawArms.at(-1).source_attested_records_added = changed;
    }
  }

  if (entityParity) {
    const { records, excluded } = applyRetrievalUnitPolicy(
      published.projection.records, published.snapshot.units, "legacy-entity-parity");
    entityParityStats = { excluded, indexed_records: records.length };
    rawArms.push(await runProjectionArm({
      name: "code-ir-entity-parity",
      records,
      storagePath: join(indexRoot, "code-ir-entity-parity"),
      model,
      snapshot: published.snapshot,
      queries: truth.queries,
      modelIdentity,
    }));
    if (sourceParity) {
      const units = new Map(published.snapshot.units.map((unit) => [unit.id, unit]));
      for (const [name, route] of [
        ["code-ir-entity-parity-source-metadata", "both"],
        ["code-ir-entity-parity-source-fts", "fts"],
        ["code-ir-entity-parity-source-vector", "vector"],
      ]) {
        const enriched = records.map((record) => {
          const unit = units.get(record.unit_id);
          const file = irFileById.get(record.source.file_id);
          return augmentSourceProjection(record, unit, bytesByPath.get(file.relative_path),
            sourceMetadata.byUnit.get(record.unit_id), { signature: true, doc: true, route });
        });
        rawArms.push(await runProjectionArm({
          name,
          records: enriched,
          storagePath: join(indexRoot, name),
          model,
          snapshot: published.snapshot,
          queries: truth.queries,
          modelIdentity,
        }));
        rawArms.at(-1).source_attested_records_added = enriched.filter((record, index) => record !== records[index]).length;
        if (joinedFacts.length && route === "both") {
          rawArms.push(await runProjectionArm({
            name: "code-ir-entity-parity-source-metadata-scip",
            records: enriched.map(withScipReferences),
            storagePath: join(indexRoot, "code-ir-entity-parity-source-metadata-scip"),
            model,
            snapshot: published.snapshot,
            queries: truth.queries,
            modelIdentity,
          }));
        }
      }
    }
  }

  if (joinedFacts.length) {
    const scipRecords = baseRecords.map(withScipReferences);
    rawArms.push(await runProjectionArm({
      name: "code-ir-plus-scip-references",
      records: scipRecords,
      storagePath: join(indexRoot, "code-ir-scip"),
      model,
      snapshot: published.snapshot,
      queries: truth.queries,
      modelIdentity,
    }));
  }
} finally {
  await baselineService.close();
  await model.dispose();
}

function withScipReferences(record) {
  const references = [...(referencesBySubject.get(record.unit_id) ?? [])].sort();
  if (references.length === 0) return record;
  const line = `scip_references: ${references.join(" ")}`;
  return {
    ...record,
    lexical_text: `${record.lexical_text}\n${line}`,
    vector_input: `${record.vector_input}\n${line}`,
  };
}

const runtime = {
  node: process.version,
  platform: process.platform,
  arch: process.arch,
  zvec_version: JSON.parse(await readFile(join(runtimeRoot, "node_modules/@zvec/zvec/package.json"), "utf8")).version,
};
const codeIR = {
  frontend: published.snapshot.frontend_versions[language],
  snapshot_id: published.snapshot.snapshot_id,
  files: published.snapshot.files.length,
  units: published.snapshot.units.length,
  facts: published.snapshot.facts.length,
  projection_records: published.projection.records.length,
  searchable_units: new Set(baseRecordsForCount(published.projection.records, published.snapshot.units).map((record) => record.unit_id)).size,
};
const provenance = {
  fixture_id: manifest.fixture_id,
  language,
  source_set_sha256: sourceDigest,
  selected_paths: selectedPaths,
  model_identity: modelIdentity,
  runtime,
  code_ir: codeIR,
  ...(sourceParity ? { source_metadata: sourceMetadataStats } : {}),
  ...(entityParity ? { retrieval_policy: entityParityStats } : {}),
  scip: scipShadowPath ? {
    producer: scipProducer ?? shadowRecord.producer ?? "unspecified SCIP producer",
    binary_sha256: await sha256File(resolve(scipBinaryPath)),
    index_sha256: actualIndexSha,
    shadow_sha256: await sha256File(resolve(scipShadowPath)),
    strict_reference_facts: joinedFacts.length,
  } : null,
};
await mkdir(dirname(outputPath), { recursive: true });
await writeFile(outputPath, JSON.stringify({ schema: "code-ir-qeval-raw-v1", provenance, arms: rawArms }, null, 2) + "\n");

function searchableRecords(records, units) {
  const byId = new Map(units.map((unit) => [unit.id, unit]));
  return records.filter((record) => {
    const unit = byId.get(record.unit_id);
    // File/opaque spans aren't answerable manifest declarations and would map
    // every line of a file to every overlapping qeval unit.
    return unit && unit.kind !== "file" && unit.kind !== "opaque";
  });
}

function baseRecordsForCount(records, units) {
  return searchableRecords(records, units);
}

async function runProjectionArm({ name, records, storagePath, model, snapshot, queries, modelIdentity }) {
  const storage = await createProjectionStorage(storagePath, model);
  const filesById = new Map();
  const fileInfos = [];
  for (const irFile of snapshot.files) {
    const raw = bytesByPath.get(irFile.relative_path);
    const path = resolve(candidateRoot, irFile.relative_path);
    const info = await stat(path);
    const file = {
      id: irFile.file_id,
      absolutePath: path,
      relativePath: irFile.relative_path,
      rootPath: candidateRoot,
      sizeBytes: raw.length,
      lastModifiedTime: info.mtimeMs,
      contentHash: irFile.sha256,
      kind: "code",
      format: irFile.language,
    };
    filesById.set(file.id, file);
    fileInfos.push(file);
  }

  const unitsById = new Map(snapshot.units.map((unit) => [unit.id, unit]));
  const recordsByUnit = new Map();
  for (const record of records) {
    const values = recordsByUnit.get(record.unit_id) ?? [];
    values.push(record);
    recordsByUnit.set(record.unit_id, values);
  }
  const fragmentsById = new Map();
  const primaryByGroup = new Map();
  const upsertDocs = [];
  let fragmentIndex = 0;
  const vectorsByInput = await embedDocuments(model, records.map((record) => record.vector_input));
  for (const [unitId, windows] of recordsByUnit) {
    windows.sort((a, b) => a.window_index - b.window_index);
    const unit = unitsById.get(unitId);
    if (!unit) throw new Error(`projection refers to missing unit ${unitId}`);
    for (const record of windows) {
      const irFile = irFileById.get(record.source.file_id);
      const file = filesById.get(record.source.file_id);
      if (!irFile || !file) throw new Error(`projection refers to missing file for ${unitId}`);
      const raw = bytesByPath.get(irFile.relative_path);
      const contentText = new TextDecoder("utf-8", { fatal: true }).decode(
        raw.subarray(record.source.start_byte, record.source.end_byte),
      );
      const symbolType = projectionSymbolType(unit.subtype, unit.kind);
      const metadata = {
        kind: "code",
        symbolType,
        symbolName: unit.name ?? null,
        scope: parentScope(unit),
        nodeType: unit.origin.syntax_kind,
        signature: null,
        doc: null,
        modifiers: [],
      };
      const range = {
        kind: "text",
        startLine: record.source.start.line,
        endLine: record.source.end.line,
        startOffset: record.source.start.column_byte,
        endOffset: record.source.end.column_byte,
      };
      const id = record.window_index === 0
        ? unit.id
        : createHash("sha256").update(`${unit.id}\0window:${record.window_index}`).digest("hex");
      const fragment = {
        id,
        group: unit.id,
        fileId: file.id,
        range,
        content: { kind: "text", text: contentText },
        metadata,
      };
      const vector = vectorsByInput.get(record.vector_input);
      if (!vector) throw new Error(`missing document vector for ${unitId}:${record.window_index}`);
      const fields = {
        group: unit.id,
        file_id: file.id,
        content_kind: "text",
        content_hash: irFile.sha256,
        metadata_kind: "code",
        symbol_type: symbolType,
        ...(unit.name ? { symbol_name: unit.name } : {}),
        ...(metadata.scope ? { symbol_scope: metadata.scope } : {}),
        node_type: unit.origin.syntax_kind,
        text: contentText,
        lexical_text: record.lexical_text,
        fragment_index: fragmentIndex++,
        range_json: JSON.stringify(range),
      };
      upsertDocs.push({
        id,
        vectors: { embedding: vector },
        fields,
      });
      fragmentsById.set(id, fragment);
      if (record.window_index === 0) primaryByGroup.set(unit.id, fragment);
    }
  }
  for (let start = 0; start < upsertDocs.length; start += 1024) {
    const statuses = storage.collection.upsertSync(upsertDocs.slice(start, start + 1024));
    const failed = statuses.find((item) => !item.ok);
    if (failed) throw new Error(`zvec failed to store projection batch: ${failed.message}`);
  }
  await storage.collection.optimize();

  const adapter = {
    listFiles: () => fileInfos,
    getEntity: (id) => {
      const fragment = primaryByGroup.get(id);
      const file = fragment && filesById.get(fragment.fileId);
      return fragment && file ? { entity: entityFor(fragment), file } : null;
    },
    searchFts: (query, limit, filter) => search("fts", query, limit, filter),
    searchVector: (vector, limit, filter) => search("vector", vector, limit, filter),
  };
  function search(path, queryOrVector, limit, filter) {
    const options = {
      fieldName: path === "fts" ? "lexical_text" : "embedding",
      ...(path === "fts" ? { fts: { matchString: queryOrVector } } : { vector: [...queryOrVector] }),
      topk: limit,
      includeVector: false,
    };
    const zvecFilter = filterString(filter);
    if (zvecFilter) options.filter = zvecFilter;
    return storage.collection.querySync(options).map((doc) => {
      const fragment = fragmentsById.get(doc.id);
      const file = fragment && filesById.get(fragment.fileId);
      if (!fragment || !file) throw new Error(`zvec returned unknown projection id ${doc.id}`);
      return { fragment, file, path, score: doc.score };
    });
  }

  const queriesOut = [];
  try {
    for (const query of queries) {
      const result = await searchWorkspaceIndex({
        routes: [{ mode: "fts", query: query.query }, { mode: "vector", query: query.query }],
        limit: 10,
        trace: true,
      }, {
        workspaceIndex: {
          id: `qeval-${name}`,
          name,
          path: storagePath,
          rootPaths: [{ absolutePath: candidateRoot, recursive: true }],
          embedding: {
            provider: model.info.provider,
            model: model.info.name,
            dimension: model.info.dimension,
            metric: model.info.metric,
          },
          indexVersion: 2,
          createdTime: 0,
          updatedTime: 0,
        },
        embeddingModel: model,
        storage: adapter,
      });
      queriesOut.push({ id: query.id, query: query.query, hits: result.hits.map((hit) => ({
        path: hit.file.relativePath,
        start_line: hit.entity.range.kind === "text" ? hit.entity.range.startLine : 1,
        end_line: hit.entity.range.kind === "text" ? hit.entity.range.endLine : 1,
        rank: hit.rank,
        entity_id: hit.entity.id,
        matched_by: hit.matchedBy,
        trace: hit.trace,
      })) });
    }
  } finally {
    storage.collection.closeSync();
  }
  return { name, queries: queriesOut, records: records.length, units: recordsByUnit.size, model_identity: modelIdentity };
}

async function createProjectionStorage(path, model) {
  await mkdir(dirname(path), { recursive: true });
  const { ZVecCollectionSchema, ZVecCreateAndOpen, ZVecDataType, ZVecIndexType, ZVecMetricType } = zvec;
  const stringField = (name, nullable = false) => ({ name, dataType: ZVecDataType.STRING, nullable });
  const indexedStringField = (name, nullable = false) => ({
    name,
    dataType: ZVecDataType.STRING,
    nullable,
    indexParams: { indexType: ZVecIndexType.INVERT },
  });
  const ftsField = (name) => ({
    name,
    dataType: ZVecDataType.STRING,
    nullable: false,
    indexParams: { indexType: ZVecIndexType.FTS, tokenizerName: "jieba", filters: ["lowercase"] },
  });
  const metric = model.info.metric === "cosine"
    ? ZVecMetricType.COSINE
    : model.info.metric === "dot" ? ZVecMetricType.IP : ZVecMetricType.L2;
  const schema = new ZVecCollectionSchema({
    name: "zvec_grep_entities",
    fields: [
      indexedStringField("group", true),
      indexedStringField("file_id"),
      stringField("content_kind"),
      stringField("content_hash", true),
      stringField("metadata_kind", true),
      indexedStringField("symbol_type", true),
      indexedStringField("symbol_name", true),
      stringField("symbol_scope", true),
      stringField("symbol_signature", true),
      stringField("symbol_doc", true),
      stringField("symbol_modifiers", true),
      stringField("node_type", true),
      stringField("heading", true),
      { name: "heading_level", dataType: ZVecDataType.INT32, nullable: true },
      ftsField("text"),
      ftsField("lexical_text"),
      { name: "fragment_index", dataType: ZVecDataType.INT32, nullable: false },
      { name: "range_json", dataType: ZVecDataType.STRING, nullable: false },
      stringField("content_base64", true),
      stringField("image_format", true),
    ],
    vectors: {
      name: "embedding",
      dataType: ZVecDataType.VECTOR_FP32,
      dimension: model.info.dimension,
      indexParams: { indexType: ZVecIndexType.HNSW, metricType: metric },
    },
  });
  return { collection: ZVecCreateAndOpen(path, schema) };
}

async function embedDocuments(model, texts) {
  const output = new Map();
  const batchSize = model.info.limits.maxBatchSize;
  for (let start = 0; start < texts.length; start += batchSize) {
    const batch = texts.slice(start, start + batchSize);
    const { vectors } = await model.embed(batch.map((text) => ({ kind: "text", text })), { purpose: "document" });
    for (const [index, text] of batch.entries()) output.set(text, vectors[index]);
  }
  return output;
}

function filterString(filter) {
  if (!filter) return undefined;
  const clauses = [];
  for (const [field, values] of [["file_id", filter.fileIds], ["group", filter.groupIds], ["symbol_name", filter.symbolNames], ["symbol_type", filter.symbolTypes]]) {
    if (!values) continue;
    const escaped = values.map((value) => `${field} = '${String(value).replaceAll("\\", "\\\\").replaceAll("'", "\\'")}'`);
    clauses.push(escaped.length ? `(${escaped.join(" OR ")})` : "file_id = '__zvec_grep_no_match__'");
  }
  return clauses.length ? clauses.join(" AND ") : undefined;
}

function entityFor(fragment) {
  return {
    id: fragment.group ?? fragment.id,
    fileId: fragment.fileId,
    range: fragment.range,
    content: fragment.content,
    metadata: fragment.metadata,
  };
}

function projectionSymbolType(subtype, kind) {
  if (["class", "interface", "alias", "function", "module", "value"].includes(subtype ?? ""))
    return subtype;
  return kind === "type" ? "class" : kind;
}

function parentScope(unit) {
  if (!unit.qualified_name || !unit.name) return null;
  const suffix = `::${unit.name}`;
  return unit.qualified_name.endsWith(suffix)
    ? unit.qualified_name.slice(0, -suffix.length)
    : null;
}

function sameUnitSources(left, right) {
  const sourceKey = (unit) => `${unit.id}\0${unit.kind}\0${unit.name ?? ""}\0${unit.source.file_id}\0${unit.source.start_byte}\0${unit.source.end_byte}`;
  const a = left.map(sourceKey).sort();
  const b = right.map(sourceKey).sort();
  return a.length === b.length && a.every((value, index) => value === b[index]);
}

function digestSourceSet(inputs) {
  const hash = createHash("sha256");
  for (const file of [...inputs].sort((a, b) => a.relative_path.localeCompare(b.relative_path))) {
    hash.update(file.relative_path);
    hash.update("\0");
    hash.update(file.bytes);
    hash.update("\0");
  }
  return hash.digest("hex");
}

async function sha256File(path) {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}
