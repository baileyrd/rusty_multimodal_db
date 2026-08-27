//! A small trait letting [`super::mmap_store::GenericMmapStore`] read/write
//! a `ScannableField::ScanValue` directly into a memory-mapped byte region,
//! generically over which fixed-width integer type it is.
//!
//! # Why this exists, and why it's this narrow
//!
//! `MmapAgeStore` (`src/durability/mmap_store.rs`, `Dog`-specific) maps
//! `age: u32` directly: `u32::to_le_bytes`/`from_le_bytes` on a known,
//! fixed 4-byte width. Generalizing that to "any `ScannableField::ScanValue`"
//! needs a trait, since `to_le_bytes`/`from_le_bytes` aren't a shared
//! stdlib trait across the integer types — only an inherent method on
//! each concrete type. This trait exists purely to give
//! `GenericMmapStore` one thing to call generically; it is **not** a
//! general-purpose byte-serialization abstraction (no varint, no
//! endianness choice beyond little-endian, no support for anything wider
//! than what this crate's two domains actually need).
//!
//! Implemented for exactly the two integer types this crate's two domains
//! use as a `ScanValue` (`u32` for `Dog::age`, `i64` for `Order::amount_cents`/
//! `created_at_unix_ms`/`discount_cents`) — not blanket-implemented for
//! every integer type Rust has, since `ADR-0009`'s design doc (§4.2) is
//! explicit that mmap durability's generalization stops at "fixed-width
//! `Copy` mutable fields," not "any `Copy` type whatsoever" (a `String` or
//! other variable-length `ScanValue` was never in scope for this mmap
//! path — a domain with one would use the in-memory `Scanned` layer
//! instead, same as it already can).
//!
//! Also implemented for [`Uuid`] — added by the record-identity-keying
//! fix (`mmap_store.rs`'s own module docs), which needs `R::Id` encodable
//! the same fixed-width way `R::ScanValue` already is, so a persisted
//! slot can carry *which* record a value belongs to instead of trusting
//! array position. `Uuid` is scoped in for the identical reason `u32`/`i64`
//! are: every domain `GenericMmapStore` is actually instantiated for
//! (`Order`) uses `Uuid` as its `Record::Id`, not because `Id` is
//! constrained to `Uuid` at the trait level.
use std::mem::size_of;
use uuid::Uuid;

/// A `ScannableField::ScanValue` that can be read from and written to a
/// fixed-width little-endian byte slice — what
/// [`super::mmap_store::GenericMmapStore`] needs to back a scannable field
/// with `MmapMut` instead of a plain `Vec`.
pub trait MmapFieldValue: Copy {
    /// The exact number of bytes this value occupies in the mapped file —
    /// used to compute each record's byte offset (`position * BYTE_WIDTH`),
    /// mirroring `MmapAgeStore`'s own `position * 4`.
    const BYTE_WIDTH: usize;

    /// Write `self` into `buf`, little-endian. `buf.len() == Self::BYTE_WIDTH`
    /// always, by construction of every call site in `mmap_store.rs`.
    fn write_le(&self, buf: &mut [u8]);

    /// Read a value from `buf`, little-endian. `buf.len() == Self::BYTE_WIDTH`
    /// always, by construction of every call site in `mmap_store.rs`.
    fn read_le(buf: &[u8]) -> Self;
}

impl MmapFieldValue for u32 {
    const BYTE_WIDTH: usize = size_of::<u32>();

    fn write_le(&self, buf: &mut [u8]) {
        buf.copy_from_slice(&self.to_le_bytes());
    }

    fn read_le(buf: &[u8]) -> Self {
        u32::from_le_bytes(
            buf.try_into()
                .expect("caller always passes a Self::BYTE_WIDTH-sized slice"),
        )
    }
}

impl MmapFieldValue for i64 {
    const BYTE_WIDTH: usize = size_of::<i64>();

    fn write_le(&self, buf: &mut [u8]) {
        buf.copy_from_slice(&self.to_le_bytes());
    }

    fn read_le(buf: &[u8]) -> Self {
        i64::from_le_bytes(
            buf.try_into()
                .expect("caller always passes a Self::BYTE_WIDTH-sized slice"),
        )
    }
}

impl MmapFieldValue for Uuid {
    const BYTE_WIDTH: usize = 16;

    /// Not actually "little-endian" — a `Uuid`'s 16 bytes are an opaque
    /// identifier, not a number this crate ever does arithmetic on, so
    /// there's no meaningful byte order to convert. Copied verbatim; the
    /// `_le` name stays only for a uniform call site across every
    /// `MmapFieldValue` impl.
    fn write_le(&self, buf: &mut [u8]) {
        buf.copy_from_slice(self.as_bytes());
    }

    fn read_le(buf: &[u8]) -> Self {
        Uuid::from_bytes(
            buf.try_into()
                .expect("caller always passes a Self::BYTE_WIDTH-sized slice"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_round_trips() {
        let mut buf = [0u8; 4];
        42u32.write_le(&mut buf);
        assert_eq!(u32::read_le(&buf), 42);
    }

    #[test]
    fn i64_round_trips_including_negative() {
        let mut buf = [0u8; 8];
        (-12_345i64).write_le(&mut buf);
        assert_eq!(i64::read_le(&buf), -12_345);
    }

    #[test]
    fn uuid_round_trips() {
        let mut buf = [0u8; 16];
        let id = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        id.write_le(&mut buf);
        assert_eq!(Uuid::read_le(&buf), id);
    }
}
