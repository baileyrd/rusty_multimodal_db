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
/// `DOGMMAP\0`, and `GMMAPST\0` (the generic store's own mmap file).
const MAGIC: [u8; 8] = *b"GENBLOB\0";

/// This blob's on-disk layout, versioned from its first release. Starts
/// at 1 *with* the header fingerprint — the `Dog` blob's version-1
/// (fingerprint-less) layout was never the generic blob's.
const BLOB_VERSION: u32 = 1;

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
    R: Serialize,
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
        bincode::serialize_into(&mut hash, self.records)?;
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
        let body = bincode::serialize(self.records)?;
        let mut hash = Fnv1a64::new();
        hash.update(&body);
        Ok(EncodedRecordBlob {
            image: encode_image(&MAGIC, BLOB_VERSION, hash.finish(), &body),
        })
    }

    /// Whether the blob at `path` already holds this record set — the
    /// check `GenericMmapStore::open` uses to skip a redundant rewrite in
    /// the common case (same dataset at `create` and every later `open`).
    /// Costs one [`Self::fingerprint`] pass over the in-memory records
    /// plus a [`HEADER_LEN`]-byte read — never a full serialization to a
    /// buffer, never a read of the body. A missing, short, foreign
    /// (`DOGBLOB\0` included), or wrong-version file counts as "not
    /// current" so that `open` heals it from the caller-supplied truth —
    /// including upgrading a mmap file written before this companion
    /// existed (`STORAGE-015-FR-003`). A serialization failure also
    /// counts as "not current": `open` then attempts the rewrite, which
    /// surfaces the same failure as a proper error instead of swallowing
    /// it here.
    pub(crate) fn is_current_at(&self, path: &Path) -> bool {
        let mut header = [0u8; HEADER_LEN];
        let read_header = File::open(path).and_then(|mut file| file.read_exact(&mut header));
        read_header.is_ok()
            && self
                .fingerprint()
                .is_ok_and(|fp| parse_header(&header, &MAGIC, BLOB_VERSION) == Ok(fp))
    }
}

/// Read and validate the blob at `path`, returning the record set it
/// holds. Every way this can fail — the file is missing, shorter than its
/// header, carries the wrong magic, records an incompatible version, its
/// body doesn't decode, or the body's bytes don't hash to the fingerprint
/// the header claims — maps to [`DurabilityError::RecordBlobUnreadable`],
/// naming the companion path and the specific cause. Never the mmap
/// file's `InvalidMagic`/`SchemaVersionMismatch`, never a panic
/// (`STORAGE-015-FR-005`).
///
/// # Errors
///
/// Returns [`DurabilityError::RecordBlobUnreadable`] as described above.
pub(crate) fn read<R>(path: &Path) -> Result<Vec<R>, DurabilityError>
where
    R: DeserializeOwned,
{
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: path.to_path_buf(),
        cause,
    };

    let bytes = std::fs::read(path).map_err(|e| unreadable(format!("cannot read file: {e}")))?;
    let claimed = parse_header(&bytes, &MAGIC, BLOB_VERSION).map_err(unreadable)?;
    let body = &bytes[HEADER_LEN..];
    let mut hash = Fnv1a64::new();
    hash.update(body);
    let actual = hash.finish();
    if actual != claimed {
        return Err(unreadable(format!(
            "fingerprint mismatch: header claims {claimed:#018x}, body hashes to {actual:#018x}"
        )));
    }
    bincode::deserialize(body).map_err(|e| unreadable(format!("body does not decode: {e}")))
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
        let header_fp = parse_header(&bytes, &MAGIC, BLOB_VERSION).unwrap();
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
}
