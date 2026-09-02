//! The `Symmetric` layer's companion "edge blob": the full edge list a
//! [`super::store::Symmetric`] was built from, persisted as one
//! write-once, `bincode`-serialized file at a caller-supplied path beside
//! the inner store's own files. The relation-layer analogue of
//! [`super::record_blob`] (`GenericMmapStore`'s companion), closing the
//! last gap in the `Employee` stack's portability: `GenericMmapStore`
//! carries its records in `<path>.records`, but the symmetric edges lived
//! only in the caller's hands, so a directory holding `<path>` and
//! `<path>.records` could not rebuild the whole stack. See
//! `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (Accepted) and
//! ADR-0018 for the decision, and
//! `docs/specifications/storage/STORAGE-016-symmetric-edge-list-portability.md`
//! for the requirements.
//!
//! # What the blob holds
//!
//! The edge list **as given** — `Vec<(Id, Id)>` in caller order — not the
//! adjacency map `Symmetric` builds from it. The map is twice the size
//! (every edge under both endpoints) and its `HashMap` iteration order is
//! nondeterministic, so two `create`s of the same edges would produce
//! different bytes and fingerprints. The list is half the size, its
//! bytes are a pure function of the input, and the unchanged
//! `Symmetric::new` rebuilds the map from it exactly as it does from a
//! caller's slice. Order is part of the fingerprint deliberately: order is
//! observable through `neighbors`'s result order, so a reordered list *is*
//! a different layer (`SYMPORT-FR-008`).
//!
//! The header layout, the FNV-1a 64 hash, the atomic
//! write-to-temp-then-rename install, and the
//! [`DurabilityError::RecordBlobUnreadable`] error variant are all shared
//! with the two record blobs rather than copied — see
//! `durability::record_blob`'s "Shared with the generic store's blob".
//! Only the magic differs, so a `GENBLOB\0` or `DOGBLOB\0` file at an
//! edge blob's path is a magic error, not a `bincode` decode attempt
//! (`SYMPORT-FR-005`).
//!
//! # The schema tag (version 2)
//!
//! Version 2 appends the relation's schema tag — the FNV-1a 64 hash of
//! `R::SCHEMA_TAG` for the record type the `Symmetric` layer is over — to
//! the shared 20-byte header, and every read path checks it before the
//! body is decoded (`SCHTAG-FR-001`, `-003`). Two relations over
//! different record types with the same `Id` type (`Employee` and some
//! other `Uuid`-keyed record) encode to the same bytes, and the magic and
//! version can't tell them apart; the tag can, so a blob written from one
//! is refused by name when read as the other rather than accepted as
//! whatever `Vec<(Id, Id)>` it decodes to. The tag is passed in by value
//! (`Symmetric` passes `R::SCHEMA_TAG`) rather than through a type
//! parameter: the blob's own type parameter is `Id`, not `R`. Layout and
//! helpers live in [`super::record_blob`]. See
//! `docs/design/BLOB-SCHEMA-TAG-DESIGN.md` (Accepted) and ADR-0019.

use super::record_blob::{encode_tagged_image, parse_tagged_header, TAGGED_HEADER_LEN};
use crate::durability::record_blob::{EncodedRecordBlob, Fnv1a64};
use crate::durability::DurabilityError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Identifies a file as one [`EdgeBlob::encode`] produced — distinct from
/// `DOGBLOB\0` (the `ProductionStore` companion), `GENBLOB\0` (the
/// `GenericMmapStore` companion), `DOGMMAP\0`, and `GMMAPST\0`.
const MAGIC: [u8; 8] = *b"GENEDGE\0";

/// This blob's on-disk layout, versioned from its first release, with
/// the header fingerprint from the start (the same footing as
/// `GENBLOB\0`). Version 2 added the schema tag after the shared header;
/// a version-1 blob is a version error on every read path and is
/// rewritten by `Symmetric::open` (`SCHTAG-FR-006`).
const BLOB_VERSION: u32 = 2;

/// The suffix appended to a stack's primary path to name the
/// single-relation edge blob — see [`edges_path`].
const EDGES_SUFFIX: &str = ".edges";

