# Controlled v2: prospective, discriminating source retrieval

**Decision:** The local hybrid vector+FTS → unit ID → source-verified IR
follow-up works as a controlled, opt-in evaluation path, but the four lanes do
**not** justify replacing or reranking default syntax search. The new
[precommitted plan](./code-ir-blind-synthetic-v2-plan.md) was followed in order:
the versioned checker and @1/@3/@10/MRR metrics were committed at `3469cc9`
*before* the new source/byte-truth and seven questions per language were
committed at `3b309ce`. One paired same-engine run per language used those
frozen inputs and no subsequent source, label, query, scoring or ranking edits.

Unlike the prior five-entity pilot, all four new lanes have **18–19 syntax
entities and 18–19 independently projected IR units**, so a top-ten miss is
possible. Go supplies one: `route-membership` is a source-verified unit in
both candidate stores, but is absent from *both* top tens. It therefore has no
syntax-discovered candidate to expand. The other lanes find every labelled
answer at @10. The two search arms tie on every Python query; Rust's held-out
`retry-membership` loses rank 1→2 under IR; TypeScript improves the stand-alone
decoy rank 4→1 but worsens the route rank 2→3. Neither source conformance nor
one TypeScript improvement overrides the per-query losses and Go miss.

## What the experiment actually did

- New `test/fixtures/code-ir-controlled-v2/{go,python,rust,typescript}/`
  source/truth: three independently source-authored files per lane, 20–21
  exact-byte labelled sites, twelve plausible lexical decoy declarations,
  same-named method/free-function lookalikes across files, two calibration and
  five prospective held-out questions. The sites and relations were specified
  from literal source spans and SHA-256s **before search**; they were not
  generated from parser output, ranked hits or qrels from the older fixtures.
- Syntax-only and published/read-validated v2 projection are separate on-disk
  ZVec indexes using the **same unchanged** local CPU model, FTS/vector
  routes, fusion, ten-result limit and source files. Syntax answers are judged
  directly against authored byte sites; label-blind syntax→IR grounding is
  scored *separately*, with ambiguity/missing mapping abstaining rather than
  hiding syntax discovery. IR projection record IDs map back to attested unit
  IDs and original source; lexical/vector prefixes are not citations.
- The offline post-discovery arm retains syntax rank, checks published snapshot
  and file hashes, then exposes only observed `contains` evidence. Calls are
  reported as **unresolved source observations** with spelling, not bound to
  a same-named target. Every emitted answer/fact citation round-tripped to the
  pinned original bytes. This is an evaluation adapter, **not** a newly wired
  default product API or an IR graph reranker.

| Language | Syntax / IR units; grounded | Calibration: syntax vs IR @1/@3/@10; MRR | Holdout: syntax vs IR @1/@3/@10; MRR | Per-query exact answer rank (syntax→IR; `—` = outside @10) |
|---|---|---|---|---|
| Go | 18 / 18; 18 | 1/2/2 vs 1/2/2; .750 vs .750 | 3/4/4 vs 3/4/4; .700 vs .700 | persist 2→2, shape 1→1, **route —→—**, retry 2→2, decoy 1→1, tenant 1→1, empty 1→1 |
| Python | 19 / 19; 19 | 1/2/2 vs 1/2/2; .750 vs .750 | 4/4/5 vs 4/4/5; .850 vs .850 | persist 2→2, shape 1→1, route 4→4, retry 1→1, decoy 1→1, tenant 1→1, empty 1→1 |
| Rust | 19 / 19; 19 | 0/2/2 vs 0/2/2; .333 vs .417 | 4/5/5 vs 3/5/5; .867 vs .767 | persist 3→2, shape 3→3, route 3→3, **retry 1→2**, decoy 1→1, tenant 1→1, empty 1→1 |
| TypeScript | 18 / 19; 18 | 1/2/2 vs 1/2/2; .667 vs .667 | 3/4/5 vs 4/5/5; .750 vs .867 | persist 3→3, shape 1→1, **route 2→3**, retry 1→1, decoy 4→1, tenant 1→1, empty 1→1 |

`IR units` counts **distinct projected units**, not TS's extra projection
window. All languages pass the prespecified >10-candidate pool gate. There
are zero *judged* missing IR sites or facts and zero false links in the
labelled cases. The four positive containment facts are source-attested;
**seven** unresolved call occurrences were discovered and correctly abstain
from target binding. The eighth (Go route) exists in the IR but is not
discoverable at @10 from either arm; it is **not** a successful follow-up.

### What this does not establish

