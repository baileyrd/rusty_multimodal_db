//! The generic store's companion "record blob": the full record set a
//! [`super::GenericMmapStore`] was built from — every field, not only the
//! one it maps — persisted as one write-once, `bincode`-serialized file
//! next to the store's own, unchanged mmap file. The `GenericMmapStore`
//! analogue of `durability::record_blob` (`ProductionStore`'s companion),
//! and the same shape of fix for the same gap: the mmap file carries one
//! durable field per record and can't rebuild the records on its own.
//! See `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` (Accepted) and
//! ADR-0017 for the decision, and
//! `docs/specifications/storage/STORAGE-015-generic-store-file-portability.md`
//! for the requirements.
//!
//! # What differs from the `Dog` blob
//!
//! Two things, both consequences of `R` being a type parameter rather
//! than `DogRecord`:
//!
//! - **The blob is `Vec<R>` alone.** The generic store has no edge set of
//!   its own (relationship layers such as `Reversed` derive theirs from
//!   the records — see `open_order_production_stack`), so there is
//!   nothing else to carry.
//! - **The fingerprint covers the whole record, mmap-backed field
//!   included**, and is computed by streaming the same `bincode` encoding
//!   the body uses into [`Fnv1a64`] — a per-field selection like
//!   `RecordBlob::fingerprint`'s "everything but `age`" would need a
//!   trait method every domain has to implement, for a saving that
//!   matters only when a caller re-`open`s with records that differ
//!   *solely* in their mmap-backed field. That case rewrites the blob
//!   (harmlessly: the mmap file remains the source of truth for that
//!   field, and `open`'s reconciliation ignores the blob's copy) instead
//!   of skipping the write. Two encodings of the same set therefore
//!   agree, byte-for-byte, and so do their fingerprints.
//!
//! The header layout, magic-then-version-then-fingerprint, the companion
//! path derivation, the FNV-1a 64 hash, the atomic write-to-temp-then-
//! rename install, and the [`DurabilityError::RecordBlobUnreadable`]
//! error variant are all shared with the `Dog` blob rather than copied —
//! see `durability::record_blob`'s "Shared with the generic store's
//! blob". Only the magic differs, so a `DOGBLOB\0` file at a generic
//! store's companion path (or a `GENBLOB\0` file at a `ProductionStore`'s)
//! is a magic error, not a `bincode` decode attempt
//! (`STORAGE-015-FR-005`).
//!
//! # The schema tag (version 2)
//!
//! The `Dog` blob holds exactly one type, so its magic *is* its schema
//! check. This blob is generic over `R`, and version 1 never recorded
//! which `R` it held: a `.records` blob written from `Employee` and read
//! as `Order` passed magic, version, and fingerprint (all of which are
//! `R`-blind) and failed, if at all, inside `bincode` — or decoded
//! silently when the two layouts happened to agree. Version 2
//! (`STORAGE-015` v0.2.0, `docs/design/BLOB-SCHEMA-TAG-DESIGN.md`,
//! ADR-0019) appends one field to the shared header: the FNV-1a 64 hash
//! of `R`'s [`SchemaTag::SCHEMA_TAG`], checked after magic and version
//! and before the fingerprint and body decode (`SCHTAG-FR-002`, `-003`).
//! The first 20 bytes are the shared header unchanged (parsed by the same
//! `parse_header`); the tag is the 8 bytes after it, and the body starts
//! at [`TAGGED_HEADER_LEN`]. The tagged helpers live here and are shared
//! with the edge blob (`super::edge_blob`, `GENEDGE\0`), which carries
//! the same tag under its own magic.
//!
//! A version-1 file is a version mismatch on the read-only paths and
//! "not current" for [`GenericRecordBlob::is_current_at`], so
//! `GenericMmapStore::open` — which has the records in hand — rewrites it
//! in the tagged layout on first use: the `DOGBLOB\0` 1 → 2 story
//! (`STORAGE-014` v0.2.0), reused (`SCHTAG-FR-005`).

