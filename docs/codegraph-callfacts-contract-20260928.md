# Language-neutral semantic callfacts contract

This is the common contract for optional, source-attested callfacts consumed by
`zg-codegraph`. It is a compatibility contract, not a request to merge the
existing Go sidecar or to make compiler tooling a runtime dependency.

## Envelope

Each language producer writes a versioned sidecar with:

```text
schema, version
context, context_sha256
files: [{path, sha256}, ...]
calls: [{path, range, caller, target_name, target,
         possible_targets, resolution}, ...]
```

- `path` is a normalized, root-relative path using `/` separators.
- `files` is the complete source-file set for that language in the graph
  snapshot. Each digest is SHA-256 over the exact source bytes.
- `context` records the producer/toolchain/build inputs that can change
  resolution. `context_sha256` is the producer fingerprint over the canonical
  context encoding and its context-file digests.
- A symbol locator is `{path, start_byte, end_byte}`. It is a source locator,
  not a persisted graph ID; the consumer joins it to the current graph only
  after validating the source snapshot.

The call range is half-open and uses UTF-8 byte offsets, 1-based lines, and
0-based UTF-8 byte columns. The caller locator encloses the observed call.
`target_name` retains the source spelling even when no target can be bound.

## Resolution classes

The consumer maps language-specific labels onto these certainty classes:

| Class | `target` | `possible_targets` | Meaning |
| --- | --- | --- | --- |
| `static` | exactly one | empty | Producer attested one local source declaration. |
| possible/ambiguous | absent | zero or more | Call may reach candidates, but no definite target is claimed. |
| `function-value` | absent | empty | Call goes through a value, closure, or other indirect callable. |
| `external` | absent | empty | Target is outside the attested source snapshot. |
| `unresolved` | absent | empty | Producer could not safely classify the call. |

The current producers retain useful language-specific labels: Go v2 uses
`interface-dispatch`; Rust v1 uses `trait-dispatch`. Both are possible edges in
the graph query and never become definite callers. The bounded TypeScript and
Python producers use `possible`/`ambiguous` and preserve the same target-shape
invariants.

## Consumer and fallback rules

1. Validate schema/version, the complete language source set, every source
   digest, context-file set/digests, context fingerprint, symbol locators, and
   call coordinates before committing an overlay.
2. A valid fact replaces only the matching syntax call site. Definite facts enter
   the resolved graph; possible candidates enter a separate possible graph;
   indirect, external, and unresolved facts remain non-definite.
3. A missing fact leaves the syntax edge in place. A malformed, stale, or
   internally inconsistent sidecar rejects the whole overlay for that language
   and restores syntax-derived edges. Graph construction and queries continue
   without the producer.
4. Query results expose the applied context fingerprint and keep definite
   callers separate from possible callers. A resolved syntax-only name match is
   not a type-certainty claim.

## Schema compatibility

The existing `zvec-grep.go-callfacts` v2 shape remains unchanged. Rust,
TypeScript, and Python use separate v1 sidecars while the contract is exercised
by the consumer: `zvec-grep.rust-callfacts`,
`zvec-grep.typescript-callfacts`, and `zvec-grep.python-callfacts`.
Language-specific context fields remain inside their sidecars; the graph
artifact exposes optional per-language fingerprints so old Go v2 artifacts and
queries remain readable.

## Deliberate non-goals

This contract does not promise runtime dispatch, macro/generated-source
expansion, closure identity, async lowering, function-pointer targets,
unsupported generic resolution, external dependency source identities, or
cross-toolchain portability. Producers must abstain rather than turn those
cases into definite edges. Semantic overlays are opt-in and do not alter search
ranking or require Graphify at runtime.
