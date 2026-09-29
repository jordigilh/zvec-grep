# TypeScript call-facts producer

This opt-in producer uses the pinned TypeScript compiler API to emit
source/context-attested call facts for `.ts` and `.tsx` files. It does not use
Graphify, SCIP, or a language server at runtime. The graph consumer keeps the
sidecar optional and falls back to syntax-derived edges when validation fails.

The tool uses the repository's `typescript` dependency (`5.9.x` in the current
workspace). Run it from the repository root:

```sh
node tools/typescript-callfacts/generate.mjs \
  --root "$PWD" \
  --project tsconfig.json
```

Run the focused producer fixture checks with
`node tools/typescript-callfacts/test.mjs`.

It writes `.zvec-grep/typescript-callfacts-v1.json`. The project file must be
under the root. The sidecar records every TypeScript/TSX source digest, the
compiler/Node versions, selected compiler options, project/config input hashes,
and UTF-8 byte ranges for calls and enclosing function/method identities.
Relative `extends` configs are recursively attested, including arbitrary JSON
names such as `config/base.json`; missing or external extends are rejected.

Supported bounded classifications are `static`, `possible`, `ambiguous`,
`function-value`, `external`, and `unresolved`. A definite target is emitted
only for one local compiler declaration that overlaps a graph function or
method. `any`, `unknown`, unions, overload ambiguity, structural/interface
calls, unsupported declarations, and external libraries remain non-definite.

The producer output is deterministic for a fixed source tree, TypeScript
version, and project configuration. It is an opt-in bounded overlay, not a
claim of complete JavaScript/TypeScript runtime dispatch coverage.