1. The held-out questions were fixed before the first replay and not used to
   tune code/text/labels afterward, but this is still a *small, synthetic,
   author-judged* suite with related scenarios across languages, not
   independently adjudicated repository-scale quality. Per-language results
   are not an independent four-repository macro-average. The previous
   Go/Python frozen workflow qeval regressions still stand.
2. The promised natural ambiguous-grounding example did **not** materialize
   in the indexed v2 lanes (all candidate entities grounded under the narrow
   shared-end-byte policy). Offline tests force an ambiguous same-name/range
   collision and verify independent syntax grading plus IR abstention, but
   a separately frozen real-source collision is still needed. Same-name
   cross-file calls are observed only, never type-resolved. No data-flow,
   verified callee, compiled Rust behavior or graph-ranking result is claimed.
3. A source-backed relationship is useful *after* discovery. It cannot repair
   the missing Go @10 candidate, and the IR-only top ten is not consistently
   better than syntax-only. Keep default syntax search as fallback; a product
   opt-in combined query would still need atomic search-index/IR snapshot
   coordination, stale-source handling, user-facing citation shape and
   operational tests. Real-repository external validity comes later.

## Reproduction and pinned artifacts

Run after `npm run build`, with a fresh separate work/output pair **outside
both checkouts** for each language. Example:

```sh
python3 scripts/replay_code_ir_controlled.py \
  --fixture test/fixtures/code-ir-controlled-v2/go \
  --model-cache /Users/jgil/.engram/zvec-grep/model \
  --work-dir /path/to/new-go-work \
  --output-dir /path/to/new-go-results
```

On this host the first paired runs and manifests reside in the approved
temporary directory
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`
under `code-ir-blind-v2-{language}-{work,results}-20260926/`. Temporary
artifacts can be cleaned; the versioned runner and the committed source/truth
are the reproduction path. At run time, implementation revision was
`3b309ce4570e6f6822e623fbab13f195a6714cbf` and the same local model
cache tree SHA-256 across lanes was
`9eccd62fe5ce1815096be4b5dff78708f27c686c0c70305017f23984e0524252`,
verified unchanged before and after each run. Each manifest also pins
implementation/dist module hashes, source set, snapshot, CPU model identity,
raw results and exact-byte scorer output.

| Language | Source set SHA-256 | Truth SHA-256 | Raw search SHA-256 | Strict result SHA-256 |
|---|---|---|---|---|
| Go | `f54166861d4d459f61fb3c7e5a628e2fededeebfc07fcf64a10c83ff30b4c822` | `ac3d5d35b437df272c6bc28fd0546430c582cde6103c8fc10977d0b21d4b77cd` | `dfceffb8a4f0366ba24898527a6f8f14b35c10a8c7a6bd03521ca0217125ae02` | `2da6d7cd618b72dfbe0ebdfbaef5166bbb2eac1305c8f5640f6b8482bdf1c995` |
| Python | `444917ab34d3bb4f829ed0cfc05351a16df7628016d6af416c1999da20798b58` | `3f1db1b0d36a6424de78d59c79bbab0016667a866b06d4f2c9aac68eb4b95af5` | `579f3f18b97f16543f43fdd442372e52725191a2da1595fb8f65422dde3a1176` | `9682346e8583496236c8a96321058994a903e790a05187e6ea0c4ec49e169120` |
| Rust | `742ef1b8f3364b2888d57c9b86370a9a70b425cdc40745511f2a2e6cf92027bb` | `61352ac56258b045acc150df2521174ea223dd6c3aeea38b2ff5e7eaa514d6a4` | `8117566611c9a3e8cfc1f76cb81f7382446bf7241045afdb45215462f1d321b9` | `4e59cca81eda9f25f3c72a4280c81934d3de904808ddc98f709d759cc564f637` |
| TypeScript | `06402f489edb1e3a14e50fa52dd75f19b564e16ce146ec0b05ed89db62510f46` | `8ed8b5b7c4d0a45a28900da932248a07e5b8d00e5bf6b96520b3151f7f3d5498` | `10944fbc12ece27e68f39ede47ebf440bdfd3924c73d3e310600bbd976a60e6f` | `ec099dfff9d6daab3351cefae5e84e557e188506336a9bf0eb1ae22b01f78c26` |

Build/typecheck/lint/format passed; JS unit suite **333 passed, one skipped**;
ten Python offline runner/qeval tests passed. A source-only rescore of the
older controlled-v1 raw runs preserved all prior exact ranks and relation
statuses after the grader separation. Broad root/integration/e2e/coverage
suites and compiled Rust execution were not run. No Engram fixture was used or
modified by this replay.
