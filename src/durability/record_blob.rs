//! The companion "record blob": the immutable half of a
//! [`crate::production::ProductionStore`]'s on-disk state — every
//! `DogRecord`'s `id`/`breed` and every `littermate_of` edge — persisted
//! as one write-once, `bincode`-serialized file next to
//! [`super::MmapAgeStore`]'s own, unchanged `ages` file.
//! See `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` (Accepted) and
//! ADR-0016 for the decision this module implements, and
//! `docs/specifications/storage/STORAGE-014-production-store-file-portability.md`
//! for the requirements it satisfies.
//!
//! # Why a second file, not a change to `MmapAgeStore`'s format
//!
//! `MmapAgeStore` persists only `age` — the one field that ever mutates —
//! because mapping a variable-length `breed: String` (or a per-record
//! edge list) into a fixed-width mmap region needs a string-heap file
//! format `ADR-0006` deliberately declined to build. That complexity
//! exists to support *in-place mutation* of variable-length data. But
//! `id`, `breed`, and the edge set are never mutated anywhere in this
//! crate (only `update_age` writes anything), so they don't need mmap's
//! in-place property at all: a plain serialize-once/deserialize-on-open
//! round trip — the exact approach `SnapshotFullStore` already proves
//! works for full `DogRecord`s — is sufficient. Splitting the two
//! concerns onto two files gets `MmapAgeStore`'s per-write, zero-loss-
//! window durability for the mutable field *and* self-contained
//! portability for everything else, without forcing one format to do
//! both jobs.
//!
//! # Layout
//!
//! A fixed header ([`MAGIC`], then [`BLOB_VERSION`] as a little-endian
//! `u32`) followed by the `bincode` encoding of [`RecordBlob`]. The header
//! makes "not a record blob at all" (a stray file at the companion path,
//! or a legacy pre-this-feature layout) detectable *before* handing bytes
//! to `bincode`, and distinct from "a record blob from an incompatible
//! build" — both surface as
//! [`DurabilityError::RecordBlobUnreadable`], never as `MmapAgeStore`'s
//! own `InvalidMagic`/`SchemaVersionMismatch`, which describe the *ages*
//! file (`STORAGE-014-FR-005`).
//!
//! # Crash safety
//!
//! [`RecordBlob::write`] never writes to the companion path directly: the
//! full image goes to a sibling temp path, is `fsync`'d, then moved into
//! place via [`std::fs::rename`] — the same write-to-temp-then-atomic-
//! rename mechanism `MmapAgeStore::write_via_rename` already established
//! for its own columnar rewrite. A crash before the rename leaves whatever
//! was at the companion path (a prior generation, or nothing) untouched;
//! a crash after leaves the new, complete file. There is never a window
//! where the companion path holds a partial blob (`STORAGE-014-FR-004`).

use super::DurabilityError;
use crate::record::DogRecord;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Identifies a file as one [`RecordBlob::write`] produced — distinct
/// bytes from `MmapAgeStore`'s `DOGMMAP\0` and `GenericMmapStore`'s own
/// magic, so pointing [`RecordBlob::read`] at either of those files fails
/// cleanly rather than being handed to `bincode`.
const MAGIC: [u8; 8] = *b"DOGBLOB\0";

/// This blob's on-disk layout, versioned from its first release —
/// same convention as `MmapAgeStore`'s `SCHEMA_VERSION`.
const BLOB_VERSION: u32 = 1;

/// [`MAGIC`] followed by [`BLOB_VERSION`] as a little-endian `u32`.
const HEADER_LEN: usize = MAGIC.len() + 4;

/// Suffix appended to the ages file's path to derive the companion
/// blob's — see [`companion_path`].
const COMPANION_SUFFIX: &str = ".records";

/// Suffix appended to the *companion* path for the temp file
/// [`RecordBlob::write`] stages into before renaming — mirrors
/// `MmapAgeStore::write_via_rename`'s own `.rewrite-tmp` convention.
const TEMP_SUFFIX: &str = ".rewrite-tmp";

/// Where a `ProductionStore` whose ages file lives at `ages_path` keeps
/// its companion record blob: `ages_path` with [`COMPANION_SUFFIX`]
/// appended (`ages.mmap` → `ages.mmap.records`). A fixed, documented
/// derivation rather than a second caller-supplied path, so the two
/// files are portable as a unit — copy both, reopen with one path
/// (`STORAGE-014-FR-001`). Appending (rather than replacing the
/// extension) means the convention never collides with or depends on
/// whatever extension the caller chose for the ages file.
pub(crate) fn companion_path(ages_path: &Path) -> PathBuf {
    let mut companion = ages_path.as_os_str().to_owned();
    companion.push(COMPANION_SUFFIX);
    PathBuf::from(companion)
}

