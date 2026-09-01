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
//! No transport encryption, no transaction semantics, no query language
//! beyond fixed field-tag addressing — all explicit non-goals of the
//! accepted design(s). **Do not expose a server built from this module
//! beyond a trusted, localhost/development network** — see ADR-0010's
//! Consequences; accepting and implementing ADR-0012 (below) closes the
//! authentication/authorization half of that gap, not the transport-
//! encryption half.
//!
//! # Authentication/authorization (`AuthConfig`), ADR-0012
//!
//! `docs/design/SERVER-AUTH-DESIGN.md` closes the "no authentication, no
//! authorization" gap ADR-0010 originally left open: [`serve`] takes an
//! [`AuthConfig`] naming which token(s) (if any) a server instance
//! accepts and the [`TokenClass`] (`ReadOnly`/`ReadWrite`) each grants.
//! `Request::Authenticate` establishes a connection's class; every other
//! request kind is rejected with `ErrorCode::Unauthenticated` until it
//! does, and `ReadOnly` is further rejected from `Request::UpdateField`
//! with `ErrorCode::Unauthorized`. `AuthConfig::default()` (no tokens
//! configured) reproduces exactly the pre-ADR-0012 unauthenticated
//! behavior (`AUTH-FR-007`) — this is purely opt-in. See this module's
//! own `handle_connection` for the full gating logic.
//!
//! # A real, schema-driven client
//!
//! [`client::SchemaDrivenClient`] is the client half of ADR-0011's schema
//! discovery: a real, reusable client that never imports a domain's own
//! `FIELD_*` constants, driving every request purely from what
//! `Request::DescribeSchema` reports at connect time. Unconditional under
//! `server` (not `research`-gated) — it has no domain-specific code at
//! all, only `Request`/`Response`/framing.

pub mod client;
pub mod dog;
#[cfg(feature = "research")]
pub mod employee;
pub mod framing;
#[cfg(feature = "research")]
pub mod order;
pub mod protocol;

use protocol::{
    DomainSchema, ErrorCode, FieldRef, ParentLookup, RecordId, Request, Response, ScanValue,
};
use std::io::{BufReader, BufWriter};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

/// One shared trait the dispatch loop is generic over, implemented by a
/// thin per-domain adapter — the dispatch loop itself never depends on
/// which concrete store it's serving. See [`dog::DogConnectionStore`]
/// (`Neighbors` only), [`order::OrderConnectionStore`] (`Parent`/`Children`
/// only), and [`employee::EmployeeConnectionStore`] (both — the first
/// domain to combine them) for the three domains this crate validates
/// against, matching this project's own "validate against a second,
/// structurally different domain" discipline
/// (`docs/decisions/ADR-0009-generic-schema-design-proposal.md`).
pub trait ConnectionStore: Send + Sync {
    /// Full-record read. `None` if `id` has no record — an ordinary
    /// outcome, not an error, matching [`crate::store::DogStore::get`]'s
    /// own convention.
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>>;

    /// Equality filter on an indexed field. `Err(ErrorCode::UnknownField)`
    /// for a tag this adapter doesn't recognize at all;
    /// `Err(ErrorCode::Unsupported)` for a recognized field with no
    /// equality-index in-process; `Err(ErrorCode::Malformed)` if `value`'s
    /// variant doesn't match the field's real type.
    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode>;

    /// Every record's value for a scannable field, unspecified order —
    /// generalizes `scan_ages`/`ScanField::scan`.
    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode>;

    /// `Ok(true)` if `id` was found and updated, `Ok(false)` if `id` has no
    /// record (an ordinary outcome, matching `update_age`'s own
    /// `NotFound` case at this layer) — `Err` only for a field/value
    /// problem, not a missing record.
    fn update_field(
        &self,
        id: RecordId,
        field: FieldRef,
        value: ScanValue,
    ) -> Result<bool, ErrorCode>;

