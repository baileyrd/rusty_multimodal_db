//! The crate's one `bincode` configuration — every on-disk and on-wire
//! `serde` encoding goes through [`encode`]/[`encode_into`]/[`decode`]
//! here, never through `bincode`'s free functions (`STORAGE-018`,
//! `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md`, ADR-0021).
//!
//! # Configuration
//!
//! [`options`] spells out, explicitly, every option `bincode`'s free
//! functions (`bincode::serialize`/`deserialize`) apply implicitly —
//! **fixint** integers, **little-endian** byte order, **no size limit** —
//! with one deliberate difference: **trailing bytes are rejected**
//! (`BINENC-FR-002`). A payload with bytes left over after the value is
//! fully decoded is a decode error, where the free functions silently
//! ignored the excess. The bytes produced are identical either way; only
//! what is accepted on decode narrows.
//!
//! # Format
//!
//! The concrete format, pinned by the golden vectors in this module's
//! tests (`BINENC-FR-004`, captured on the code as it stood before the
//! codec existed and reproduced byte-for-byte by it):
//!
//! - integers: their natural width, little-endian (`u8` = 1 byte,
//!   `u16` = 2, `u32` = 4, `u64`/`i64`/`usize` = 8); `bool` = 1 byte;
//!   `f64` = 8 bytes of IEEE 754 bits, little-endian; `char` = its
//!   UTF-8 bytes, no length
//! - `String`/`Vec<T>`/`&[T]`/`BTreeMap`: a `u64` element count, then
//!   the elements
//! - `Option<T>`: one byte (`0` = `None`, `1` = `Some`), then the payload
//! - structs and tuples: their fields in declaration order, no names, no
//!   count
//! - enums: the variant index as a `u32`, then the variant's fields
//! - `Uuid`: its 16 raw bytes as a length-prefixed byte string — a `u64`
//!   `16`, then the bytes big-endian (24 bytes per id)
//!
//! # Stability
//!
//! `bincode` 1.x documents its output as stable across minor revisions
//! *provided the same configuration is used* — which is the whole point
//! of stating the configuration here rather than relying on the free
//! functions' defaults. `Cargo.toml` pins `bincode = "1"`; the format
//! above is `bincode` 1.x's and a `bincode` 2 migration is a format
//! change like any other below (`BINENC-FR-003`).
//!
//! # Evolution rules
//!
//! Because the encoding is positional and unnamed, the *types* it
//! encodes are part of the format (`BINENC-FR-005`):
//!
//! - **appending** a new variant to the end of an enum is compatible —
//!   every existing index keeps its meaning;
//! - **reordering or inserting** enum variants, **adding, removing or
//!   reordering struct fields**, or **changing an integer's width** is a
//!   format change, and takes the owning format's version bump — the
//!   blob's `BLOB_VERSION`, the slot file's header version, or, for the
//!   wire protocol, a `SERVER-001` amendment (`Request`/`Response` carry
//!   no version of their own).
//!
//! Every serialized type in the crate is `#[derive(Serialize,
//! Deserialize)]` with no `serde` attributes that rename, skip, flatten
//! or default — keep it that way; each of those changes the bytes.

use bincode::Options;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// The one configuration — see the module docs. Built fresh per call
/// (it's a zero-sized value; `bincode` offers no way to hold it in a
/// `const`).
fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_no_limit()
        .reject_trailing_bytes()
}

/// Encode `value` to a fresh `Vec<u8>` under the crate's configuration.
pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, bincode::Error> {
    options().serialize(value)
}

/// Encode `value` straight into `writer` under the crate's configuration
/// — used where the bytes are hashed rather than kept (the blob
/// fingerprints), so the `Vec` [`encode`] would allocate is never built.
pub(crate) fn encode_into<W: Write, T: Serialize + ?Sized>(
    writer: W,
    value: &T,
) -> Result<(), bincode::Error> {
    options().serialize_into(writer, value)
}

/// Decode a `T` from exactly `bytes` under the crate's configuration —
/// fails if `bytes` is short, malformed, or has anything left over after
/// the value (`BINENC-FR-002`).
pub(crate) fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, bincode::Error> {
    options().deserialize(bytes)
}

