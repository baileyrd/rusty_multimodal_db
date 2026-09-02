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
//! `u32`, then the record set's [`RecordBlob::fingerprint`] as a
//! little-endian `u64`) followed by the `bincode` encoding of
//! [`RecordBlob`]. The header makes "not a record blob at all" (a stray
//! file at the companion path, or a legacy pre-this-feature layout)
//! detectable *before* handing bytes to `bincode`, and distinct from "a
//! record blob from an incompatible build" — both surface as
//! [`DurabilityError::RecordBlobUnreadable`], never as `MmapAgeStore`'s
//! own `InvalidMagic`/`SchemaVersionMismatch`, which describe the *ages*
//! file (`STORAGE-014-FR-005`).
//!
//! # The header fingerprint (blob version 2)
//!
//! Version 1 had no fingerprint, so `ProductionStore::open` could only
//! decide "is the blob on disk already this record set?" by serializing
//! the caller's set and comparing it byte-for-byte with a full read of the
//! file — measured at +27% on `open` at 1M records (`RESULTS.md`). Version
//! 2 puts a 64-bit content fingerprint in the header, so that decision
//! becomes one `fingerprint` pass over the in-memory records plus a
//! 20-byte read ([`RecordBlob::is_current_at`]) — no serialization and no
//! full-file read on the steady-state path. The same fingerprint lets
//! [`RecordBlob::read`] detect a body that decodes but doesn't match what
//! the header claims (a spliced or bit-flipped file).
//!
//! The fingerprint covers exactly the *immutable* content the blob exists
//! to carry — record count, each record's `id` and `breed` in order, edge
//! count, each edge in order — and deliberately **not** `age`: ages live
//! in the ages file (the source of truth), and the copies `DogRecord`
//! carries into the blob are seeds `open` never reads back once the ages
//! file holds the record. Two record sets that differ only in seed ages
//! therefore share a fingerprint and `open` leaves the blob alone, which
//! is the correct outcome (the ages file already reconciled them). The
//! hash is FNV-1a 64, written out inline: it is a fixed, published
//! function (stable across Rust versions, unlike `DefaultHasher`, whose
//! keys are explicitly unspecified), adequate for "did the caller hand me
//! a different dataset?" — this is a change detector against an honest
//! caller, not a defense against an adversary crafting collisions — and
//! this crate adds no dependency for what fits in ten lines (same
//! footing as the hand-written PEM decoder in `server::tls`).
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
//!
//! # Shared with the generic store's blob
//!
//! `GenericMmapStore` carries its own companion (`STORAGE-015`, module
//! `generic::record_blob`) in the *same* header layout under a different
//! magic. The pieces that don't depend on `DogRecord` — [`Fnv1a64`], the
//! [`HEADER_LEN`]-byte header's [`parse_header`]/[`encode_image`],
//! [`companion_path`], and [`EncodedRecordBlob`]'s atomic write — are
//! `pub(crate)` and parameterized by magic/version so both blobs share one
//! implementation rather than two copies that could drift.

use super::DurabilityError;
use crate::record::DogRecord;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Identifies a file as one [`RecordBlob::write`] produced — distinct
/// bytes from `MmapAgeStore`'s `DOGMMAP\0` and `GenericMmapStore`'s own
/// magic, so pointing [`RecordBlob::read`] at either of those files fails
/// cleanly rather than being handed to `bincode`.
const MAGIC: [u8; 8] = *b"DOGBLOB\0";

/// This blob's on-disk layout, versioned from its first release —
/// same convention as `MmapAgeStore`'s `SCHEMA_VERSION`. Version 1 was
/// `MAGIC` + version + body; version 2 added the header fingerprint (see
/// module docs). A version-1 blob is reported as a version mismatch by
/// [`RecordBlob::read`] and counts as stale for [`RecordBlob::is_current_at`],
/// so `ProductionStore::open` rewrites it in the new layout on first use.
const BLOB_VERSION: u32 = 2;

/// Byte offset of the little-endian `u32` [`BLOB_VERSION`] in the header.
const VERSION_OFFSET: usize = MAGIC.len();

/// Byte offset of the little-endian `u64` [`RecordBlob::fingerprint`] in
/// the header.
const FINGERPRINT_OFFSET: usize = VERSION_OFFSET + 4;

