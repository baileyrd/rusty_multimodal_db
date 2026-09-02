//! **Use [`production::ProductionStore`]** for a `Dog`-shaped store, or
//! **[`generic::production::GenericProductionStore`]** (built the same
//! way as [`generic::order_customer`]'s `Order`/`Customer` demonstrates)
//! for your own record type — those two, plus whatever's needed to
//! implement/compose against them (the `Record`/`IndexedField`/
//! `ScannableField`/`SymmetricRelation`/`ChildOf` traits in
//! [`generic::traits`], the query traits in [`generic::query`], the
//! composable store layers in [`generic::store`], and the error types
//! each returns), are this crate's whole public contract. Everything
//! else — the three other `Dog` storage backends, the seven other
//! durability variants, the three other concurrency strategies, the
//! `Dog`-specific dataset generator, and every historical spike/
//! comparison module — is the *evidence* this recommendation is built on,
//! not part of it, and lives behind the `research` Cargo feature (off by
//! default): `cargo build --features research` to compile it in and see
//! the underlying benchmark comparisons this crate's whole prior history
//! produced. See `RESULTS.md` for the numbers and
//! `docs/decisions/ADR-0008-production-default.md`/`ADR-0009` for the
//! acceptance records.
//!
//! # Start here: [`production::ProductionStore`]
//!
//! `CanonicalCachedStore`'s storage architecture, made durable via mmap,
//! made safe for concurrent reader/writer access via one global `RwLock`.
//! It implements both [`store::DogStore`] (drop-in for single-owner code)
//! and [`concurrency::ConcurrentStore`] (for genuine multi-threaded
//! sharing). See `docs/decisions/ADR-0008-production-default.md` for which
//! round justified each of the three layers, and `RESULTS.md`'s
//! `## Production recommendation` section for the numbers.
//!
//! # A second, generalized story: [`generic::production::GenericProductionStore`]
//!
//! Everything above is `Dog`-specific, by design — the charter's whole
//! point was measuring one fixed record shape across storage layouts. A
//! separate, later arc asked whether that same storage/durability/
//! concurrency recipe generalizes to *any* record type, validated it
//! against a second, structurally different domain (`Order`/`Customer` —
//! a directed relation, a currency-like field, an enum categorical field),
//! and promoted the result into [`generic`], a real generic library:
//! `Record`/`IndexedField`/`ScannableField`/`SymmetricRelation`/`ChildOf`
//! traits, composable store wrapper layers, and
//! [`generic::production::GenericProductionStore`] — the generic
//! equivalent of [`production::ProductionStore`], wired to the same mmap
//! durability and global `RwLock` concurrency. Unlike the `Dog` side,
//! [`generic::mmap_store::GenericMmapStore`] and the composable layers in
//! [`generic::store`] stay part of the public contract even though
//! they're "internals" of [`generic::production::GenericProductionStore`]
//! — building your *own* domain's store the way
//! [`generic::order_customer`] (gated, reference-only) demonstrates for
//! `Order`/`Customer` means composing them directly. See [`generic`]'s
//! own module docs for the four-spike validation history and
//! `docs/decisions/ADR-0009-generic-schema-design-proposal.md` (Accepted)
//! for the acceptance record. This is new, parallel capability — nothing
//! above changed to build it.
//!
//! # A third arc: a network server, behind `server`
//!
//! [`server`] puts a thin, real network listener in front of
//! [`production::ProductionStore`]/[`generic::production::GenericProductionStore`]
//! — a `Request`/`Response` wire protocol over length-prefixed `bincode`
//! framing, thread-per-connection, reusing whichever `RwLock` the wrapped
//! store already manages (no new lock at this layer). Off by default
//! behind its own `server` feature (distinct from `research` — this is new
//! capability, not a benchmarked alternative), and validated against three
//! domains: `Dog` ([`server::dog::DogConnectionStore`], `Neighbors` only),
//! `Order`/`Customer` ([`server::order::OrderConnectionStore`],
//! `Parent`/`Children` only), and `Employee`
//! ([`server::employee::EmployeeConnectionStore`], both — the first
//! domain to combine them, self-referential on `reports_to`/`ChildOf`
//! and `collaborates_with`/`SymmetricRelation` at once), the latter two
//! additionally behind `research` since their reference domains are. A
//! client that doesn't know a domain at compile time can send
//! `Request::DescribeSchema` first to discover its fields, types, and
//! supported operations at runtime (see
//! `docs/decisions/ADR-0011-server-schema-discovery.md`, Accepted) —
//! field *tags* stay the wire addressing scheme either way.
//! [`server::client::SchemaDrivenClient`] is a real, reusable client
//! built exactly this way: every request addressed by discovered field
//! name, capability checks run client-side first, no domain-specific
//! `FIELD_*` constant imported anywhere in it. Authentication/authorization
//! is real and implemented ([`server::AuthConfig`]/`Request::Authenticate`,
//! `docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012, Accepted) but purely
//! opt-in — a server started with no configured tokens behaves exactly as
//! before. Atomic multi-operation transactions are real and implemented
//! too (`Request::Transaction`, `docs/design/SERVER-TRANSACTION-DESIGN.md`,
//! ADR-0013, Accepted): a batch of writes applied all-or-nothing, isolated
//! from concurrent connections — explicitly not crash-atomic and not a
//! multi-round-trip interactive session, see that design's own
//! "Non-goals". Native transport encryption is real and implemented too
//! ([`server::TlsConfig`], `docs/design/SERVER-TLS-DESIGN.md`, ADR-0014,
//! Accepted): a TLS server handshake via `rusty_tls` (this owner's own
//! ecosystem-wide `rustls` wrapper, not a direct `rustls` dependency —
//! this crate's first git dependency) before any framed traffic, also
//! purely opt-in — a server started with no `TlsConfig` behaves exactly
//! as before. **No query language beyond fixed field-tag addressing, and
//! `TlsConfig`/`AuthConfig` must both be configured together before
//! exposing a server beyond a trusted network** — see [`server`]'s
//! own module docs and `docs/decisions/ADR-0010-server-query-layer-proposal.md`
//! (Accepted) before using it; this is new, parallel capability, same as
//! `generic` above. Two exceptions to "nothing in `production`/
//! `generic::production` changed to build it": validating `Employee`
//! surfaced a real gap in `generic::store`/`generic::production`
//! (`Reversed` never forwarded `Neighbors`) — fixed there directly, see
//! `docs/decisions/ADR-0009-generic-schema-design-proposal.md`'s
//! "Acceptance and implementation" addendum; and `Request::Transaction`'s
//! atomicity guarantee needed a new critical-section primitive on both
//! [`production::ProductionStore`] (`production::TransactionalStore`) and
//! [`generic::production::GenericProductionStore`] (`with_exclusive`) —
//! purely additive, no existing method's behavior changed.
//!
//! # Everything else: benchmarked alternatives, behind `research`
//!
//! `store`, `durability`, and `concurrency` hold the other three `Dog`
//! storage backends, seven other durability variants, and three other
//! concurrency strategies this recommendation is built on — the evidence,
//! not dead code, but not compiled into a default build either. Each
//! variant lost outright, tied, or won only in a narrow corner the
//! production pick's own module docs and `RESULTS.md` name explicitly.
//! Reach for one of them directly (with `--features research`) only if
//! your workload's shape is genuinely one of those narrow corners (e.g.
//! `ShardedStore` for a small, write-heavy, high-thread-count deployment —
//! see `docs/decisions/ADR-0008-production-default.md`); otherwise, use
//! [`production::ProductionStore`].
//!
//! See `docs/charter/CHARTER.md` for the original hypothesis under test
//! and `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for
//! why the first three backends are compared this way.

