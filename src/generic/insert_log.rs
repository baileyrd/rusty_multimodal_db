//! The insert log — `<path>.inserts`, the append-only companion file
//! holding every record inserted into a [`super::mmap_store::GenericMmapStore`]
//! at runtime since its record blob was last rewritten (`INS-FR-002`/
//! `INS-FR-003`, ADR-0046, `docs/design/SERVER-INSERT-DESIGN.md`).
//!
//! # Why a log beside the blob, not a rewrite of it
//!
//! The record blob (`super::record_blob`, `STORAGE-015`) is one
//! fingerprinted `bincode` `Vec<R>` image, rewritten whole whenever the
//! record set changes at `open`. Rewriting it on every runtime insert is
//! O(records) per write — ~76 B/record measured in `STORAGE-015`, so a
//! 100 K-record store would write ~7.6 MB per insert. This log makes an
//! insert O(record): one length-prefixed entry appended and `fsync`ed.
//! `GenericMmapStore::open` folds the log back into the blob through
//! the rewrite it already performs for a changed record set, then
//! removes the log (`INS-FR-004`), so the blob's single-image and
//! persisted-order properties are untouched and a reopen compacts for
//! free.
//!
//! # Format
//!
//! The blob's own 28-byte tagged header (`super::record_blob::
//! encode_tagged_image`) under the magic `GENINSL\0`, version 2: the
//! fingerprint field is always zero — the log is per-entry framed, not
//! fingerprinted whole — and the tag hash refuses a log written for
//! another record type by name, exactly as the blob does
//! (`SCHTAG-FR-003`). Then zero or more entries, each a **kind byte**
//! (`0` an item, `1` a tombstone — `DEL-FR-003`, ADR-0051), a `u32`
//! little-endian length, and that many bytes of `bincode` (`crate::
//! codec`): the record `R` for an item, its `R::Id` for a tombstone.
//! Version 1 (`INS-FR-003`, ADR-0046) had no kind byte — every entry an
//! item; it is still read, and rewritten as version 2 on its next
//! append. A torn tail — a kind or length whose payload never fully
//! landed — is dropped at read, the batch journal's rule (`JRN-FR-003`,
//! `src/server/journal.rs`): the entry it belonged to was never
//! acknowledged, since `append` returns only after `sync_data`.
//!
//! The same format serves the edge logs beside every edge blob
//! (`LNK-FR-003`, over `(Id, Id)` items) — there a tombstone names an
//! id every edge of which is to go (`DEL-FR-004`).
//!
//! The header and the first entry are written in one `write_all` so a
//! crash can never leave a header-only or half-header file that a later
//! `append` would misread.

use super::record_blob::{encode_tagged_image, parse_tagged_header, TAGGED_HEADER_LEN};
use super::traits::SchemaTag;
use crate::durability::DurabilityError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAGIC: [u8; 8] = *b"GENINSL\0";
/// Version 1 (`INS-FR-003`): `u32`-length-prefixed items, nothing else.
/// Version 2 (`DEL-FR-003`, ADR-0051): every entry carries a leading
/// kind byte — [`KIND_ITEM`] for an item, [`KIND_TOMBSTONE`] for a
/// deletion naming a key — so one ordered log can carry a delete and a
/// later re-insert of the same id. A version-1 log is still read (every
/// entry an item) and is rewritten as version 2 on its next append.
const LOG_VERSION: u32 = 2;
const LOG_VERSION_1: u32 = 1;
/// Where the shared header keeps its version: after the 8-byte magic
/// (`crate::durability::record_blob`'s `VERSION_OFFSET`).
const VERSION_OFFSET: usize = 8;
const KIND_ITEM: u8 = 0;
const KIND_TOMBSTONE: u8 = 1;

/// One entry of a version-2 log (`DEL-FR-003`): an item, or a tombstone
/// for the key an earlier entry (or the blob) holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogEntry<T, K> {
    Item(T),
    Tombstone(K),
}
const LOG_SUFFIX: &str = ".inserts";

/// Where a `GenericMmapStore` whose mmap file lives at `path` keeps its
/// insert log — `<path>.inserts`, the `<path>.records` derivation with
/// a different suffix.
pub(crate) fn log_path(path: &Path) -> PathBuf {
    let mut log = path.as_os_str().to_owned();
    log.push(LOG_SUFFIX);
    PathBuf::from(log)
}