/// Magic, then the blob version as a little-endian `u32`, then the
/// fingerprint as a little-endian `u64`: 20 bytes. The one header layout
/// every companion blob in this crate uses (see module docs, "Shared
/// with the generic store's blob").
pub(crate) const HEADER_LEN: usize = FINGERPRINT_OFFSET + 8;

/// FNV-1a 64 — see module docs for why this hash, and why inline. Also
/// an [`io::Write`](std::io::Write) sink, so a `serde` encoder can stream
/// straight into it (the generic blob fingerprints its records that way).
pub(crate) struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub(crate) fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    pub(crate) fn finish(&self) -> u64 {
        self.0
    }
}

impl Write for Fnv1a64 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Validate a fixed [`HEADER_LEN`]-byte header against the `magic` and
/// `expected_version` of the blob kind the caller is reading, and return
/// the fingerprint it records. Shared by [`RecordBlob::read`] (which then
/// decodes the body) and [`RecordBlob::is_current_at`] (which needs
/// nothing else), and by the generic blob under its own magic. `bytes`
/// may be the whole file or just its first [`HEADER_LEN`] bytes. Errors
/// are the `cause` half of a [`DurabilityError::RecordBlobUnreadable`].
pub(crate) fn parse_header(
    bytes: &[u8],
    magic: &[u8; 8],
    expected_version: u32,
) -> Result<u64, String> {
    if bytes.len() < HEADER_LEN || bytes[0..magic.len()] != magic[..] {
        return Err(
            "magic number mismatch or file too short for a header — not a record blob".to_owned(),
        );
    }
    // Bounds already checked above (`bytes.len() >= HEADER_LEN`), so the
    // fixed-width slices below can't panic — same pattern
    // `read_wal_entries` uses for its length prefix.
    let mut version = [0u8; 4];
    version.copy_from_slice(&bytes[VERSION_OFFSET..FINGERPRINT_OFFSET]);
    let found = u32::from_le_bytes(version);
    if found != expected_version {
        return Err(format!(
            "blob version mismatch: file has {found}, this build expects {expected_version}"
        ));
    }
    let mut fingerprint = [0u8; 8];
    fingerprint.copy_from_slice(&bytes[FINGERPRINT_OFFSET..HEADER_LEN]);
    Ok(u64::from_le_bytes(fingerprint))
}