    /// The "one hop up" side of a directed relation. See
    /// [`ParentLookup`]'s own doc comment for why this preserves the
    /// not-found/no-parent distinction rather than collapsing it.
    /// `Err(ErrorCode::Unsupported)` for a domain with no directed
    /// relation at all (e.g. `Dog`).
    fn parent(&self, id: RecordId) -> Result<ParentLookup, ErrorCode>;

    /// The "one hop down" side of a directed relation.
    /// `Err(ErrorCode::Unsupported)` for a domain with no directed
    /// relation at all.
    fn children(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode>;

    /// A symmetric relation (e.g. `Dog`'s `littermate_of`).
    /// `Err(ErrorCode::Unsupported)` for a domain with no symmetric
    /// relation at all (e.g. `Order`/`Customer`).
    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode>;

    /// This domain's schema, for a client that doesn't know it at compile
    /// time — ADR-0011. Infallible: every `ConnectionStore` implementor
    /// knows its own field/relation shape unconditionally, no store access
    /// needed.
    fn describe(&self) -> DomainSchema;
}

fn err_response(code: ErrorCode) -> Response {
    let message = match code {
        ErrorCode::UnknownField => "unrecognized field tag for this domain",
        ErrorCode::Unsupported => "this operation is not available for this field/domain",
        ErrorCode::Malformed => "the supplied value does not match this field's type",
        ErrorCode::Unauthenticated => "this connection has not presented a recognized token",
        ErrorCode::Unauthorized => "this connection's token does not permit this operation",
    };
    Response::Err {
        code,
        message: message.to_string(),
    }
}

/// The two static permission classes a configured token can grant — see
/// `docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012. Deliberately coarse:
/// `ReadOnly` is blocked only from [`Request::UpdateField`]; both classes
/// can do everything else, including `DescribeSchema` (`AUTH-FR-003`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    ReadOnly,
    ReadWrite,
}

/// Which tokens (if any) this server instance accepts, and what
/// [`TokenClass`] each grants (`AUTH-FR-005`). Built once at server
/// startup and shared (`Arc`) across every connection thread [`serve`]
/// spawns.
///
/// Tokens are never logged or echoed back on any path, including error
/// messages (`AUTH-FR-005`).
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    read_only_token: Option<String>,
    read_write_token: Option<String>,
}

impl AuthConfig {
    /// Build directly from already-known tokens. Use this from tests and
    /// from any caller that already has its tokens from its own config
    /// source — see [`AuthConfig::from_env`]'s own doc comment for why
    /// tests specifically must not use real environment variables.
    pub fn new(read_only_token: Option<String>, read_write_token: Option<String>) -> Self {
        Self {
            read_only_token,
            read_write_token,
        }
    }

    /// Build from `SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN`
    /// at process startup (`AUTH-FR-005`). Reserved for a real server
    /// binary's own one-time startup — `cargo test` runs many tests in
    /// parallel within one process, so reading real process-wide
    /// environment variables from a test would race with every other test
    /// doing the same; tests use [`AuthConfig::new`] instead.
    pub fn from_env() -> Self {
        Self {
            read_only_token: std::env::var("SERVER_AUTH_READ_ONLY_TOKEN").ok(),
            read_write_token: std::env::var("SERVER_AUTH_READ_WRITE_TOKEN").ok(),
        }
    }

    /// No tokens configured at all — `AUTH-FR-007`: every connection
    /// behaves exactly as it did before this feature existed, and
    /// `Authenticate` becomes a no-op success.
    pub fn is_configured(&self) -> bool {
        self.read_only_token.is_some() || self.read_write_token.is_some()
    }

