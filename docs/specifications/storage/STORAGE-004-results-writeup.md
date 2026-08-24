# STORAGE-004 — Results and decision writeup

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-003`
- Supersedes: none

## Purpose and scope

Define the structure and standard of evidence for `RESULTS.md`, the
deliverable that turns benchmark output into a decision-supporting
document.

## Non-goals

Not a place to declare one overall winner across all workloads — see
ADR-0001; a workload-by-workload verdict is the required shape.

## Requirements

- `STORAGE-004-FR-001`: `RESULTS.md` is structured as workload × dataset
  size × backend (a table per workload, columns for the three backends,
  rows for the three sizes, or equivalent).
- `STORAGE-004-FR-002`: Each workload section ends with a short verdict
  specific to that workload — which backend(s) won, by roughly how much,
  and whether the margin changes with dataset size.
- `STORAGE-004-FR-003`: Any workload where the canonical-store approach
  loses to a naive baseline (AoS or SoA) is explicitly and separately
  called out, not buried in a table.
- `STORAGE-004-FR-004`: Any workload where the canonical-store approach
  wins clearly is explicitly and separately called out.
- `STORAGE-004-FR-005`: States which cache-miss measurement path was
  used (per ADR-0002) and whether real counter numbers were obtained in
  this pass or deferred to a follow-up run on other hardware — never
  silently omitted.
- `STORAGE-004-FR-006`: Ends with an "open questions" section naming
  concretely what the benchmarks in this pass didn't settle (e.g.
  write-heavy mixed workloads, memory overhead per backend, behavior at
  the 1M+ boundary, whether pushing to 10M+ changes any verdict).

## Context and terminology

n/a beyond ADR-0001/ADR-0002.

## Architecture and interfaces

`RESULTS.md` at repo root. May be hand-written from Criterion's text/HTML
output, or partially generated — either is acceptable as long as the
numbers reported are real numbers from an actual `cargo bench` run, never
placeholder or estimated values.

## Data/state and invariants

n/a.

## Errors, failure, recovery, and observability

n/a.

## Acceptance criteria

- A reader who has not seen the benchmark code can determine, per
  workload, which backend they'd pick and why, without needing to open
  raw Criterion output.
- No workload's verdict is omitted; no overall single-winner claim is
  made across workloads that didn't agree.

## Verification plan

Manual review: does every one of the four workloads have a verdict; does
the canonical-store win/loss get called out explicitly; is the cache-miss
measurement status stated plainly.

## Traceability

Implements: "Results and decision framework" deliverable. Depends on:
`STORAGE-003`'s actual benchmark run output.

## Open questions

None beyond what `RESULTS.md` itself will surface once the benchmarks
run.

## Change history

- 0.1.0 (2026-08-24): Initial accepted draft.
