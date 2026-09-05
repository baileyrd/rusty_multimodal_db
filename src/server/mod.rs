//! A network server/query layer in front of [`crate::production::ProductionStore`]/
//! [`crate::generic::production::GenericProductionStore`] — accepted design,
//! `docs/design/SERVER-QUERY-LAYER-DESIGN.md`, ADR-0010. Off by default
//! behind the `server` Cargo feature: this module adds a real,
//! network-listening binary capability, distinct from the `research`
//! feature's benchmarked-alternative/historical-spike bucket, so it's
//! opted into deliberately rather than pulled in incidentally by
//! `--all-features`.
//!
//! # This is a thin translation layer, not a new storage engine
//!
//! [`dispatch`] and [`serve`] never touch a record's bytes directly —
//! every operation goes through the existing, already-validated
//! [`ConnectionStore`] adapter around a real store type
//! ([`dog::DogConnectionStore`] wraps [`crate::production::ProductionStore`];
//! [`order::OrderConnectionStore`]/[`employee::EmployeeConnectionStore`] wrap
//! [`crate::generic::production::GenericProductionStore`]). Concurrency
//! across client connections adds no new lock: it collapses onto whatever
//! `RwLock` the wrapped store already manages internally, per
//! `docs/FUTURE-GROWTH.md`'s own "Path to a server / query layer" section.
//!
//! # What this does not provide
//!
//! No query language beyond fixed field-tag addressing — an explicit
//! non-goal of the accepted design(s). **Do not expose a server built
//! from this module beyond a trusted, localhost/development network
//! unless both `ServeOptions` and `TlsConfig` (below) are configured** —
//! see ADR-0010's Consequences; ADR-0012 closed the authentication/
//! authorization half of that gap, ADR-0014 closes the transport-
//! encryption half, but neither alone is the whole story (a
//! `TlsConfig`-only server still lets anyone who can connect do
//! anything; a `ServeOptions`-only server still puts tokens and every
//! record value in plaintext on the wire). ADR-0010's third named gap,
//! "no transaction semantics," is now partly closed too — see "Atomic
//! transactions" below for exactly which slice.
//!
//! # Authentication/authorization (`ServeOptions`), ADR-0012
//!
//! `docs/design/SERVER-AUTH-DESIGN.md` closes the "no authentication, no
//! authorization" gap ADR-0010 originally left open: [`serve`] takes an
//! [`ServeOptions`] naming which token(s) (if any) a server instance
//! accepts and the [`TokenClass`] (`ReadOnly`/`ReadWrite`) each grants.
//! `Request::Authenticate` establishes a connection's class; every other
//! request kind is rejected with `ErrorCode::Unauthenticated` until it
//! does, and `ReadOnly` is further rejected from `Request::UpdateField`/
//! `Request::Transaction` with `ErrorCode::Unauthorized`.
//! `ServeOptions::default()` (no tokens configured) reproduces exactly the
//! pre-ADR-0012 unauthenticated behavior (`AUTH-FR-007`) — this is purely
//! opt-in. See this module's own `handle_connection` for the full gating
//! logic.
//!
//! # Atomic transactions, ADR-0013
//!
//! `docs/design/SERVER-TRANSACTION-DESIGN.md` closes one bounded slice of
//! ADR-0010's "no transaction semantics" gap: `Request::Transaction { updates }`
//! batches several `UpdateField`-shaped writes and applies them
//! all-or-nothing — every operation's precondition is checked before any
//! write happens, isolated from concurrent connections by holding the
//! wrapped store's existing lock for the whole batch (no new lock; see
//! `crate::production::TransactionalStore`/
//! `crate::generic::production::GenericProductionStore::with_exclusive`).
//! **Deliberately not a multi-round-trip interactive session** (no
//! `Begin`/`Commit`, transaction state held open across several
//! requests — a real, unaccepted-elsewhere liveness risk) **and not
//! crash-atomic** (a process crash mid-batch can leave a partial batch
//! durably applied) — see that design document's own "Non-goals" for the
//! full account. Both halves were later taken by
//! `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`: a *buffered*
//! session (`SERVER-001` FR-024, ADR-0024) that stages writes
//! per connection and commits them as one batch, holding no lock across
//! round trips; and an opt-in redo journal (`SERVER-001` FR-025,
//! ADR-0025, [`journal`]) that an adapter built with `with_journal`
//! appends and `fsync`s before a batch's first write — since `SERVER-001`
//! FR-027 (ADR-0026) as a leader/follower group commit that runs outside
//! the store's exclusive section and applies in journal order — and replays on the
//! next open, so a batch answered `Ok` survives a crash whole.
//!
//! # A real, schema-driven client
//!
//! [`client::SchemaDrivenClient`] is the client half of ADR-0011's schema
//! discovery: a real, reusable client that never imports a domain's own
//! `FIELD_*` constants, driving every request purely from what
//! `Request::DescribeSchema` reports at connect time. Unconditional under
//! `server` (not `research`-gated) — it has no domain-specific code at
//! all, only `Request`/`Response`/framing.
//!
//! # Native transport encryption (`TlsConfig`), ADR-0014
//!
//! `docs/design/SERVER-TLS-DESIGN.md` closes the transport-encryption half
//! of ADR-0010's gap that ADR-0012 explicitly left open: [`serve`] takes
//! an optional [`TlsConfig`] (`None` reproduces today's plaintext behavior
//! exactly, `TLS-FR-008`). When configured, every accepted connection
//! performs a TLS server handshake — via
//! [`rusty_tls::TlsAcceptor`](https://github.com/Rusty-Mill/rusty_mill/tree/main/crates/rusty_tls),
//! this owner's own ecosystem-wide `rustls` wrapper, not a direct
//! `rustls` dependency (see that design's own "Ecosystem check" for why)
//! — before any framed `Request`/`Response` traffic, including
//! `Authenticate`, is ever read or written. `dispatch`/`ConnectionStore`
//! remain completely unaware transport encryption exists, and
//! `src/server/framing.rs` needed zero changes, since its functions were
//! already generic over `Read`/`Write`. Explicitly not mTLS — client
//! identity remains exactly [`ServeOptions`]'s existing shared-secret token
//! scheme, now traveling encrypted rather than plaintext.
//!
//! # Protocol version (`Hello`), ADR-0022
//!
//! `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md` gives the wire *shape*
//! (the set of `Request`/`Response` variants) a version:
//! [`crate::server::protocol::PROTOCOL_VERSION`], the "Protocol versions"
//! table and the append-only compatibility rules in
//! [`crate::server::protocol`]'s own docs. A client may send
//! `Request::Hello { protocol_version }` as its **first** frame;
//! `handle_connection` answers it itself — before the `Authenticate`
//! intercept and the authentication gate, on plain and TLS connections
//! alike — with `Response::Hello { min(client, PROTOCOL_VERSION) }`
//! (`PROTO-FR-003`). Version 0, or a `Hello` that is not the first frame,
//! is `ErrorCode::Malformed` and the connection stays open
//! (`PROTO-FR-004`). A client that never says `Hello` speaks version 1
//! (the `SERVER-001` v0.9.1 shape) and is served exactly as before — no
//! existing client, test, or bench changed (`PROTO-FR-002`). Since
//! protocol version 3 (`SERVER-001` FR-024, ADR-0024) the negotiated
//! version *is* kept per connection, because the first gated variants —
//! the transaction-session requests — consult it: rule 3 in
//! [`crate::server::protocol`] got its first branch exactly where
//! ADR-0022 said it would (see `handle_connection`'s own "Transaction
//! sessions" section).
//!
//! # Two features: `client` and `server` (`ECO-FR-001`–`003`, ADR-0043)
//!
//! Since `SERVER-001` v0.35.1 this module exists under the `client` Cargo
//! feature, which compiles exactly the half a program needs to *talk* to a
//! server — `framing`, `protocol`, `client` (and its private SQL front
//! end), PEM loading, and `TlsConfigError` — with no `serve`, no
//! domain adapter, no journal, no logs. The `server` feature implies
//! `client` and adds the rest (the `serve` submodule, re-exported here so
//! every pre-split path still resolves). `rusty_tls` is the one
//! dependency both halves share, for the client side of TLS.