    /// Check `token` against every configured token in constant time
    /// (`AUTH-FR-006`, via the `subtle` crate rather than a hand-rolled
    /// comparison — see this crate's `Cargo.toml` for why). Both slots are
    /// always checked, never short-circuited on the first match, so
    /// neither which slot (if either) matched nor how many slots are
    /// configured is observable from timing.
    fn check(&self, token: &str) -> Option<TokenClass> {
        use subtle::ConstantTimeEq;
        let mut result: Option<TokenClass> = None;
        if let Some(read_write) = &self.read_write_token {
            if bool::from(read_write.as_bytes().ct_eq(token.as_bytes())) {
                result = Some(TokenClass::ReadWrite);
            }
        }
        if let Some(read_only) = &self.read_only_token {
            if bool::from(read_only.as_bytes().ct_eq(token.as_bytes())) {
                result = Some(TokenClass::ReadOnly);
            }
        }
        result
    }
}

/// Translate one [`Request`] into a [`Response`] against `store` — the
/// entire request-handling logic, independent of framing or sockets, kept
/// separate so it can be tested (see this module's tests) without a real
/// TCP connection.
pub fn dispatch<S: ConnectionStore + ?Sized>(store: &S, req: Request) -> Response {
    match req {
        Request::GetById { id } => match store.get(id) {
            Some(fields) => Response::Record { id, fields },
            None => Response::NotFound,
        },
        Request::FilterEq { field, value } => match store.filter_eq(field, &value) {
            Ok(records) => Response::RecordList { records },
            Err(code) => err_response(code),
        },
        Request::ScanField { field } => match store.scan_field(field) {
            Ok(values) => Response::ScanValues { values },
            Err(code) => err_response(code),
        },
        Request::UpdateField { id, field, value } => match store.update_field(id, field, value) {
            Ok(true) => Response::Ok,
            Ok(false) => Response::NotFound,
            Err(code) => err_response(code),
        },
        Request::Parent { id } => match store.parent(id) {
            Ok(ParentLookup::Parent(parent_id)) => Response::Id { id: parent_id },
            Ok(ParentLookup::NoParent) => Response::NoParent,
            Ok(ParentLookup::ChildNotFound) => Response::NotFound,
            Err(code) => err_response(code),
        },
        Request::Children { id } => match store.children(id) {
            Ok(records) => Response::RecordList { records },
            Err(code) => err_response(code),
        },
        Request::Neighbors { id } => match store.neighbors(id) {
            Ok(records) => Response::RecordList { records },
            Err(code) => err_response(code),
        },
        Request::DescribeSchema => Response::Schema(store.describe()),
        // `Authenticate` is intercepted directly by `handle_connection`,
        // which has the per-connection state (and `AuthConfig`) this
        // function has no way to reach — a store has no notion of "this
        // connection". Reaching this arm at all means `handle_connection`
        // let an `Authenticate` request fall through, which never happens
        // in the real dispatch loop; kept exhaustive rather than `_ =>`
        // so a future `Request` variant can't silently skip this decision.
        Request::Authenticate { .. } => err_response(ErrorCode::Unsupported),
    }
}

/// Write `resp` and flush, reporting whether the connection is still
/// usable — folds the write-then-flush-then-check-both boilerplate every
/// response path in [`handle_connection`] needs (there are now several,
/// since auth gating adds early-return response paths that don't go
/// through [`dispatch`]).
fn send_response(writer: &mut BufWriter<TcpStream>, resp: &Response) -> bool {
    if framing::write_message(writer, resp).is_err() {
        return false;
    }
    use std::io::Write;
    writer.flush().is_ok()
}

