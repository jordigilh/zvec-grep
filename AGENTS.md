# Repository agent instructions

## Pull-request approval

- Creating a pull request or draft pull request requires explicit approval from
  the user for that specific PR in the current conversation.
- Do not infer approval from a request to commit changes, push a branch, prepare
  a handoff, or document work.
- Before creating a PR, state the target repository, base branch, head branch,
  title, and draft/ready status, then wait for approval unless the user already
  specified those details explicitly.
- This requirement applies equally to PRs targeting the upstream repository,
  the user's fork, or any other repository.

## Code review (model-agnostic)

These rules apply to every reviewer, whatever model runs it and whether or not it
wrote the change.

- **Fresh context is mandatory.** A review must run in a context that did not
  author the diff: a fresh subagent, a new session, or an external reviewer such
  as CodeRabbit. If only a self-review is possible, say so explicitly and treat
  its verdict as weaker; never present it as independent.
- **Adversarial prompt, not an open question.** Asking "is this correct?" invites
  confirmation. The brief must require enumerating failure modes and answering
  each one, in this order, for any concurrency, resource, or lifecycle change:
  happy path, timeout, cancellation before/during/after the work, dropped
  future or abort, a second concurrent caller, late completion after a timeout,
  panic, and a current-thread runtime.
- **Review the delta, plus what it touches.** Review `last-reviewed-commit..HEAD`
  and anything the change interacts with. Do not re-review untouched code, and
  do not skip the interaction points (callers, identity recording, CI wiring).
- **Ground every finding in code.** Cite `file:line` and quote it. Verify against
  the actual dependency source when the claim is about a library's semantics.
  A finding you cannot ground is reported as a question, not an assertion.
- **Severity and domains.** Label each finding `blocker` / `major` / `minor` /
  `nit`, and state the domain: functional correctness, stability and
  availability, data integrity, performance, security, cancellation and
  ownership, test quality, or documentation claims.
- **No silent skips.** Every finding gets a disposition: fixed, rebutted with
  evidence, or accepted as a tracked follow-up with an explicit reason. Report
  the table.
- **Loop to quiet.** Repeat review rounds until a round produces no new
  actionable findings. Only then request the external reviewer (CodeRabbit) and
  repeat the loop on its findings. CodeRabbit's checklist is the floor, not the
  ceiling.

### Misses this repository has actually shipped, and now checks for

- A blocking call (`recv`, `join`, `sleep`) inside an `async fn` — starves a
  current-thread runtime; await with a deadline instead, and never join a thread
  that may be stuck.
- Joining a worker after a readiness timeout — a late but successful start
  leaves the sender alive and the join waits forever; mark the worker abandoned
  and detach.
- Ignoring a closed reply channel — a dropped future releases the admission
  permit while the worker keeps computing; check `is_closed()` per chunk.
- Cancellation checked only at the top of a loop — a cancel during the final
  iteration returns success; check after the work, before publishing it.
- Cancellation ignored while waiting on a lock — a waiter's token must leave the
  other caller's load intact and return without acquiring the result.
- fp32 constants silently promoting an fp16 path — assert output dtypes, not
  just values.
- Tests that pass for the wrong reason — inputs that fp16 can represent
  exactly, assertions that hold for swapped operands, or a test that never
  verifies batching/ownership.
- Documentation that claims what the tests do not do (device pinning, CI lanes,
  "verified" numerics).
- CI lanes that cannot build or link at all on the target they run on.

### Reviewer tooling

- Use the engram MCP tools for semantic code and corpus searches; escalate if
  engram fails instead of silently falling back to plain shell search.
- Never use `sed`, `awk`, `perl`, or `tr` to edit; every change goes through the
  edit/write tools.
- Reviewers are read-only. They report; the author (or the orchestrator) applies
  fixes.
- Every review brief carries the verification commands (fmt, clippy with and
  without the relevant features, tests) and requires the output tails.
