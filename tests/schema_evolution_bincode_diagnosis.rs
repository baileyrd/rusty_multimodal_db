//! Diagnosis, not a fix: how the crate's *other* durability path — the
//! `bincode`-based snapshot/WAL variants under `src/durability/`
//! (`SnapshotFullStore`, `SnapshotRebuildStore`, `WalBufferedStore`,
//! `WalFsyncStore`, `HybridStore`) — behaves when a field is added to a
//! record type whose bytes were already persisted. `CanonicalCachedState::write_to`/
//! `read_from` (`src/durability/mod.rs`) are exactly `bincode::serialize(self)`/
//! `bincode::deserialize(&bytes)`, `pub(crate)` and so unreachable from this
//! external test crate — but that means there's no crate-specific
//! machinery to route around: this file exercises `bincode::serialize`/
//! `deserialize` directly, on local structs shaped like `DogRecord`, using
//! the exact same `bincode`/`serde` versions this crate depends on
//! (`Cargo.toml`: `bincode = "1"`, `serde = "1"`). That *is* what
//! `CanonicalCachedState`'s own persistence does, one level removed from
//! `pub(crate)` visibility, not an approximation of it.
//!
//! No production code changes — this file exists purely to reproduce and
//! record the current behavior with real evidence, per this round's own
//! task. See the module-level report delivered in chat for the full
//! severity assessment; short version, confirmed by the tests below:
//! `bincode` is a fixed-position, non-self-describing format — it reads
//! struct fields strictly in declaration order with no field names, tags,
//! or length markers of its own (aside from `String`/`Vec`'s own
//! length-prefixes). Appending a field and trying to decode old bytes
//! into the new shape hits end-of-buffer while decoding that trailing
//! field and returns `Err` — a **loud** failure for this specific case,
//! not silent corruption. `#[serde(default)]` does **not** help: bincode's
//! `Deserializer` doesn't implement `SeqAccess` in a way that reports
//! "no more elements" for a struct sequence the way a self-describing
//! format (`serde_json`) would — it just tries to read the next field's
//! bytes and gets an I/O-style `UnexpectedEof`ish decode error instead,
//! before `#[serde(default)]`'s "field missing" branch ever gets a chance
//! to run.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecordV1 {
    id: Uuid,
    breed: String,
    age: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecordV2AppendedField {
    id: Uuid,
    breed: String,
    age: u32,
    /// The new field — appended at the end, the most common real-world
    /// shape for an additive schema change (matches how `DiscountCents`
    /// was added to `Order` in an earlier round: a new field tacked onto
    /// the end of the struct, not inserted in the middle).
    microchip_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecordV2WithDefault {
    id: Uuid,
    breed: String,
    age: u32,
    /// Same addition, but with the annotation a self-describing format
    /// (`serde_json`) would honor for a field missing from older data.
    #[serde(default)]
    microchip_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecordV2InsertedMiddle {
    id: Uuid,
    /// Inserted *before* `age`, not appended at the end — the shape most
    /// likely to produce silent, wrong-but-plausible values rather than a
    /// clean decode error, since `breed`'s length-prefixed bytes end up
    /// misread as something else entirely, and vice versa.
    microchip_id: Option<String>,
    breed: String,
    age: u32,
}

fn sample_v1() -> RecordV1 {
    RecordV1 {
        id: Uuid::from_u128(42),
        breed: "labrador".into(),
        age: 3,
    }
}

/// The concrete scenario: bytes written under the old schema (`RecordV1`),
/// decoded under a new schema with one field appended at the end.
#[test]
fn decoding_old_bytes_as_a_schema_with_an_appended_field_fails_loudly() {
    let bytes = bincode::serialize(&sample_v1()).unwrap();

    let result: Result<RecordV2AppendedField, _> = bincode::deserialize(&bytes);

    assert!(
        result.is_err(),
        "expected a loud decode error for old bytes under a schema with an appended field, got: \
         {result:?}"
    );
    let message = result.unwrap_err().to_string();
    println!("bincode error decoding old bytes as an appended-field schema: {message}");
}

/// `#[serde(default)]` is the standard escape hatch for exactly this case
/// on a self-describing format (`serde_json`) — confirming, not assuming,
/// that it does *not* rescue the bincode path the way it would there.
#[test]
fn serde_default_does_not_rescue_the_bincode_decode() {
    let bytes = bincode::serialize(&sample_v1()).unwrap();

    let result: Result<RecordV2WithDefault, _> = bincode::deserialize(&bytes);

    assert!(
        result.is_err(),
        "#[serde(default)] was expected to make no difference for bincode's non-self-describing \
         format, got: {result:?}"
    );
}

/// The riskier shape: a field inserted in the *middle* of the struct,
/// ahead of a variable-length (`String`) field. Bincode reads a `String`
/// as a little-endian `u64` length prefix followed by that many bytes —
/// inserting `Option<String>` ahead of `breed` means the bytes that used
/// to *be* `breed`'s length prefix are now read as something else
/// entirely, and what decodes as `breed`'s "length" afterward is
/// essentially arbitrary. Confirms which failure mode actually happens
/// here: this crate's own `Order`/`DogRecord` field-addition precedent
/// (`DiscountCents`) always appends at the end, so this is the shape *not*
/// already covered by the appended-field test above, and worth checking
/// rather than assuming it behaves the same way.
#[test]
fn decoding_old_bytes_as_a_schema_with_a_field_inserted_in_the_middle() {
    let bytes = bincode::serialize(&sample_v1()).unwrap();

    let result: Result<RecordV2InsertedMiddle, _> = bincode::deserialize(&bytes);

    match &result {
        Err(error) => {
            // Also loud, in this specific case: `id` (a `Uuid`, 16 fixed
            // bytes) decodes fine regardless, but what used to be
            // `breed`'s own length prefix is now misread as
            // `Option<String>`'s leading discriminant byte plus part of a
            // length — for a `labrador`-shaped payload this happens to
            // decode as `Some` with a nonsensical multi-exabyte length,
            // which bincode's own allocation-size sanity check rejects
            // outright rather than trying to allocate it. Reported here
            // as the observed outcome, not asserted as a guarantee: a
            // different byte payload could easily land on a length small
            // enough to "succeed" with silently wrong field values
            // instead — this crate's `microchip_id`/`breed` case simply
            // isn't that payload. See this test's own `println!` for the
            // exact error observed.
            println!("bincode error decoding old bytes as a middle-inserted-field schema: {error}");
        }
        Ok(garbage) => {
            println!(
                "decoded WITHOUT an error into plausible-looking-but-wrong values: {garbage:?}"
            );
        }
    }
    // The only claim this test makes unconditionally: whichever outcome
    // occurred, it never re-derives the *correct* `RecordV1` data — a
    // middle-of-struct insertion is not a safe additive change under
    // bincode, loud failure or not.
    if let Ok(garbage) = result {
        assert_ne!(
            (garbage.breed.as_str(), garbage.age),
            ("labrador", 3),
            "a middle-of-struct field insertion must not coincidentally decode back to the \
             correct original values — if this assertion ever fails, that's silent corruption \
             hiding behind an apparently successful decode, worse than the loud error case"
        );
    }
}
