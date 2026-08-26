# rusty_multimodal_db

A benchmark harness, not a database — but if you're looking for what to
actually use, it's `ProductionStore` (`src/production.rs`):
`CanonicalCachedStore`'s storage architecture, made durable via mmap, made
safe for concurrent reader/writer access via one global `RwLock`. Six
rounds of empirical work (row/column/graph, mixed-workload, durability,
and three concurrency-throughput passes) converged on that combination —
see `docs/decisions/ADR-0008-production-default.md` for which round
justified each layer and `RESULTS.md`'s `## Production recommendation`
section for the numbers.

A second, separate story lives alongside it: `crate::generic`
(`src/generic/`) is a real, public generic record/schema/query library —
the same storage/durability/concurrency recipe, generalized to any record
type instead of hardcoded to `Dog`, validated against a second,
structurally different domain (`Order`/`Customer`) across four spikes
before promotion. `crate::generic::production::GenericProductionStore` is
its `ProductionStore` equivalent — durable via mmap, safe for concurrent
access via `RwLock`, generic. See `docs/decisions/ADR-0009-generic-schema-design-proposal.md`
(Accepted) and `RESULTS.md`'s `## Generic schema library` section. New,
parallel capability — nothing above changed to build it.

Everything below this point — three other storage backends, seven other
durability variants, three other concurrency strategies — is the
benchmarked evidence that recommendation is built on, not the recommended
path. It exists to empirically test one storage-design hypothesis before
anyone commits engineering time to building around it:

> A canonical store, keyed by UUID, can serve as the single source of
> truth for row-oriented, column-oriented, and graph-oriented access —
> with row/column/graph implemented as **views** over that one canonical
> store rather than as separate physical copies of the data.

It's tested against two conventional baselines, all three behind the same
`DogStore` trait so results are directly comparable:

- **AoS** (array of structs) — `Vec<DogRecord>`, row-oriented.
- **SoA** (struct of arrays) — parallel `Vec<Uuid>`/`Vec<String>`/`Vec<u32>`,
  column-oriented.
- **Canonical** — `HashMap<Uuid, DogRecord>` as the only physical copy,
  with column-scan and one-hop-lookup access (both a shared-attribute
  grouping, `same_breed`, and real edge traversal over a generated
  `littermate_of` relationship, `neighbors`) implemented as derived views
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
cargo test --all-features            # unit tests, including ProductionStore's and GenericProductionStore's flagship integration tests
cargo bench                          # wall-clock Criterion suite (cross-platform), ProductionStore listed first in every workload
cargo bench --bench concurrency      # concurrency throughput sweep, ProductionStore included alongside every strategy
cargo bench --bench generic_production   # GenericProductionStore (Order/Customer) get/scan/filter/parent/children sweep
cargo bench --features perf-events --bench cache_events   # cache-miss counts, Linux bare-metal only — see ADR-0002
```

See `AGENTS.md` for the full canonical command set and change rules.

## License

MIT — see `LICENSE`.
