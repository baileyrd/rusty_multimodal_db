//! Golden byte vectors for the crate's `bincode` encoding (`STORAGE-018`,
//! `BINENC-FR-004`), captured on the code as it stood before the codec
//! existed — every call site still used `bincode`'s free functions when
//! these literals were recorded. The codec itself (`BINENC-FR-001`)
//! arrives in the next commit and must reproduce every byte below.

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
