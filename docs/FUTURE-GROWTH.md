# Future Growth

This document captures directions this project could grow in beyond its current scope, and what each would actually require. Nothing here is planned or scheduled — it exists so a future decision to pursue any of it starts from an honest accounting instead of a guess.

## Nothing here is architecturally blocked

Every current boundary in this project is a deliberate scope line from a specific round, not a structural limitation:

* The multi-process append fix targets local filesystems only (`O_APPEND`'s atomicity guarantee excludes NFS) — a real, identifiable piece of work if that assumption ever needs to change, not a rewrite.
* The multi-process fix covers slot creation specifically. Broader multi-writer coordination wasn't needed yet, not ruled out.
* The `research` feature flag means every benchmarked alternative — 4 storage backends, 8 durability variants, 4 concurrency strategies — is still in the codebase, just not compiled into a default build. Nothing was deleted at any point in this project's history.
* The generic schema layer (`crate::generic`) was validated against a toy domain (`Order`/`Customer`) and a real one (requirements traceability). Nothing schema-specific is baked into the storage engine itself.
* Staying off crates.io is a current decision (a `Cargo.toml`/publishing choice), not a technical constraint.

## Path to a server / query layer

The storage engine's public API (`get`/`scan`/`filter`/`update`/relationship traversal) is already a clean boundary a network layer could sit on top of without changing the engine itself.

Genuinely additive — no rework of the storage layer required:

* A binary that owns the store and listens on a socket, translating requests into calls against the existing API and serializing results back out.
* Concurrency across client connections is actually simpler than the cross-process case already solved: if one server process owns the file, client requests never touch it directly — this collapses back to the already-solved in-process concurrency problem (`RwLock`), not the harder cross-process one.
* A query language would compile down to primitives that already exist (filter-by-field, scan, relationship traversal) — real design work, but the storage layer wouldn't need to change to accommodate it.

Genuinely new — not incremental extensions of existing work:

* Authentication/authorization. Doesn't exist in any form today; a network-exposed store needs it from the start, not as an add-on.
* Session/transaction semantics across multiple requests. Every operation today is single-shot; a protocol has to define what a "connection" guarantees across several of them.
* The query language itself — real parser and language design, not a small extension.

## Path to SQLite/DuckDB parity

This is a different tier of project, not a natural extension of the current one — each of the three items below is roughly a multi-year effort on its own, and SQLite and DuckDB aren't even the same target to aim at (SQLite: row-store, transactional; DuckDB: column-store, analytical). "Parity with both" isn't one destination.

The big three:

1. SQL. A parser, a query planner, a cost-based optimizer, and an execution engine. Every query today is a hand-written Rust method call against a specific schema's traits — there's no declarative language layer at all.
2. Transactions. Individual writes are crash-safe and torn-write-safe today, but "do these N operations atomically, or roll all of them back" doesn't exist as a concept. This needs a real transaction manager — likely its own MVCC or log-based design.
3. Arbitrary joins. Relationships in this engine are hand-declared, purpose-built traversal primitives (`littermate_of`, `belongs_to`) — built in advance, specific to a schema. SQL lets you join any two tables on any predicate at query time; nothing like that exists here, and every relationship currently has to be designed and built ahead of time.

Smaller, but still real:

* Dynamic/runtime schema (`ALTER TABLE`-style changes). Schema here is a compile-time Rust concept.
* A query optimizer for aggregation (`GROUP BY`, `AVG`, multi-table joins) — DuckDB's core identity is vectorized execution over exactly this; nothing comparable exists here.
* Client ecosystem — drivers for other languages, a CLI, general tooling. This is Rust-only and embedded today.
* Decades of hardening. SQLite's reliability record is the product of 20+ years and one of the largest test suites in software. This project's crash-safety work is real and genuinely tested, but young by comparison.

What's already solid and wouldn't need to be redone: the storage engine itself, real measured durability, real measured concurrency (single- and multi-process), and a working generic schema layer proven against more than one domain. SQL, transactions, and arbitrary joins would be built on top of that foundation, not require rebuilding it — but each is a serious, standalone effort, not a small extension of this project.