/// Append one record to the log at `log`, creating the file (header
/// included) on first use. When this returns `Ok`, the entry is on disk
/// (`sync_data`) — the durability `GenericMmapStore::insert` promises.
///
/// # Errors
///
/// Returns [`DurabilityError::Serde`] if `record` can't be serialized,
/// [`DurabilityError::Io`] if the file can't be opened, written, or
/// synced, or if the encoded record exceeds a `u32` length prefix.
pub(crate) fn append<R>(log: &Path, record: &R) -> Result<(), DurabilityError>
where
    R: Serialize + SchemaTag,
{
    append_item(log, R::SCHEMA_TAG, record)
}

/// [`append`] over any serializable item under an explicit `tag` —
/// `LNK-FR-003` (ADR-0047): an edge log holds `(Id, Id)` pairs, which
/// carry no `SchemaTag` of their own, tagged with the *record type's*
/// tag exactly as the edge blob is. The record-facing [`append`] is this
/// with `R::SCHEMA_TAG`.
pub(crate) fn append_item<T>(log: &Path, tag: &str, item: &T) -> Result<(), DurabilityError>
where
    T: Serialize + ?Sized,
{
    append_entry(log, tag, KIND_ITEM, &crate::codec::encode(item)?)
}

/// `DEL-FR-003` (ADR-0051): append a tombstone for `key` — the id of a
/// record, or the id every edge of which is to go — under `tag`'s log.
/// Synced before returning, exactly as [`append_item`].
pub(crate) fn append_tombstone<K>(log: &Path, tag: &str, key: &K) -> Result<(), DurabilityError>
where
    K: Serialize + ?Sized,
{
    append_entry(log, tag, KIND_TOMBSTONE, &crate::codec::encode(key)?)
}

/// One `kind` + `u32` length + payload entry, after the header on a new
/// file. A version-1 log found here (written before `DEL-FR-003`, never
/// reopened since) is rewritten as version 2 first — its entries are all
/// items — so a file is never a mix of the two layouts.
fn append_entry(log: &Path, tag: &str, kind: u8, payload: &[u8]) -> Result<(), DurabilityError> {
    let len = u32::try_from(payload.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "encoded record too large for the insert log's u32 length prefix",
        )
    })?;
    upgrade_if_version_1(log, tag)?;
    let mut file = OpenOptions::new().append(true).create(true).open(log)?;
    let mut image = if file.metadata()?.len() == 0 {
        encode_tagged_image(&MAGIC, LOG_VERSION, 0, tag, &[])
    } else {
        Vec::new()
    };
    image.push(kind);
    image.extend_from_slice(&len.to_le_bytes());
    image.extend_from_slice(payload);
    file.write_all(&image)?;
    file.sync_data()?;
    Ok(())
}

/// The version a log file on disk declares, or `None` when there is no
/// file (or too little of one to carry a header).
fn on_disk_version(log: &Path) -> Result<Option<u32>, DurabilityError> {
    let bytes = match std::fs::read(log) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(bytes
        .get(VERSION_OFFSET..VERSION_OFFSET + 4)
        .map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]])))
}

/// Rewrite a version-1 log as version 2 in place (every entry an item),
/// via a temporary file and a rename so a crash leaves one or the other.
fn upgrade_if_version_1(log: &Path, tag: &str) -> Result<(), DurabilityError> {
    if on_disk_version(log)? != Some(LOG_VERSION_1) {
        return Ok(());
    }
    let bytes = std::fs::read(log)?;
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: log.to_path_buf(),
        cause,
    };
    parse_tagged_header(&bytes, &MAGIC, LOG_VERSION_1, tag).map_err(unreadable)?;
    let mut image = encode_tagged_image(&MAGIC, LOG_VERSION, 0, tag, &[]);
    for (_, payload) in raw_entries(&bytes, LOG_VERSION_1) {
        image.push(KIND_ITEM);
        image.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        image.extend_from_slice(payload);
    }
    let mut tmp = log.as_os_str().to_owned();
    tmp.push(".upgrade");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, &image)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&tmp)?
        .sync_data()?;
    std::fs::rename(&tmp, log)?;
    Ok(())
}