/// The edge list a `Symmetric` layer was built from, exactly as
/// `create`/`open` receive it and in caller order. Held by reference: the
/// layer encodes and fingerprints the caller's slice before building its
/// adjacency map, so no copy of the list is made just to persist it.
pub(crate) struct EdgeBlob<'a, Id> {
    edges: &'a [(Id, Id)],
    /// The schema tag of the record type the relation is over
    /// (`R::SCHEMA_TAG`), written into the header and checked by
    /// [`Self::is_current_at`].
    tag: &'static str,
}

impl<'a, Id> EdgeBlob<'a, Id>
where
    Id: Serialize,
{
    pub(crate) fn new(edges: &'a [(Id, Id)], tag: &'static str) -> Self {
        Self { edges, tag }
    }

    /// The 64-bit content fingerprint the header carries: FNV-1a 64 over
    /// the `bincode` encoding of the whole edge list, streamed into the
    /// hasher rather than materialized.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if serialization fails.
    pub(crate) fn fingerprint(&self) -> Result<u64, DurabilityError> {
        let mut hash = Fnv1a64::new();
        bincode::serialize_into(&mut hash, self.edges)?;
        Ok(hash.finish())
    }

    /// Serialize the edge list into its complete on-disk image (header +
    /// body). The body is encoded once; the fingerprint is taken over
    /// those same bytes so it can never disagree with what is written.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if serialization fails.
    pub(crate) fn encode(&self) -> Result<EncodedRecordBlob, DurabilityError> {
        let body = bincode::serialize(self.edges)?;
        let mut hash = Fnv1a64::new();
        hash.update(&body);
        Ok(EncodedRecordBlob {
            image: encode_tagged_image(&MAGIC, BLOB_VERSION, hash.finish(), self.tag, &body),
        })
    }

    /// Whether the blob at `path` already holds this edge list, in this
    /// order — the check `Symmetric::open` uses to skip a redundant
    /// rewrite in the common case (`SYMPORT-FR-004`). Costs one
    /// [`Self::fingerprint`] pass over the in-memory edges plus a
    /// [`TAGGED_HEADER_LEN`]-byte read — never a full serialization to a
    /// buffer, never a read of the body. A missing, short, foreign
    /// (`GENBLOB\0` included), wrong-version, or wrong-tag file counts as
    /// "not current" so that `open` heals it from the caller-supplied
    /// truth — including a directory written before the edge blob existed
    /// or before it carried a tag. A serialization
    /// failure also counts as "not current": `open` then attempts the
    /// rewrite, which surfaces the same failure as a proper error instead
    /// of swallowing it here.
    pub(crate) fn is_current_at(&self, path: &Path) -> bool {
        let mut header = [0u8; TAGGED_HEADER_LEN];
        let read_header = File::open(path).and_then(|mut file| file.read_exact(&mut header));
        read_header.is_ok()
            && self.fingerprint().is_ok_and(|fp| {
                parse_tagged_header(&header, &MAGIC, BLOB_VERSION, self.tag) == Ok(fp)
            })
    }
}

