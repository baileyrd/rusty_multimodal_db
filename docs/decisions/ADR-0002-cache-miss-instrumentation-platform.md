# ADR-0002: Cache-miss instrumentation via a Linux-only, feature-gated Criterion measurement — not attempted on Windows

- Status: Accepted
- Date: 2026-08-24
- Deciders: baileyrd
- Related: `docs/charter/CHARTER.md`, ADR-0001, `STORAGE-002`
- Supersedes/Superseded by: none

## Context

The hypothesis under test is specifically about memory locality. Wall-clock
Criterion numbers alone can mislead here — a backend can win on wall-clock
time for reasons unrelated to cache behavior (allocator noise, branch
prediction, turbo-boost variance) while still being the worse design for
the access pattern actually being tested. Cache-miss *counts* are the more
direct signal, and the standard way to get them on Linux is `perf stat`
(hardware performance counters via the kernel's `perf_event_open`
syscall), which the `criterion-perf-events` crate wraps as a Criterion
measurement plugin.

Two platform facts complicate this:

1. The owner's primary dev machine is native Windows, not WSL2. `perf` is
   a Linux kernel interface; it does not exist on native Windows.
2. Hardware performance counters require either bare-metal access or a
   hypervisor that passes through the vPMU (virtual performance monitoring
   unit) to the guest. This repo was bootstrapped from a cloud development
   session running in a virtualized/containerized Linux environment.
   Verified directly in that environment before writing this ADR:

   ```
   $ perf stat -e cache-misses,cache-references,instructions,cycles -- /bin/true
    Performance counter stats for '/bin/true':
       <not supported>      cache-misses
       <not supported>      cache-references
       <not supported>      instructions
       <not supported>      cycles
   ```

   All four counters report `<not supported>` — the hypervisor is not
   exposing PMU access to this guest. So even though this session runs on
   Linux, it cannot produce real hardware cache-miss numbers either. This
   rules out "just run it here" as an option, and confirms the platform
   gap is real, not hypothetical.

The owner named a specific bare-metal Fedora Server machine (`baileyai`)
as available for this purpose.

## Decision drivers

- Don't silently drop cache-miss measurement — the task is explicit that
  this must be flagged, not quietly skipped.
- Don't fabricate or guess at cache-miss numbers from an environment that
  can't produce them; a plausible-looking made-up number is worse than an
  honest "not measured here."
- Wall-clock Criterion benchmarks must still run everywhere (Windows,
  this cloud session, `baileyai`) — cache-miss instrumentation is an
  addition, not a gate on the rest of the suite.
- Minimize dependencies and platform-specific code paths; don't build a
  custom ETW-based Windows solution when there's no evidence it would be
  reliably usable without elevated privileges and a much larger
  implementation (`xperf`/ETW hardware-counter tracing is not a stable,
  well-trodden path for a small benchmark crate the way `perf_event_open`
  is on Linux).

## Considered options

1. **Build a Windows-native ETW/hardware-counter path.** Investigated and
   rejected for this pass. Windows exposes CPU performance counters
   through ETW's `PerfInfo`/hardware-counter providers, but consuming
   them programmatically from a benchmark crate requires either
   administrator privileges plus a non-trivial ETW session setup, or a
   third-party profiler (Intel VTune, Windows Performance Analyzer) run
   out-of-process — not something `cargo bench` can drive directly the
   way `perf stat -- <binary>` can. This is a real capability gap, not
   just inconvenience, and building it would be disproportionate to a
   benchmark harness whose job is to answer one storage-design question.
2. **Skip cache-miss measurement entirely, ship wall-clock only.**
   Rejected — this is exactly the "silently skip" outcome the task rules
   out, and wall-clock alone is the weaker signal for a
   locality-sensitive hypothesis.
3. **Gate cache-miss measurement behind a Cargo feature, using
   `criterion-perf-events` (wraps Linux `perf_event_open`), Linux-only by
   `cfg`, documented as needing to run on real hardware (`baileyai`) —
   and keep the wall-clock suite fully cross-platform.** Chosen.

## Decision

- The wall-clock Criterion benchmark suite (`benches/workloads.rs`) has no
  platform gate and runs on Windows, this cloud Linux session, and
  `baileyai` alike.
- A second, feature-gated benchmark target uses `criterion-perf-events`
  (`cache-misses` / `cache-references` hardware events via
  `perf_event_open`) behind a `perf-events` Cargo feature, itself gated to
  `cfg(target_os = "linux")`. It is not part of the default `cargo bench`
  run.
- This crate does not attempt to produce real cache-miss numbers from
  within this bootstrap session, because this environment's own `perf
  stat` run above shows the hardware counters are not exposed here. The
  feature is built and its invocation documented in `RESULTS.md`/README
  as something to run on `baileyai` or another bare-metal Linux box the
  owner controls; results from that run, once available, get folded into
  `RESULTS.md` as a follow-up.
- No Windows-native equivalent is built in this pass. If the owner later
  needs cache-miss numbers gathered directly on the Windows dev machine,
  that's a separate, explicitly-scoped follow-up (see open questions in
  `RESULTS.md`), not something this ADR commits to.

## Consequences

### Positive

- Honest about what could and couldn't be measured from within this
  session — no fabricated numbers.
- The rest of the suite (dataset generator, three backends, wall-clock
  benchmarks) stays fully cross-platform and useful on its own.
- The `perf-events` feature is ready to run as-is on `baileyai` with no
  further code changes — just `cargo bench --features perf-events
  --bench cache_events` on that machine.

### Negative / tradeoffs

- `RESULTS.md` from this session reports wall-clock numbers only; the
  cache-miss numbers this hypothesis most needs are deferred to a
  follow-up run on hardware this session doesn't have access to. This is
  flagged explicitly in `RESULTS.md`'s open questions rather than
  presented as a completed measurement. **Resolved**: obtained on
  `baileyai` via PR #3 — see the "Validation and revisit triggers"
  update below and `RESULTS.md`'s cache-miss section.
- No Windows-native cache-miss path exists, so the owner's primary dev
  machine can't self-serve this measurement without either using
  `baileyai` or WSL2 (which the owner has already moved away from) or a
  third-party Windows profiler outside this crate's scope.

## Validation and revisit triggers

- Validated by: `cargo bench --features perf-events --bench cache_events`
  succeeding and reporting real (non-`<not supported>`) counter values
  when run on `baileyai` or equivalent bare-metal Linux.
  **Done**: run on `baileyai` (PR #3, `fe59233` → merge commit `ec67ba3`)
  — real hardware counters, all four backends, all four workloads. One
  operational note for future runs on the same box: `perf_event_paranoid`
  was `2` by default, which fails every counter with a `PermissionDenied`
  error; `sudo sysctl -w kernel.perf_event_paranoid=1` (a session-only,
  non-persisted write) was sufficient. Results and the `scan_ages`
  finding they resolved are in `RESULTS.md`'s cache-miss section.
- Revisit if: the owner wants Windows-native counters badly enough to
  justify the ETW investigation this ADR declined to do now.
- Revisit if: `baileyai` also turns out not to expose PMU access (e.g. if
  it's itself virtualized) — in that case the cache-miss question would
  need a different resolution and should come back to this ADR rather
  than being silently dropped again. **Did not occur** — `baileyai` had
  working PMU access once `perf_event_paranoid` was relaxed.