/// Every complete `(kind, payload)` entry after the header, in order; a
/// torn tail (a kind or length that landed, a payload that did not) ends
/// the walk. Version 1 has no kind byte: every entry is an item.
fn raw_entries(bytes: &[u8], version: u32) -> Vec<(u8, &[u8])> {
    let mut entries = Vec::new();
    let mut pos = TAGGED_HEADER_LEN;
    loop {
        let kind = if version == LOG_VERSION_1 {
            KIND_ITEM
        } else {
            match bytes.get(pos) {
                Some(&kind) => {
                    pos += 1;
                    kind
                }
                None => break,
            }
        };
        let Some(len_bytes) = bytes.get(pos..pos + 4) else {
            break;
        };
        let len =
            u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
        let Some(payload) = bytes.get(pos + 4..pos + 4 + len) else {
            break; // torn tail: the length landed, the payload did not
        };
        entries.push((kind, payload));
        pos += 4 + len;
    }
    entries
}

/// Every complete entry in the log at `log`, oldest first. A missing
/// file is an empty log — the state of every store nothing was ever
/// inserted into. A torn trailing entry is dropped (see module docs).
///
/// # Errors
///
/// Returns [`DurabilityError::RecordBlobUnreadable`], naming the log
/// path, if the file exists but is shorter than its header, carries the
/// wrong magic, records another version, was written for another
/// record type, or holds a complete entry that doesn't decode; or
/// [`DurabilityError::Io`] if it can't be read for any other reason.
#[cfg(test)]
pub(crate) fn read<R>(log: &Path) -> Result<Vec<R>, DurabilityError>
where
    R: DeserializeOwned + SchemaTag,
{
    read_items(log, R::SCHEMA_TAG)
}

/// [`read`] over any deserializable item under an explicit `tag` — the
/// edge log's reader (`LNK-FR-003`).
#[cfg(test)]
pub(crate) fn read_items<T>(log: &Path, tag: &str) -> Result<Vec<T>, DurabilityError>
where
    T: DeserializeOwned,
{
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: log.to_path_buf(),
        cause,
    };
    let mut items = Vec::new();
    for (index, kind, payload) in read_raw(log, tag)? {
        if kind == KIND_ITEM {
            items.push(
                crate::codec::decode(&payload)
                    .map_err(|e| unreadable(format!("entry {index} does not decode: {e}")))?,
            );
        }
    }
    Ok(items)
}

/// `DEL-FR-003` (ADR-0051): every entry of the log at `log`, in order —
/// items as `T`, tombstones as `K`. A version-1 log reads as items only;
/// a version-2 log's kind byte decides. A missing log is empty; a torn
/// tail is dropped; a foreign tag or an unknown kind is
/// [`DurabilityError::RecordBlobUnreadable`] naming the log. (The
/// fingerprint the header claims is always zero for a log and is
/// ignored — every entry is framed and decoded on its own.)
pub(crate) fn read_entries<T, K>(
    log: &Path,
    tag: &str,
) -> Result<Vec<LogEntry<T, K>>, DurabilityError>
where
    T: DeserializeOwned,
    K: DeserializeOwned,
{
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: log.to_path_buf(),
        cause,
    };
    let mut entries = Vec::new();
    for (index, kind, payload) in read_raw(log, tag)? {
        let entry = match kind {
            KIND_ITEM => LogEntry::Item(
                crate::codec::decode(&payload)
                    .map_err(|e| unreadable(format!("entry {index} does not decode: {e}")))?,
            ),
            KIND_TOMBSTONE => LogEntry::Tombstone(
                crate::codec::decode(&payload)
                    .map_err(|e| unreadable(format!("tombstone {index} does not decode: {e}")))?,
            ),
            other => {
                return Err(unreadable(format!(
                    "entry {index} has unknown kind {other}"
                )))
            }
        };
        entries.push(entry);
    }
    Ok(entries)
}

/// The header check and the framing walk both readers share: every
/// complete entry as `(index, kind, payload)`, undecoded.
fn read_raw(log: &Path, tag: &str) -> Result<Vec<(usize, u8, Vec<u8>)>, DurabilityError> {
    let unreadable = |cause: String| DurabilityError::RecordBlobUnreadable {
        path: log.to_path_buf(),
        cause,
    };
    let bytes = match std::fs::read(log) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let version = match on_disk_version(log)? {
        Some(LOG_VERSION_1) => LOG_VERSION_1,
        _ => LOG_VERSION,
    };
    parse_tagged_header(&bytes, &MAGIC, version, tag).map_err(unreadable)?;
    Ok(raw_entries(&bytes, version)
        .into_iter()
        .enumerate()
        .map(|(index, (kind, payload))| (index, kind, payload.to_vec()))
        .collect())
}

