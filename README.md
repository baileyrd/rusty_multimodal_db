# rusty_multimodal_db

A benchmark harness, not a database. It exists to empirically test one
storage-design hypothesis before anyone commits engineering time to
building around it:

> A canonical store, keyed by UUID, can serve as the single source of
> truth for row-oriented, column-oriented, and (eventually)
> graph-oriented access — with row/column/graph implemented as **views**
> over that one canonical store rather than as separate physical copies
> of the data.

It's tested against two conventional baselines, all three behind the same
`DogStore` trait so results are directly comparable:

- **AoS** (array of structs) — `Vec<DogRecord>`, row-oriented.
- **SoA** (struct of arrays) — parallel `Vec<Uuid>`/`Vec<String>`/`Vec<u32>`,
  column-oriented.
- **Canonical** — `HashMap<Uuid, DogRecord>` as the only physical copy,
  with column-scan and one-hop-lookup access implemented as derived views
  over it.

See `docs/charter/CHARTER.md` for the full framing, `docs/decisions/` for
why the comparison is structured this way, and (once benchmarks have run)
`RESULTS.md` for the numbers and the verdict per workload.

## Status

Bootstrap in progress. See `docs/PROJECT-STATUS.md` for the current
checkpoint and `docs/roadmap/ROADMAP.md` for what's next.

## Repo name

This repo is named `rusty_multimodal_db` on GitHub. `docs/charter/CHARTER.md`
records a naming discrepancy worth knowing about: the task that seeded
this repo suggested `rusty_multimodel_bench` (multi-**model** access —
row/column/graph — not multimodal *data*) as a less ambiguous name once
the benchmark's shape was clear.

## Using this repo

```sh
cargo test --all-features            # unit tests
cargo bench                          # wall-clock Criterion suite (cross-platform)
cargo bench --features perf-events --bench cache_events   # cache-miss counts, Linux bare-metal only — see ADR-0002
```

See `AGENTS.md` for the full canonical command set and change rules.

## License

MIT — see `LICENSE`.
