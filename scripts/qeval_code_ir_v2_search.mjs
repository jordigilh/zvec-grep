// Frozen, syntax-only versus published/read-validated v2 IR projection.
// Evaluation adapter only; does not change the application's default index.
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

function option(name) {
  const index = process.argv.indexOf(name);
  const value = index < 0 ? undefined : process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`missing ${name}`);
  return value;
}

const fixtureRoot = resolve(option("--fixture"));
const baselineRoot = resolve(option("--baseline-root"));
const candidateRoot = resolve(option("--candidate-root"));
const sidecarRoot = resolve(option("--sidecar-root"));
const indexRoot = resolve(option("--index-root"));
const modelCache = resolve(option("--model-cache"));
const modelIdentity = option("--model");
const outputPath = resolve(option("--output"));
const runtimeRoot = resolve(import.meta.dirname, "..");
const dist = join(runtimeRoot, "dist/engine");
const [
  { createZvecGrep },
  { createEmbeddingModel },
  { publishIR, readPublishedIR },
  { searchWorkspaceIndex },
] = await Promise.all([
  import(pathToFileURL(join(dist, "service/index.js"))),
  import(pathToFileURL(join(dist, "models/index.js"))),
  import(pathToFileURL(join(dist, "code-ir/sidecar.js"))),
  import(pathToFileURL(join(dist, "pipeline/search/index.js"))),
]);
const requireFromRuntime = createRequire(join(runtimeRoot, "package.json"));
const zvec = requireFromRuntime("@zvec/zvec");

const manifest = JSON.parse(
  await readFile(join(fixtureRoot, "manifest.json"), "utf8"),
);
const truth = JSON.parse(
  await readFile(join(fixtureRoot, "truth.json"), "utf8"),
);
const qrels = JSON.parse(
  await readFile(join(fixtureRoot, "qrels.json"), "utf8"),
);
const language = manifest.language;
if (!["go", "python", "rust", "typescript"].includes(language))
  throw new Error(`unsupported qeval language: ${language}`);
