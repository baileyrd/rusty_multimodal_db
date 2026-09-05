"""Reference Python client for rusty_multimodal_db's server/query layer.

Standard library only. See SERVER-002 (the wire specification) and
ADR-0043. This package is a *reference implementation* that proves the
specification is sufficient, not a distributed product: no PyPI
packaging, no async, no connection pooling.
"""

from .client import (
    Client,
    ClientError,
    ProtocolError,
    ServerError,
    TlsOptions,
    UnknownFieldError,
    UnsupportedError,
)
from .protocol import PROTOCOL_VERSION, AggregateFn, CompareOp, ErrorCode, ValueKind

__all__ = [
    "Client",
    "ClientError",
    "ProtocolError",
    "ServerError",
    "TlsOptions",
    "UnknownFieldError",
    "UnsupportedError",
    "PROTOCOL_VERSION",
    "AggregateFn",
    "CompareOp",
    "ErrorCode",
    "ValueKind",
]
