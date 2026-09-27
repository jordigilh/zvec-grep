import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import {
  tokenizeCode,
  verifyGoCandidates,
  rerankGoCandidates,
} from "../../scripts/go_retrieval_rerank.mjs";

const sha = (buffer) => createHash("sha256").update(buffer).digest("hex");

function setup() {
  const bytes = Buffer.from(
    "func listWorkflows() {}\nfunc recordDiscovery() {}\nfunc fake() {}\n",
  );
  const fragment = (id, name, rank) => {
    const startOffset = bytes.toString().indexOf(`func ${name}(`);
    const endOffset = bytes.toString().indexOf("\n", startOffset);
    return {
      entity_id: id,
      rank,
      file: { relativePath: "internal/util.go", contentHash: sha(bytes) },
      entity: {
        id,
        range: { kind: "text", startOffset, endOffset },
        content: {
          kind: "text",
          text: bytes.toString().slice(startOffset, endOffset),
        },
        metadata: { symbolName: name },
      },
    };
  };
  return {
    sources: new Map([["internal/util.go", bytes]]),
    pool: [
      fragment("a", "fake", 1),
      fragment("b", "recordDiscovery", 2),
      fragment("c", "listWorkflows", 3),
    ],
  };
}

test("Go name and text are tokenized symmetrically without using truth labels", () => {
  assert.deepEqual(tokenizeCode("Where are listWorkflows IDs?"), [
    "list",
    "workflow",
    "ids",
  ]);
  assert.deepEqual(tokenizeCode("record_discoveries"), [
    "record",
    "discoverie",
  ]);
  assert.deepEqual(tokenizeCode("IF IN should or"), []);
});

test("candidate reordering uses only verified source, preserves IDs and has deterministic ties", () => {
  const { sources, pool } = setup();
  const verified = verifyGoCandidates(pool, sources);
  assert.equal(verified.length, 3);
  assert.deepEqual(
    rerankGoCandidates(
      "Where are list workflows recorded for discovery?",
      verified,
      2,
    ).map((row) => row.entity_id),
    ["b", "c"],
  );
  assert.deepEqual(
    rerankGoCandidates("unmatched zzzz", verified, 3).map(
      (row) => row.entity_id,
    ),
    ["a", "b", "c"],
  );
  const repeated = rerankGoCandidates(
    "Where are list workflows recorded for discovery?",
    verified,
    3,
  );
  assert.deepEqual(
    repeated.map((row) => row.rank),
    [1, 2, 3],
  );
  assert.deepEqual(
    new Set(repeated.map((row) => row.entity_id)).size,
    repeated.length,
  );
  assert.equal(pool[0].rank, 1);
});

test("rejects duplicate identities, stale bytes, invented source and missing ranks", () => {
  const { sources, pool } = setup();
  assert.throws(
    () => verifyGoCandidates([pool[0], pool[0]], sources),
    /duplicate|rank/i,
  );
  assert.throws(
    () => verifyGoCandidates([{ ...pool[0], rank: 4 }], sources),
    /rank/i,
  );
  assert.throws(
    () =>
      verifyGoCandidates(
        [
          {
            ...pool[0],
            file: {
              ...pool[0].file,
              contentHash: sha(Buffer.from("changed")),
            },
          },
        ],
        sources,
      ),
    /hash/i,
  );
  const fake = structuredClone(pool[0]);
  fake.entity.content.text = "made up";
  assert.throws(() => verifyGoCandidates([fake], sources), /source/i);
  const wrongPath = structuredClone(pool[0]);
  wrongPath.file.relativePath = "../other.go";
  assert.throws(() => verifyGoCandidates([wrongPath], sources), /source|path/i);
  assert.throws(() => rerankGoCandidates("query", [], 10), /pool|limit/i);
});