const selectedPaths = [
  ...new Set(manifest.units.map((unit) => unit.path)),
].sort();
const files = [];
for (const relativePath of selectedPaths) {
  const baselineBytes = await readFile(join(baselineRoot, relativePath));
  const candidateBytes = await readFile(join(candidateRoot, relativePath));
  const fixtureBytes = await readFile(join(fixtureRoot, relativePath));
  if (
    !baselineBytes.equals(fixtureBytes) ||
    !candidateBytes.equals(fixtureBytes)
  )
    throw new Error(
      `paired qeval source differs from frozen fixture: ${relativePath}`,
    );
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

const publishedManifest = await publishIR(
  sidecarRoot,
  manifest.source.repository,
  files,
  "projection",
  modelIdentity,
  "source-metadata-entity-v1",
);
const {
  manifest: active,
  snapshot,
  projection,
} = await readPublishedIR(sidecarRoot);
if (
  active.ir_snapshot_id !== publishedManifest.ir_snapshot_id ||
  active.projection_version !== 2 ||
  active.projection_policy !== "source-metadata-entity-v1" ||
  active.projection_model_identity !== modelIdentity ||
  projection.version !== 2 ||
  projection.policy !== active.projection_policy ||
  projection.records.length === 0
)
  throw new Error(
    "published Code IR v2 projection did not match requested snapshot/policy",
  );
const irFileById = new Map(snapshot.files.map((file) => [file.file_id, file]));
const bytesByPath = new Map(
  files.map((file) => [file.relative_path, Buffer.from(file.bytes)]),
);

zvec.ZVecInitialize({ logLevel: zvec.ZVecLogLevel.WARN });
const model = createEmbeddingModel(modelIdentity, {
  modelCacheDir: modelCache,
  device: "cpu",
});
let baselineService;
const arms = [];
try {
  await model.prepare?.();
  baselineService = await createZvecGrep({
    root: baselineRoot,
    embeddingModel: model,
    embeddingModelOwnership: "borrowed",
  });
  const result = await baselineService.index({
    rebuild: true,
    resetPaths: true,
    rootPaths: [
      {
        absolutePath: baselineRoot,
        recursive: true,
        include: manifest.source.include,
        exclude: manifest.source.exclude,
      },
    ],
  });
  if (result.filesScanned !== selectedPaths.length)
    throw new Error(
      `baseline scanned ${result.filesScanned} files, expected ${selectedPaths.length}`,
    );

  const baselineQueries = [];
  for (const query of truth.queries) {
    const response = await baselineService.context({
      query: query.query,
      limit: 10,
      trace: true,
      autoUpdate: false,
    });
    if (response.source !== "index")
      throw new Error(`baseline did not use index for ${query.id}`);
    baselineQueries.push({
      id: query.id,
      query: query.query,
      hits: response.items.map((item) => ({
        path: item.file.relativePath,
        start_line: item.range.kind === "text" ? item.range.startLine : 1,
        end_line: item.range.kind === "text" ? item.range.endLine : 1,
        rank: item.rank,
        entity_id: item.entityId,
        matched_by: item.matchedBy,
        trace: item.trace,
      })),
    });
  }
  arms.push({ name: "syntax-only", queries: baselineQueries });
  arms.push(
    await runProjectionArm({
      records: projection.records,
      storagePath: join(indexRoot, "code-ir-v2"),
    }),
  );
} finally {
  await baselineService?.close();
  await model.dispose();
}

const provenance = {
  fixture_id: manifest.fixture_id,
  language,
  source_set_sha256: sourceDigest,
  selected_paths: selectedPaths,
  model_identity: modelIdentity,
  runtime: {
    node: process.version,
    platform: process.platform,
    arch: process.arch,
    zvec_version: JSON.parse(
      await readFile(
        join(runtimeRoot, "node_modules/@zvec/zvec/package.json"),
        "utf8",
      ),
    ).version,
  },
  code_ir: {
    frontend: snapshot.frontend_versions[language],
    snapshot_id: snapshot.snapshot_id,
    files: snapshot.files.length,
    units: snapshot.units.length,
    facts: snapshot.facts.length,
    projection_version: projection.version,
    projection_policy: projection.policy,
    projection_records: projection.records.length,
    searchable_units: new Set(
      projection.records.map((record) => record.unit_id),
    ).size,
  },
  scip: null,
};
await mkdir(dirname(outputPath), { recursive: true });
await writeFile(
  outputPath,
  JSON.stringify(
    { schema: "code-ir-v2-qeval-raw-v1", provenance, arms },
    null,
    2,
  ) + "\n",
);

async function runProjectionArm({ records, storagePath }) {
  const collection = await createProjectionStorage(storagePath);
  try {
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
    const fragmentsById = new Map();
    const primaryByGroup = new Map();
    const upsertDocs = [];
    const vectorsByInput = await embedDocuments(
      records.map((record) => record.vector_input),
    );
    for (const [fragmentIndex, record] of records.entries()) {
      const unit = unitsById.get(record.unit_id);
      const irFile = irFileById.get(record.source.file_id);
      const file = filesById.get(record.source.file_id);
      if (!unit || !irFile || !file)
        throw new Error(
          `v2 record refers to missing unit/file ${record.unit_id}`,
        );
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
      const id =
        record.window_index === 0
          ? unit.id
          : createHash("sha256")
              .update(`${unit.id}\0window:${record.window_index}`)
              .digest("hex");
      const fragment = {
        id,
        group: unit.id,
        fileId: file.id,
        range,
        content: { kind: "text", text: contentText },
        metadata,
      };
      const vector = vectorsByInput.get(record.vector_input);
      if (!vector)
        throw new Error(
          `missing document vector for ${unit.id}:${record.window_index}`,
        );
      upsertDocs.push({
        id,
        vectors: { embedding: vector },
        fields: {
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
          fragment_index: fragmentIndex,
          range_json: JSON.stringify(range),
        },
      });
      fragmentsById.set(id, fragment);
      if (record.window_index === 0) primaryByGroup.set(unit.id, fragment);
    }
    for (let start = 0; start < upsertDocs.length; start += 1024) {
      const statuses = collection.upsertSync(
        upsertDocs.slice(start, start + 1024),
      );
      const failed = statuses.find((item) => !item.ok);
      if (failed)
        throw new Error(
          `zvec failed to store projection batch: ${failed.message}`,
        );
    }
    await collection.optimize();
    const storage = {
      listFiles: () => fileInfos,
      getEntity: (id) => {
        const fragment = primaryByGroup.get(id);
        const file = fragment && filesById.get(fragment.fileId);
        return fragment && file ? { entity: entityFor(fragment), file } : null;
      },
      searchFts: (query, limit, filter) => search("fts", query, limit, filter),
      searchVector: (vector, limit, filter) =>
        search("vector", vector, limit, filter),
    };
    function search(route, queryOrVector, limit, filter) {
      const options = {
        fieldName: route === "fts" ? "lexical_text" : "embedding",
        ...(route === "fts"
          ? { fts: { matchString: queryOrVector } }
          : { vector: [...queryOrVector] }),
        topk: limit,
        includeVector: false,
      };
      const zvecFilter = filterString(filter);
      if (zvecFilter) options.filter = zvecFilter;
      return collection.querySync(options).map((doc) => {
        const fragment = fragmentsById.get(doc.id);
        const file = fragment && filesById.get(fragment.fileId);
        if (!fragment || !file)
          throw new Error(`zvec returned unknown projection id ${doc.id}`);
        return { fragment, file, path: route, score: doc.score };
      });
    }
    const queries = [];
    for (const query of truth.queries) {
      const response = await searchWorkspaceIndex(
        {
          routes: [
            { mode: "fts", query: query.query },
            { mode: "vector", query: query.query },
          ],
          limit: 10,
          trace: true,
        },
        {
          workspaceIndex: {
            id: "qeval-code-ir-v2",
            name: "code-ir-v2",
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
          storage,
        },
      );
      queries.push({
        id: query.id,
        query: query.query,
        hits: response.hits.map((hit) => ({
          path: hit.file.relativePath,
          start_line:
            hit.entity.range.kind === "text" ? hit.entity.range.startLine : 1,
          end_line:
            hit.entity.range.kind === "text" ? hit.entity.range.endLine : 1,
          rank: hit.rank,
          entity_id: hit.entity.id,
          matched_by: hit.matchedBy,
          trace: hit.trace,
        })),
      });
    }
    return {
      name: "code-ir-v2",
      queries,
      records: records.length,
      units: new Set(records.map((record) => record.unit_id)).size,
      model_identity: modelIdentity,
    };
  } finally {
    collection.closeSync();
  }
}

async function createProjectionStorage(path) {
  await mkdir(dirname(path), { recursive: true });
  const {
    ZVecCollectionSchema,
    ZVecCreateAndOpen,
    ZVecDataType,
    ZVecIndexType,
    ZVecMetricType,
  } = zvec;
  const stringField = (name, nullable = false) => ({
    name,
    dataType: ZVecDataType.STRING,
    nullable,
  });
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
    indexParams: {
      indexType: ZVecIndexType.FTS,
      tokenizerName: "jieba",
      filters: ["lowercase"],
    },
  });
  const metric =
    model.info.metric === "cosine"
      ? ZVecMetricType.COSINE
      : model.info.metric === "dot"
        ? ZVecMetricType.IP
        : ZVecMetricType.L2;
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
  return ZVecCreateAndOpen(path, schema);
}