/// Read and validate the blob at `path`, returning the edge list it holds
/// in persisted order. Every way this can fail — the file is missing,
/// shorter than its header, carries the wrong magic, records an
/// incompatible version, carries a schema tag other than `tag`'s, its
/// body doesn't decode, or the body's bytes don't hash to the fingerprint
/// the header claims — maps to [`DurabilityError::RecordBlobUnreadable`],
/// naming the edge blob path and the specific cause. Never a panic, never
/// a silently empty list (`SYMPORT-FR-005`, `SCHTAG-FR-003`).
///
/// # Errors
///
/// Returns [`DurabilityError::RecordBlobUnreadable`] as described above.
pub(crate) fn read<Id>(path: &Path, tag: &'static str) -> Result<Vec<(Id, Id)>, DurabilityError>
where
    Id: DeserializeOwned,
{
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: path.to_path_buf(),
        cause,
    };

    let bytes = std::fs::read(path).map_err(|e| unreadable(format!("cannot read file: {e}")))?;
    let claimed = parse_tagged_header(&bytes, &MAGIC, BLOB_VERSION, tag).map_err(unreadable)?;
    let body = &bytes[TAGGED_HEADER_LEN..];
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

/// The single-relation convention for where a stack whose primary file
/// lives at `path` keeps its one `Symmetric` layer's edge blob:
/// `<path>.edges`, beside `<path>.records`. A stack with two symmetric
/// relations over one store needs two distinct paths and must derive them
/// itself — `Symmetric` takes the path as an argument for exactly that
/// reason.
pub(crate) fn edges_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(EDGES_SUFFIX);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::super::record_blob::{tag_hash, TAG_OFFSET};
    use super::super::traits::SchemaTag;
    use super::*;
    use crate::durability::record_blob::HEADER_LEN;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    /// The tag every test writes under — a stand-in for `R::SCHEMA_TAG`.
    const TAG: &str = "edge_blob::tests::Node";

    /// A different record type's tag over the same `Uuid` id: the case
    /// only the tag can tell apart.
    const OTHER_TAG: &str = "edge_blob::tests::Other";

    fn sample() -> Vec<(Uuid, Uuid)> {
        vec![
            (Uuid::from_u128(1), Uuid::from_u128(2)),
            (Uuid::from_u128(2), Uuid::from_u128(3)),
            (Uuid::from_u128(1), Uuid::from_u128(3)),
        ]
    }

    #[test]
    fn edges_path_appends_a_fixed_suffix_beside_the_records_blob() {
        assert_eq!(
            edges_path(Path::new("/x/store.mmap")),
            PathBuf::from("/x/store.mmap.edges")
        );
        assert_ne!(
            edges_path(Path::new("/x/store.mmap")),
            super::super::record_blob::blob_path(Path::new("/x/store.mmap"))
        );
    }

    #[test]
    fn encode_then_read_round_trips_uuid_pairs_in_order() {
        let dir = fresh_temp_dir("edge_blob_roundtrip").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let edges = sample();
        let blob = EdgeBlob::new(&edges, TAG);
        blob.encode().unwrap().write(&path).unwrap();
        let loaded: Vec<(Uuid, Uuid)> = read(&path, TAG).unwrap();
        assert_eq!(loaded, edges);
        assert!(blob.is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streamed_fingerprint_equals_the_one_written_in_the_header() {
        let dir = fresh_temp_dir("edge_blob_fingerprint").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let edges = sample();
        let blob = EdgeBlob::new(&edges, TAG);
        blob.encode().unwrap().write(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header_fp = parse_tagged_header(&bytes, &MAGIC, BLOB_VERSION, TAG).unwrap();
        assert_eq!(header_fp, blob.fingerprint().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reordered_or_different_edge_list_is_not_current() {
        let dir = fresh_temp_dir("edge_blob_stale").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let edges = sample();
        EdgeBlob::new(&edges, TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let mut reordered = sample();
        reordered.swap(0, 2);
        assert!(!EdgeBlob::new(&reordered, TAG).is_current_at(&path));
        assert!(!EdgeBlob::new(&edges[..2], TAG).is_current_at(&path));
        let mut flipped = sample();
        flipped[0] = (flipped[0].1, flipped[0].0);
        assert!(!EdgeBlob::new(&flipped, TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_edge_list_round_trips() {
        let dir = fresh_temp_dir("edge_blob_empty").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let edges: Vec<(Uuid, Uuid)> = Vec::new();
        EdgeBlob::new(&edges, TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let loaded: Vec<(Uuid, Uuid)> = read(&path, TAG).unwrap();
        assert!(loaded.is_empty());
        assert!(EdgeBlob::new(&edges, TAG).is_current_at(&path));
        assert!(!EdgeBlob::new(&sample(), TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_blob_is_unreadable_and_not_current() {
        let dir = fresh_temp_dir("edge_blob_missing").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let edges = sample();
        assert!(!EdgeBlob::new(&edges, TAG).is_current_at(&path));
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { path: p, cause }) => {
                assert_eq!(p, path);
                assert!(cause.starts_with("cannot read file"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_file_is_a_header_error_not_a_panic() {
        let dir = fresh_temp_dir("edge_blob_short").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        std::fs::write(&path, &MAGIC[..5]).unwrap();
        assert!(!EdgeBlob::new(&sample(), TAG).is_current_at(&path));
        assert!(matches!(
            read::<Uuid>(&path, TAG),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_generic_record_blob_at_the_edges_path_is_a_magic_error() {
        let dir = fresh_temp_dir("edge_blob_genblob_magic").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        // A record blob whose body happens to be a `Vec<(Uuid, Uuid)>`
        // and whose tag is this relation's too: only the magic tells the
        // two apart, and it must — before the tag is even looked at.
        #[derive(serde::Serialize)]
        struct Pair(Uuid, Uuid);
        impl SchemaTag for Pair {
            const SCHEMA_TAG: &'static str = TAG;
        }
        let edges = sample();
        let pairs: Vec<Pair> = edges.iter().map(|&(a, b)| Pair(a, b)).collect();
        super::super::record_blob::GenericRecordBlob::new(&pairs)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("magic number mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!EdgeBlob::new(&edges, TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dog_blob_at_the_edges_path_is_a_magic_error() {
        let dir = fresh_temp_dir("edge_blob_dog_magic").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let dog = crate::durability::record_blob::RecordBlob {
            records: crate::durability::test_support::sample_records(),
            edges: crate::durability::test_support::sample_edges(),
        };
        dog.write(&path).unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("magic number mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_version_is_a_version_error() {
        let dir = fresh_temp_dir("edge_blob_version").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        EdgeBlob::new(&sample(), TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[MAGIC.len()..MAGIC.len() + 4]
            .copy_from_slice(&BLOB_VERSION.wrapping_add(1).to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("blob version mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!EdgeBlob::new(&sample(), TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_that_does_not_match_the_header_fingerprint_is_rejected() {
        let dir = fresh_temp_dir("edge_blob_tamper").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        EdgeBlob::new(&sample(), TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff; // flips a byte of the last edge's second id
        std::fs::write(&path, &bytes).unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("fingerprint mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_body_is_a_fingerprint_error_not_a_panic() {
        let dir = fresh_temp_dir("edge_blob_truncated").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        EdgeBlob::new(&sample(), TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("fingerprint mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blob_written_under_another_tag_is_a_tag_error_not_a_decode() {
        let dir = fresh_temp_dir("edge_blob_other_tag").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        let edges = sample();
        EdgeBlob::new(&edges, OTHER_TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        // Same `Id`, same bytes in the body: a version-1 blob would have
        // decoded this happily (`SCHTAG-FR-001`).
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { path: p, cause }) => {
                assert_eq!(p, path);
                assert!(
                    cause.starts_with(
                        "schema tag mismatch: this store expects `edge_blob::tests::Node`"
                    ),
                    "{cause}"
                );
                assert!(
                    cause.contains(&format!("{:#018x}", tag_hash(OTHER_TAG))),
                    "{cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!EdgeBlob::new(&edges, TAG).is_current_at(&path));
        assert!(EdgeBlob::new(&edges, OTHER_TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_1_blob_is_a_version_error_before_the_tag_is_looked_at() {
        let dir = fresh_temp_dir("edge_blob_v1").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        EdgeBlob::new(&sample(), TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        // Rebuild the exact version-1 image: version field 1, no tag
        // between the shared header and the body (`SCHTAG-FR-006`).
        let bytes = std::fs::read(&path).unwrap();
        let mut v1 = Vec::with_capacity(bytes.len() - 8);
        v1.extend_from_slice(&bytes[..HEADER_LEN]);
        v1.extend_from_slice(&bytes[TAGGED_HEADER_LEN..]);
        v1[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&path, &v1).unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("blob version mismatch: file has 1"),
                    "{cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!EdgeBlob::new(&sample(), TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_cut_inside_the_tag_is_a_short_tagged_header_error() {
        let dir = fresh_temp_dir("edge_blob_short_tag").unwrap();
        let path = edges_path(&dir.join("store.mmap"));
        EdgeBlob::new(&sample(), TAG)
            .encode()
            .unwrap()
            .write(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..TAG_OFFSET + 3]).unwrap();
        match read::<Uuid>(&path, TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("file too short for a tagged header"),
                    "{cause}"
                );
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(!EdgeBlob::new(&sample(), TAG).is_current_at(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
