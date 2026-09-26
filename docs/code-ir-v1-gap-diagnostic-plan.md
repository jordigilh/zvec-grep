# Diagnosing syntax-index to Code IR v1 retrieval gaps (issue #8)

**Review slice:** [PR #18](https://github.com/jordigilh/zvec-grep/pull/18),
branch `spike/code-ir-v1-gap-diagnostics`, child of the opt-in
[factorial ablation PR #17](https://github.com/jordigilh/zvec-grep/pull/17).
No default ranking change, query-specific boosts or SCIP. The four frozen
fixture sources, manifests, truth and qrels are **read-only**; output belongs
in new directories outside both source checkouts.

## Question, units of observation and stop conditions

Go nDCG@10 drops 0.588774 → 0.489163 and Python 0.546521 → 0.492982
from the existing syntax index to the **eligible** IR v1 view, before the v2
unit policy or combined source metadata. Compare the complete indexed syntax
entity inventory with the published/read-validated IR v2 records (and v1 if
available), then connect candidate identity and source coverage to the existing
five-arm per-query ranked traces. The ranked trace describes top ten hits only:
absence there does not imply a missing indexed candidate. Keep language lanes
separate, and distinguish *exact source-byte equality* from name/path/line
overlap and from actual query relevance.

The syntax index contains 38 Go and 33 Python public entities, equal to the
selected v2 record counts. An initial read-only check found 37/38 Go and 33/33
Python matched by name/path/line **only**, not yet byte-verified. This makes
"too many IR units" an incomplete explanation and motivates exact inventories.
An unvalidated scratch check suggests **27/38 exact Go byte ranges** and
**33/33 exact Python byte ranges** versus the v2 selected units; Go type sites
often differ by a leading `type ` and one `WorkflowState` range is much broader.
The checked, reproducible diagnostic must establish the actual denominators
before interpreting scores.

1. Open each **already built** baseline index read-only via the engine storage
   API. Read the final published v2 sidecar through `readPublishedIR` and verify
   source-set and fixture identity against the raw five-arm run; never edit the
   index/sidecar or rederive an IR index from labels. Require unique identity
   keys and explicit unknown/ambiguous buckets. Compare normalized file path,
   name, kind, source bytes, start/end bytes, line range and actual source text.
   Do not equate a matching line range with matching boundaries.
2. Link each ranked hit by its **entity ID** to the corresponding indexed
   baseline entity or IR record; report unmatched/ambiguous IDs rather than
   guessing. Classify matched versus unmatched candidate hits, per-query
   positive qrel units at @10, and FTS/vector/fusion rank changes. Keep the
   frozen Engram line-overlap mapping unchanged, and explicitly call out its
   enclosing-class/impl credit. Preserve baseline→v1 and v1→policy/text/v2
   comparisons separately; do not infer the cause of an index-wide loss from
   one top-ten trace.
3. Only after inventory diagnostics, consider **another predeclared, paired
   replay** where a common *exact-byte* subset uses identical candidate IDs,
   source windows, FTS/vector text and scorer while changing **one** input at
   a time. If exact identity/byte/window controls cannot hold, report the
   mismatch instead of manufacturing a causal score. Do not tune frozen
   queries, change hybrid fusion or enable a language lane from synthetic qrels.

### Prespecified follow-up after the inventory check

The actual indexed-entity inventory (before any new rank replay) confirms all
33 Python baseline entities match a selected v2 unit by path, name and exact
byte range; the original syntax extractor also reproduces all 33 stored
fragments without embedding-only window overrides. The other languages cannot
make that full-corpus exact-source assertion (Go 11/38 range mismatches; Rust
and TypeScript also have nonmatching or unmatched candidates). Therefore run
**Python only** in one additional opt-in five-arm replay plus three controls on
the *same 33 selected IR units, original source windows and unchanged engine*:

| Control | FTS text | Vector text |
|---|---|---|
| `code-ir-policy-only` | v1 validated record | v1 validated record |
| `code-ir-v1-baseline-fts` | original syntax extractor | v1 validated record |
| `code-ir-v1-baseline-vector` | v1 validated record | original syntax extractor |
| `code-ir-v1-baseline-both` | original syntax extractor | original syntax extractor |

Re-extract the staged baseline source with the existing `CodeExtractor`, **same
indexing token budget**, and verify all reconstructed fragment IDs, source
ranges, content and metadata against the already-built syntax storage; reject
windowed/embedding-only overrides. Build baseline lexical and embedding inputs
using the unchanged index functions; only replace text fields in derivative
IR records. Even if both-text output matches syntax-only, do not attribute
individual token effects or generalize beyond Python; if it does not, inspect
storage ID/tie-breaking/index differences before any ranking change. The
Python source/qrels/scorer/model remain frozen and no default search changes.

## TDD and provenance

Start with failing offline tests for duplicate/ambiguous match keys, missing or
mutated source bytes, name-only or line-only false matches, non-unique entity
IDs, ranked hits absent from indexed inventory, altered provenance, and
misordered query IDs. Add a small exact-byte positive/negative example that
distinguishes an enclosing class hit from a method hit. Report exact match
denominators and ranked-hit classifications **per language**. Pin raw run,
baseline index, sidecar, model/scorer, source and diagnostic implementation
hashes; ensure repeatable output. Independently judged real-Kubernaut retrieval
and negative relationship sites remain missing, so even complete inventory
conformance is not an enablement gate.