async function embedDocuments(texts) {
  const vectors = new Map();
  for (
    let start = 0;
    start < texts.length;
    start += model.info.limits.maxBatchSize
  ) {
    const batch = texts.slice(start, start + model.info.limits.maxBatchSize);
    const { vectors: output } = await model.embed(
      batch.map((text) => ({ kind: "text", text })),
      { purpose: "document" },
    );
    for (const [index, text] of batch.entries())
      vectors.set(text, output[index]);
  }
  return vectors;
}

function filterString(filter) {
  if (!filter) return undefined;
  const clauses = [];
  for (const [field, values] of [
    ["file_id", filter.fileIds],
    ["group", filter.groupIds],
    ["symbol_name", filter.symbolNames],
    ["symbol_type", filter.symbolTypes],
  ]) {
    if (!values) continue;
    const escaped = values.map(
      (value) =>
        `${field} = '${String(value).replaceAll("\\", "\\\\").replaceAll("'", "\\'")}'`,
    );
    clauses.push(
      escaped.length
        ? `(${escaped.join(" OR ")})`
        : "file_id = '__zvec_grep_no_match__'",
    );
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
  if (
    ["class", "interface", "alias", "function", "module", "value"].includes(
      subtype ?? "",
    )
  )
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

function digestSourceSet(inputs) {
  const hash = createHash("sha256");
  for (const file of [...inputs].sort((a, b) =>
    a.relative_path.localeCompare(b.relative_path),
  )) {
    hash.update(file.relative_path);
    hash.update("\0");
    hash.update(file.bytes);
    hash.update("\0");
  }
  return hash.digest("hex");
}
