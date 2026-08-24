# Repository Development Workflow

## Authority

`main` is authoritative.

## Executor detection

Detected fresh each session from environment capabilities, never from a
repository-stored flag. See the `rust-repo-lifecycle` skill's
`references/executor-modes.md`.

## Roles

- Planner/reviewer: repository-aware planner, instruction author, PR
  reviewer, correction author, merge gate.
- Implementer: bounded implementer and validator — either the same
  session (Claude mode) or a separate agent a human relays to (Codex
  mode).
- Human (Codex mode only): coordinator who transfers prompts and
  opens/updates PRs.

## Source of truth

- Treat current `main` as authoritative.
- Read `AGENTS.md` and `docs/PROJECT-STATUS.md` plus their routed
  authorities.
- Inspect commits after recorded checkpoints.
- Report conflicts; do not rely on conversation memory over repository
  evidence.

## Outer loop

1. `next` — planner inspects current state and produces one complete
   implementation packet (see roadmap for the next dependency-ready
   unit).
2. (Codex mode: user relays it to Codex.) Implementer implements,
   validates, commits, and reports.
3. PR opened — `PR created`.

## Inner loop

1. Reviewer inspects the actual exact head, diff, scope, authorities,
   tests, docs, threads, and CI.
2. Pass → merge the exact reviewed head.
3. Otherwise → one correction packet; implementer updates the same
   branch; `branch updated`; re-review the new exact head.

## Safeguards

- Never merge failing, pending, missing, stale, or older-head CI.
- Restart review if the head changes.
- Don't begin a competing increment while a PR is active.
- Distinguish code failures from infrastructure/account failures.
- Don't silently expand scope or resolve authority conflicts.
- Ask before anything hard to reverse: `DogRecord` schema changes,
  dependency/toolchain bumps (per the charter's engineering constraints).

## ADRs

Write one per delivery cycle during active major development (this
project's current regime — see `docs/roadmap/ROADMAP.md`); taper to
decisions-that-matter once the baseline (all four `STORAGE-*` units
implemented and `RESULTS.md` published) is stable and complete.

## `next`

Verify merge, refresh `main`, reconcile `docs/PROJECT-STATUS.md`, select
the next dependency-ready roadmap unit.
