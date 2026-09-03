# Server `ServeOptions` Consolidation Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, `ADR-0032` option (a); folding
  `TlsConfig`'s consolidation out and closing as not warranted both
  declined; no changes requested). Acceptance authorizes the design;
  implementation follows as its own unit — see `ADR-0032`'s
  "Acceptance and implementation" section.
- Date: 2026-09-03
- Related: `ADR-0025` (the precedent against a fifth `serve` parameter,
  first stated), `ADR-0029` (*a second cross-cutting `serve` option
  appears — `ServeOptions` (option 3) becomes worth its breaking
  change*, and its own "a fourth cross-cutting `AuthConfig` knob"
  tradeoff), `ADR-0030` (restates the trigger, does not pull it),
  `ADR-0031` (*a fourth or fifth cross-cutting `AuthConfig` knob
  appears after this* — restates it again, still deferred),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.24.0.

## Purpose and scope

Every accepted design since `ADR-0025` has faced the same choice —
where does the next cross-cutting, opt-in server behavior live? — and
answered it the same way: hang it on `AuthConfig`, not add a fifth
`serve` parameter. That was the right call each time taken alone. Five
rounds later, `AuthConfig` carries tokens, certificate-derived classes,
an audit sink, a rate-limit budget, and an access-log sink — five
independent, unrelated concerns wearing one name that describes only
the first of them — while `TlsConfig` has sat beside it as `serve`'s
own second parameter since v0.9.0, never folded in, precisely because
each individual round judged that fold not worth forcing.

This round asks the question directly, with the full list in view
instead of one knob at a time: does the accumulated shape still earn
its name, and is now the moment to consolidate?

**In scope:**

- Whether `AuthConfig` (identity: tokens, certificate classes) and the
  three opt-in policy/observability sinks it also carries (audit,
  rate limit, access log) belong in one type or several.
- Whether `TlsConfig` — `serve`'s other cross-cutting parameter —
  belongs inside that consolidated type or stays separate.
- The resulting `serve` signature.
- A rename, if the chosen shape no longer matches the name `AuthConfig`.

**Out of scope (see "Non-goals")**: any new server *capability* — no
new gate, sink, limiter, or wire behavior; a real `shutdown`/drain
story; per-connection (rather than per-server) configuration.

## Non-goals

- **New functionality.** This is a config-surface reshape. Every
  existing behavior — what a token grants, what a certificate classes,
  what the audit/access logs record, what the rate limiter refuses —
  is unchanged bit for bit. A reader who diffs behavior, not types,
  should see nothing.
- **A generic plugin/extension mechanism.** The trigger this round
  answers is about *five now-named* concerns, not an open-ended "n
  more will arrive" architecture. If a sixth concern appears later,
  the same judgment call this round makes applies again then; this
  design does not try to pre-empt it with a registry or trait-object
  bag.