#[cfg(test)]
mod tests {
    use crate::test_support::assert_golden_eq;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    /// One enum with each variant shape `serde` distinguishes. The
    /// variant index is a `u32` (fixint: four bytes, little-endian),
    /// followed by the variant's fields in order — exactly as `Request`/
    /// `Response`/`ScanValue` encode.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Shape {
        Unit,
        Tuple(u8, u16),
        Struct { a: u32 },
    }

    /// Fields in declaration order, no names, no length — a struct is
    /// the concatenation of its fields.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Point {
        x: i32,
        y: i32,
    }

    /// `BINENC-FR-002`: a payload with bytes left over after the value is
    /// a decode error here, where `bincode`'s free function accepts it —
    /// and the bytes the two produce are identical (`BINENC-FR-001`).
    #[test]
    fn trailing_bytes_are_rejected_and_the_free_function_bytes_are_ours() {
        let value = Shape::Tuple(1, 2);
        let ours = super::encode(&value).unwrap();
        assert_eq!(ours, bincode::serialize(&value).unwrap());

        let mut padded = ours.clone();
        padded.push(0xff);
        let free: Shape = bincode::deserialize(&padded).unwrap();
        assert_eq!(free, value, "the free function ignores the excess");
        let err = super::decode::<Shape>(&padded).unwrap_err();
        assert!(
            err.to_string().contains("bytes remaining"),
            "expected a trailing-bytes error, got: {err}"
        );
        assert_eq!(super::decode::<Shape>(&ours).unwrap(), value);
    }

    #[test]
    fn encode_into_writes_the_same_bytes_as_encode() {
        let value = Point { x: 1, y: -1 };
        let mut buf = Vec::new();
        super::encode_into(&mut buf, &value).unwrap();
        assert_eq!(buf, super::encode(&value).unwrap());
    }

    #[test]
    fn integers_are_fixed_width_little_endian() {
        assert_golden_eq("u8", &0x2au8, &[0x2a]);
        assert_golden_eq("u16", &0x1234u16, &[0x34, 0x12]);
        assert_golden_eq("u32", &0xdead_beefu32, &[0xef, 0xbe, 0xad, 0xde]);
        assert_golden_eq(
            "u64",
            &1u64,
            &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        );
        assert_golden_eq(
            "i64",
            &-2i64,
            &[0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        );
    }

    #[test]
    fn bool_char_and_f64_have_their_natural_widths() {
        assert_golden_eq("bool", &true, &[0x01]);
        // A `char` is its UTF-8 bytes, no length prefix.
        assert_golden_eq("char", &'é', &[0xc3, 0xa9]);
        // IEEE 754 bits, little-endian.
        assert_golden_eq(
            "f64",
            &1.5f64,
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x3f],
        );
    }

    #[test]
    fn sequences_carry_a_u64_length_prefix() {
        assert_golden_eq(
            "String",
            &"hi".to_owned(),
            &[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68, 0x69],
        );
        assert_golden_eq(
            "Vec<u8>",
            &vec![1u8, 2, 3],
            &[
                0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03,
            ],
        );
    }

    #[test]
    fn option_is_a_one_byte_discriminant_then_the_payload() {
        assert_golden_eq(
            "Option<u32>::Some",
            &Some(1u32),
            &[0x01, 0x01, 0x00, 0x00, 0x00],
        );
        assert_golden_eq("Option<u32>::None", &None::<u32>, &[0x00]);
    }

    #[test]
    fn enum_variants_are_a_u32_index_then_their_fields() {
        assert_golden_eq("unit variant", &Shape::Unit, &[0x00, 0x00, 0x00, 0x00]);
        assert_golden_eq(
            "tuple variant",
            &Shape::Tuple(1, 2),
            &[0x01, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00],
        );
        assert_golden_eq(
            "struct variant",
            &Shape::Struct { a: 3 },
            &[0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00],
        );
    }

    #[test]
    fn tuples_and_structs_are_their_fields_in_order() {
        assert_golden_eq("tuple", &(7u8, false), &[0x07, 0x00]);
        assert_golden_eq(
            "struct",
            &Point { x: 1, y: -1 },
            &[0x01, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff],
        );
    }

    /// `uuid`'s `serde` impl (non-human-readable side) emits the 16 raw
    /// bytes as a `bytes` value — so a `u64` length of 16 precedes them.
    /// Every id in every blob and every frame in this crate pays those
    /// eight bytes; this pins that they stay.
    #[test]
    fn uuid_is_a_length_prefixed_16_byte_string() {
        assert_golden_eq(
            "Uuid",
            &Uuid::from_u128(1),
            &[
                0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // len = 16
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // big-endian u128 = 1
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            ],
        );
    }
}
