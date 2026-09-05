"""The wire encoding, byte for byte — SERVER-002 §4 (ECO-FR-007, ADR-0043).

bincode 1.x with fixint integers and little-endian byte order:

- integers at their natural width, little-endian; ``bool`` one byte;
  ``f64`` eight IEEE-754 bytes; ``usize`` is a ``u64``
- ``String``/``Vec<T>``: a ``u64`` element count, then the elements
- ``Option<T>``: one byte (0 = None, 1 = Some), then the payload
- structs/tuples: their fields in declaration order, no names, no count
- enums: the variant index as a ``u32``, then the variant's fields
- ``Uuid``: a length-prefixed byte string — ``u64`` 16, then 16 bytes

Trailing bytes after a decoded value are an error (STORAGE-018,
``BINENC-FR-002``). Standard library only.
"""

from __future__ import annotations

import struct
import uuid


class CodecError(ValueError):
    """The bytes are not a valid encoding of the expected shape."""


class Writer:
    __slots__ = ("buf",)

    def __init__(self) -> None:
        self.buf = bytearray()

    def bytes(self) -> bytes:
        return bytes(self.buf)

    def raw(self, b: bytes) -> None:
        self.buf += b

    def u8(self, v: int) -> None:
        self.buf += struct.pack("<B", v)

    def u16(self, v: int) -> None:
        self.buf += struct.pack("<H", v)

    def u32(self, v: int) -> None:
        self.buf += struct.pack("<I", v)

    def u64(self, v: int) -> None:
        self.buf += struct.pack("<Q", v)

    def i64(self, v: int) -> None:
        self.buf += struct.pack("<q", v)

    def f64(self, v: float) -> None:
        self.buf += struct.pack("<d", v)

    def bool(self, v: bool) -> None:
        self.u8(1 if v else 0)

    def string(self, s: str) -> None:
        data = s.encode("utf-8")
        self.u64(len(data))
        self.raw(data)

    def uuid(self, u: uuid.UUID) -> None:
        self.u64(16)
        self.raw(u.bytes)

    def enum_index(self, i: int) -> None:
        self.u32(i)

    def option(self, value, write_payload) -> None:
        if value is None:
            self.u8(0)
        else:
            self.u8(1)
            write_payload(value)

    def vec(self, items, write_item) -> None:
        self.u64(len(items))
        for item in items:
            write_item(item)


class Reader:
    __slots__ = ("data", "pos")

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.pos = 0

    def _take(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise CodecError(
                f"unexpected end of input: need {n} bytes at offset {self.pos}, have {len(self.data) - self.pos}"
            )
        out = self.data[self.pos : self.pos + n]
        self.pos += n
        return out

    def finish(self) -> None:
        if self.pos != len(self.data):
            raise CodecError(f"{len(self.data) - self.pos} trailing byte(s) after the value")

    def u8(self) -> int:
        return struct.unpack("<B", self._take(1))[0]

    def u16(self) -> int:
        return struct.unpack("<H", self._take(2))[0]

    def u32(self) -> int:
        return struct.unpack("<I", self._take(4))[0]

    def u64(self) -> int:
        return struct.unpack("<Q", self._take(8))[0]

    def i64(self) -> int:
        return struct.unpack("<q", self._take(8))[0]

    def f64(self) -> float:
        return struct.unpack("<d", self._take(8))[0]

    def bool(self) -> bool:
        b = self.u8()
        if b not in (0, 1):
            raise CodecError(f"bool byte {b} is neither 0 nor 1")
        return b == 1

    def string(self) -> str:
        n = self.u64()
        try:
            return self._take(n).decode("utf-8")
        except UnicodeDecodeError as e:
            raise CodecError(f"string is not UTF-8: {e}") from e

    def uuid(self) -> uuid.UUID:
        n = self.u64()
        if n != 16:
            raise CodecError(f"uuid byte string has length {n}, expected 16")
        return uuid.UUID(bytes=self._take(16))

    def enum_index(self) -> int:
        return self.u32()

    def option(self, read_payload):
        tag = self.u8()
        if tag == 0:
            return None
        if tag == 1:
            return read_payload()
        raise CodecError(f"option tag {tag} is neither 0 nor 1")

    def vec(self, read_item) -> list:
        n = self.u64()
        return [read_item() for _ in range(n)]