/// The immutable half of a `ProductionStore`'s state, exactly as
/// `create`/`open` receive it: every record (its `age` field is carried
/// along because `DogRecord` already serializes it, but on read it is
/// only ever the *seed* `open`'s reconciliation uses for a record the
/// ages file doesn't yet hold — the ages file stays the source of truth
/// for every live age) and every `littermate_of` edge, in caller order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordBlob {
    pub(crate) records: Vec<DogRecord>,
    pub(crate) edges: Vec<(Uuid, Uuid)>,
}

impl RecordBlob {
    /// Serialize `self` into its complete on-disk image (header + body).
    /// Split out from [`Self::write`] so `ProductionStore::create`/`open`
    /// can encode *before* handing `records`/`edges` to
    /// `MmapAgeStore` by value — no clone of the record set just to
    /// persist it.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if serialization fails.
    pub(crate) fn encode(&self) -> Result<EncodedRecordBlob, DurabilityError> {
        let body = bincode::serialize(self)?;
        let mut image = Vec::with_capacity(HEADER_LEN + body.len());
        image.extend_from_slice(&MAGIC);
        image.extend_from_slice(&BLOB_VERSION.to_le_bytes());
        image.extend_from_slice(&body);
        Ok(EncodedRecordBlob { image })
    }

    /// [`Self::encode`] then [`EncodedRecordBlob::write`]. A test
    /// convenience — `ProductionStore` always encodes first so it can
    /// move `records`/`edges` on, then writes the encoded image.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if serialization fails, or
    /// [`DurabilityError::Io`] if the temp file can't be created,
    /// written, synced, or renamed into place.
    #[cfg(test)]
    pub(crate) fn write(&self, path: &Path) -> Result<(), DurabilityError> {
        self.encode()?.write(path)
    }

