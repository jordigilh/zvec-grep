# Semantic call-facts validation

This is a local-only validator for independently authored semantic call-facts
witness qrels. It is outside zvec runtime, build, and CI dependencies. The qrel
files pin a checkout revision, witness source digests, producer schema/version,
sidecar bytes, context fingerprint, and selected producer/toolchain fields.

The qrels are bounded witness sets, not whole-repository accuracy claims. They
score normalized resolution classes (`static`, `possible`, `function-value`,
`external`, and `unresolved`) and exact source targets for selected static
calls. The producer sidecar's complete source and context file lists are also
re-hashed before scoring.

## Validation

Generate a sidecar outside the source checkout when possible, then run the
matching qrel. `--root` is the source tree used to generate the sidecar;
`--revision-root` may point at the original Git checkout when the source tree
is a read-only staging copy.

```sh
python3 tools/codegraph-semantic-benchmark/validate_callfacts_qrels.py \
  --root /path/to/staged-root \
  --revision-root /path/to/checkout \
  --qrels tools/codegraph-semantic-benchmark/real-qrels/<name>.json \
  --sidecar /path/to/staged-root/.zvec-grep/<sidecar>.json \
  --output /path/to/report.json
```

The producer remains opt-in. Use the language-specific commands documented in
`tools/{go,rust,typescript,python}-callfacts/README.md`; do not add those
toolchains or Graphify to normal application dependencies.