use super::traits::SchemaTag;
use crate::durability::record_blob::{
    companion_path, encode_image, parse_header, EncodedRecordBlob, Fnv1a64, HEADER_LEN,
};
use crate::durability::DurabilityError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Identifies a file as one [`GenericRecordBlob::encode`] produced —
/// distinct from `DOGBLOB\0` (the `ProductionStore` companion),
/// `GENEDGE\0` (the `Symmetric` edge blob), `DOGMMAP\0`, and `GMMAPST\0`
/// (the generic store's own mmap file).
const MAGIC: [u8; 8] = *b"GENBLOB\0";

/// This blob's on-disk layout, versioned from its first release. Version
/// 1 was the shared 20-byte header (magic, version, body fingerprint —
/// the `Dog` blob's fingerprint-less version 1 was never the generic
/// blob's) followed by the body; version 2 adds the 8-byte schema tag
/// between header and body (see module docs). A version-1 blob is a
/// version mismatch for [`read`] and stale for
/// [`GenericRecordBlob::is_current_at`].
const BLOB_VERSION: u32 = 2;

/// Byte offset of the little-endian `u64` schema tag hash: immediately
/// after the shared header's fingerprint.
pub(crate) const TAG_OFFSET: usize = HEADER_LEN;

/// The shared 20-byte header plus the 8-byte schema tag: 28 bytes. Where
/// the body of every version-2 generic blob (`GENBLOB\0` and `GENEDGE\0`)
/// begins, and exactly what [`GenericRecordBlob::is_current_at`] reads.
pub(crate) const TAGGED_HEADER_LEN: usize = TAG_OFFSET + 8;

/// The 8 bytes a [`SchemaTag::SCHEMA_TAG`] becomes in a header: FNV-1a 64
/// over the tag's UTF-8 bytes alone — no length prefix, no terminator —
/// by the same [`Fnv1a64`] the body fingerprint uses, so the same string
/// hashes the same on every platform and in every build (`SCHTAG-FR-009`).
/// The empty string is the FNV offset basis.
pub(crate) fn tag_hash(tag: &str) -> u64 {
    let mut hash = Fnv1a64::new();
    hash.update(tag.as_bytes());
    hash.finish()
}

/// Assemble a complete tagged on-disk image: the shared
/// [`HEADER_LEN`]-byte header (via [`encode_image`], unchanged), then the
/// [`tag_hash`] of `tag` as a little-endian `u64`, then `body`. The
/// inverse of [`parse_tagged_header`] for the header half.
pub(crate) fn encode_tagged_image(
    magic: &[u8; 8],
    version: u32,
    fingerprint: u64,
    tag: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut image = encode_image(magic, version, fingerprint, &[]);
    image.reserve(8 + body.len());
    image.extend_from_slice(&tag_hash(tag).to_le_bytes());
    image.extend_from_slice(body);
    image
}

/// Validate a fixed [`TAGGED_HEADER_LEN`]-byte header — magic, then
/// version, then schema tag, strictly in that order — and return the body
/// fingerprint it claims. The shared 20 bytes go through the unchanged
/// [`parse_header`], so its two causes are exactly what a `DOGBLOB\0`
/// reader would report; a file that passes both but stops short of the
/// tag is "file too short for a tagged header"; a tag that isn't
/// `expected_tag`'s hash is the one new cause, naming the expected tag
/// string (the readable name lives on the expecting side) and both hashes
/// (`SCHTAG-FR-004`). A version-1 file is reported as a version mismatch,
/// never a tag mismatch. `bytes` may be the whole file or just its first
/// [`TAGGED_HEADER_LEN`] bytes. Errors are the `cause` half of a
/// [`DurabilityError::RecordBlobUnreadable`].
pub(crate) fn parse_tagged_header(
    bytes: &[u8],
    magic: &[u8; 8],
    expected_version: u32,
    expected_tag: &str,
) -> Result<u64, String> {
    let fingerprint = parse_header(bytes, magic, expected_version)?;
    if bytes.len() < TAGGED_HEADER_LEN {
        return Err("file too short for a tagged header".to_owned());
    }
    // Bounds checked just above, so the fixed-width slice can't panic —
    // the same pattern `parse_header` uses for its own fields.
    let mut tag = [0u8; 8];
    tag.copy_from_slice(&bytes[TAG_OFFSET..TAGGED_HEADER_LEN]);
    let found = u64::from_le_bytes(tag);
    let expected = tag_hash(expected_tag);
    if found != expected {
        return Err(format!(
            "schema tag mismatch: this store expects `{expected_tag}` ({expected:#018x}), file holds {found:#018x}"
        ));
    }
    Ok(fingerprint)
}