    /// Read and validate the blob at `path`. Every way this can fail —
    /// the file is missing, shorter than its header, carries the wrong
    /// magic, records an incompatible version, or its body doesn't
    /// decode — maps to one distinctly-named variant,
    /// [`DurabilityError::RecordBlobUnreadable`], naming the path and the
    /// specific cause. Never `InvalidMagic`/`SchemaVersionMismatch`
    /// (those describe the ages file), never a panic
    /// (`STORAGE-014-FR-005`).
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::RecordBlobUnreadable`] as described
    /// above.
    pub(crate) fn read(path: &Path) -> Result<Self, DurabilityError> {
        let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
            path: path.to_path_buf(),
            cause,
        };

        let bytes =
            std::fs::read(path).map_err(|e| unreadable(format!("cannot read file: {e}")))?;
        if bytes.len() < HEADER_LEN || bytes[0..MAGIC.len()] != MAGIC {
            return Err(unreadable(
                "magic number mismatch or file too short for a header — not a record blob"
                    .to_owned(),
            ));
        }
        // Bounds already checked above (`bytes.len() >= HEADER_LEN`), so
        // indexing the four version bytes directly can't panic — same
        // pattern `read_wal_entries` uses for its length prefix.
        let v = MAGIC.len();
        let found = u32::from_le_bytes([bytes[v], bytes[v + 1], bytes[v + 2], bytes[v + 3]]);
        if found != BLOB_VERSION {
            return Err(unreadable(format!(
                "blob version mismatch: file has {found}, this build expects {BLOB_VERSION}"
            )));
        }
        bincode::deserialize(&bytes[HEADER_LEN..])
            .map_err(|e| unreadable(format!("body does not decode: {e}")))
    }

    /// [`Self::encode`] then [`EncodedRecordBlob::is_current_at`]. A
    /// serialization failure counts as "not current" — the subsequent
    /// [`Self::write`] surfaces the real error.
    #[cfg(test)]
    pub(crate) fn is_current_at(&self, path: &Path) -> bool {
        self.encode()
            .is_ok_and(|encoded| encoded.is_current_at(path))
    }
}

/// A [`RecordBlob`] already serialized to its on-disk image — what
/// `ProductionStore::create`/`open` hold onto after moving the live
/// `records`/`edges` into `MmapAgeStore`.
pub(crate) struct EncodedRecordBlob {
    image: Vec<u8>,
}

impl EncodedRecordBlob {
    /// Atomically install this image at `path` (already the companion
    /// path — callers derive it via [`companion_path`]), via
    /// write-to-temp-then-rename. See module docs for the crash-safety
    /// argument.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the temp file can't be created,
    /// written, synced, or renamed into place.
    pub(crate) fn write(&self, path: &Path) -> Result<(), DurabilityError> {
        let mut temp_path = path.as_os_str().to_owned();
        temp_path.push(TEMP_SUFFIX);
        let temp_path = PathBuf::from(temp_path);

        {
            let mut temp_file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)?;
            temp_file.write_all(&self.image)?;
            temp_file.sync_all()?;
        }
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }

    /// Whether the file at `path` is byte-for-byte this image — the check
    /// `ProductionStore::open` uses to skip a redundant rewrite in the
    /// common case (same dataset at `create` and every later `open`,
    /// which is every benchmark/test call site in this crate). A byte
    /// comparison rather than a decode-and-`PartialEq`: `bincode`'s
    /// encoding is deterministic for equal inputs, and comparing bytes
    /// costs one file read with no allocation of a second record set. A
    /// missing or unreadable blob counts as "not current" so that `open`
    /// heals it from the caller-supplied truth — including upgrading an
    /// ages file written before this companion existed.
    pub(crate) fn is_current_at(&self, path: &Path) -> bool {
        std::fs::read(path).is_ok_and(|existing| existing == self.image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durability::test_support::*;
    use crate::test_support::fresh_temp_dir;

    fn sample_blob() -> RecordBlob {
        RecordBlob {
            records: sample_records(),
            edges: sample_edges(),
        }
    }

    #[test]
    fn companion_path_appends_a_fixed_suffix() {
        assert_eq!(
            companion_path(Path::new("/x/ages.mmap")),
            PathBuf::from("/x/ages.mmap.records")
        );
        // No dependence on the ages file having any particular extension.
        assert_eq!(
            companion_path(Path::new("/x/ages")),
            PathBuf::from("/x/ages.records")
        );
    }

    #[test]
    fn write_then_read_round_trips_including_breed_and_edges() {
        let dir = fresh_temp_dir("record_blob_roundtrip").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        let blob = sample_blob();
        blob.write(&path).unwrap();
        let loaded = RecordBlob::read(&path).unwrap();
        assert_eq!(loaded, blob);
        assert_eq!(loaded.records[2].breed, "poodle");
        assert!(blob.is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_blob_is_unreadable_not_a_panic() {
        let dir = fresh_temp_dir("record_blob_missing").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        let result = RecordBlob::read(&path);
        assert!(
            matches!(result, Err(DurabilityError::RecordBlobUnreadable { .. })),
            "expected RecordBlobUnreadable, got {:?}",
            result.err()
        );
        assert!(!sample_blob().is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pointing `read` at an ages file (the wrong magic) must fail as
    /// `RecordBlobUnreadable` — never `InvalidMagic`, which is reserved
    /// for `MmapAgeStore`'s own file.
    #[test]
    fn the_wrong_magic_fails_distinctly_from_the_ages_file_errors() {
        let dir = fresh_temp_dir("record_blob_magic").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        std::fs::write(&path, b"DOGMMAP\0\x01\0\0\0junk").unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("magic"), "unexpected cause: {cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_shorter_than_the_header_is_unreadable_not_a_panic() {
        let dir = fresh_temp_dir("record_blob_short").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        std::fs::write(&path, [0u8; 3]).unwrap();
        assert!(matches!(
            RecordBlob::read(&path),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mismatched_blob_version_is_unreadable_with_a_version_cause() {
        let dir = fresh_temp_dir("record_blob_version").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        sample_blob().write(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[MAGIC.len()..HEADER_LEN].copy_from_slice(&BLOB_VERSION.wrapping_add(1).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("version"), "unexpected cause: {cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_body_is_unreadable_with_a_decode_cause() {
        let dir = fresh_temp_dir("record_blob_truncated").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        sample_blob().write(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("decode"), "unexpected cause: {cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The write-to-temp-then-rename path: a stale temp file left behind
    /// by an interrupted earlier write (the crash-before-rename case) is
    /// overwritten and consumed by the next successful write, and a prior
    /// generation at the real path is replaced whole — never partially.
    #[test]
    fn write_replaces_a_prior_generation_whole_and_consumes_a_stale_temp_file() {
        let dir = fresh_temp_dir("record_blob_rename").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        let mut temp_path = path.as_os_str().to_owned();
        temp_path.push(TEMP_SUFFIX);
        let temp_path = PathBuf::from(temp_path);

        std::fs::write(&path, b"prior generation, not a real blob").unwrap();
        std::fs::write(&temp_path, b"partial image from an interrupted write").unwrap();

        sample_blob().write(&path).unwrap();

        assert_eq!(RecordBlob::read(&path).unwrap(), sample_blob());
        assert!(!temp_path.exists(), "temp file must be renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_current_at_detects_a_changed_record_set() {
        let dir = fresh_temp_dir("record_blob_current").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        sample_blob().write(&path).unwrap();

        let mut fewer = sample_blob();
        fewer.records.pop();
        assert!(!fewer.is_current_at(&path));

        let mut no_edges = sample_blob();
        no_edges.edges.clear();
        assert!(!no_edges.is_current_at(&path));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