/// Serve one already-accepted connection until the client disconnects or a
/// framing error occurs. Never panics on a bad client: a malformed or
/// oversized frame ends the connection after (when possible) one
/// [`Response::Err`], never the process — `SERVER-FR-004`.
///
/// # Authentication gating (`AUTH-FR-001`/`AUTH-FR-002`/`AUTH-FR-003`/`AUTH-FR-007`)
///
/// `auth` is checked once per connection, not per request, to decide the
/// starting state: if no tokens are configured at all, the connection
/// starts already authenticated at [`TokenClass::ReadWrite`] and every
/// request is allowed exactly as before this feature existed
/// (`AUTH-FR-007`) — `Request::Authenticate` still round-trips
/// successfully in that case, but as a no-op. Otherwise the connection
/// starts unauthenticated: every request except `Authenticate` is
/// rejected with `ErrorCode::Unauthenticated` (including `DescribeSchema`
/// — `AUTH-FR-002`) until a recognized token is presented, after which its
/// [`TokenClass`] gates only `Request::UpdateField` (`AUTH-FR-003`).
fn handle_connection<S: ConnectionStore + ?Sized>(stream: TcpStream, store: &S, auth: &AuthConfig) {
    // This is a synchronous request/response protocol: each side writes a
    // small frame, then blocks reading the other side's small frame back.
    // Left at its default, Nagle's algorithm delays a small write hoping
    // to coalesce it with more data, which collides with the peer's own
    // delayed-ACK timer — the textbook interaction that turns every
    // round trip into a ~40ms stall. Disabling it is the correct fix for
    // this protocol shape, not just a benchmark convenience: confirmed
    // directly (a concurrent-client integration test went from ~36s to
    // well under a second after this one call).
    let _ = stream.set_nodelay(true);
    let peer_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    let mut writer = BufWriter::new(peer_stream);

    let mut authenticated: Option<TokenClass> = if auth.is_configured() {
        None
    } else {
        Some(TokenClass::ReadWrite)
    };

    loop {
        let req: Request = match framing::read_message(&mut reader) {
            Ok(req) => req,
            Err(_) => return, // client disconnected, or a framing/decode error — end the connection
        };

        if let Request::Authenticate { token } = &req {
            let resp = if !auth.is_configured() {
                Response::Ok
            } else {
                match auth.check(token) {
                    Some(class) => {
                        authenticated = Some(class);
                        Response::Ok
                    }
                    None => err_response(ErrorCode::Unauthenticated),
                }
            };
            if !send_response(&mut writer, &resp) {
                return;
            }
            continue;
        }

        let class = match authenticated {
            Some(class) => class,
            None => {
                if !send_response(&mut writer, &err_response(ErrorCode::Unauthenticated)) {
                    return;
                }
                continue;
            }
        };

        if class == TokenClass::ReadOnly && matches!(req, Request::UpdateField { .. }) {
            if !send_response(&mut writer, &err_response(ErrorCode::Unauthorized)) {
                return;
            }
            continue;
        }

        let resp = dispatch(store, req);
        if !send_response(&mut writer, &resp) {
            return;
        }
    }
}