/// The record set a `GenericMmapStore` was built from, exactly as
/// `create`/`open` receive it and in caller order. Held by reference: the
/// store encodes and fingerprints the caller's `Vec<R>` *before* moving
/// it into its own maps, so no clone of the set is made just to persist
/// it.
pub(crate) struct GenericRecordBlob<'a, R> {
    records: &'a [R],
}

impl<'a, R> GenericRecordBlob<'a, R>
where
    R: Serialize + SchemaTag,
{
    pub(crate) fn new(records: &'a [R]) -> Self {
        Self { records }
    }

    /// The 64-bit content fingerprint the header carries: FNV-1a 64 over
    /// the `bincode` encoding of the whole record set, streamed into the
    /// hasher rather than materialized (see module docs for why the whole
    /// record, and why no intermediate buffer).
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if serialization fails.
    pub(crate) fn fingerprint(&self) -> Result<u64, DurabilityError> {
        let mut hash = Fnv1a64::new();
        crate::codec::encode_into(&mut hash, self.records)?;
        Ok(hash.finish())
    }

    /// Serialize the record set into its complete on-disk image (header +
    /// body). The body is encoded once; the fingerprint is taken over
    /// those same bytes so it can never disagree with what is written.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if serialization fails.
    pub(crate) fn encode(&self) -> Result<EncodedRecordBlob, DurabilityError> {
        let body = crate::codec::encode(self.records)?;
        let mut hash = Fnv1a64::new();
        hash.update(&body);
        Ok(EncodedRecordBlob {
            image: encode_tagged_image(&MAGIC, BLOB_VERSION, hash.finish(), R::SCHEMA_TAG, &body),
        })
    }

    /// Whether the blob at `path` already holds this record set, tagged
    /// for this `R` — the check `GenericMmapStore::open` uses to skip a
    /// redundant rewrite in the common case (same dataset at `create` and
    /// every later `open`). Costs one [`Self::fingerprint`] pass over the
    /// in-memory records plus a [`TAGGED_HEADER_LEN`]-byte read — never a
    /// full serialization to a buffer, never a read of the body. True only
    /// when magic, version, tag, and fingerprint all match
    /// (`SCHTAG-FR-005`); a missing, short, foreign (`DOGBLOB\0`
    /// included), version-1, or wrong-`R` file counts as "not current" so
    /// that `open` heals it from the caller-supplied truth — including
    /// upgrading a mmap file written before this companion existed
    /// (`STORAGE-015-FR-003`). A serialization failure also counts as
    /// "not current": `open` then attempts the rewrite, which surfaces the
    /// same failure as a proper error instead of swallowing it here.
    pub(crate) fn is_current_at(&self, path: &Path) -> bool {
        let mut header = [0u8; TAGGED_HEADER_LEN];
        let read_header = File::open(path).and_then(|mut file| file.read_exact(&mut header));
        read_header.is_ok()
            && self.fingerprint().is_ok_and(|fp| {
                parse_tagged_header(&header, &MAGIC, BLOB_VERSION, R::SCHEMA_TAG) == Ok(fp)
            })
    }
}

