# Go SCIP / Code IR correctness and freshness (2026-09-25)

## Frozen Go lane: independent source evidence

The frozen Go qeval corpus is the same 10-file, 38-unit
`go-workflow-discovery-v1` fixture used by the retrieval evaluation. A new
independent inventory generated with Go 1.26.0's standard `go/parser` records
declaration byte ranges, identifier byte ranges, kinds and source lines for all
38 manifest units. The generator does not consume SCIP or tree-sitter output.
Comparing it with the committed Code IR frontend `web-tree-sitter-ir-v1.5`
produced:

| Measure | Result |
|---|---:|
| Exact manifest units matched by file + byte span + kind | **38/38** |
| Identifier token slices round-tripped | **38/38** |
| Missing / ambiguous / span-or-kind mismatches | **0 / 0 / 0** |
| Code IR total units / files | 68 / 10 |

The AST inventory source-set SHA-256 is
`94183663d9e5f81b753dde7c460428c2414c46867b936b5cc42f9827dab0827f`, exactly
the frozen qrels source digest. Reproduce with:

```sh
go run scripts/go_ir_source_inventory.go \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --output /tmp/go-source-inventory.json
python3 scripts/compare_go_ir_inventory.py \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --inventory /tmp/go-source-inventory.json \
  --ir /path/to/staged-root/syntax-ir-go.json
```

The fixture’s original five Go relation labels remain intact. A second,
independently source-authored Go inventory adds 15 call/type-reference sites
for **20 total**. Against the custom producer and gopls, all 20/20 sites reached
the labeled exact target. The label inventory SHA-256 is
`4bb2a47ee2aa5cf0a9818abf90f716ab1fb3ec6c12e99d687a6d0bcb370b8efb`.
Separately, 31/31 eligible, unique cross-file SCIP candidates agreed with
gopls; **14/45** cross-file candidates were omitted because package-symbol
targets had multiple definitions. Those 31 are LSP agreement, not manually
adjudicated precision. No implementation relationship was emitted by this
Go fixture.

## Custom scip-go build and strict join

The custom build used for the spike is pinned by executable SHA-256
`eb09bc8ca1315c474308b9f6ca356f4ec744b45f9986519d65319c825b214e0e` (upstream
base `343fa9ce00444421071d3659443a78cbfab3291f` plus the PR change). A fresh
index on unchanged fixture bytes:

- indexed all 10 selected documents and declared UTF-8 encoding in each;
- emitted 471 primary occurrences; the paired stock-versus-patch comparison
  found the same 471/471 primary site tuples as published scip-go;
- emitted 58/150 valid definition enclosing ranges;
- strict-joined 117/117 selected-local reference candidates to Code IR v1.5;
- retained all five original source-authored Go links.

The index SHA-256 for the fresh v1.5 run is
`994e177b83353f9f5c3a1b7bc45a9f7a8171aeb66ddde463acb3575222b55e82`. Counts
cover this fixture’s producer population only; they do not establish precision
for all 321 raw references.

## Dirty-snapshot check with the custom producer

In a disposable copy of the staged fixture, only
`internal/discovery/state.go` changed (`WorkflowState.Contains` body); the
referring `internal/selection/validator.go` SHA-256 stayed
`53dd2664b62da582a5d4710bc5e10848feff895c109ae3832ca41a7853bddef1`.

- Selected source identity changed from SCIP digest
  `13ca96abdeb8ef919a746f1ec1afb3ae5b6d59e8b0282d98109bc2b6a8c4f53d` to
  `ae5cf743a712339f074606bde9c711c534f3ef3c65fed6c0eb3f6e32de2f3ef3`.
- Applying the old SCIP artifact to the new source snapshot was rejected with
  `stale full selected source set`.
- The custom scip-go binary indexed the dirty copy successfully. Its fresh SCIP
  artifact SHA-256 is
  `d6950103e5d4d842ea4646ffbb49a28ac933bd7585706ac1f9548bb21ad9ac67`;
  all 10 documents declared UTF-8, and all 472 primary ranges and 58 present
  definition-enclosing ranges round-tripped against the dirty bytes.
- A fresh Code IR snapshot for the dirty copy also matched all 38 independent
  AST declaration spans and kinds.

The fixture-pinned SCIP recheck rejected both the old and the freshly generated
dirty index with `stale full selected source set`: that importer is deliberately
bound to the frozen qeval source digest. The fresh producer output was validated
for complete file coverage, declared encoding and source-byte validity, not
promoted as a fixture-independent semantic join. A manifest-driven dirty
snapshot importer and dependency-complete attestation remain future work.

Reproduction is `python3 scripts/scip_go_freshness_probe.py` with the clean
staged root, Code IR worktree and custom binary paths. It copies and changes
only a temporary staging tree; the Engram fixture and Kubernaut checkout were
not modified.

## Upstream PR

[scip-go PR #298](https://github.com/scip-code/scip-go/pull/298) remains open.
All reported checks, including `hugo`, have now passed. No maintainer review is
posted yet; GitHub still reports `REVIEW_REQUIRED` / `BLOCKED`. The local custom
binary remains the evaluation producer until upstream merges or otherwise
addresses the contribution.

## Immutable Kubernaut Go corpus probe

The custom binary was also run on an archive of Kubernaut commit
`9dcc3816d0eeb75174b86b5f45e4c7d5efc95c17`. The live worktree contained nine
untracked paths; the probe staged only the commit archive and left the live
tree unchanged.

| Measure | Result |
|---|---:|
| Selected SCIP documents | 1,070 |
| Selected Go source bytes | 13,633,512 |
| Source-set SHA-256 | `d857f85a7522845e34dd197b08202a5a627c9c0a8c5b10d4bb30638552e5a9b0` |
| Occurrences | 498,542 |
| Primary byte spans round-tripped | **498,542 / 498,542** |
| Declared document encoding | UTF-8 in all documents |
| Definitions with enclosing ranges | 30,559 |
| Valid definition enclosing spans | **30,559 / 30,559** |

The index SHA-256 is
`7dce2bf1407a8de29c4df0f3896ac64f1b459e5ab5178cbda32608798fb1d85c`. This
is broad producer/source-map coverage on Kubernaut, **not** a relationship
precision or retrieval qeval: no adjudicated Kubernaut qrels were applied.
The detailed probe and archive are under
`/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/kubernaut-scip-probe-9dcc3816-20260925/`.
