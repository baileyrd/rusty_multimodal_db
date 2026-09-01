//! A schema-driven client: connects to a server/query-layer instance,
//! calls `Request::DescribeSchema` once at connect time, and drives every
//! subsequent request by field *name* rather than a compile-time-known
//! `FieldRef` constant — the client half of ADR-0011's own "genuinely
//! usable, not just descriptive" bar. `tests/server_dog_integration.rs`'s,
//! `tests/server_order_integration.rs`'s, and
//! `tests/server_employee_integration.rs`'s own schema-driven tests each
//! proved this is possible with test-local, one-off code; this module is
//! that logic promoted into real, reusable library API, so a future
//! caller (or those same tests) doesn't have to reimplement it by hand.
//!
//! # What "doesn't know a domain at compile time" means here
//!
//! [`SchemaDrivenClient`] never imports a `FIELD_*` constant from
//! [`super::dog`]/[`super::order`]/[`super::employee`] — every field is
//! addressed by the `&str` name [`Request::DescribeSchema`]'s response
//! names it. [`SchemaDrivenClient::get`] returns `(String, ScanValue)`
//! pairs, not `(FieldRef, ScanValue)`, for the same reason: a caller with
//! no compile-time schema knowledge has nothing useful to do with a bare
//! `u16` tag on its own.
//!
//! # Capability checks happen client-side first
//!
//! [`DomainSchema`]'s own `FieldCapabilities`/`RelationCapabilities` are
//! already trustworthy — every `ConnectionStore` implementor's
//! `describe()` is required to report its real, honest shape
//! (`SERVER-001-FR-010`). [`SchemaDrivenClient::filter_eq`]/`scan`/
//! `update`/`parent`/`children`/`neighbors` all check the relevant
//! capability locally before sending anything, returning
//! [`ClientError::Unsupported`] without paying a round trip for something
//! the schema already ruled out. This is an optimization, not a trust
//! boundary: the server's own `dispatch` (`src/server/mod.rs`) still
//! enforces the identical rules independently, so a client that skipped
//! this check (or a different, buggy client) would still get back a
//! typed `Response::Err`, never undefined behavior.

use super::framing::{self, FrameError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldDescriptor, ParentLookup, RecordId, Request, Response, ScanValue,
};
use std::fmt;
use std::io::{BufReader, BufWriter, Write};
use std::net::{TcpStream, ToSocketAddrs};

