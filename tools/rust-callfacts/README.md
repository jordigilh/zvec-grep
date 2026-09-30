# Rust call-facts producer

This is an opt-in, compiler-attested Rust call-facts producer. It uses the
matching `rustc_driver`/HIR APIs from a pinned rustup toolchain; it does not
invoke Graphify, SCIP, or rust-analyzer at runtime.

The language-neutral envelope and certainty rules are documented in
[`docs/codegraph-callfacts-contract-20260928.md`](../../docs/codegraph-callfacts-contract-20260928.md).
Rust keeps its own `zvec-grep.rust-callfacts` v1 sidecar so the existing Go v2
sidecar remains compatible.

## Prerequisites

The producer must be built and run with the same compiler and compiler
libraries. For the repository toolchain:

```sh
rustup component add rustc-dev --toolchain 1.98.0-aarch64-apple-darwin
```

The script never mixes the Homebrew compiler with rustup's private compiler
libraries. `rustc-dev` is not a normal application dependency and is not
vendored by this repository.

## Generate an opt-in sidecar

From the repository root:

```sh
tools/rust-callfacts/generate.sh \
  --root "$PWD" \
  --manifest-path "$PWD/rust/Cargo.toml"
```

The command runs `cargo check --workspace --all-targets` through the temporary
`rustc_driver` wrapper and writes:

```text
.zvec-grep/rust-callfacts-v1.json
```

Use `--no-all-targets` for a narrower check or pass additional Cargo arguments
after `--`. The sidecar is optional; remove it or let validation reject it to
return to syntax-derived Rust edges.

## Contract and conservative classifications

The sidecar records:

- source SHA-256 digests for every Rust file in the graph snapshot;
- hashes of Cargo manifests/lockfiles, rust-toolchain files, and Cargo config;
- rustc version/commit, host, target, edition, and relevant build settings;
- exact UTF-8 byte ranges for calls and enclosing function/method identities;
- `static`, `trait-dispatch`, `function-value`, `external`, and `unresolved`
  classifications.

Direct free-function and inherent-method targets are emitted as definite only
when their local source declaration is attested. Trait declarations are
emitted as possible targets, not concrete implementation sets. Function
values, closures, macros, generated code, external declarations, and any
range that cannot be mapped back to the current source snapshot remain
unresolved or are left to the syntax fallback.

The graph consumer validates schema/version, all source hashes, the complete
context-file set, context fingerprint, caller/target source identities, and
call coordinates before replacing a syntax edge. Invalid or stale facts are
ignored without making graph queries fail.

For a quick producer/consumer smoke check, first build the CLI and then run the
producer against a small Cargo crate. Querying the resulting
`.zvec-grep/codegraph-v2.json.zst` reports `static` calls in
`callers_by_depth`, while trait-dispatch candidates remain in
`possible_callers_by_depth`.

`rustc_driver` and HIR APIs are compiler-private and version-coupled. This is a
bounded MVP for the pinned toolchain, not a portability or broad Rust accuracy
claim.
