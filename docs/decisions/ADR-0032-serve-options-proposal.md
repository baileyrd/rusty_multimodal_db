# ADR-0032: Consolidate `AuthConfig` and `TlsConfig` into one `ServeOptions`

- Status: **Proposed**
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-SERVE-OPTIONS-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0025` (the original "no fifth
  `serve` parameter" precedent), `ADR-0029` (first names *a second
  cross-cutting `serve` option appears — `ServeOptions` (option 3)
  becomes worth its breaking change*, and separately *a fourth
  cross-cutting `AuthConfig` knob*), `ADR-0030` (restates, does not
  pull), `ADR-0031` (restates again — *a fourth or fifth cross-cutting
  `AuthConfig` knob appears after this*),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.24.0.
- Supersedes/Superseded by: none. Renames and reshapes the type every
  prior auth/policy ADR (`ADR-0012`, `ADR-0023`, `ADR-0028`–`0031`)
  extended; changes no gate decision, no response, no wire.

## Context

Four separate ADRs have now named the same trigger — `AuthConfig`
accreting cross-cutting concerns beyond authentication, and `TlsConfig`
sitting beside it as `serve`'s own second parameter since v0.9.0 —
and each time judged the marginal addition not worth forcing a
migration for. `AuthConfig` today holds five independent concerns
(tokens, certificate-derived classes, an audit sink, a rate-limit
budget, an access-log sink) under a name that describes only the
first. The trigger has fired four times with the same wording; this
ADR is the first to evaluate the accumulated question directly rather
than the next marginal step.

The owner selected this round over a smaller bounded completion
(`Admitted.classed_by_certificate`, taken first as `SERVER-001` v0.24.0
/ FR-034) when offered the choice.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0031` took.

## Decision drivers

- The trigger has fired four times with the same wording (`ADR-0029`
  ×2, `ADR-0030`, `ADR-0031`); declining to ever evaluate it directly
  makes it permanently rhetorical.
- `AuthConfig`'s name has drifted from its actual scope; a reader
  encountering `with_audit`/`with_rate_limit`/`with_access_log` on a
  type named `AuthConfig` has to already know this history to not be
  confused.
- This crate is not published (`docs/FUTURE-GROWTH.md`) — every caller
  of `AuthConfig`/`TlsConfig` is internal, so a rename/reshape here has
  a fully enumerable, mechanical blast radius (~85 call sites read
  from `main` `1fe23f9`), not an unknown-public breaking change.
- Change no gate decision, no sink behavior, no wire, no
  `PROTOCOL_VERSION`.

## Considered options

1. **Consolidate into one `ServeOptions`, `TlsConfig` folded in as a
   field, `serve` drops to three parameters** — proposed. Matches the
   trigger's own literal phrasing across four ADRs; the flat
   `with_X`-chaining builder style this crate has used for every prior
   addition continues unchanged, only the type's name and one field
   change.
2. **Consolidate the five `AuthConfig` concerns under a new name, but
   leave `TlsConfig` as `serve`'s own second parameter.** A smaller
   diff, but leaves exactly the asymmetry this round exists to
   resolve — `TlsConfig` the one cross-cutting concern still outside
   the consolidated type, for no reason a reader could infer from the
   code alone.
3. **Split by concern instead of consolidating**: identity
   (`ServeOptions`: tokens, certificate classes) separate from
   observability/policy (a second struct: audit, rate limit, access
   log), both passed to `serve`. Rejected — recreates the "many
   `serve` parameters" shape `ADR-0025` first ruled against, with
   fewer, larger parameters instead of many small ones; no net
   simplification, and splits `is_configured()`'s existing logic
   (already reading certificate classes and tokens together) across
   two types for no behavioral reason.