/// Remove the log at `log` — only after every entry it held is in the
/// record blob (`GenericMmapStore::open`'s fold). A missing file is
/// already the state this leaves behind, not an error.
///
/// # Errors
///
/// Returns [`DurabilityError::Io`] if the file exists and can't be
/// removed.
pub(crate) fn clear(log: &Path) -> Result<(), DurabilityError> {
    match std::fs::remove_file(log) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
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
    }

    impl SchemaTag for Item {
        const SCHEMA_TAG: &'static str = "insert_log::tests::Item";
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Other {
        id: u32,
        label: String,
    }

    impl SchemaTag for Other {
        const SCHEMA_TAG: &'static str = "insert_log::tests::Other";
    }

    fn item(id: u32) -> Item {
        Item {
            id,
            label: format!("item {id}"),
        }
    }

    #[test]
    fn log_path_appends_a_fixed_suffix() {
        assert_eq!(
            log_path(Path::new("/x/store.mmap")),
            PathBuf::from("/x/store.mmap.inserts")
        );
    }

    #[test]
    fn a_missing_log_reads_as_empty_and_clear_is_a_no_op() {
        let dir = fresh_temp_dir("insert_log_missing").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        assert_eq!(read::<Item>(&log).unwrap(), Vec::<Item>::new());
        clear(&log).unwrap();
        assert!(!log.exists());
    }

    #[test]
    fn appended_records_read_back_in_order_and_clear_removes_the_file() {
        let dir = fresh_temp_dir("insert_log_round_trip").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        append(&log, &item(1)).unwrap();
        append(&log, &item(2)).unwrap();
        append(&log, &item(3)).unwrap();
        assert_eq!(read::<Item>(&log).unwrap(), vec![item(1), item(2), item(3)]);
        clear(&log).unwrap();
        assert!(!log.exists());
        assert_eq!(read::<Item>(&log).unwrap(), Vec::<Item>::new());
    }

    #[test]
    fn the_header_is_written_exactly_once() {
        let dir = fresh_temp_dir("insert_log_header_once").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        append(&log, &item(1)).unwrap();
        let after_one = std::fs::metadata(&log).unwrap().len();
        append(&log, &item(1)).unwrap();
        let after_two = std::fs::metadata(&log).unwrap().len();
        let entry = crate::codec::encode(&item(1)).unwrap().len() as u64 + 5;
        assert_eq!(after_one, TAGGED_HEADER_LEN as u64 + entry);
        assert_eq!(after_two, after_one + entry);
    }

    /// `LNK-FR-003`: an item with no `SchemaTag` of its own, logged under
    /// an explicit tag — the edge log's shape — round-trips, and is
    /// refused by name under another tag.
    #[test]
    fn tagged_items_round_trip_and_a_foreign_tag_is_refused() {
        let dir = fresh_temp_dir("insert_log_items").unwrap();
        let log = log_path(&dir.join("store.mmap.edges"));
        append_item(&log, "edges::Item", &(1u32, 2u32)).unwrap();
        append_item(&log, "edges::Item", &(2u32, 3u32)).unwrap();
        assert_eq!(
            read_items::<(u32, u32)>(&log, "edges::Item").unwrap(),
            vec![(1, 2), (2, 3)]
        );
        assert!(matches!(
            read_items::<(u32, u32)>(&log, "edges::Other"),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
    }

    #[test]
    fn a_torn_tail_is_dropped_and_every_complete_entry_kept() {
        let dir = fresh_temp_dir("insert_log_torn").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        append(&log, &item(1)).unwrap();
        append(&log, &item(2)).unwrap();
        let mut bytes = std::fs::read(&log).unwrap();
        // Chop the last entry in half: its length prefix is intact, its
        // payload is not.
        let cut = bytes.len() - 3;
        bytes.truncate(cut);
        std::fs::write(&log, &bytes).unwrap();
        assert_eq!(read::<Item>(&log).unwrap(), vec![item(1)]);
    }

    #[test]
    fn a_log_for_another_record_type_is_refused_by_name() {
        let dir = fresh_temp_dir("insert_log_foreign").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        append(&log, &item(1)).unwrap();
        let err = read::<Other>(&log).unwrap_err();
        match err {
            DurabilityError::RecordBlobUnreadable { path, cause } => {
                assert_eq!(path, log);
                assert!(cause.contains("schema tag mismatch"), "{cause}");
                assert!(cause.contains("insert_log::tests::Other"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
    }

    #[test]
    fn a_wrong_magic_and_a_short_file_are_refused_by_name() {
        let dir = fresh_temp_dir("insert_log_magic").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        std::fs::write(&log, b"GENBLOB\0junkjunkjunkjunkjunkjunk").unwrap();
        assert!(matches!(
            read::<Item>(&log),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
        std::fs::write(&log, b"GENINSL\0").unwrap();
        assert!(matches!(
            read::<Item>(&log),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
    }

    #[test]
    fn a_complete_entry_that_does_not_decode_is_an_error() {
        let dir = fresh_temp_dir("insert_log_bad_entry").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        append(&log, &item(1)).unwrap();
        let mut bytes = std::fs::read(&log).unwrap();
        // A complete version-2 item entry of two bytes that is not an `Item`.
        bytes.push(KIND_ITEM);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[0xff, 0xff]);
        std::fs::write(&log, &bytes).unwrap();
        let err = read::<Item>(&log).unwrap_err();
        assert!(
            matches!(&err, DurabilityError::RecordBlobUnreadable { cause, .. } if cause.contains("does not decode")),
            "{err:?}"
        );
    }

    /// `DEL-FR-003` (ADR-0051): a version-2 log carries items and
    /// tombstones in order; `read_items` drops the tombstones, `read_entries`
    /// keeps them; a tombstone under a foreign tag is refused by name.
    #[test]
    fn tombstones_interleave_with_items_in_order() {
        let dir = fresh_temp_dir("insert_log_tombstones").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        append(&log, &item(1)).unwrap();
        append_tombstone(&log, Item::SCHEMA_TAG, &1u32).unwrap();
        append(&log, &item(2)).unwrap();
        append_tombstone(&log, Item::SCHEMA_TAG, &7u32).unwrap();
        assert_eq!(
            read_entries::<Item, u32>(&log, Item::SCHEMA_TAG).unwrap(),
            vec![
                LogEntry::Item(item(1)),
                LogEntry::Tombstone(1),
                LogEntry::Item(item(2)),
                LogEntry::Tombstone(7),
            ]
        );
        assert_eq!(read::<Item>(&log).unwrap(), vec![item(1), item(2)]);
        match read_entries::<Other, u32>(&log, Other::SCHEMA_TAG) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.contains("schema tag mismatch"), "{cause}")
            }
            other => panic!("expected a tag mismatch, got {other:?}"),
        }
    }

    /// `DEL-FR-003`: a version-1 log (no kind bytes) still reads, every
    /// entry an item, and is rewritten as version 2 by its next append —
    /// after which a tombstone can follow the old items.
    #[test]
    fn a_version_1_log_reads_as_items_and_upgrades_on_append() {
        let dir = fresh_temp_dir("insert_log_v1_upgrade").unwrap();
        let log = log_path(&dir.join("store.mmap"));
        let mut image = encode_tagged_image(&MAGIC, LOG_VERSION_1, 0, Item::SCHEMA_TAG, &[]);
        for n in [1u32, 2] {
            let payload = crate::codec::encode(&item(n)).unwrap();
            image.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            image.extend_from_slice(&payload);
        }
        std::fs::write(&log, &image).unwrap();
        assert_eq!(on_disk_version(&log).unwrap(), Some(LOG_VERSION_1));
        assert_eq!(read::<Item>(&log).unwrap(), vec![item(1), item(2)]);
        append_tombstone(&log, Item::SCHEMA_TAG, &1u32).unwrap();
        assert_eq!(on_disk_version(&log).unwrap(), Some(LOG_VERSION));
        assert_eq!(
            read_entries::<Item, u32>(&log, Item::SCHEMA_TAG).unwrap(),
            vec![
                LogEntry::Item(item(1)),
                LogEntry::Item(item(2)),
                LogEntry::Tombstone(1),
            ]
        );
    }
}