/// Read and validate the blob at `path`, returning the record set it
/// holds. Every way this can fail — the file is missing, shorter than its
/// header, carries the wrong magic, records an incompatible version, was
/// written from a different `R` (a schema tag mismatch, refused before
/// `bincode` sees a byte — `SCHTAG-FR-003`), its body doesn't decode, or
/// the body's bytes don't hash to the fingerprint the header claims —
/// maps to [`DurabilityError::RecordBlobUnreadable`], naming the
/// companion path and the specific cause. Never the mmap file's
/// `InvalidMagic`/`SchemaVersionMismatch`, never a panic
/// (`STORAGE-015-FR-005`).
///
/// # Errors
///
/// Returns [`DurabilityError::RecordBlobUnreadable`] as described above.
pub(crate) fn read<R>(path: &Path) -> Result<Vec<R>, DurabilityError>
where
    R: DeserializeOwned + SchemaTag,
{
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: path.to_path_buf(),
        cause,
    };

    let bytes = std::fs::read(path).map_err(|e| unreadable(format!("cannot read file: {e}")))?;
    let claimed =
        parse_tagged_header(&bytes, &MAGIC, BLOB_VERSION, R::SCHEMA_TAG).map_err(unreadable)?;
    let body = &bytes[TAGGED_HEADER_LEN..];
    let mut hash = Fnv1a64::new();
    hash.update(body);
    let actual = hash.finish();
    if actual != claimed {
        return Err(unreadable(format!(
            "fingerprint mismatch: header claims {claimed:#018x}, body hashes to {actual:#018x}"
        )));
    }
    crate::codec::decode(body).map_err(|e| unreadable(format!("body does not decode: {e}")))
}