/// Assemble a complete on-disk image: the [`HEADER_LEN`]-byte header
/// (`magic`, `version`, `fingerprint`, little-endian) followed by `body`.
/// The inverse of [`parse_header`] for the header half; shared for the
/// same reason.
pub(crate) fn encode_image(
    magic: &[u8; 8],
    version: u32,
    fingerprint: u64,
    body: &[u8],
) -> Vec<u8> {
    let mut image = Vec::with_capacity(HEADER_LEN + body.len());
    image.extend_from_slice(magic);
    image.extend_from_slice(&version.to_le_bytes());
    image.extend_from_slice(&fingerprint.to_le_bytes());
    image.extend_from_slice(body);
    image
}

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
/// whatever extension the caller chose for the ages file. The generic
/// store uses the same derivation for its own companion
/// (`STORAGE-015-FR-001`).
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
    /// The 64-bit content fingerprint the header carries: FNV-1a 64 over
    /// the record count, each record's `id` bytes, `breed` length and
    /// `breed` bytes in order, the edge count, and each edge's two `id`s
    /// in order. Ages are excluded on purpose (see module docs). Lengths
    /// are hashed so `("ab", "c")` and `("a", "bc")` can't collide by
    /// concatenation, and order matters because the blob preserves caller
    /// order and `bincode` would encode a reordering differently anyway.
    pub(crate) fn fingerprint(&self) -> u64 {
        let mut hash = Fnv1a64::new();
        hash.update(&(self.records.len() as u64).to_le_bytes());
        for record in &self.records {
            hash.update(record.id.as_bytes());
            hash.update(&(record.breed.len() as u64).to_le_bytes());
            hash.update(record.breed.as_bytes());
        }
        hash.update(&(self.edges.len() as u64).to_le_bytes());
        for (from, to) in &self.edges {
            hash.update(from.as_bytes());
            hash.update(to.as_bytes());
        }
        hash.finish()
    }

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
        let body = crate::codec::encode(self)?;
        Ok(EncodedRecordBlob {
            image: encode_image(&MAGIC, BLOB_VERSION, self.fingerprint(), &body),
        })
    }

    /// Write `self` at `path` in the *version-1* layout (no fingerprint)
    /// — what a blob written by a build before this version bump looks
    /// like, for tests that check the upgrade path.
    #[cfg(test)]
    pub(crate) fn write_legacy_v1(&self, path: &Path) -> Result<(), DurabilityError> {
        let body = crate::codec::encode(self)?;
        let mut image = Vec::with_capacity(VERSION_OFFSET + 4 + body.len());
        image.extend_from_slice(&MAGIC);
        image.extend_from_slice(&1u32.to_le_bytes());
        image.extend_from_slice(&body);
        EncodedRecordBlob { image }.write(path)
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
    /// magic, records an incompatible version, its body doesn't decode,
    /// or the decoded body's [`Self::fingerprint`] isn't the one the
    /// header claims — maps to one distinctly-named variant,
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
        let claimed = parse_header(&bytes, &MAGIC, BLOB_VERSION).map_err(unreadable)?;
        let blob: Self = crate::codec::decode(&bytes[HEADER_LEN..])
            .map_err(|e| unreadable(format!("body does not decode: {e}")))?;
        let actual = blob.fingerprint();
        if actual != claimed {
            return Err(unreadable(format!(
                "fingerprint mismatch: header claims {claimed:#018x}, body hashes to {actual:#018x}"
            )));
        }
        Ok(blob)
    }

    /// Whether the blob at `path` already holds this record set — the
    /// check `ProductionStore::open` uses to skip a redundant rewrite in
    /// the common case (same dataset at `create` and every later `open`,
    /// which is every benchmark/test call site in this crate). Costs one
    /// [`Self::fingerprint`] pass over the in-memory records plus a
    /// [`HEADER_LEN`]-byte read — never a serialization, never a read of
    /// the body. A missing, short, foreign, or version-1 file counts as
    /// "not current" so that `open` heals it from the caller-supplied
    /// truth — including upgrading an ages file written before this
    /// companion existed, or a blob written before the fingerprint was.
    pub(crate) fn is_current_at(&self, path: &Path) -> bool {
        let mut header = [0u8; HEADER_LEN];
        let read_header = File::open(path).and_then(|mut file| file.read_exact(&mut header));
        read_header.is_ok() && parse_header(&header, &MAGIC, BLOB_VERSION) == Ok(self.fingerprint())
    }
}