#[cfg(feature = "server")]
pub mod access;
#[cfg(feature = "server")]
pub mod audit;
pub mod client;
#[cfg(feature = "server")]
pub mod dog;
#[cfg(all(feature = "server", feature = "research"))]
pub mod employee;
/// `Entity`'s adapter — `server`-gated alone, not `server` +
/// `research` (`ENT-FR-006`, ADR-0037): matching `Reminder`'s own
/// front-door precedent, not `order`/`employee`'s.
#[cfg(feature = "server")]
pub mod entity;
pub mod framing;
#[cfg(feature = "server")]
pub mod journal;
#[cfg(all(feature = "server", feature = "research"))]
pub mod order;
mod pem;
pub mod protocol;
/// `Reminder`'s adapter — `server`-gated alone, not `server` +
/// `research` (`RMD-FR-006`, ADR-0036): unlike `order`/`employee`,
/// `Reminder` is real, deployable capability, not reference material.
#[cfg(feature = "server")]
pub mod reminder;
mod sql;

/// The server body — [`ConnectionStore`], [`dispatch`], [`serve`],
/// [`ServeOptions`], [`TlsConfig`], and every evaluator — behind the
/// `server` feature and re-exported here unchanged, so
/// `crate::server::ServeOptions` and friends are the same paths they were
/// before `ECO-FR-001` (ADR-0043) split the client half out. Under
/// `client` alone this module does not exist: `framing`, `protocol`,
/// `client`, `sql`, `pem`, and [`TlsConfigError`] are the whole surface.
#[cfg(feature = "server")]
mod serve;
#[cfg(feature = "server")]
pub use serve::*;

use std::io;

/// Everything that can go wrong building a [`TlsConfig`].
#[derive(Debug)]
pub enum TlsConfigError {
    /// Reading a certificate/key file failed (missing file, permission
    /// denied, ...).
    Io(io::Error),
    /// A certificate/key file's contents weren't valid PEM.
    Pem(pem::PemError),
    /// `rusty_tls` rejected the decoded certificate chain or private key.
    Tls(rusty_tls::Error),
}

impl std::fmt::Display for TlsConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsConfigError::Io(e) => write!(f, "reading TLS certificate/key file: {e}"),
            TlsConfigError::Pem(e) => write!(f, "parsing TLS certificate/key PEM: {e}"),
            TlsConfigError::Tls(e) => write!(f, "building TLS config: {e}"),
        }
    }
}

impl std::error::Error for TlsConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsConfigError::Io(e) => Some(e),
            TlsConfigError::Pem(e) => Some(e),
            TlsConfigError::Tls(e) => Some(e),
        }
    }
}
