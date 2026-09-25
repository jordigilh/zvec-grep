# scip-go PR #298 revised-head revalidation (2026-09-25)

## Decision

The maintainer's requested changes have been addressed in the upstream PR
branch at **`c1926a380beef870dde29ded54e4f8655bb875f9`**: one declaration
prepass now handles function names as well as types/values/fields, interface
method declaration ranges were added, and the standalone visitor tests were
replaced with the project's snapshot tests plus an existing encoding assertion.
This was done concurrently in the isolated upstream checkout; this spike did
not edit the PR branch or the owning Code IR worktree.

**Result:** the new producer remains compatible with the frozen Go/Code IR
experiment. Every one of its **471 primary site/role/symbol tuples** and every
existing definition enclosing range match the older PR build on identical Go
source. Code IR v1.5 still strict-joins **117/117** local candidates and yields
an identical set of **218** syntax-plus-shadow facts. The independent Go label
replay remains **20/20** correct with **3/3** selected-local abstentions. The
new binary, SCIP output and shadow artifacts have new SHA-256s; do not reuse
the older pinned hashes for a new index.

## Frozen 10-file Go corpus

| Same-source comparison | Earlier PR commit `23715d7` | Revised PR commit `c1926a3` |
|---|---:|---:|
| Producer binary SHA-256 | `eb09bc8ca1315c474308b9f6ca356f4ec744b45f9986519d65319c825b214e0e` (original build, `vcs.modified=true`) | `19dd50f17f0a58733cd5993578708853fff26f9c7fc98c7f98a6d93482c80289` (`vcs.modified=false`) |
| SCIP SHA-256 on the same staged root | `9f073e847663c6859fac865f3b51e8acc838552ffd53b223572b9e127d413b91` | `96b2f7cc078e8c2f31021c4ae3dfd539b82d580cc7d7c0ebf0c418d9226f3614` |
| Selected documents / UTF-8 encoding | 10/10 / 10/10 | 10/10 / 10/10 |
| Occurrences / definitions / references | 471 / 150 / 321 | **Identical complete primary multiset** |
| Valid definition enclosures | 58/150 | **58/150**; 0 added/removed/changed on this fixture |
| Strict local reference joins with Code IR v1.5 | 117/117 | **117/117** |
| Schema/runtime-valid syntax + shadow facts | 218 | **218, same factset SHA-256** |
| Source-labeled links / negative local bindings | 20/20 / 0/3 | **20/20 / 0/3** |

Both staged roots use the same selected-source-set SHA-256
`13ca96abdeb8ef919a746f1ec1afb3ae5b6d59e8b0282d98109bc2b6a8c4f53d`.
The TypeScript Code IR v1.5 snapshot ID is unchanged:
`38924ec8adb36ec929b2a9ac01f3ec09ce786fe8d355fad98e17a42507dc6be6`.
The revised strict shadow SHA-256 is
`9d81bf8a56e023e4ae4dca001130f549937c341d5f8c5df09e20f99cceea0120`.
Both full factsets have SHA-256
`b69865da05a6d4508cd6dd889ecbcff7a0942cb75dc8f718b140e19b783f916b`
after canonical fact-ID sorting, with zero changed/added/removed facts.

The older previously evaluated artifact at
`scip-go-v15-inventory-20260925/go/index.scip` has a third SHA-256
(`994e177b…`) because the binary file can vary between runs/roots. Its
same-source shadow factset, rather than the binary digest alone, was compared
to the new strict shadow.

## Immutable Kubernaut revision

The revised producer also indexed an isolated `git archive` of Kubernaut
`9dcc3816d0eeb75174b86b5f45e4c7d5efc95c17`, excluding the live checkout's
nine unrelated untracked paths. Both producer versions saw the same **1,070**
documents, **13,633,512** selected Go source bytes and selected source-set
SHA-256 `d857f85a7522845e34dd197b08202a5a627c9c0a8c5b10d4bb30638552e5a9b0`.

| Kubernaut producer check | Original custom build | Revised PR build |
|---|---:|---:|
| All primary site/role/symbol tuples | 498,542 | **498,542, same complete per-document multiset** |
| Declared encoding | UTF-8 on 1,070/1,070 | UTF-8 on 1,070/1,070 |
| Valid primary source slices | 498,542/498,542 | **498,542/498,542** |
| Valid definition enclosing ranges | 30,559/30,559 | **30,926/30,926** (+367, 0 removed/changed) |
| SCIP artifact SHA-256 | `7dce2bf1407a8de29c4df0f3896ac64f1b459e5ab5178cbda32608798fb1d85c` | `f882285e4c0c7ac64977701dfb9dac368cecdd0573a1c2af9330068f5a67b1d2` |

The first new enclosures sampled by the per-document comparator are named
`ClusterRegistry` and `MCPResourceClient` interface methods under Kubernaut's
`docs/spikes/` tree, consistent with the PR's interface-method change. The
sample does **not** adjudicate every one of the +367. This Kubernaut run is a
producer/source-map probe, not a Kubernaut Code IR join, relation-precision
estimate, or independently judged retrieval qeval.

## Reproduce and contribution status

The read-only [`scripts/compare_scip_go_revisions.py`](../scripts/compare_scip_go_revisions.py)
compares the full primary-site multiset and enclosing-range deltas per document
on equal source bytes. [`scripts/compare_shadow_factsets.py`](../scripts/compare_shadow_factsets.py)
checks all canonical unit and fact shapes after each shadow separately passed
the snapshot/source/SCIP index gates. The revised producer passed `go test
./...` in its upstream checkout. The paired Go artifact/staged source and
strict shadow are under
`/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/scip-go-pr298-c1926a3-eval/`;
the immutable Kubernaut run is under
`/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/kubernaut-scip-pr298-c1926a3-9dcc3816/`.

At this check, [PR #298](https://github.com/scip-code/scip-go/pull/298) is
**open**, all checks on `c1926a3` pass, and GitHub still reports
`CHANGES_REQUESTED` / `BLOCKED` until the maintainer re-reviews the
responses and new commit. Continue pinning the producer binary used for each
evaluation rather than treating the embedded ToolInfo `0.2.7` as a unique
version. No zvec-grep default or ranking policy changed as a result of this
revalidation.