4. **Close as not yet warranted.** Five knobs on one struct is a
   naming smell, not a functional problem — every field still carries
   its own doc comment naming the ADR that added it. A legitimate
   choice if the owner judges the cost of a mechanical ~85-site rename
   not worth paying yet.

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/mod.rs`: `AuthConfig` renamed to `ServeOptions`, gains
  `tls: Option<TlsConfig>`; every existing builder/accessor method
  (`with_certificate_class`, `with_certificate_class_pem_file`,
  `with_audit`, `with_rate_limit`, `with_access_log`, `audit()`,
  `rate_limit()`, `access_log()`, `is_configured()`,
  `class_for_certificate`) keeps its name and behavior unchanged; one
  new method, `with_tls(TlsConfig) -> Self`.
  `ServeOptions::new`/`from_env` keep reading exactly the token/
  certificate environment variables they read today and stay
  infallible; `TlsConfig::from_env` stays its own separately-fallible
  call, composed via `.with_tls(..)` after the caller handles its
  `Result`, exactly the two-step pattern `dog_server.rs`'s `main`
  already uses for every other conditionally-configured piece.
- `pub fn serve<S>(listener: TcpListener, store: Arc<S>, options:
  ServeOptions)` — three parameters, down from four.
  `handle_connection` takes one `options: &ServeOptions` in place of
  its two current `auth`/`tls` references.
- A straight rename, no deprecation shim: no `AuthConfig` alias or
  re-export survives, per this crate's own "no backwards-compatibility
  hacks for an unpublished crate" convention. `TokenClass`,
  `RateLimit`, `RateLimitParseError`, `TlsConfigError`, and every
  sink/type these concerns already use are untouched.
- `Clone` on `ServeOptions`: left to the implementation unit's
  judgment (design's own "Data/state and invariants" — `serve` needs
  no `Clone` bound at all, since it moves the value once into
  `Arc::new`; today's `AuthConfig` derive is unused dead capability,
  not load-bearing).
- No `Cargo.toml`, wire, `PROTOCOL_VERSION`, or store change; every
  existing test passes with a mechanical rename and zero assertion
  changes (`SRV-FR-006`) — the check that this round is, in fact,
  behavior-preserving.

## Consequences

### Positive

- `serve` reaches its simplest possible configuration shape: two
  mechanics parameters, one config.
- The type's name matches its scope for the first time since v0.19.0.
- The trigger four ADRs named is finally evaluated on its own terms,
  not deferred a fifth time by default.

### Negative / tradeoffs

- **A ~85-site mechanical rename** across `src/bin/dog_server.rs`,
  `benches/server.rs`, and eight test files — real diff size, though
  each site is a type-name substitution with no logic change.
- **No behavior verifiable by inspection alone** — the claim "nothing
  observable changes" is only as good as the existing test suite's
  coverage of every gate/sink/limiter path; the verification plan
  leans on that suite passing unmodified as the actual evidence.
- **The next (sixth) cross-cutting concern still needs its own
  judgment call** — this design does not install a mechanism that
  makes future growth automatically fine; it resets the count, it does
  not remove the question.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0031`. Every claim about the current code (`AuthConfig`'s
  field list and call-site count, `TlsConfig`'s shape and
  `rusty_tls::TlsAcceptor`'s own `Clone` derive, `serve`'s exact
  parameter passing) read from `main` `1fe23f9` and, for the upstream
  claim, `rusty_tls`'s own vendored source. No probe: the mechanism is
  a rename and a field move, verified by the existing test suite
  passing unmodified, not new code needing new confidence.
- Revisit if: a sixth cross-cutting concern is proposed after this —
  the same "is it time to look at the whole list again" question this
  round asked applies again, with no presumption either way.
- Revisit if: this crate is ever published — the "internal-only
  caller" argument for treating this as a low-risk rename would no
  longer hold, and any future reshape would need real deprecation
  handling it does not need today.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — consolidate
  into `ServeOptions`, `TlsConfig` folded in, `serve` drops to three
  parameters; **(b)** accept option 2 instead — consolidate the five
  `AuthConfig` concerns under a new name but leave `TlsConfig` as
  `serve`'s own separate parameter; **(c)** close as not warranted —
  `AuthConfig` and `TlsConfig` stand as they are, the trigger restated
  again for whenever a sixth concern arrives. Proposed in this PR.