/// Everything that can go wrong driving a [`SchemaDrivenClient`]: framing/
/// I/O failure, a field name the discovered schema doesn't have, an
/// operation this domain's schema doesn't support (checked locally before
/// sending — see this module's own doc comment), the server's own typed
/// [`ErrorCode`], or a response shape that doesn't match what the request
/// kind should have produced (a `dispatch` bug, not something a correct
/// server should ever send).
#[derive(Debug)]
pub enum ClientError {
    Frame(FrameError),
    UnknownField(String),
    Unsupported(&'static str),
    Server(ErrorCode, String),
    UnexpectedResponse(&'static str),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Frame(e) => write!(f, "framing error: {e}"),
            ClientError::UnknownField(name) => {
                write!(f, "no field named {name:?} in this domain's schema")
            }
            ClientError::Unsupported(what) => {
                write!(f, "{what} is not supported by this domain's schema")
            }
            ClientError::Server(code, message) => write!(f, "server error {code:?}: {message}"),
            ClientError::UnexpectedResponse(expected) => {
                write!(f, "expected a {expected} response, got a different shape")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<FrameError> for ClientError {
    fn from(e: FrameError) -> Self {
        ClientError::Frame(e)
    }
}

/// A real client to one server/query-layer domain, built entirely from
/// what [`Request::DescribeSchema`] reports at connect time — see this
/// module's own doc comment.
pub struct SchemaDrivenClient {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    schema: DomainSchema,
}

impl SchemaDrivenClient {
    /// Connects, disables Nagle's algorithm (`SERVER-001-FR-006` — see
    /// `src/server/mod.rs`'s own doc comment for why this isn't optional
    /// for this protocol's synchronous request/response shape), then
    /// immediately sends `Request::DescribeSchema` and keeps the result
    /// for every subsequent field-name lookup this client does.
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(addr).map_err(FrameError::from)?;
        stream.set_nodelay(true).map_err(FrameError::from)?;
        let peer = stream.try_clone().map_err(FrameError::from)?;
        let mut reader = BufReader::new(stream);
        let mut writer = BufWriter::new(peer);

        framing::write_message(&mut writer, &Request::DescribeSchema)?;
        writer.flush().map_err(FrameError::from)?;
        let schema = match framing::read_message(&mut reader)? {
            Response::Schema(schema) => schema,
            _ => return Err(ClientError::UnexpectedResponse("Schema")),
        };

        Ok(Self {
            reader,
            writer,
            schema,
        })
    }

    /// The schema discovered at connect time — every field's name, wire
    /// type, and per-operation capability, plus which relation kinds this
    /// domain supports.
    pub fn schema(&self) -> &DomainSchema {
        &self.schema
    }

    fn field(&self, name: &str) -> Result<&FieldDescriptor, ClientError> {
        self.schema
            .fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| ClientError::UnknownField(name.to_string()))
    }

    fn roundtrip(&mut self, req: Request) -> Result<Response, ClientError> {
        framing::write_message(&mut self.writer, &req)?;
        self.writer.flush().map_err(FrameError::from)?;
        Ok(framing::read_message(&mut self.reader)?)
    }

    /// Full-record read, `None` if `id` has no record. Fields come back
    /// named, not tagged — `(name, value)` pairs, in whatever order the
    /// server returned them (fixed per adapter, not re-sorted here).
    pub fn get(&mut self, id: RecordId) -> Result<Option<Vec<(String, ScanValue)>>, ClientError> {
        match self.roundtrip(Request::GetById { id })? {
            Response::Record { fields, .. } => {
                let named = fields
                    .into_iter()
                    .map(|(tag, value)| {
                        let name = self
                            .schema
                            .fields
                            .iter()
                            .find(|f| f.tag == tag)
                            .map(|f| f.name.clone())
                            .unwrap_or_else(|| tag.to_string());
                        (name, value)
                    })
                    .collect();
                Ok(Some(named))
            }
            Response::NotFound => Ok(None),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Record or NotFound")),
        }
    }

    /// Equality filter by field name. Checked against the discovered
    /// schema first — `Err(ClientError::Unsupported)`, no round trip, if
    /// this field isn't `filter_eq`-capable.
    pub fn filter_eq(
        &mut self,
        field_name: &str,
        value: ScanValue,
    ) -> Result<Vec<RecordId>, ClientError> {
        let field = self.field(field_name)?;
        if !field.capabilities.filter_eq {
            return Err(ClientError::Unsupported("filter_eq on this field"));
        }
        let tag = field.tag;
        match self.roundtrip(Request::FilterEq { field: tag, value })? {
            Response::RecordList { records } => Ok(records),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("RecordList")),
        }
    }

    /// Every record's value for a scannable field, by name.
    pub fn scan(&mut self, field_name: &str) -> Result<Vec<ScanValue>, ClientError> {
        let field = self.field(field_name)?;
        if !field.capabilities.scan {
            return Err(ClientError::Unsupported("scan on this field"));
        }
        let tag = field.tag;
        match self.roundtrip(Request::ScanField { field: tag })? {
            Response::ScanValues { values } => Ok(values),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("ScanValues")),
        }
    }

    /// `Ok(true)` if `id` was found and updated, `Ok(false)` if `id` has
    /// no record.
    pub fn update(
        &mut self,
        id: RecordId,
        field_name: &str,
        value: ScanValue,
    ) -> Result<bool, ClientError> {
        let field = self.field(field_name)?;
        if !field.capabilities.update {
            return Err(ClientError::Unsupported("update on this field"));
        }
        let tag = field.tag;
        match self.roundtrip(Request::UpdateField {
            id,
            field: tag,
            value,
        })? {
            Response::Ok => Ok(true),
            Response::NotFound => Ok(false),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Ok or NotFound")),
        }
    }

    /// The directed relation's "one hop up" — see [`ParentLookup`]'s own
    /// doc comment for the three-way not-found/no-parent/parent
    /// distinction this preserves. `Err(ClientError::Unsupported)`
    /// locally if this domain has no directed relation at all.
    pub fn parent(&mut self, id: RecordId) -> Result<ParentLookup, ClientError> {
        if !self.schema.relations.parent_children {
            return Err(ClientError::Unsupported("Parent on this domain"));
        }
        match self.roundtrip(Request::Parent { id })? {
            Response::Id { id } => Ok(ParentLookup::Parent(id)),
            Response::NoParent => Ok(ParentLookup::NoParent),
            Response::NotFound => Ok(ParentLookup::ChildNotFound),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Id, NoParent, or NotFound")),
        }
    }

    /// The directed relation's "one hop down".
    pub fn children(&mut self, id: RecordId) -> Result<Vec<RecordId>, ClientError> {
        if !self.schema.relations.parent_children {
            return Err(ClientError::Unsupported("Children on this domain"));
        }
        match self.roundtrip(Request::Children { id })? {
            Response::RecordList { records } => Ok(records),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("RecordList")),
        }
    }

    /// The symmetric relation.
    pub fn neighbors(&mut self, id: RecordId) -> Result<Vec<RecordId>, ClientError> {
        if !self.schema.relations.neighbors {
            return Err(ClientError::Unsupported("Neighbors on this domain"));
        }
        match self.roundtrip(Request::Neighbors { id })? {
            Response::RecordList { records } => Ok(records),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("RecordList")),
        }
    }
}
