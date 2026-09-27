// Opt-in Go-only unchanged syntax control and one source-verified reranking arm.
// Qrel grades are never passed to the search service or reranker.
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  rerankGoCandidates,
  verifyGoCandidates,
} from "./go_retrieval_rerank.mjs";

const [fixtureArg, stageArg, modelCacheArg, modelIdentity, outputArg] =
  process.argv.slice(2);
const rerank = process.argv.slice(7).includes("--rerank");
if (!fixtureArg || !stageArg || !modelCacheArg || !modelIdentity || !outputArg)
  throw new Error(
    "expected fixture, stage, model cache, model identity, output",
  );
const fixture = resolve(fixtureArg);
const stage = resolve(stageArg);
const manifest = JSON.parse(await readFile(join(fixture, "manifest.json")));
const truth = JSON.parse(await readFile(join(fixture, "truth.json")));
const qrels = JSON.parse(await readFile(join(fixture, "qrels.json")));
if (
  manifest.fixture_id !== "go-workflow-discovery-v1" ||
  manifest.language !== "go" ||
  truth.queries.length !== 8 ||
  truth.queries.some(
    (row, index) =>
      row.id !== qrels.queries[index]?.id ||
      row.query !== qrels.queries[index]?.query,
  )
)
  throw new Error("not the pinned Go qeval fixture");
const sourceHash = createHash("sha256");
const sources = new Map();
const paths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
for (const path of paths) {
  const bytes = await readFile(join(stage, path));
  if (!bytes.equals(await readFile(join(fixture, path))))
    throw new Error(`staged source differs: ${path}`);
  sourceHash.update(path).update("\0").update(bytes).update("\0");
  sources.set(path, bytes);
}
if (sourceHash.digest("hex") !== qrels.source.snapshot_sha256)
  throw new Error("staged source set differs from qrels");

const dist = join(import.meta.dirname, "../dist/engine");
const [
  { createZvecGrep },
  { createEmbeddingModel },
  { createWorkspaceIndexStorage },
] = await Promise.all([
  import(pathToFileURL(join(dist, "service/index.js"))),
  import(pathToFileURL(join(dist, "models/index.js"))),
  import(pathToFileURL(join(dist, "storage/index.js"))),
]);
const model = createEmbeddingModel(modelIdentity, {
  modelCacheDir: resolve(modelCacheArg),
  device: "cpu",
});
let service;
try {
  await model.prepare?.();
  service = await createZvecGrep({
    root: stage,
    embeddingModel: model,
    embeddingModelOwnership: "borrowed",
  });
  const result = await service.index({
    rebuild: true,
    resetPaths: true,
    rootPaths: [
      {
        absolutePath: stage,
        recursive: true,
        include: manifest.source.include,
        exclude: manifest.source.exclude,
      },
    ],
  });
  if (result.filesScanned !== paths.length)
    throw new Error("baseline indexed file count differs from frozen fixture");

  const storage = createWorkspaceIndexStorage({
    storagePath: join(stage, ".zvec-grep"),
    readOnly: true,
  });
  let indexed;
  const byId = new Map();
  try {
    const indexedFiles = storage.listFiles();
    if (indexedFiles.length !== paths.length)
      throw new Error("indexed file count differs from frozen fixture");
    indexed = indexedFiles.flatMap((file) => {
      if (!paths.includes(file.relativePath))
        throw new Error(`unexpected indexed file: ${file.relativePath}`);
      if (
        createHash("sha256")
          .update(sources.get(file.relativePath))
          .digest("hex") !== file.contentHash
      )
        throw new Error("indexed file hash differs from frozen source");
      return storage.listEntitiesByFile(file.id).map(({ entity }) => {
        if (byId.has(entity.id)) throw new Error("duplicate indexed entity");
        byId.set(entity.id, { file, entity });
        return {
          path: file.relativePath,
          start_line: entity.range.startLine,
          end_line: entity.range.endLine,
          entity_id: entity.id,
          symbol_name: entity.metadata?.symbolName ?? null,
        };
      });
    });
  } finally {
    storage.close();
  }
  const arms = [];
  const poolItems = [];
  for (const limit of [10, 30]) {
    const queries = [];
    for (const query of truth.queries) {
      const response = await service.context({
        query: query.query,
        limit,
        trace: true,
        autoUpdate: false,
      });
      if (response.source !== "index")
        throw new Error(`syntax control did not use index for ${query.id}`);
      if (limit === 30) poolItems.push(response.items);
      queries.push({
        id: query.id,
        query: query.query,
        hits: response.items.map((item) => ({
          path: item.file.relativePath,
          start_line: item.range.kind === "text" ? item.range.startLine : 1,
          end_line: item.range.kind === "text" ? item.range.endLine : 1,
          rank: item.rank,
          entity_id: item.entityId,
          trace: item.trace,
        })),
      });
    }
    arms.push({ name: `syntax-${limit}`, queries });
  }
  if (rerank) {
    const queries = [];
    for (const [index, query] of truth.queries.entries()) {
      const items = poolItems[index];
      const pool = arms[1].queries[index].hits;
      if (items.length !== 30 || pool.length !== items.length)
        throw new Error(
          `candidate pool is not thirty indexed hits: ${query.id}`,
        );
      const verified = verifyGoCandidates(
        items.map((item) => {
          const stored = byId.get(item.entityId);
          if (
            !stored ||
            stored.file.relativePath !== item.file.relativePath ||
            stored.entity.range.kind !== "text" ||
            item.range.kind !== "text" ||
            stored.entity.range.startLine !== item.range.startLine ||
            stored.entity.range.endLine !== item.range.endLine
          )
            throw new Error(
              `context hit differs from stored source: ${query.id}`,
            );
          return { ...stored, entity_id: item.entityId, rank: item.rank };
        }),
        sources,
      );
      const original = new Map(pool.map((hit) => [hit.entity_id, hit]));
      queries.push({
        id: query.id,
        query: query.query,
        hits: rerankGoCandidates(query.query, verified, 10).map((row) => {
          const source = original.get(row.entity_id);
          if (
            !source ||
            source.path !== row.path ||
            source.start_line !== row.start_line ||
            source.end_line !== row.end_line
          )
            throw new Error("rerank candidate source range differs from pool");
          return {
            ...source,
            rank: row.rank,
            hybrid_rank: row.hybrid_rank,
            lexical_rank: row.lexical_rank,
            alignment: row.alignment,
            rerank_score: row.score,
          };
        }),
      });
    }
    arms.push({ name: "go-source-rerank", queries });
  }
  await writeFile(
    resolve(outputArg),
    JSON.stringify(
      {
        schema: "go-retrieval-diagnostic-v1",
        provenance: {
          source_set_sha256: qrels.source.snapshot_sha256,
          model_identity: modelIdentity,
          indexed_entities: indexed.length,
          rerank_policy: rerank ? "go-source-lexical-rrf-v1" : null,
        },
        indexed,
        arms,
      },
      null,
      2,
    ) + "\n",
  );
} finally {
  await service?.close();
  await model.dispose();
}
