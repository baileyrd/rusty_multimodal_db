"""A schema-driven reference client — SERVER-002 §6/§7 (ECO-FR-007/009).

Mirrors ``SchemaDrivenClient``'s posture: ``Hello`` first, then optional
``Authenticate``, then ``DescribeSchema`` (and ``DescribeRelations`` at
protocol 12+); every field addressed by name; capability and version
checks client-side before a frame is sent. Standard library only —
``socket`` for TCP, ``ssl`` for TLS. Sessions are deliberately not
implemented (see ADR-0043's Non-goals). No SQL front end: build
``query``/``aggregate``/``join`` requests directly.
"""

from __future__ import annotations

import socket
import ssl
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Sequence, Tuple
import uuid

from . import protocol as p
from .protocol import PROTOCOL_VERSION


class ClientError(Exception):
    pass


class ServerError(ClientError):
    def __init__(self, code: p.ErrorCode, message: str) -> None:
        super().__init__(f"{code.name}: {message}")
        self.code = code
        self.message = message


class UnsupportedError(ClientError):
    """Refused locally, no frame sent: the schema or the negotiated version rules it out."""


class UnknownFieldError(ClientError):
    pass


class ProtocolError(ClientError):
    """The server answered with a shape this request never produces."""


@dataclass(frozen=True)
class TlsOptions:
    server_name: str
    cafile: Optional[str] = None
    client_cert: Optional[str] = None
    client_key: Optional[str] = None


def _to_scan_value(kind: p.ValueKind, value: Any):
    if isinstance(value, (p.U32, p.I64, p.Bool, p.Str, p.F64, p.StrList)):
        return value
    if kind is p.ValueKind.U32:
        return p.U32(int(value))
    if kind is p.ValueKind.I64:
        return p.I64(int(value))
    if kind is p.ValueKind.Bool:
        return p.Bool(bool(value))
    if kind is p.ValueKind.Str:
        return p.Str(str(value))
    if kind is p.ValueKind.StrList:
        return p.StrList(tuple(str(s) for s in value))
    raise TypeError(kind)


