# Four-language call-facts benchmark (2026-09-28)

## Scope

This is a source-pinned synthetic comparison of all four language paths against
Graphify's deterministic `--code-only --no-cluster` extractor. Rust,
TypeScript, and Python use the small hand-authored holdouts below. Go uses the
existing larger source-pinned scale fixture so its result includes both the
small holdout comparison and a higher-volume performance measurement.

Fixtures and independently authored truth files are:

- `rust/crates/zg-codegraph/tests/fixtures/go-callgraph-large-20260928/`
- `rust/crates/zg-codegraph/tests/fixtures/rust-graphify-holdout-20260928/`
- `rust/crates/zg-codegraph/tests/fixtures/typescript-graphify-holdout-20260928/`
- `rust/crates/zg-codegraph/tests/fixtures/python-graphify-holdout-20260928/`

The oracle distinguishes definite static calls, possible dispatch, and
non-definite calls. Graphify does not expose those certainty classes, so its
ordinary `calls` edges are counted as definite for static precision/recall and
as unsafe promotions when they occur at a possible or non-definite site.
Possible-target recall uses the fixture's symbol vocabulary; for Rust this is
the nominal trait method, consistent with the producer contract rather than a
claim that the producer enumerates every concrete implementation.

## Reproduction

The reusable Rust/TypeScript/Python runner is
`tools/multilanguage-callgraph-benchmark/benchmark.py`. Its recorded run used
five fresh repetitions per language. Go was measured with
`tools/go-callgraph-benchmark/benchmark.py`, also with five fresh repetitions:

```sh
python3 tools/multilanguage-callgraph-benchmark/benchmark.py \
  --output /private/var/folders/.../zg-callgraph-benchmark-20260928 \
  --repetitions 5 \
  --force
```

```sh
python3 tools/go-callgraph-benchmark/benchmark.py \
  rust/crates/zg-codegraph/tests/fixtures/go-callgraph-large-20260928 \
  --output /private/var/folders/.../zg-go-callgraph-benchmark-20260928 \
  --repetitions 5 \
  --force
```

The outputs were kept outside the repository. Both runners record JSON and CSV
rows for Graphify extraction, zvec syntax-only graph construction, the
language producer, semantic graph construction, and producer-plus-graph
end-to-end time. The Go helper is built once before repetitions; that setup
took 965 ms and is reported separately.

Environment: macOS arm64, Graphify `graphifyy==0.9.71`, Node `v26.7.0`,
Python `3.14.6`, and the zvec debug binary from this working tree. The Rust
producer itself used its pinned rustup `1.98.0` toolchain with `rustc-dev`;
the default `rustc` shown by the host shell was `1.98.1` Homebrew and is not
the producer compiler.

## Results

Static precision and recall are over definite `(caller, target)` pairs. The Go
scale fixture has 2,376 definite static calls across 8,688 source lines and 94
files. The smaller holdouts have Rust 3/5, TypeScript 4/7, and Python 4/8
definite calls/call sites.

| Language | Path | Median wall time | Range | Static precision | Static recall | Unsafe definite sites / dynamic promotions |
|---|---|---:|---:|---:|---:|---:|
| Go (8,688 lines/94 files) | Graphify code-only | 1,216 ms | 1,177–1,365 ms | 0.985 | 0.995 | 12/24 dynamic promoted |
| Go | zvec syntax-only graph | 284 ms | 279–360 ms | 0.990 | 0.990 | 12/24 dynamic promoted |
| Go | zvec call-facts producer | 213 ms | 193–557 ms | — | — | 0/24 dynamic promoted |
| Go | zvec semantic end-to-end | 550 ms | 538–888 ms | 1.000 | 1.000 | 0/24 dynamic promoted |
| Rust (43 lines) | Graphify code-only | 300 ms | 287–397 ms | 0.75 | 1.00 | 1 |
| Rust | zvec syntax-only graph | 45 ms | 38–49 ms | 1.00 | 1.00 | 0 |
| Rust | zvec call-facts producer | 1,310 ms | 1,264–2,489 ms | 1.00 | 1.00 | 0 |
| Rust | zvec semantic end-to-end | 1,353 ms | 1,307–2,540 ms | 1.00 | 1.00 | 0 |
| TypeScript (49 lines) | Graphify code-only | 298 ms | 291–307 ms | 0.60 | 0.75 | 1 |
| TypeScript | zvec syntax-only graph | 44 ms | 37–45 ms | 1.00 | 0.50 | 0 |
| TypeScript | zvec call-facts producer | 418 ms | 406–450 ms | 1.00 | 1.00 | 0 |
| TypeScript | zvec semantic end-to-end | 466 ms | 454–493 ms | 1.00 | 1.00 | 0 |
| Python (55 lines) | Graphify code-only | 294 ms | 268–314 ms | 1.00 | 0.50 | 0 |
| Python | zvec syntax-only graph | 42 ms | 36–52 ms | 0.67 | 0.50 | 1 |
| Python | zvec call-facts producer | 1,276 ms | 1,228–1,603 ms | 1.00 | 1.00 | 0 |
| Python | zvec semantic end-to-end | 1,314 ms | 1,275–1,639 ms | 1.00 | 1.00 | 0 |

Possible-target recall and producer class accuracy add the certainty detail:

| Language / path | Possible-target recall | Producer class accuracy |
|---|---:|---:|
| Go / zvec semantic graph | 144/144 | — |
| Go / Graphify | 12/144 | — |
| Rust / zvec semantic graph | 1/1 | 5/5 |
| Rust / Graphify | 0/1 | — |
| TypeScript / zvec semantic graph | 0/1 | 6/7 |
| TypeScript / Graphify | 1/1 | — |
| Python / zvec semantic graph | 1/1 | 8/8 |
| Python / Graphify | 0/1 | — |

## Interpretation

- **Rust:** the producer and consumer preserve the trait call as possible;
  Graphify binds the same site as a definite concrete `Impl.work` edge. The
  Rust producer timing includes compiling its pinned `rustc_driver` wrapper and
  running Cargo check, so it is not comparable to Graphify extraction alone.
- **Go:** the larger scale fixture reaches 1.000/1.000 with source-attested
  type facts, compared with Graphify's 0.985/0.995. It also preserves all
  144/144 interface candidates and promotes none of 24 dynamic sites. Go's
  helper compilation is excluded from each repetition, unlike Rust's wrapper
  compilation, so producer timing is not directly interchangeable across
  languages.
- **TypeScript:** compiler facts recover both typed receiver method bindings
  that syntax-only misses, with no unsafe definite promotion. The interface
  call is conservatively non-definite rather than emitted as a possible local
  candidate, which explains the `0/1` possible-target recall and the one class
  mismatch.
- **Python:** the producer now queries the LSP at the attribute name rather
  than the receiver, so typed `Alpha`/`Beta` methods resolve statically. A
  direct `Protocol` method definition is recognized from the AST and remains
  possible. This reaches 1.00/1.00 on the fixture without unsafe promotions;
  it is still not a whole-Python accuracy claim.

These results assert that all four producer/consumer paths run, validate
their source/context-attested sidecars, and improve or preserve the expected
certainty behavior on these fixtures. They do not establish repository-wide
accuracy, language completeness, or general performance superiority over
Graphify.