/// A [`RecordBlob`] already serialized to its on-disk image — what
/// `ProductionStore::create`/`open` hold onto after moving the live
/// `records`/`edges` into `MmapAgeStore`. Nothing here is
/// `DogRecord`-specific: the generic blob encodes into the same type to
/// share [`Self::write`].
pub(crate) struct EncodedRecordBlob {
    pub(crate) image: Vec<u8>,
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
        bytes[VERSION_OFFSET..FINGERPRINT_OFFSET]
            .copy_from_slice(&BLOB_VERSION.wrapping_add(1).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("version"), "unexpected cause: {cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!sample_blob().is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A blob from before the fingerprint existed (version 1) is a version
    /// mismatch for `read` and stale for `is_current_at` — the combination
    /// that makes `ProductionStore::open` rewrite it in the new layout.
    #[test]
    fn a_version_1_blob_is_reported_as_a_version_mismatch_and_as_stale() {
        let dir = fresh_temp_dir("record_blob_v1").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        sample_blob().write_legacy_v1(&path).unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("file has 1"), "unexpected cause: {cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!sample_blob().is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A body that decodes but isn't what the header claims (here: the
    /// fingerprint bytes flipped; equivalently, a body spliced in from a
    /// different blob) is unreadable with a fingerprint cause.
    #[test]
    fn a_body_that_does_not_match_the_header_fingerprint_is_unreadable() {
        let dir = fresh_temp_dir("record_blob_fingerprint").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        sample_blob().write(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[FINGERPRINT_OFFSET] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("fingerprint"), "unexpected cause: {cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!sample_blob().is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The fingerprint is a fixed function of the immutable content: the
    /// same value on every call and every build for the same records
    /// (pinned so an accidental change to the hashed fields or the hash
    /// itself shows up as a test failure, not a silent rewrite of every
    /// deployed blob on its next `open`), and different for any change to
    /// an `id`, a `breed`, an edge, or ordering — but not to an age.
    #[test]
    fn fingerprint_is_stable_order_sensitive_and_ignores_ages() {
        let base = sample_blob().fingerprint();
        assert_eq!(
            base, 0xd98e_f572_3749_8d59,
            "pinned FNV-1a 64 of the sample set"
        );

        let mut aged = sample_blob();
        aged.records[0].age = 99;
        assert_eq!(
            aged.fingerprint(),
            base,
            "ages are the ages file's business"
        );

        let mut rebred = sample_blob();
        rebred.records[0].breed = "husky".to_owned();
        assert_ne!(rebred.fingerprint(), base);

        let mut reidentified = sample_blob();
        reidentified.records[0].id = Uuid::from_u128(42);
        assert_ne!(reidentified.fingerprint(), base);

        let mut reordered = sample_blob();
        reordered.records.swap(0, 1);
        assert_ne!(reordered.fingerprint(), base);

        let mut reversed_edge = sample_blob();
        reversed_edge.edges[0] = (reversed_edge.edges[0].1, reversed_edge.edges[0].0);
        assert_ne!(reversed_edge.fingerprint(), base);
    }

    /// `BINENC-FR-004`: the `DOGBLOB\0` body for one record and one edge,
    /// pinned byte for byte — record count, then each record's id (a
    /// `u64` length of 16 and the 16 big-endian bytes), breed (`u64`
    /// length, UTF-8), and `u32` age; then edge count and each pair of
    /// ids. A file assembled from the shared header and exactly these
    /// bytes reads back as the record set, which is what "a blob written
    /// by a build before this pin is still readable" means concretely.
    #[test]
    fn one_record_one_edge_body_encodes_to_its_pinned_bytes() {
        let blob = RecordBlob {
            records: vec![DogRecord::new(Uuid::from_u128(1), "labrador", 3)],
            edges: vec![(Uuid::from_u128(1), Uuid::from_u128(2))],
        };
        const BODY: [u8; 108] = [
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // records.len() = 1
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // id: len = 16
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, //
            0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // breed: len = 8
            b'l', b'a', b'b', b'r', b'a', b'd', b'o', b'r', //
            0x03, 0x00, 0x00, 0x00, // age
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // edges.len() = 1
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // from
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, //
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // to
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
        ];
        crate::test_support::assert_golden_eq("DOGBLOB body", &blob, &BODY);

        let image = blob.encode().unwrap().image;
        assert_eq!(&image[HEADER_LEN..], &BODY);
        assert_eq!(
            image[..HEADER_LEN],
            encode_image(&MAGIC, BLOB_VERSION, blob.fingerprint(), &[])
        );

        let dir = fresh_temp_dir("record_blob_golden").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        std::fs::write(
            &path,
            encode_image(&MAGIC, BLOB_VERSION, blob.fingerprint(), &BODY),
        )
        .unwrap();
        assert_eq!(RecordBlob::read(&path).unwrap(), blob);
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

    /// `STORAGE-018` implementation-time finding: unlike the two generic
    /// blobs, this one decodes the body *before* fingerprinting, and its
    /// fingerprint is over the decoded records rather than the bytes —
    /// so a body with junk appended used to be silently accepted. Under
    /// the codec's trailing-bytes rejection (`BINENC-FR-002`) it is now a
    /// decode error, the same `RecordBlobUnreadable` a truncated body
    /// gets.
    #[test]
    fn a_body_with_trailing_junk_is_unreadable_with_a_decode_cause() {
        let dir = fresh_temp_dir("record_blob_trailing_junk").unwrap();
        let path = companion_path(&dir.join("ages.mmap"));
        sample_blob().write(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(&[0xaa, 0xbb]);
        std::fs::write(&path, &bytes).unwrap();
        match RecordBlob::read(&path).err() {
            Some(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("body does not decode"),
                    "unexpected cause: {cause}"
                );
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

        // Different seed ages, same immutable content: current. The ages
        // file is the source of truth for ages; rewriting the blob for
        // them would be wasted I/O on every `open` after an `update_age`.
        let mut aged = sample_blob();
        aged.records[0].age = 99;
        assert!(aged.is_current_at(&path));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