/// Benchmark/dataset-building infrastructure for the `Dog` comparison —
/// not part of the recommended API, gated behind the `research` feature.
/// Also available under plain `#[cfg(test)]` (regardless of `research`):
/// `concurrency::test_support::run_concurrency_stress_test` — used by
/// [`production::ProductionStore`]'s own flagship, always-on test — needs
/// [`bench_support::build_dataset`] to build its stress-test input.
/// `#[cfg(test)]` code never ships to a downstream consumer's build
/// regardless of feature flags, so this doesn't widen what an external
/// consumer actually sees. See this module's own docs.
#[cfg(any(test, feature = "research"))]
pub mod bench_support;
pub(crate) mod codec;
pub mod concurrency;
pub mod durability;
/// Synthetic `DogRecord` dataset generation, used to build benchmark
/// input — not needed to use [`production::ProductionStore`] itself (a
/// real caller supplies its own records). Gated behind the `research`
/// feature; also available under plain `#[cfg(test)]`, same reason as
/// `bench_support` (which uses this to build its own `Dataset`) — see
/// that module's doc comment.
#[cfg(any(test, feature = "research"))]
pub mod generator;
/// A generic record/schema/query library: any domain implementing
/// [`generic::traits::Record`] and friends gets equality-indexed lookup,
/// scannable-field access, symmetric/directed relationship traversal, and
/// — via [`generic::production::GenericProductionStore`] — real mmap
/// durability and `RwLock` concurrency, generalized from `ProductionStore`
/// above rather than hardcoded to `Dog`. Validated against `Dog` and a
/// second, structurally different domain (`Order`/`Customer`, this
/// module's real reference implementation) across four spikes before
/// promotion — see this module's own doc comment for the full history,
/// and ADR-0009 (Accepted) for the acceptance record. New, parallel
/// capability: nothing above changed to build this.
pub mod generic;
/// Historical validation spikes that led to `generic` above — kept as the
/// measurement record, not part of the recommended API surface. Gated
/// behind the `research` feature. See its own module docs.
#[cfg(feature = "research")]
pub mod generic_spike;
pub mod production;
pub mod record;
/// A network server/query layer in front of [`production::ProductionStore`]/
/// [`generic::production::GenericProductionStore`] — accepted design,
/// `docs/design/SERVER-QUERY-LAYER-DESIGN.md`, ADR-0010 (Accepted). Off by
/// default behind the `server` Cargo feature, distinct from `research`:
/// this is new, real, additive capability, not a benchmarked-alternative
/// or historical-spike module, and it introduces a real
/// network-listening binary surface. Authentication/authorization
/// (`AuthConfig`, ADR-0012, Accepted) and native transport encryption
/// (`TlsConfig`, ADR-0014, Accepted, via `rusty_tls` — this crate's first
/// git dependency, not a direct `rustls` dependency) are both implemented
/// and purely opt-in — see this module's own doc comment and ADR-0010's
/// Consequences before enabling it, and never expose a server built from
/// it beyond a trusted, localhost/development network unless both are
/// configured together.
#[cfg(feature = "server")]
pub mod server;
pub mod store;
/// The crate's one shared scratch-directory helper — unconditionally
/// available (unlike `bench_support`) since [`production::ProductionStore`]'s
/// own infallible constructors need it. See its own module docs.
mod test_support;

#[cfg(feature = "research")]
pub use generator::{generate, generate_littermates, GeneratorConfig};
pub use production::ProductionStore;
pub use record::DogRecord;
pub use store::{DogStore, StoreError};
