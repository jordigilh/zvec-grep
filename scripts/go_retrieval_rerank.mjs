// One-shot, Go-only offline retrieval hypothesis. No qrel or truth dependencies.
import { createHash } from "node:crypto";

const stop = new Set([
  "a",
  "an",
  "and",
  "are",
  "as",
  "at",
  "be",
  "by",
  "for",
  "from",
  "how",
  "if",
  "in",
  "into",
  "is",
  "it",
  "of",
  "on",
  "or",
  "should",
  "that",
  "the",
  "their",
  "this",
  "to",
  "what",
  "when",
  "where",
  "which",
  "who",
  "with",
  "function",
  "does",
  "do",
  "can",
  "isnt",
  "not",
  "become",
]);

export function tokenizeCode(text) {
  if (typeof text !== "string") throw new Error("text must be a string");
  return text
    .replace(/([A-Z])([A-Z][a-z]{2,})/g, "$1 $2")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean)
    .map((token) =>
      token.length > 4 && token.endsWith("s") ? token.slice(0, -1) : token,
    )
    .filter((token) => !stop.has(token));
}

export function verifyGoCandidates(pool, sources) {
  if (!Array.isArray(pool) || pool.length === 0 || !Array.isArray([...sources]))
    throw new Error("missing candidate pool or sources");
  const identities = new Set();
  return pool.map((candidate, index) => {
    const { file, entity } = candidate;
    const path = file?.relativePath;
    if (
      !Number.isSafeInteger(candidate.rank) ||
      candidate.rank !== index + 1 ||
      !candidate.entity_id ||
      candidate.entity_id !== entity?.id ||
      identities.has(candidate.entity_id)
    )
      throw new Error("duplicate candidate identity or noncontiguous rank");
    identities.add(candidate.entity_id);
    if (
      typeof path !== "string" ||
      path.startsWith("/") ||
      path.includes("\\") ||
      path.split("/").some((part) => !part || part === "." || part === "..")
    )
      throw new Error("unsafe source path");
    const raw = sources.get(path);
    if (
      !Buffer.isBuffer(raw) ||
      createHash("sha256").update(raw).digest("hex") !== file.contentHash
    )
      throw new Error("source hash differs from indexed candidate");
    const text = new TextDecoder("utf-8", { fatal: true }).decode(raw);
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
      throw new Error("candidate content is not exact source");
    return Object.freeze({
      entity_id: candidate.entity_id,
      rank: candidate.rank,
      path,
      start_line: entity.range.startLine,
      end_line: entity.range.endLine,
      name: entity.metadata?.symbolName ?? "",
      source: entity.content.text,
    });
  });
}

export function rerankGoCandidates(query, verified, limit = 10) {
  if (
    !Array.isArray(verified) ||
    verified.length === 0 ||
    !Number.isInteger(limit) ||
    limit < 1 ||
    limit > verified.length
  )
    throw new Error("invalid rerank pool/limit");
  const queryTokens = new Set(tokenizeCode(query));
  const documents = verified.map((row) => ({
    row,
    name: new Set(tokenizeCode(row.name)),
    source: new Set(tokenizeCode(row.source)),
    path: new Set(tokenizeCode(row.path)),
  }));
  const weight = new Map(
    [...queryTokens].map((token) => [
      token,
      1 +
        Math.log(
          (verified.length + 1) /
            (1 +
              documents.filter(
                (doc) =>
                  doc.name.has(token) ||
                  doc.source.has(token) ||
                  doc.path.has(token),
              ).length),
        ),
    ]),
  );
  const denominator =
    3.25 * [...weight.values()].reduce((sum, idf) => sum + idf, 0);
  const lexical = documents.map((doc) => ({
    ...doc,
    alignment: denominator
      ? [...weight].reduce(
          (sum, [token, idf]) =>
            sum +
            idf *
              ((doc.name.has(token) ? 2 : 0) +
                (doc.source.has(token) ? 1 : 0) +
                (doc.path.has(token) ? 0.25 : 0)),
          0,
        ) / denominator
      : 0,
  }));
  lexical.sort(
    (a, b) =>
      b.alignment - a.alignment ||
      a.row.rank - b.row.rank ||
      a.row.entity_id.localeCompare(b.row.entity_id),
  );
  const lexicalRank = new Map(
    lexical.map((doc, index) => [doc.row.entity_id, index + 1]),
  );
  const combined = lexical.map((doc) => ({
    ...doc.row,
    hybrid_rank: doc.row.rank,
    lexical_rank: doc.alignment ? lexicalRank.get(doc.row.entity_id) : null,
    alignment: doc.alignment,
    score:
      1 / (60 + doc.row.rank) +
      (doc.alignment ? 0.5 / (60 + lexicalRank.get(doc.row.entity_id)) : 0),
  }));
  combined.sort(
    (a, b) =>
      b.score - a.score ||
      a.hybrid_rank - b.hybrid_rank ||
      a.entity_id.localeCompare(b.entity_id),
  );
  return combined
    .slice(0, limit)
    .map((row, index) => ({ ...row, rank: index + 1 }));
}
