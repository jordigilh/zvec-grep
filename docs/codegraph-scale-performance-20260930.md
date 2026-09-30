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
runtime, build, and CI dependencies. The interrupted macOS reproduction in
this worktree produced no durable timing report and is intentionally not used
as evidence.
