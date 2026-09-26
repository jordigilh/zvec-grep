# Code IR one-pass source metadata (2026-09-26)

## Decision and scope

The TypeScript Code IR frontend now derives **raw, source-anchored** signature
headers and immediately adjacent documentation in the same tree-sitter parse
that creates syntax units. It does not call the legacy extractor for a second
parse, consult a semantic producer, change retrieval-unit selection, or add
generated text to the source evidence. This is the first syntax/frontend
milestone of [issue #8's implementation plan](./code-ir-implementation-plan-issue-8.md),
not a ranking change or a claim of complete declaration coverage.

The frontend version changes from `web-tree-sitter-ir-v1.5` to
`web-tree-sitter-ir-v1.6` so snapshots with different metadata cannot share an
identity. Existing unit/fact IDs remain based on source and are not rekeyed by
metadata alone. Both `signature` and `documentation` store the **literal source
slice** and checked UTF-8 byte `SourceRef`; normalized display strings from
language adapters are not passed off as citations.

Only type/function/method declarations have signature headers. For definitions
with a parser body boundary, the header ends before the body; wrappers such as
Python decorators, TypeScript exports, and Go type declarations are traversed
within the **same** parse. Bounded, one-line bodyless declarations can use
their complete line without a trailing brace or semicolon. A bodyless
multiline declaration with no dependable boundary abstains rather than
claiming a truncated first line is a signature. Function-valued fields and
variable initializers remain IR units but are not mislabeled as signatures.
Adjacent documentation must be separated only by one newline plus optional
indentation; a blank line detaches it. Rust attributes may sit between a doc
comment and its declaration. The raw comment region, including contiguous
comment lines, is retained without cleaning or synthesizing text. Unknown or
oversized (over 1,200 code units) signatures abstain. No Python docstring or
unmapped generated-source comment is inferred by this increment.

## TDD and checks

The new four-language Unicode/CRLF, decorator/attribute/export, detached
comment, type-header, multiline and initializer controls were run **red**
against v1.5 (missing fields or incomplete headers) before the implementation
made them green. The version-gate assertion also failed on v1.5 before the
version bump. The source validator rejects tampered anchored text.
`test/fixtures/code-ir-v1/examples.json` now includes literal signature refs
for its four short conformance examples; these are *not* the frozen Engram qeval
sources, truth or qrels.

After the changes, `npm run build`, 14 focused Code IR/sidecar tests,
`npm run lint`, `npm run format:check`, `npm run typecheck`, 297 unit tests
(296 pass, one pre-existing skip), `npm run code-ir:cross-runtime`, and the
Rust `zg-code-ir` test validating the TypeScript-produced conformance examples
passed. The repository-wide `npm run test:run:root` **did not complete** within
the 120-second command timeout (42 passed before interruption; no assertion
failure was observed); it is not reported as a passing suite.

Read-only extraction of the frozen fixture paths (as selected by each
`manifest.json`) compared this branch with the previously built v1.5 frontend
in `spike/code-ir-design`. These are **emitted-unit metadata counts**, not
independently adjudicated signature/documentation coverage denominators:

| Language | Files | Units | Facts | Units with signature | Units with documentation | Unit/fact ID order vs. v1.5 |
|---|---:|---:|---:|---:|---:|---|
| Go | 10 | 68 | 101 | 38 | 5 | unchanged |
| Python | 8 | 53 | 91 | 33 | 0 | unchanged |
| Rust | 14 | 63 | 146 | 36 | 0 | unchanged |
| TypeScript | 8 | 46 | 71 | 33 | 0 | unchanged |

Every emitted ref is checked against original source bytes during extraction
and again by `validateSnapshot`; source and qeval fixtures were read, never
modified. The v1.6 snapshot identity differs from v1.5 in each lane as
intended. No paired retrieval qeval was run for this increment: the metadata
exists in canonical IR, but no default projection uses it yet.

## Follow-up gates

1. Independently label signature and comment spans/kinds for each language,
   including negative, generated/unsupported and multiline cases. Quantify
   missing metadata and examine cases that abstain; emitted refs alone are not
   coverage.
2. Design a versioned retrieval-unit/text policy that consumes these refs
   without making fine-grained semantic units disappear. Replay the unchanged
   per-language qrels and compare against the same-engine baseline; Go and
   Python previously failed their aggregate retrieval gates.
3. Build independently judged real-Kubernaut retrieval cases before making
   a real-repository quality or default-enablement claim. Semantic producers
   remain optional and out of this milestone.