- **Publishing to crates.io, or any external-consumer compatibility
  story.** `docs/FUTURE-GROWTH.md` already names staying off crates.io
  as a current decision; every caller of `AuthConfig`/`TlsConfig`
  today is inside this repository (`src/bin/dog_server.rs`, this
  crate's own tests and benches). A rename or reshape here is an
  internal refactor with a fully enumerable blast radius, not a
  breaking change against an unknown public.

## Context and terminology

**What `AuthConfig` holds today** (`src/server/mod.rs`), and how it
got there:

| Field | Since | Trigger that added it |
|---|---|---|
| `read_only_token`, `read_write_token` | v0.6.0 (`FR-016`) | `ADR-0012`, the original design |
| `certificate_classes: Vec<(Vec<u8>, TokenClass)>` | v0.21.0 (`FR-031`) | `ADR-0028`, `ADR-0023`'s first revisit trigger |
| `audit: Option<Arc<dyn audit::AuditSink>>` | v0.19.0 (`FR-029`) | `ADR-0029` |
| `rate_limit: Option<Arc<FailureTable>>` | v0.22.0 (`FR-032`) | `ADR-0030`, `ADR-0029`'s first revisit trigger |
| `access_log: Option<Arc<dyn access::AccessSink>>` | v0.23.0 (`FR-033`) | `ADR-0031`, `ADR-0029`'s second revisit trigger |

Every addition was reasoned about individually, and every one of those
five reasoning passes reached the same "hang it on `AuthConfig`"
answer while explicitly naming and deferring the alternative
(`ADR-0025`'s "Negative/tradeoffs", `ADR-0029`'s Non-goals and both
revisit triggers above, `ADR-0030`'s restatement, `ADR-0031`'s
Non-goals and its own restatement). None of those five decisions was
wrong in isolation — each judged the marginal cost of one more opt-in
method lower than the cost of a real migration, at that moment. This
design is the first to look at the accumulated result rather than the
marginal step.

**What `TlsConfig` holds**, unchanged in shape since v0.13.0
(`ADR-0023`): the TLS acceptor, whether client certificates are
required, and (since `ADR-0028`) nothing else new — `certificate_classes`
lives on `AuthConfig`, not `TlsConfig`, because a certificate's class
is an authorization decision, not a transport one. `TlsConfig` has
its own fallible constructors (`new`, `new_with_client_auth`,
`from_pem_files*`, `from_env() -> Option<Result<Self, TlsConfigError>>`)
— unlike every `AuthConfig` builder method, building a `TlsConfig` can
fail (a bad PEM file, an invalid root set), and `dog_server.rs` handles
that failure at startup, separately from the infallible
`AuthConfig::from_env()` call.

**`serve`'s current signature** (`src/server/mod.rs`):

```rust
pub fn serve<S: ConnectionStore + 'static>(
    listener: TcpListener,
    store: Arc<S>,
    auth: AuthConfig,
    tls: Option<TlsConfig>,
)
```

Four parameters: two are the actual serving mechanics (`listener`,
`store`); two are configuration bags with unrelated names for what is,
functionally, one "how should this server admit, classify, and watch
its connections" question. `handle_connection` receives `auth` and
`tls` as two separate references throughout, and threads them
independently everywhere a decision depends on either.

**Blast radius, read from `main` `1fe23f9`**: `AuthConfig` is
constructed at roughly 85 call sites across `src/bin/dog_server.rs`,
`benches/server.rs`, and eight test files (`grep -c` per file:
`server_tls_integration.rs` 23, `server_transaction_integration.rs`
16, `server_auth_integration.rs` 7, `server_protocol_version.rs` 7,
`server_schema_driven_client.rs` 3, `server_dog_integration.rs` 2,
`server_employee_integration.rs`/`server_order_integration.rs` 1
each, `src/server/mod.rs`'s own unit tests 11, `dog_server.rs` 10).
Every one is a mechanical `AuthConfig::` → `ServeOptions::` rename;
none depends on `AuthConfig`'s internal field layout, since every
field is private and reached only through the public builder methods
this design keeps unchanged in name and behavior.

## Requirements

- `SRV-FR-001` — **One consolidated type.** A single public struct —
  proposed name `ServeOptions` — replaces `AuthConfig` and absorbs
  `TlsConfig` as one more field, `tls: Option<TlsConfig>`. `TlsConfig`
  itself is unchanged (still its own type, still its own fallible
  constructors) — only where it is *held* changes.
- `SRV-FR-002` — **Every existing builder method keeps its name and
  behavior**, moved without modification: `with_certificate_class`,
  `with_certificate_class_pem_file`, `with_audit`, `with_rate_limit`,
  `with_access_log`, `audit()`, `rate_limit()`, `access_log()`,
  `is_configured()`, `class_for_certificate` (`pub(crate)`), the
  private `check`/`is_throttled`/`note_failure`. One new method,
  `with_tls(TlsConfig) -> Self`, in the same builder style.
- `SRV-FR-003` — **Constructors stay split exactly as today.**
  `ServeOptions::new(read_only_token, read_write_token)` and
  `ServeOptions::from_env()` keep reading exactly the token/certificate
  environment variables they read today, and stay infallible — neither
  reads a TLS variable or can fail. `TlsConfig::from_env()` stays its
  own fallible call, exactly as today; a caller composes
  `ServeOptions::from_env().with_tls(tls_config)` after handling that
  `Result` itself, the same two-step `dog_server.rs`'s `main` already
  performs today for every other conditionally-configured piece
  (`rate_limit_from_env_value`, `access_sink_from`, `audit_sink_from`).
  Folding TLS construction itself into one fallible `from_env` is
  explicitly rejected (see "Considered options") — it would be the one
  behavior change this design does not want to make.
- `SRV-FR-004` — **`serve` drops to three parameters**:
  `serve<S>(listener: TcpListener, store: Arc<S>, options: ServeOptions)`.
  `handle_connection` takes one `options: &ServeOptions` instead of
  two separate `auth`/`tls` references; every internal call site reads
  `options.tls`/`options.<method>()` in place of the two prior names.
- `SRV-FR-005` — **A straight rename, not a deprecation shim.** No
  `AuthConfig` type alias, re-export, or `#[deprecated]` stub survives
  the change — every internal caller is updated in the same unit, per
  this crate's own "no backwards-compatibility hacks for an
  unpublished crate" convention. `TokenClass`, `RateLimit`,
  `RateLimitParseError`, `TlsConfigError`, and every sink/type these
  five concerns already use are untouched.
- `SRV-FR-006` — **No behavior change of any kind.** Every gate
  decision, every audit/access-log line, every rate-limit computation,
  every certificate match, every TLS handshake outcome is byte-for-byte
  identical before and after. This requirement is what makes the
  round low-risk despite its ~85-site blast radius: the diff is a
  rename plus a field move, checkable mechanically (every existing
  test, unmodified in intent, must still pass after a name substitution).

## Considered options

**Whether to consolidate at all.**

1. **Close as not yet warranted.** Five knobs on one struct is a
   naming smell, not a functional problem; `cargo doc` still renders
   each field's own doc comment naming the ADR that added it, so a
   reader is never actually lost. Rejected as *this* round's answer
   only because the trigger has now fired for the fourth time
   (`ADR-0029` ×2, `ADR-0030`, `ADR-0031`) with the same word each
   time — declining to ever evaluate the accumulated question directly
   would make the trigger permanently rhetorical. Recorded as a live
   option the owner may still choose (option (c) below).
2. **Consolidate into one `ServeOptions`, `TlsConfig` folded in —
   proposed.** Matches the trigger's own literal phrasing across four
   ADRs; `serve` reaches its simplest possible shape (3 params: the
   two mechanics, one config); the one flat builder this crate has
   used for every prior addition continues unchanged in *style*
   (`with_X` chaining), only its name and one extra field change.
3. **Consolidate, but leave `TlsConfig` as `serve`'s own second
   parameter.** A smaller diff (~85 sites become a rename, but
   `serve` stays 4 params) and keeps the transport-vs-policy line the
   crate has drawn since v0.13.0 (`certificate_classes` lives on the
   policy side deliberately, precisely because it is *not* a transport
   decision — `TlsConfig` itself is). Rejected: it leaves the exact
   asymmetry this round exists to resolve (TLS the one cross-cutting
   concern still outside the consolidated type, for no reason a reader
   could infer from the code alone), and gets none of `serve`'s
   simplification.
4. **Split by concern instead of consolidating** — identity
   (`ServeOptions`: tokens, certificate classes) separate from
   observability/policy (a second struct: audit, rate limit, access
   log), both passed to `serve`. Rejected: this recreates the "many
   `serve` parameters" shape `ADR-0025` first ruled against, just with
   fewer, larger parameters instead of many small ones — no net
   simplification, and it splits `is_configured()`'s existing logic
   (which already reads both certificate classes *and* tokens together)
   across two types for no behavioral reason.

**Naming.**

1. **`ServeOptions` — proposed.** The exact name every revisit-trigger
   note across four ADRs already uses; adopting it costs nothing extra
   in review and matches what a reader searching this history for the
   phrase will already expect.
2. **`ServerConfig`.** Equally reasonable; rejected only to keep the
   established name and avoid a second bikeshed this round does not
   need.

**Constructor shape for TLS.**

1. **Kept separate, composed via `with_tls` — proposed** (`SRV-FR-003`).
   Preserves `TlsConfig::from_env`'s existing fallibility exactly;
   `ServeOptions::from_env` stays infallible, as it is today.
2. **Fold TLS construction into one fallible `ServeOptions::from_env()
   -> Result<Self, ServeOptionsError>`.** Rejected: this is the one
   real behavior change available in this round, and nothing asked
   for it — `dog_server.rs`'s existing two-step pattern (build the
   fallible piece, then `.with_X()` it onto the infallible piece)
   already handles every other conditionally-configured concern this
   way (`rate_limit_from_env_value`, `access_sink_from`,
   `audit_sink_from`, `certificate_classes_from_env_values`); TLS
   fitting the same shape is consistency, not a compromise.

## Proposed shape

```rust
// src/server/mod.rs — renamed from AuthConfig, one field added
pub struct ServeOptions {
    read_only_token: Option<String>,
    read_write_token: Option<String>,
    certificate_classes: Vec<(Vec<u8>, TokenClass)>,
    audit: Option<Arc<dyn audit::AuditSink>>,
    rate_limit: Option<Arc<FailureTable>>,
    access_log: Option<Arc<dyn access::AccessSink>>,
    tls: Option<TlsConfig>,               // new: SRV-FR-001
}

impl ServeOptions {
    pub fn new(read_only_token: Option<String>, read_write_token: Option<String>) -> Self;
    pub fn from_env() -> Self;             // unchanged behavior, SRV-FR-003
    pub fn with_tls(mut self, tls: TlsConfig) -> Self;   // new
    pub fn with_certificate_class(self, leaf_der: Vec<u8>, class: TokenClass) -> Self;   // unchanged
    pub fn with_certificate_class_pem_file(self, path: impl AsRef<Path>, class: TokenClass) -> io::Result<Self>;  // unchanged
    pub fn with_audit(self, sink: Arc<dyn audit::AuditSink>) -> Self;   // unchanged
    pub fn with_rate_limit(self, limit: RateLimit) -> Self;   // unchanged
    pub fn with_access_log(self, sink: Arc<dyn access::AccessSink>) -> Self;   // unchanged
    pub fn audit(&self) -> &dyn audit::AuditSink;   // unchanged
    pub fn rate_limit(&self) -> Option<RateLimit>;   // unchanged
    pub fn access_log(&self) -> &dyn access::AccessSink;   // unchanged
    pub fn is_configured(&self) -> bool;   // unchanged
    pub(crate) fn class_for_certificate(&self, leaf_der: &[u8]) -> Option<TokenClass>;   // unchanged
    fn check(&self, token: &str) -> Option<TokenClass>;   // unchanged, private
    fn is_throttled(&self, peer: Option<IpAddr>) -> bool;   // unchanged, private
    fn note_failure(&self, peer: Option<IpAddr>) -> bool;   // unchanged, private
}

// serve drops to three parameters (SRV-FR-004)
pub fn serve<S: ConnectionStore + 'static>(
    listener: TcpListener,
    store: Arc<S>,
    options: ServeOptions,
);

// handle_connection takes one reference instead of two
fn handle_connection<S: ConnectionStore + ?Sized>(
    stream: TcpStream,
    store: &S,
    options: &ServeOptions,
);
```

`Debug` for `ServeOptions` is the current hand-written `AuthConfig`
impl, unchanged field-for-field, plus one more line for `tls`
(configured/none, never its acceptor or certificate bytes — matching
every other field's own secrecy posture).

`src/bin/dog_server.rs`'s `main` composes exactly as it does today,
with one extra step:

```rust
let tls = /* build_tls_config(), unchanged, still fallible */;
let options = ServeOptions::from_env()
    /* .with_rate_limit(..), .with_audit(..), .with_access_log(..), */
    /* .with_certificate_class(..), as today, each still conditional */
    .with_tls_if(tls);   // or: if let Some(t) = tls { options.with_tls(t) } else { options }
serve(listener, store, options);
```

(The exact `dog_server.rs` composition — whether a `with_tls_if`
convenience helper is worth adding alongside `with_tls`, or a plain
`if let` at the call site suffices — is left to the implementation
unit's judgment; either is a few lines, and `SRV-FR-006` fixes the
observable behavior regardless of which reads better.)

## Data/state and invariants

- Every invariant each of the five concerns already holds
  (`is_configured()`'s certificate-classes-or-tokens logic, the
  fail-open sink posture, `FailureTable`'s bounded eviction, exact-DER
  certificate matching) is unchanged — this design moves fields, it
  does not touch what any of them mean or how they are read.
- **`Clone`, checked precisely rather than assumed.** `AuthConfig`
  derives `Clone` today (`#[derive(Clone, Default)]`), but `serve`
  itself never calls `.clone()` on it — it moves the value once into
  `Arc::new(auth)` — and a repo-wide grep finds no call site that
  clones an `AuthConfig` value either; the derive is unused today, not
  load-bearing. `TlsConfig` does *not* currently derive `Clone` (`serve`
  moves it the same way, into its own `Arc::new(tls)`), though it
  could: its only fields, `rusty_tls::TlsAcceptor` (`#[derive(Clone)]`
  upstream, confirmed by reading `rusty_tls`'s own source) and a `bool`,
  are both `Clone`. Two honest options for `ServeOptions`, left to the
  implementation unit since neither changes any observable behavior:
  keep the `Clone` derive (also deriving it on `TlsConfig`, trivially
  possible) purely for parity with today's `AuthConfig` API surface, or
  drop it (nothing internal needs it, and dropping an unused derive is
  this crate's own stated preference over carrying dead capability).
  Either way, `serve`'s own `Arc::new(options)` needs no `Clone` bound
  at all — a fact this design should not have asserted otherwise.

## Errors, failure, recovery, and observability

- No new failure mode. `TlsConfig` construction keeps its exact
  existing fallibility and error type (`TlsConfigError`); nothing
  about `ServeOptions` itself can fail to construct.
- No change to any sink's fail-open posture, any gate's refusal
  behavior, or any audit/access-log line.

## Security, privacy, and compatibility

- No security-relevant behavior changes — this is a config-surface
  rename plus one field relocation, not a new gate or a changed
  decision. Every existing security property (constant-time token
  comparison, fail-open sinks, exact-DER certificate matching, the
  audit/access-log secrecy invariants) is preserved by construction,
  since the code implementing each is moved, not rewritten.
- Compatibility: this crate is not published (`docs/FUTURE-GROWTH.md`);
  every caller is internal and updated in the same unit. There is no
  external consumer to break.

## Acceptance criteria

1. `ServeOptions` exists with every field and method `SRV-FR-001`–`003`
   describe; `AuthConfig` no longer exists (no alias, no re-export).
2. `serve` takes exactly three parameters (`SRV-FR-004`); `handle_connection`
   takes one `options: &ServeOptions` in place of the two prior
   references.
3. Every existing test passes unmodified in intent — a mechanical
   `AuthConfig` → `ServeOptions` substitution across all ~85 call
   sites, `Option<TlsConfig>` positional arguments replaced with
   `.with_tls(..)`/omitted, with zero assertion changes required
   anywhere (`SRV-FR-006`).
4. `cargo doc --all-features --no-deps` renders `ServeOptions` with
   every field's existing doc comment (the ADR/FR that added it)
   intact, plus one new comment for `tls`.
5. No `Cargo.toml`, wire, `PROTOCOL_VERSION`, or store change; no
   behavior difference observable from outside the process (same
   gates, same audit/access-log lines, same rate-limit computation,
   same TLS handshake requirements).

## Verification plan

- The full existing test suite (`cargo test --all-features`, `cargo
  test` under default features) is the verification: since `SRV-FR-006`
  requires zero behavior change, every current assertion must still
  hold after the rename with no edits beyond the type/parameter names
  themselves. Any assertion that needs a *behavioral* edit to pass
  would be evidence the rename was not, in fact, behavior-preserving.
- `cargo doc --all-features --no-deps`: no new warnings, `ServeOptions`
  renders cleanly in place of `AuthConfig`.
- A `git diff --stat` scoped to confirm the change touches naming/
  parameter-passing sites only — no line inside any gate's decision
  logic, sink's `record`/`line` implementation, or `FailureTable`'s
  eviction logic should appear in the diff.

## Traceability

- → `SERVER-001` next minor / FR (`SRV-FR-001`–`006`), `ADR-0032`;
  resolves `ADR-0029`'s third revisit trigger (*a second cross-cutting
  `serve` option appears*) and `ADR-0031`'s own restatement (*a fourth
  or fifth cross-cutting `AuthConfig` knob appears*) by pointer.
- Roadmap: `SERVER-SERVE-OPTIONS-DESIGN` (this document), then
  `SERVER-SERVE-OPTIONS` as the implementation unit if accepted.

## Open questions

- Whether a `with_tls_if(Option<TlsConfig>)` convenience builder is
  worth adding to `ServeOptions` alongside `with_tls`, purely for
  `dog_server.rs`'s own composition ergonomics — left to the
  implementation unit, not a design decision.
- Whether a sixth cross-cutting concern arriving after this round
  should trigger the same direct "is it time to look at the whole
  list again" question this design asks, or simply hang on
  `ServeOptions` as every prior one did on `AuthConfig` — not decided;
  the same judgment call applies each time it comes up.

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting the
  `ServeOptions` consolidation round over a smaller bounded completion
  (`Admitted.classed_by_certificate`, taken first as `SERVER-001`
  v0.24.0 / FR-034), directly pulling the trigger `ADR-0029` first
  named and `ADR-0030`/`ADR-0031` each restated and deferred. (PR #169.)
- 2026-09-03: Accepted as proposed. No content change. Implementation
  follows as `SERVER-001`'s next minor / FR.