/// Where a `GenericMmapStore` whose mmap file lives at `path` keeps its
/// companion blob — the same `<path>.records` derivation as the
/// `ProductionStore` companion (`STORAGE-015-FR-001`).
pub(crate) fn blob_path(path: &Path) -> std::path::PathBuf {
    companion_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fresh_temp_dir;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Item {
        id: u32,
        label: String,
        weight: i64,
    }

    impl SchemaTag for Item {
        const SCHEMA_TAG: &'static str = "record_blob::tests::Item";
    }

    /// Byte-for-byte the same record as [`Item`] under a different tag —
    /// the case only the tag can tell apart (`SCHTAG-FR-001`).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Twin {
        id: u32,
        label: String,
        weight: i64,
    }

    impl SchemaTag for Twin {
        const SCHEMA_TAG: &'static str = "record_blob::tests::Twin";
    }

    fn sample() -> Vec<Item> {
        vec![
            Item {
                id: 1,
                label: "one".to_owned(),
                weight: 10,
            },
            Item {
                id: 2,
                label: "two".to_owned(),
                weight: 20,
            },
        ]
    }

    #[test]
    fn blob_path_matches_the_production_store_companion_derivation() {
        assert_eq!(
            blob_path(Path::new("/x/store.mmap")),
            std::path::PathBuf::from("/x/store.mmap.records")
        );
    }

    #[test]
    fn encode_then_read_round_trips_every_field() {
        let dir = fresh_temp_dir("generic_blob_roundtrip").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        let items = sample();
        let blob = GenericRecordBlob::new(&items);
        blob.encode().unwrap().write(&path).unwrap();
        let loaded: Vec<Item> = read(&path).unwrap();
        assert_eq!(loaded, items);
        assert!(blob.is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streamed_fingerprint_equals_the_one_written_in_the_header() {
        let dir = fresh_temp_dir("generic_blob_fingerprint").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        let items = sample();
        let blob = GenericRecordBlob::new(&items);
        blob.encode().unwrap().write(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header_fp =
            parse_tagged_header(&bytes, &MAGIC, BLOB_VERSION, Item::SCHEMA_TAG).unwrap();
        assert_eq!(header_fp, blob.fingerprint().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_different_record_set_is_not_current() {
        let dir = fresh_temp_dir("generic_blob_stale").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        let items = sample();
        GenericRecordBlob::new(&items)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let mut changed = sample();
        changed[1].weight = 21;
        assert!(!GenericRecordBlob::new(&changed).is_current_at(&path));
        assert!(!GenericRecordBlob::new(&items[..1]).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_blob_is_unreadable_and_not_current() {
        let dir = fresh_temp_dir("generic_blob_missing").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        let items = sample();
        assert!(!GenericRecordBlob::new(&items).is_current_at(&path));
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { path: p, cause }) => {
                assert_eq!(p, path);
                assert!(cause.starts_with("cannot read file"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dog_blob_at_the_companion_path_is_a_magic_error() {
        let dir = fresh_temp_dir("generic_blob_dog_magic").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        let dog = crate::durability::record_blob::RecordBlob {
            records: crate::durability::test_support::sample_records(),
            edges: crate::durability::test_support::sample_edges(),
        };
        dog.write(&path).unwrap();
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("magic number mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!GenericRecordBlob::new(&sample()).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_version_is_a_version_error() {
        let dir = fresh_temp_dir("generic_blob_version").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        GenericRecordBlob::new(&sample())
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[MAGIC.len()..MAGIC.len() + 4]
            .copy_from_slice(&BLOB_VERSION.wrapping_add(1).to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("blob version mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_that_does_not_match_the_header_fingerprint_is_rejected() {
        let dir = fresh_temp_dir("generic_blob_tamper").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        GenericRecordBlob::new(&sample())
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff; // flips a byte of the last record's `weight`
        std::fs::write(&path, &bytes).unwrap();
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("fingerprint mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_body_is_a_fingerprint_error_not_a_panic() {
        let dir = fresh_temp_dir("generic_blob_truncated").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        GenericRecordBlob::new(&sample())
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert!(matches!(
            read::<Item>(&path),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tag_hash_is_fnv1a_64_over_the_tag_bytes() {
        // FNV-1a 64 test vectors: the offset basis for "", and the
        // well-known value for "a".
        assert_eq!(tag_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(tag_hash("a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(tag_hash(Item::SCHEMA_TAG), tag_hash(Twin::SCHEMA_TAG));
    }

    #[test]
    fn the_header_carries_the_tag_hash_right_after_the_shared_header() {
        let image = GenericRecordBlob::new(&sample()).encode().unwrap().image;
        assert_eq!(&image[..MAGIC.len()], &MAGIC);
        let mut tag = [0u8; 8];
        tag.copy_from_slice(&image[TAG_OFFSET..TAGGED_HEADER_LEN]);
        assert_eq!(u64::from_le_bytes(tag), tag_hash(Item::SCHEMA_TAG));
        assert_eq!(TAGGED_HEADER_LEN, 28);
        let twin = vec![Twin {
            id: 1,
            label: "one".to_owned(),
            weight: 10,
        }];
        let twin_image = GenericRecordBlob::new(&twin).encode().unwrap().image;
        let item_image = GenericRecordBlob::new(&sample()[..1])
            .encode()
            .unwrap()
            .image;
        // Same body, same fingerprint — only the tag differs.
        assert_eq!(&twin_image[..TAG_OFFSET], &item_image[..TAG_OFFSET]);
        assert_eq!(
            &twin_image[TAGGED_HEADER_LEN..],
            &item_image[TAGGED_HEADER_LEN..]
        );
        assert_ne!(twin_image, item_image);
    }

    #[test]
    fn a_blob_of_another_type_with_the_same_shape_is_a_tag_error_not_a_decode() {
        let dir = fresh_temp_dir("generic_blob_other_tag").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        let twins = vec![Twin {
            id: 1,
            label: "one".to_owned(),
            weight: 10,
        }];
        GenericRecordBlob::new(&twins)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        // A version-1 blob would have decoded this as `Vec<Item>` without
        // complaint; version 2 refuses it by name.
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { path: p, cause }) => {
                assert_eq!(p, path);
                assert!(
                    cause.starts_with(
                        "schema tag mismatch: this store expects `record_blob::tests::Item`"
                    ),
                    "{cause}"
                );
                assert!(
                    cause.contains(&format!("{:#018x}", tag_hash(Twin::SCHEMA_TAG))),
                    "{cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!GenericRecordBlob::new(&sample()[..1]).is_current_at(&path));
        assert!(GenericRecordBlob::new(&twins).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_1_blob_is_a_version_error_before_the_tag_is_looked_at() {
        let dir = fresh_temp_dir("generic_blob_v1").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        GenericRecordBlob::new(&sample())
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        // The exact version-1 image: version field 1, no tag between the
        // shared header and the body (`SCHTAG-FR-006`).
        let bytes = std::fs::read(&path).unwrap();
        let mut v1 = Vec::with_capacity(bytes.len() - 8);
        v1.extend_from_slice(&bytes[..HEADER_LEN]);
        v1.extend_from_slice(&bytes[TAGGED_HEADER_LEN..]);
        v1[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&path, &v1).unwrap();
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("blob version mismatch: file has 1, this build expects 2"),
                    "{cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!GenericRecordBlob::new(&sample()).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_cut_inside_the_tag_is_a_short_tagged_header_error() {
        let dir = fresh_temp_dir("generic_blob_short_tag").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        GenericRecordBlob::new(&sample())
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        for cut in [HEADER_LEN, HEADER_LEN + 3, TAGGED_HEADER_LEN - 1] {
            std::fs::write(&path, &bytes[..cut]).unwrap();
            match read::<Item>(&path) {
                Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                    assert!(
                        cause.starts_with("file too short for a tagged header"),
                        "{cause}"
                    );
                }
                other => panic!("expected RecordBlobUnreadable at cut {cut}, got {other:?}"),
            }
            assert!(!GenericRecordBlob::new(&sample()).is_current_at(&path));
        }
        // Cut inside the shared header: still the shared header's error.
        std::fs::write(&path, &bytes[..HEADER_LEN - 1]).unwrap();
        match read::<Item>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("magic number mismatch or file too short"),
                    "{cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `BINENC-FR-004`: the `GENBLOB\0` body for one `Order`, pinned byte
    /// for byte — the record count, then the record's fields in
    /// declaration order: two ids (each a `u64` length of 16 and the 16
    /// big-endian bytes), `amount_cents` as an `i64`, `status` as a `u32`
    /// variant index, `created_at_unix_ms` and `discount_cents` as
    /// `i64`s. The fingerprint is FNV-1a 64 over exactly these bytes, so
    /// a tagged image built from them reads back as the record set — a
    /// blob written by a build before this pin is still readable.
    /// `Order` lives behind `research`, so this runs with `--all-features`
    /// (the sweep and CI both do).
    #[cfg(feature = "research")]
    #[test]
    fn one_order_body_encodes_to_its_pinned_bytes() {
        use crate::generic::order_customer::{Order, OrderStatus};
        use uuid::Uuid;

        let orders = vec![Order {
            id: Uuid::from_u128(1),
            customer_id: Uuid::from_u128(2),
            amount_cents: 1000,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 5,
            discount_cents: 0,
        }];
        const BODY: [u8; 84] = [
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // len = 1
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // id
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, //
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // customer_id
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, //
            0xe8, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // amount_cents = 1000
            0x01, 0x00, 0x00, 0x00, // OrderStatus::Shipped
            0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // created_at_unix_ms
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // discount_cents
        ];
        crate::test_support::assert_golden_eq("GENBLOB body", &orders, &BODY);

        let blob = GenericRecordBlob::new(&orders);
        let image = blob.encode().unwrap().image;
        assert_eq!(&image[TAGGED_HEADER_LEN..], &BODY);
        let mut hash = Fnv1a64::new();
        hash.update(&BODY);
        assert_eq!(blob.fingerprint().unwrap(), hash.finish());

        let dir = fresh_temp_dir("generic_blob_golden").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        std::fs::write(
            &path,
            encode_tagged_image(
                &MAGIC,
                BLOB_VERSION,
                hash.finish(),
                Order::SCHEMA_TAG,
                &BODY,
            ),
        )
        .unwrap();
        assert_eq!(read::<Order>(&path).unwrap(), orders);
        assert!(blob.is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `STORAGE-018` acceptance criterion 5: this blob fingerprints the
    /// body *bytes* before decoding, so a body with junk appended is
    /// still caught as a fingerprint mismatch — the codec's trailing-
    /// bytes rejection (`BINENC-FR-002`) never gets to run, and the
    /// error a reader sees is unchanged from before the codec existed.
    #[cfg(feature = "research")]
    #[test]
    fn a_body_with_trailing_junk_is_still_a_fingerprint_mismatch() {
        use crate::generic::order_customer::Order;
        let orders: Vec<Order> = Vec::new();
        let blob = GenericRecordBlob::new(&orders);
        let mut image = blob.encode().unwrap().image;
        image.extend_from_slice(&[0xaa, 0xbb]);

        let dir = fresh_temp_dir("generic_blob_trailing_junk").unwrap();
        let path = blob_path(&dir.join("store.mmap"));
        std::fs::write(&path, &image).unwrap();
        match read::<Order>(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("fingerprint mismatch"),
                    "unexpected cause: {cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