class Client:
    def __init__(self, sock, negotiated: int, schema: p.DomainSchema, relations) -> None:
        self._sock = sock
        self.server_protocol_version = negotiated
        self.schema = schema
        self.relations = relations
        self._by_name: Dict[str, p.FieldDescriptor] = {f.name: f for f in schema.fields}
        self._by_tag: Dict[int, p.FieldDescriptor] = {f.tag: f for f in schema.fields}

    # ---- connection lifecycle ----

    @classmethod
    def connect(
        cls,
        host: str,
        port: int,
        token: Optional[str] = None,
        tls: Optional[TlsOptions] = None,
        protocol_version: int = PROTOCOL_VERSION,
        timeout: Optional[float] = 10.0,
    ) -> "Client":
        raw = socket.create_connection((host, port), timeout=timeout)
        raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        sock = raw
        if tls is not None:
            ctx = ssl.create_default_context(cafile=tls.cafile)
            if tls.client_cert:
                ctx.load_cert_chain(tls.client_cert, tls.client_key)
            sock = ctx.wrap_socket(raw, server_hostname=tls.server_name)
        # Hello is the optional first frame; a server answers min(client, server).
        reply = _exchange(sock, p.Hello(protocol_version))
        if not isinstance(reply, p.HelloResp):
            raise ProtocolError(f"expected Hello, got {type(reply).__name__}")
        negotiated = reply.protocol_version
        if token is not None:
            _expect_ok(_exchange(sock, p.Authenticate(token)))
        schema_reply = _exchange(sock, p.DescribeSchema())
        if not isinstance(schema_reply, p.Schema):
            raise ProtocolError(f"expected Schema, got {type(schema_reply).__name__}")
        relations: List[p.RelationDescriptor] = []
        if negotiated >= 12:
            rel = _exchange(sock, p.DescribeRelations())
            if not isinstance(rel, p.Relations):
                raise ProtocolError(f"expected Relations, got {type(rel).__name__}")
            relations = list(rel.relations)
        return cls(sock, negotiated, schema_reply.schema, relations)

    def close(self) -> None:
        self._sock.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    # ---- helpers ----

    def field(self, name: str) -> p.FieldDescriptor:
        try:
            return self._by_name[name]
        except KeyError:
            raise UnknownFieldError(name) from None

    def _name(self, tag: int) -> str:
        f = self._by_tag.get(tag)
        return f.name if f else str(tag)

    def _named(self, fields) -> List[Tuple[str, Any]]:
        return [(self._name(tag), p.scan_value_py(v)) for tag, v in fields]

    def _gate(self, req) -> None:
        need = p.REQUEST_INTRODUCED_AT[type(req)]
        if self.server_protocol_version < need:
            raise UnsupportedError(
                f"{type(req).__name__} needs protocol {need}, negotiated {self.server_protocol_version}"
            )

    def _roundtrip(self, req):
        self._gate(req)
        reply = _exchange(self._sock, req)
        if isinstance(reply, p.Err):
            raise ServerError(reply.code, reply.message)
        return reply

    def _predicates(self, conditions: Sequence[Tuple[str, p.CompareOp, Any]]) -> List[p.Predicate]:
        out = []
        for name, op, value in conditions:
            f = self.field(name)
            if op not in (p.CompareOp.Eq, p.CompareOp.Ne) and f.value_kind not in (
                p.ValueKind.U32,
                p.ValueKind.I64,
            ):
                raise UnsupportedError(f"{name}: an ordering comparator needs a U32 or I64 field")
            if f.value_kind is p.ValueKind.StrList:
                raise UnsupportedError(f"{name}: a StrList field cannot be filtered")
            out.append(p.Predicate(f.tag, op, _to_scan_value(f.value_kind, value)))
        return out

    def _selection(self, names: Optional[Sequence[str]]):
        if names is None:
            return p.SelectAll()
        return p.SelectFields(tuple(self.field(n).tag for n in names))

    # ---- requests ----

    def get(self, record_id: uuid.UUID) -> Optional[List[Tuple[str, Any]]]:
        reply = self._roundtrip(p.GetById(record_id))
        if isinstance(reply, p.NotFound):
            return None
        if isinstance(reply, p.Record):
            return self._named(reply.fields)
        raise ProtocolError(type(reply).__name__)

    def filter_eq(self, field_name: str, value: Any) -> List[uuid.UUID]:
        f = self.field(field_name)
        if not f.capabilities.filter_eq:
            raise UnsupportedError(f"filter_eq on {field_name}")
        reply = self._roundtrip(p.FilterEq(f.tag, _to_scan_value(f.value_kind, value)))
        if isinstance(reply, p.RecordList):
            return list(reply.records)
        raise ProtocolError(type(reply).__name__)

    def scan(self, field_name: str) -> List[Any]:
        f = self.field(field_name)
        if not f.capabilities.scan:
            raise UnsupportedError(f"scan on {field_name}")
        reply = self._roundtrip(p.ScanField(f.tag))
        if isinstance(reply, p.ScanValues):
            return [p.scan_value_py(v) for v in reply.values]
        raise ProtocolError(type(reply).__name__)

    def update(self, record_id: uuid.UUID, field_name: str, value: Any) -> bool:
        f = self.field(field_name)
        if not f.capabilities.update:
            raise UnsupportedError(f"update on {field_name}")
        reply = self._roundtrip(p.UpdateField(record_id, f.tag, _to_scan_value(f.value_kind, value)))
        if isinstance(reply, p.Ok):
            return True
        if isinstance(reply, p.NotFound):
            return False
        raise ProtocolError(type(reply).__name__)

    def transaction(self, updates: Sequence[Tuple[uuid.UUID, str, Any]]) -> None:
        ops = []
        for record_id, field_name, value in updates:
            f = self.field(field_name)
            if not f.capabilities.update:
                raise UnsupportedError(f"update on {field_name}")
            ops.append(p.TransactionOp(record_id, f.tag, _to_scan_value(f.value_kind, value)))
        reply = self._roundtrip(p.Transaction(tuple(ops)))
        if isinstance(reply, p.Ok):
            return
        if isinstance(reply, p.TransactionFailed):
            raise ServerError(reply.code, f"operation {reply.index}: {reply.message}")
        raise ProtocolError(type(reply).__name__)

    def parent(self, record_id: uuid.UUID) -> Optional[uuid.UUID]:
        if not self.schema.relations.parent_children:
            raise UnsupportedError("Parent on this domain")
        reply = self._roundtrip(p.ParentReq(record_id))
        if isinstance(reply, p.Id):
            return reply.id
        if isinstance(reply, (p.NoParent, p.NotFound)):
            return None
        raise ProtocolError(type(reply).__name__)

    def children(self, record_id: uuid.UUID) -> List[uuid.UUID]:
        if not self.schema.relations.parent_children:
            raise UnsupportedError("Children on this domain")
        return self._record_list(p.ChildrenReq(record_id))

    def neighbors(self, record_id: uuid.UUID, relation: Optional[str] = None) -> List[uuid.UUID]:
        if not self.schema.relations.neighbors:
            raise UnsupportedError("Neighbors on this domain")
        if relation is None:
            return self._record_list(p.NeighborsReq(record_id))
        return self._record_list(p.NeighborsByRelation(record_id, relation))

    def _record_list(self, req) -> List[uuid.UUID]:
        reply = self._roundtrip(req)
        if isinstance(reply, p.RecordList):
            return list(reply.records)
        raise ProtocolError(type(reply).__name__)

    def relation_kinds(self) -> List[str]:
        reply = self._roundtrip(p.ListRelationKinds())
        if isinstance(reply, p.RelationKinds):
            return list(reply.kinds)
        raise ProtocolError(type(reply).__name__)

    def query(
        self,
        select: Optional[Sequence[str]] = None,
        where: Sequence[Tuple[str, p.CompareOp, Any]] = (),
        limit: Optional[int] = None,
    ) -> List[Tuple[uuid.UUID, List[Tuple[str, Any]]]]:
        reply = self._roundtrip(p.Query(self._selection(select), tuple(self._predicates(where)), limit))
        if isinstance(reply, p.Rows):
            return [(rid, self._named(fields)) for rid, fields in reply.rows]
        raise ProtocolError(type(reply).__name__)

    def aggregate(
        self,
        group_by: Sequence[str],
        aggregates: Sequence[Tuple[p.AggregateFn, Optional[str]]],
        where: Sequence[Tuple[str, p.CompareOp, Any]] = (),
        limit: Optional[int] = None,
    ) -> List[Tuple[List[Tuple[str, Any]], List[Any]]]:
        specs = []
        for func, name in aggregates:
            if func is p.AggregateFn.Count:
                if name is not None:
                    raise UnsupportedError("COUNT takes no field (COUNT(*) only)")
                specs.append(p.AggregateSpec(func, None))
            else:
                f = self.field(name)
                if f.value_kind not in (p.ValueKind.U32, p.ValueKind.I64):
                    raise UnsupportedError(f"{func.name} needs a U32 or I64 field")
                specs.append(p.AggregateSpec(func, f.tag))
        for name in group_by:
            if self.field(name).value_kind is p.ValueKind.StrList:
                raise UnsupportedError(f"GROUP BY {name}: a StrList field is not groupable")
        reply = self._roundtrip(
            p.Aggregate(
                tuple(self.field(n).tag for n in group_by),
                tuple(self._predicates(where)),
                tuple(specs),
                limit,
            )
        )
        if isinstance(reply, p.Groups):
            return [(self._named(g.key), [p.scan_value_py(v) for v in g.values]) for g in reply.groups]
        raise ProtocolError(type(reply).__name__)

    def join(
        self,
        relation: str,
        left_select: Optional[Sequence[str]] = None,
        right_select: Optional[Sequence[str]] = None,
        left_where: Sequence[Tuple[str, p.CompareOp, Any]] = (),
        right_where: Sequence[Tuple[str, p.CompareOp, Any]] = (),
        limit: Optional[int] = None,
    ) -> List[Tuple[uuid.UUID, List[Tuple[str, Any]], uuid.UUID, List[Tuple[str, Any]]]]:
        descriptor = next((r for r in self.relations if r.name == relation), None)
        if descriptor is None:
            raise UnsupportedError(f"ON {relation}: not a relation this domain lists")
        if descriptor.target_table is not None:
            raise UnsupportedError(f"ON {relation}: its rows live in another table")
        spec = p.JoinSpec(
            descriptor.kind,
            None,
            self._selection(left_select),
            self._selection(right_select),
            tuple(self._predicates(left_where)),
            tuple(self._predicates(right_where)),
            limit,
        )
        reply = self._roundtrip(p.Join(spec))
        if isinstance(reply, p.JoinedRows):
            return [
                (r.left_id, self._named(r.left), r.right_id, self._named(r.right)) for r in reply.rows
            ]
        raise ProtocolError(type(reply).__name__)


def _exchange(sock, req):
    sock.sendall(p.frame(p.encode_request(req)))
    return p.decode_response(p.read_frame(sock))


def _expect_ok(reply) -> None:
    if isinstance(reply, p.Ok):
        return
    if isinstance(reply, p.Err):
        raise ServerError(reply.code, reply.message)
    raise ProtocolError(f"expected Ok, got {type(reply).__name__}")
