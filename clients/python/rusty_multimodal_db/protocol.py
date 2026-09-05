"""Every wire shape at protocol version 12 — SERVER-002 §5 (ECO-FR-007).

One class per request/response variant and per struct, each with a
declarative ``_spec`` (field name, type spec) that ``encode``/``decode``
walk. Type specs: ``"u16"``, ``"u32"``, ``"u64"``, ``"i64"``, ``"f64"``,
``"bool"``, ``"str"``, ``"uuid"``, ``("opt", T)``, ``("vec", T)``,
``("tuple", T1, T2)``, an ``IntEnum`` subclass (a fieldless enum: its
``u32`` index), a struct class, or an *enum family* (a list of variant
classes in index order — the variant's index as a ``u32``, then its
fields). Exactly the layout ``src/server/protocol.rs`` derives.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field as dc_field
from enum import IntEnum
from typing import Any, ClassVar, List, Optional, Tuple
import uuid

from .codec import CodecError, Reader, Writer

PROTOCOL_VERSION = 12
MAX_FRAME_BYTES = 16 * 1024 * 1024

SESSION_READ_YOUR_WRITES = 1
SESSION_VALIDATE_ON_STAGE = 2
SESSION_SNAPSHOT_ISOLATION = 4


# ---- fieldless enums (a u32 index on the wire) ----


class ValueKind(IntEnum):
    U32 = 0
    I64 = 1
    Bool = 2
    Str = 3
    StrList = 4


class CompareOp(IntEnum):
    Eq = 0
    Ne = 1
    Lt = 2
    Le = 3
    Gt = 4
    Ge = 5


class AggregateFn(IntEnum):
    Count = 0
    Sum = 1
    Avg = 2
    Min = 3
    Max = 4


class ErrorCode(IntEnum):
    UnknownField = 0
    Unsupported = 1
    Malformed = 2
    Unauthenticated = 3
    Unauthorized = 4
    RecordNotFound = 5
    NoSession = 6
    SessionOpen = 7
    SessionFull = 8
    Journal = 9
    Conflict = 10


# ---- ScanValue (enum family) ----


@dataclass(frozen=True)
class U32:
    value: int
    _index: ClassVar[int] = 0
    _spec: ClassVar[list] = [("value", "u32")]


@dataclass(frozen=True)
class I64:
    value: int
    _index: ClassVar[int] = 1
    _spec: ClassVar[list] = [("value", "i64")]


@dataclass(frozen=True)
class Bool:
    value: bool
    _index: ClassVar[int] = 2
    _spec: ClassVar[list] = [("value", "bool")]


@dataclass(frozen=True)
class Str:
    value: str
    _index: ClassVar[int] = 3
    _spec: ClassVar[list] = [("value", "str")]


@dataclass(frozen=True)
class F64:
    value: float
    _index: ClassVar[int] = 4
    _spec: ClassVar[list] = [("value", "f64")]


@dataclass(frozen=True)
class StrList:
    value: Tuple[str, ...]
    _index: ClassVar[int] = 5
    _spec: ClassVar[list] = [("value", ("vec", "str"))]

    def __post_init__(self):
        object.__setattr__(self, "value", tuple(self.value))


ScanValue = [U32, I64, Bool, Str, F64, StrList]


def scan_value_py(v) -> Any:
    """The plain Python value inside a ScanValue (list for StrList)."""
    return list(v.value) if isinstance(v, StrList) else v.value


# ---- Selection, JoinRelation (enum families) ----


@dataclass(frozen=True)
class SelectAll:
    _index: ClassVar[int] = 0
    _spec: ClassVar[list] = []


@dataclass(frozen=True)
class SelectFields:
    fields: Tuple[int, ...]
    _index: ClassVar[int] = 1
    _spec: ClassVar[list] = [("fields", ("vec", "u16"))]

    def __post_init__(self):
        object.__setattr__(self, "fields", tuple(self.fields))


Selection = [SelectAll, SelectFields]


@dataclass(frozen=True)
class Neighbors:
    label: Optional[str] = None
    _index: ClassVar[int] = 0
    _spec: ClassVar[list] = [("label", ("opt", "str"))]


@dataclass(frozen=True)
class Parent:
    _index: ClassVar[int] = 1
    _spec: ClassVar[list] = []


@dataclass(frozen=True)
class Children:
    _index: ClassVar[int] = 2
    _spec: ClassVar[list] = []


JoinRelation = [Neighbors, Parent, Children]


# ---- structs ----


@dataclass(frozen=True)
class Predicate:
    field: int
    op: CompareOp
    value: Any  # a ScanValue
    _spec: ClassVar[list] = [("field", "u16"), ("op", CompareOp), ("value", ScanValue)]


@dataclass(frozen=True)
class TransactionOp:
    id: uuid.UUID
    field: int
    value: Any
    _spec: ClassVar[list] = [("id", "uuid"), ("field", "u16"), ("value", ScanValue)]


@dataclass(frozen=True)
class AggregateSpec:
    func: AggregateFn
    field: Optional[int]
    _spec: ClassVar[list] = [("func", AggregateFn), ("field", ("opt", "u16"))]


@dataclass(frozen=True)
class AggregateGroup:
    key: Tuple[Tuple[int, Any], ...]
    values: Tuple[Any, ...]
    _spec: ClassVar[list] = [
        ("key", ("vec", ("tuple", "u16", ScanValue))),
        ("values", ("vec", ScanValue)),
    ]

    def __post_init__(self):
        object.__setattr__(self, "key", tuple(tuple(k) for k in self.key))
        object.__setattr__(self, "values", tuple(self.values))


@dataclass(frozen=True)
class FieldCapabilities:
    filter_eq: bool
    scan: bool
    update: bool
    _spec: ClassVar[list] = [("filter_eq", "bool"), ("scan", "bool"), ("update", "bool")]


@dataclass(frozen=True)
class FieldDescriptor:
    tag: int
    name: str
    value_kind: ValueKind
    capabilities: FieldCapabilities
    _spec: ClassVar[list] = [
        ("tag", "u16"),
        ("name", "str"),
        ("value_kind", ValueKind),
        ("capabilities", FieldCapabilities),
    ]


@dataclass(frozen=True)
class RelationCapabilities:
    parent_children: bool
    neighbors: bool
    _spec: ClassVar[list] = [("parent_children", "bool"), ("neighbors", "bool")]


@dataclass(frozen=True)
class DomainSchema:
    fields: Tuple[FieldDescriptor, ...]
    relations: RelationCapabilities
    _spec: ClassVar[list] = [("fields", ("vec", FieldDescriptor)), ("relations", RelationCapabilities)]

    def __post_init__(self):
        object.__setattr__(self, "fields", tuple(self.fields))


@dataclass(frozen=True)
class JoinSpec:
    relation: Any  # a JoinRelation
    right_table: Optional[str]
    left: Any  # a Selection
    right: Any
    left_filter: Tuple[Predicate, ...]
    right_filter: Tuple[Predicate, ...]
    limit: Optional[int]
    _spec: ClassVar[list] = [
        ("relation", JoinRelation),
        ("right_table", ("opt", "str")),
        ("left", Selection),
        ("right", Selection),
        ("left_filter", ("vec", Predicate)),
        ("right_filter", ("vec", Predicate)),
        ("limit", ("opt", "u64")),
    ]

    def __post_init__(self):
        object.__setattr__(self, "left_filter", tuple(self.left_filter))
        object.__setattr__(self, "right_filter", tuple(self.right_filter))


@dataclass(frozen=True)
class JoinedRow:
    left_id: uuid.UUID
    left: Tuple[Tuple[int, Any], ...]
    right_id: uuid.UUID
    right: Tuple[Tuple[int, Any], ...]
    _spec: ClassVar[list] = [
        ("left_id", "uuid"),
        ("left", ("vec", ("tuple", "u16", ScanValue))),
        ("right_id", "uuid"),
        ("right", ("vec", ("tuple", "u16", ScanValue))),
    ]

    def __post_init__(self):
        object.__setattr__(self, "left", tuple(tuple(k) for k in self.left))
        object.__setattr__(self, "right", tuple(tuple(k) for k in self.right))


@dataclass(frozen=True)
class RelationDescriptor:
    name: str
    kind: Any  # a JoinRelation
    target_table: Optional[str]
    _spec: ClassVar[list] = [("name", "str"), ("kind", JoinRelation), ("target_table", ("opt", "str"))]


# ---- Request (enum family, indices 0..20) ----


def _variant(index, spec):
    def deco(cls):
        cls._index = index
        cls._spec = spec
        return dataclass(frozen=True)(cls)

    return deco


FIELDS = ("vec", ("tuple", "u16", ScanValue))


@_variant(0, [("id", "uuid")])
class GetById:
    id: uuid.UUID


@_variant(1, [("field", "u16"), ("value", ScanValue)])
class FilterEq:
    field: int
    value: Any


@_variant(2, [("field", "u16")])
class ScanField:
    field: int


@_variant(3, [("id", "uuid"), ("field", "u16"), ("value", ScanValue)])
class UpdateField:
    id: uuid.UUID
    field: int
    value: Any


@_variant(4, [("id", "uuid")])
class ParentReq:
    id: uuid.UUID


@_variant(5, [("id", "uuid")])
class ChildrenReq:
    id: uuid.UUID


@_variant(6, [("id", "uuid")])
class NeighborsReq:
    id: uuid.UUID


@_variant(7, [])
class DescribeSchema:
    pass


@_variant(8, [("token", "str")])
class Authenticate:
    token: str


@_variant(9, [("updates", ("vec", TransactionOp))])
class Transaction:
    updates: Tuple[TransactionOp, ...]

    def __post_init__(self):
        object.__setattr__(self, "updates", tuple(self.updates))


@_variant(10, [("protocol_version", "u32")])
class Hello:
    protocol_version: int


@_variant(11, [])
class Begin:
    pass


@_variant(12, [])
class Commit:
    pass


@_variant(13, [])
class Rollback:
    pass


@_variant(14, [("flags", "u32")])
class BeginWith:
    flags: int


@_variant(15, [("select", Selection), ("filter", ("vec", Predicate)), ("limit", ("opt", "u64"))])
class Query:
    select: Any
    filter: Tuple[Predicate, ...]
    limit: Optional[int]

    def __post_init__(self):
        object.__setattr__(self, "filter", tuple(self.filter))


@_variant(
    16,
    [
        ("group_by", ("vec", "u16")),
        ("filter", ("vec", Predicate)),
        ("aggregates", ("vec", AggregateSpec)),
        ("limit", ("opt", "u64")),
    ],
)
class Aggregate:
    group_by: Tuple[int, ...]
    filter: Tuple[Predicate, ...]
    aggregates: Tuple[AggregateSpec, ...]
    limit: Optional[int]

    def __post_init__(self):
        object.__setattr__(self, "group_by", tuple(self.group_by))
        object.__setattr__(self, "filter", tuple(self.filter))
        object.__setattr__(self, "aggregates", tuple(self.aggregates))


@_variant(17, [("id", "uuid"), ("relation", "str")])
class NeighborsByRelation:
    id: uuid.UUID
    relation: str


@_variant(18, [])
class ListRelationKinds:
    pass


@_variant(19, [("spec", JoinSpec)])
class Join:
    spec: JoinSpec


@_variant(20, [])
class DescribeRelations:
    pass


Request = [
    GetById, FilterEq, ScanField, UpdateField, ParentReq, ChildrenReq, NeighborsReq,
    DescribeSchema, Authenticate, Transaction, Hello, Begin, Commit, Rollback, BeginWith,
    Query, Aggregate, NeighborsByRelation, ListRelationKinds, Join, DescribeRelations,
]

# The protocol version each request first appeared at (compatibility rule
# 4: never send one below its version) — SERVER-002 §8.
REQUEST_INTRODUCED_AT = {
    GetById: 1, FilterEq: 1, ScanField: 1, UpdateField: 1, ParentReq: 1, ChildrenReq: 1,
    NeighborsReq: 1, DescribeSchema: 1, Authenticate: 1, Transaction: 1, Hello: 2,
    Begin: 3, Commit: 3, Rollback: 3, BeginWith: 5, Query: 8, Aggregate: 9,
    NeighborsByRelation: 10, ListRelationKinds: 10, Join: 12, DescribeRelations: 12,
}


# ---- Response (enum family, indices 0..16) ----


@_variant(0, [("id", "uuid"), ("fields", FIELDS)])
class Record:
    id: uuid.UUID
    fields: Tuple[Tuple[int, Any], ...]

    def __post_init__(self):
        object.__setattr__(self, "fields", tuple(tuple(f) for f in self.fields))


@_variant(1, [("records", ("vec", "uuid"))])
class RecordList:
    records: Tuple[uuid.UUID, ...]

    def __post_init__(self):
        object.__setattr__(self, "records", tuple(self.records))


@_variant(2, [("values", ("vec", ScanValue))])
class ScanValues:
    values: Tuple[Any, ...]

    def __post_init__(self):
        object.__setattr__(self, "values", tuple(self.values))


@_variant(3, [("id", "uuid")])
class Id:
    id: uuid.UUID


@_variant(4, [("schema", DomainSchema)])
class Schema:
    schema: DomainSchema


@_variant(5, [])
class NotFound:
    pass


@_variant(6, [])
class NoParent:
    pass


@_variant(7, [])
class Ok:
    pass


@_variant(8, [("code", ErrorCode), ("message", "str")])
class Err:
    code: ErrorCode
    message: str


@_variant(9, [("index", "u64"), ("code", ErrorCode), ("message", "str")])
class TransactionFailed:
    index: int
    code: ErrorCode
    message: str


@_variant(10, [("protocol_version", "u32")])
class HelloResp:
    protocol_version: int


@_variant(11, [("index", "u32")])
class Staged:
    index: int


@_variant(12, [("rows", ("vec", ("tuple", "uuid", FIELDS)))])
class Rows:
    rows: Tuple[Tuple[uuid.UUID, Tuple[Tuple[int, Any], ...]], ...]

    def __post_init__(self):
        object.__setattr__(
            self, "rows", tuple((rid, tuple(tuple(f) for f in fields)) for rid, fields in self.rows)
        )


@_variant(13, [("groups", ("vec", AggregateGroup))])
class Groups:
    groups: Tuple[AggregateGroup, ...]

    def __post_init__(self):
        object.__setattr__(self, "groups", tuple(self.groups))


@_variant(14, [("kinds", ("vec", "str"))])
class RelationKinds:
    kinds: Tuple[str, ...]

    def __post_init__(self):
        object.__setattr__(self, "kinds", tuple(self.kinds))


@_variant(15, [("rows", ("vec", JoinedRow))])
class JoinedRows:
    rows: Tuple[JoinedRow, ...]

    def __post_init__(self):
        object.__setattr__(self, "rows", tuple(self.rows))


@_variant(16, [("relations", ("vec", RelationDescriptor))])
class Relations:
    relations: Tuple[RelationDescriptor, ...]

    def __post_init__(self):
        object.__setattr__(self, "relations", tuple(self.relations))


Response = [
    Record, RecordList, ScanValues, Id, Schema, NotFound, NoParent, Ok, Err, TransactionFailed,
    HelloResp, Staged, Rows, Groups, RelationKinds, JoinedRows, Relations,
]


# ---- the spec walker ----


def _is_family(spec) -> bool:
    return isinstance(spec, list)


def _is_struct(spec) -> bool:
    return isinstance(spec, type) and hasattr(spec, "_spec") and not hasattr(spec, "_index")


def _encode_value(w: Writer, spec, value) -> None:
    if isinstance(spec, str):
        getattr(w, {"str": "string"}.get(spec, spec))(value)
    elif isinstance(spec, tuple):
        kind = spec[0]
        if kind == "opt":
            w.option(value, lambda v: _encode_value(w, spec[1], v))
        elif kind == "vec":
            w.vec(value, lambda v: _encode_value(w, spec[1], v))
        elif kind == "tuple":
            for sub, v in zip(spec[1:], value):
                _encode_value(w, sub, v)
        else:
            raise TypeError(spec)
    elif _is_family(spec):
        cls = type(value)
        if cls not in spec:
            raise TypeError(f"{cls.__name__} is not a variant of this enum")
        w.enum_index(cls._index)
        _encode_fields(w, cls._spec, value)
    elif isinstance(spec, type) and issubclass(spec, IntEnum):
        w.enum_index(int(value))
    elif _is_struct(spec):
        _encode_fields(w, spec._spec, value)
    else:
        raise TypeError(spec)


def _encode_fields(w: Writer, fields, value) -> None:
    for name, sub in fields:
        _encode_value(w, sub, getattr(value, name))


def _decode_value(r: Reader, spec):
    if isinstance(spec, str):
        return getattr(r, {"str": "string"}.get(spec, spec))()
    if isinstance(spec, tuple):
        kind = spec[0]
        if kind == "opt":
            return r.option(lambda: _decode_value(r, spec[1]))
        if kind == "vec":
            return r.vec(lambda: _decode_value(r, spec[1]))
        if kind == "tuple":
            return tuple(_decode_value(r, sub) for sub in spec[1:])
        raise TypeError(spec)
    if _is_family(spec):
        index = r.enum_index()
        if index >= len(spec):
            raise CodecError(f"variant index {index} is out of range for this enum ({len(spec)} variants)")
        cls = spec[index]
        return cls(**_decode_fields(r, cls._spec))
    if isinstance(spec, type) and issubclass(spec, IntEnum):
        index = r.enum_index()
        try:
            return spec(index)
        except ValueError as e:
            raise CodecError(f"{spec.__name__} index {index} is out of range") from e
    if _is_struct(spec):
        return spec(**_decode_fields(r, spec._spec))
    raise TypeError(spec)


def _decode_fields(r: Reader, fields) -> dict:
    return {name: _decode_value(r, sub) for name, sub in fields}


def encode_request(req) -> bytes:
    w = Writer()
    _encode_value(w, Request, req)
    return w.bytes()


def decode_request(data: bytes):
    r = Reader(data)
    value = _decode_value(r, Request)
    r.finish()
    return value


def encode_response(resp) -> bytes:
    w = Writer()
    _encode_value(w, Response, resp)
    return w.bytes()


def decode_response(data: bytes):
    r = Reader(data)
    value = _decode_value(r, Response)
    r.finish()
    return value


# ---- framing (SERVER-002 §3) ----


def frame(payload: bytes) -> bytes:
    if len(payload) > MAX_FRAME_BYTES:
        raise CodecError(f"frame of {len(payload)} bytes exceeds the {MAX_FRAME_BYTES}-byte limit")
    return len(payload).to_bytes(4, "little") + payload


def read_exact(sock, n: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < n:
        chunk = sock.recv(n - len(chunks))
        if not chunk:
            raise ConnectionError("connection closed by the server")
        chunks += chunk
    return bytes(chunks)


def read_frame(sock) -> bytes:
    n = int.from_bytes(read_exact(sock, 4), "little")
    if n > MAX_FRAME_BYTES:
        raise CodecError(f"frame of {n} bytes exceeds the {MAX_FRAME_BYTES}-byte limit")
    return read_exact(sock, n)
