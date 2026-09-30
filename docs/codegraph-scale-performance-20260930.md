# Large-repository codegraph profile (2026-09-30)

Issue [#25](https://github.com/jordigilh/zvec-grep/issues/25) is a deferred
throughput investigation. The controlled baseline was run on the dedicated
`helios08` host using the source-only Kubernaut Go corpus: 3,125 Go files and
926,775 Go source lines. The host was Linux x86_64 with 40 cores; Graphify was
`graphifyy 0.9.71`; zvec was the debug binary from source commit `1153a05`;
and the Go helper used Go `1.26.7-X:nodwarf5`. Each phase used three fresh
repetitions on the same host.

## Median phase profile

| Phase | Median |
| --- | ---: |
| Graphify code-only extraction | 20.566 s |
| zvec syntax-only graph | 116.018 s |
| Go call-facts producer | 25.416 s |
| zvec semantic graph | 122.677 s |
| semantic end-to-end (producer + graph) | 148.093 s |

Relative to Graphify's code-only extraction, the zvec syntax graph was 5.64×
slower, the semantic graph 5.97× slower, and semantic end-to-end 7.20×
slower. These ratios are diagnostic only: the tools emit different graph
shapes and relation sets, so they are not an accuracy or feature-equivalence
claim.

The profile is a baseline for issue #25, not an optimization authorization.
Correctness, semantic certainty, persistence compatibility, and lifecycle
behavior remain the release gates; Graphify remains local-only and outside
runtime, build, and CI dependencies.

## Current-branch macOS exploratory profile

After the issue #26 validation gates closed, the current branch was profiled on
the available macOS arm64 host. This is a same-host phase profile, not a
replacement for the controlled `helios08` baseline or a cross-host comparison.
The source-only staging root was derived from Kubernaut checkout
`f9adfaeb52da4c844515220c6a676b9e24fad321` (the checkout was dirty), excluding
path components `docs`, `spikes`, `.git`, and `.zvec-grep`, while preserving the
root Go module files and `go:embed` assets. The staged input contained 3,125 Go
files and 927,025 source lines. Each phase used three fresh hard-linked input
trees.

The zvec binary was built from this branch at `862fde9`; its SHA-256 was
`603cfa32c2175d5afe0c0c3c9316d6873d0fa8e548a148e19daa9cd1499c2af6`.
Graphify remained `graphifyy 0.9.71`; the Go toolchain was `go1.26.0` and Rust
was `1.98.1`.

### Plain JSON output

| Phase | Median | Min–max | Output shape |
| --- | ---: | ---: | --- |
| Graphify code-only extraction | 24.107 s | 21.086–26.811 s | 27,600 nodes / 188,174 edges |
| zvec syntax-only graph | 78.090 s | 74.996–82.186 s | 29,112 nodes / 564,692 edges |
| Go call-facts producer | 24.202 s | 17.664–26.432 s | 82,187 facts |
| zvec semantic graph | 91.424 s | 68.628–103.274 s | 29,112 nodes / 564,692 edges |
| semantic end-to-end | 117.856 s | 86.291–127.475 s | producer + semantic graph |

The plain graph artifact was 480,128,021 bytes. Relative to Graphify's local
code-only phase, the plain-output zvec syntax, semantic, and end-to-end phases
were 3.24×, 3.79×, and 4.89× respectively. These ratios remain diagnostic:
the graph shapes and relation sets are different.

### Default compressed output

The default `.zvec-grep/codegraph-v2.json.zst` path was measured separately on
three fresh trees. Compression and the publication path are therefore included
in these timings.

| Phase | Median | Min–max |
| --- | ---: | ---: |
| zvec syntax-only graph | 61.704 s | 60.996–61.746 s |
| Go call-facts producer | 15.274 s | 14.964–16.051 s |
| zvec semantic graph | 63.450 s | 59.014–64.516 s |
| semantic end-to-end | 79.479 s | 74.288–79.502 s |

The compressed graph was 27,712,794 bytes and contained the same 29,112 nodes,
564,692 edges, and 377,790 call edges as the plain-output graph. The plain and
compressed tables should not be mixed when evaluating serialization overhead.

### First hotspot hypothesis

A 20-second macOS `sample` capture during a default syntax build attributed
14,211 of 14,266 main-thread samples to `resolve_relation_edges`. The hottest
children were candidate-name formatting and allocator/reallocator activity.
The current implementation scans all graph nodes for every relation edge before
applying same-file disambiguation; the profile's actionable next experiment is
to pre-index candidates by relation-compatible kind, simple name, qualified name,
and source path while preserving the existing candidate ordering and resolution
classes.

No optimization is included in this evidence update. The next change must be
benchmarked against the lifecycle and real-repository qrels, then rerun through
the same plain/compressed phase matrix before any throughput claim is revised.