/// Accept connections on `listener` and serve each one on its own OS
/// thread against the same shared `store` — the thread-per-connection
/// model ADR-0010 chose over an async runtime. Every connection thread
/// takes only `&S`; all coordination is whatever locking `store` already
/// does internally (see this module's own doc comment). `auth` is shared
/// (`Arc`) across every connection thread the same way `store` is — see
/// this module's own `handle_connection` for the gating it performs. Runs until
/// `listener` itself errors (e.g. the socket is closed) or forever
/// otherwise — a real deployment's shutdown/drain story is an explicit
/// non-goal of the accepted design, not solved here.
pub fn serve<S: ConnectionStore + 'static>(listener: TcpListener, store: Arc<S>, auth: AuthConfig) {
    let auth = Arc::new(auth);
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(_) => continue, // one bad accept doesn't take down the server
        };
        let store = Arc::clone(&store);
        let auth = Arc::clone(&auth);
        thread::spawn(move || handle_connection(stream, store.as_ref(), auth.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal in-memory `ConnectionStore` fixture, independent of any
    /// real domain adapter — exercises `dispatch`'s own logic (response
    /// shape per request kind, error-code mapping) without needing
    /// `ProductionStore`/`GenericProductionStore` at all.
    struct FixtureStore;

    const FIELD_A: FieldRef = 0;

    impl ConnectionStore for FixtureStore {
        fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
            if id == RecordId::from_u128(1) {
                Some(vec![(FIELD_A, ScanValue::U32(7))])
            } else {
                None
            }
        }
        fn filter_eq(
            &self,
            field: FieldRef,
            _value: &ScanValue,
        ) -> Result<Vec<RecordId>, ErrorCode> {
            if field == FIELD_A {
                Ok(vec![RecordId::from_u128(1)])
            } else {
                Err(ErrorCode::UnknownField)
            }
        }
        fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
            if field == FIELD_A {
                Ok(vec![ScanValue::U32(7)])
            } else {
                Err(ErrorCode::UnknownField)
            }
        }
        fn update_field(
            &self,
            id: RecordId,
            field: FieldRef,
            value: ScanValue,
        ) -> Result<bool, ErrorCode> {
            match (field, &value) {
                (FIELD_A, ScanValue::U32(_)) => Ok(id == RecordId::from_u128(1)),
                (FIELD_A, _) => Err(ErrorCode::Malformed),
                _ => Err(ErrorCode::UnknownField),
            }
        }
        fn parent(&self, id: RecordId) -> Result<ParentLookup, ErrorCode> {
            if id == RecordId::from_u128(1) {
                Ok(ParentLookup::Parent(RecordId::from_u128(100)))
            } else if id == RecordId::from_u128(2) {
                Ok(ParentLookup::NoParent)
            } else {
                Ok(ParentLookup::ChildNotFound)
            }
        }
        fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
            Ok(vec![RecordId::from_u128(1)])
        }
        fn neighbors(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
            Err(ErrorCode::Unsupported)
        }
        fn describe(&self) -> DomainSchema {
            use protocol::{FieldCapabilities, FieldDescriptor, RelationCapabilities, ValueKind};
            DomainSchema {
                fields: vec![FieldDescriptor {
                    tag: FIELD_A,
                    name: "a".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: true,
                        update: true,
                    },
                }],
                relations: RelationCapabilities {
                    parent_children: true,
                    neighbors: false,
                },
            }
        }
    }

    #[test]
    fn get_by_id_found_and_not_found() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::GetById {
                    id: RecordId::from_u128(1)
                }
            ),
            Response::Record {
                id: RecordId::from_u128(1),
                fields: vec![(FIELD_A, ScanValue::U32(7))],
            }
        );
        assert_eq!(
            dispatch(
                &store,
                Request::GetById {
                    id: RecordId::from_u128(99)
                }
            ),
            Response::NotFound
        );
    }

    #[test]
    fn update_field_maps_found_missing_and_malformed() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::UpdateField {
                    id: RecordId::from_u128(1),
                    field: FIELD_A,
                    value: ScanValue::U32(9)
                }
            ),
            Response::Ok
        );
        assert_eq!(
            dispatch(
                &store,
                Request::UpdateField {
                    id: RecordId::from_u128(99),
                    field: FIELD_A,
                    value: ScanValue::U32(9)
                }
            ),
            Response::NotFound
        );
        assert_eq!(
            dispatch(
                &store,
                Request::UpdateField {
                    id: RecordId::from_u128(1),
                    field: FIELD_A,
                    value: ScanValue::Bool(true)
                }
            ),
            err_response(ErrorCode::Malformed)
        );
    }

    #[test]
    fn parent_preserves_the_not_found_versus_no_parent_distinction() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Parent {
                    id: RecordId::from_u128(1)
                }
            ),
            Response::Id {
                id: RecordId::from_u128(100)
            }
        );
        assert_eq!(
            dispatch(
                &store,
                Request::Parent {
                    id: RecordId::from_u128(2)
                }
            ),
            Response::NoParent
        );
        assert_eq!(
            dispatch(
                &store,
                Request::Parent {
                    id: RecordId::from_u128(3)
                }
            ),
            Response::NotFound
        );
    }

    #[test]
    fn describe_schema_returns_the_fixture_store_own_shape() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(&store, Request::DescribeSchema),
            Response::Schema(store.describe())
        );
    }

    #[test]
    fn unsupported_operation_reports_a_typed_error_not_a_panic() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Neighbors {
                    id: RecordId::from_u128(1)
                }
            ),
            err_response(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn dispatch_never_routes_authenticate_to_a_store() {
        // handle_connection intercepts Authenticate before dispatch is ever
        // called with it — this only documents that dispatch itself stays
        // exhaustive and safe if that invariant were ever violated.
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Authenticate {
                    token: "irrelevant".into()
                }
            ),
            err_response(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn auth_config_default_is_unconfigured() {
        assert!(!AuthConfig::default().is_configured());
        assert_eq!(AuthConfig::default().check("anything"), None);
    }

    #[test]
    fn auth_config_check_maps_each_token_to_its_own_class() {
        let auth = AuthConfig::new(Some("ro-secret".into()), Some("rw-secret".into()));
        assert!(auth.is_configured());
        assert_eq!(auth.check("ro-secret"), Some(TokenClass::ReadOnly));
        assert_eq!(auth.check("rw-secret"), Some(TokenClass::ReadWrite));
        assert_eq!(auth.check("wrong"), None);
        // A prefix or superstring of a real token must not match — rules
        // out an accidental substring/prefix comparison bug.
        assert_eq!(auth.check("ro-secret-extra"), None);
        assert_eq!(auth.check("ro-secre"), None);
    }

    #[test]
    fn auth_config_works_with_only_one_class_configured() {
        let read_only_only = AuthConfig::new(Some("ro-secret".into()), None);
        assert_eq!(
            read_only_only.check("ro-secret"),
            Some(TokenClass::ReadOnly)
        );
        assert_eq!(read_only_only.check("rw-secret"), None);

        let read_write_only = AuthConfig::new(None, Some("rw-secret".into()));
        assert_eq!(
            read_write_only.check("rw-secret"),
            Some(TokenClass::ReadWrite)
        );
        assert_eq!(read_write_only.check("ro-secret"), None);
    }

    /// `AUTH-FR-006`'s empirical half: a wrong token that differs from the
    /// configured one at the very first byte must not check measurably
    /// faster than one that differs only at the very last byte — the
    /// classic signature of an early-exit (non-constant-time) comparison.
    /// Measured directly against `AuthConfig::check` (not over a real TCP
    /// round trip, unlike the rest of this crate's server tests): a
    /// network hop's own jitter (microseconds to milliseconds) would
    /// completely swamp the signal this specific claim is about — a
    /// difference on the order of one byte comparison in a same-length
    /// byte string. Every configured token is checked unconditionally
    /// regardless of position (see `check`'s own doc comment), so this is
    /// expected to hold structurally, not just empirically; the timing
    /// measurement is still real evidence per `SERVER-AUTH-DESIGN.md`'s
    /// own "not just a read-through" verification plan.
    #[test]
    fn token_comparison_time_does_not_depend_on_where_the_mismatch_is() {
        use std::time::Instant;

        let configured = "a".repeat(64);
        let auth = AuthConfig::new(None, Some(configured.clone()));

        let mut differs_at_start = "b".to_string();
        differs_at_start.push_str(&"a".repeat(63));
        let mut differs_at_end = "a".repeat(63);
        differs_at_end.push('b');
        assert_eq!(differs_at_start.len(), configured.len());
        assert_eq!(differs_at_end.len(), configured.len());

        const ITERATIONS: u32 = 20_000;

        // Warm up (first-touch page faults, branch predictor, etc.) before
        // the real measurement, same discipline this crate's own
        // benchmarks already use.
        for _ in 0..1_000 {
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_start)));
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_end)));
        }

        let start_timer = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_start)));
        }
        let start_elapsed = start_timer.elapsed();

        let end_timer = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_end)));
        }
        let end_elapsed = end_timer.elapsed();

        let ratio = start_elapsed.as_secs_f64() / end_elapsed.as_secs_f64().max(1e-12);
        assert!(
            (0.2..5.0).contains(&ratio),
            "mismatch-position timing ratio {ratio} is well outside a noise-only \
             range (first-byte-diff: {start_elapsed:?}, last-byte-diff: {end_elapsed:?}) \
             — investigate for an early-exit comparison"
        );
    }
}
