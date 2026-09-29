# Python call-facts producer

This opt-in producer combines Python's standard-library AST with a pinned
Pyright language server. AST traversal supplies complete call-site and enclosing
function/method ranges; Pyright supplies definition bindings. It does not use
Graphify, SCIP, or runtime tracing.

Install the isolated tool dependency once:

```sh
npm install --prefix tools/python-callfacts
```

Generate the sidecar from the repository root:

```sh
python3 tools/python-callfacts/generate.py --root "$PWD"
```

Run the focused producer fixture checks with
`python3 tools/python-callfacts/test.py`.

The default project configuration is `pyrightconfig.json`, then
`pyproject.toml` when present. Use `--project` to select a relative config.
The output is `.zvec-grep/python-callfacts-v1.json` and contains complete
Python source digests, Pyright/Python/typeshed attestation, configuration input
hashes, and UTF-8 byte ranges for calls and enclosing functions/methods.

The bounded classifications are `static`, `possible`, `ambiguous`, `external`,
and `unresolved`. Unbound names, `Any`/`Unknown`, dynamic imports, monkey
patching, runtime decorators, and callable values that Pyright cannot safely
bind remain non-definite. Pyright definition results outside the source root
are recorded as `external`; multiple local definitions are `ambiguous`.

The producer fails without publishing a new artifact when Python source cannot
be parsed or the language server cannot provide a response. Repeated runs are
deterministic for fixed source bytes, Pyright, Python, typeshed, and project
configuration. The graph consumer rejects stale or malformed artifacts and
returns to syntax-derived edges.
