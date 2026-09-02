//! The batch journal behind crash-atomic transactions (`SERVER-001`
//! FR-025, ADR-0025, `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`
//! Part B): a redo log of *intended* writes that a domain adapter
//! (`DogConnectionStore::with_journal` and its `Order`/`Employee`
//! siblings) appends and `fsync`s **before** applying a batch, replays on
//! the next open, and checkpoints by size.
//!
//! # Why redo suffices, and why it lives here
//!
//! Every operation this protocol can batch is an idempotent overwrite of
//! a fixed-width slot keyed by a record id that is never deleted at
//! runtime, for a field whose type the schema fixes (the "no runtime
//! deletion, fixed schema" invariant `ADR-0013` already relied on).
//! Re-applying an applied operation changes nothing; applying one that
//! never landed produces the state it would have. So a log of intended
//! writes, durable before the first slot write and replayed in order,
//! restores a batch's all-or-nothing outcome from *any* intermediate
//! state — no prior values recorded, no slot file changed. The journal
//! is the adapter's, not the store's, because the adapter is the only
//! layer that both sees a batch (`ConnectionStore::apply_transaction`)
//! and knows how to apply one operation to its store; a store knows
//! neither `TransactionOp` nor `FieldRef`.
//!
//! # Format and discipline
//!
//! `TXNJRNL\0`, a `u32` LE format version, then entries of
//! `[u32 LE len][crate::codec(Vec<TransactionOp>)]` — `STORAGE-018`'s
//! encoding, so a journal written by one build replays under the next.
//! `BatchJournal::append` writes an entry and `sync_data`s; a crash
//! before that returns leaves a *torn tail* — an entry whose length
//! prefix or payload is incomplete — which `BatchJournal::open` drops
//! (it was never acknowledged) and truncates away. A complete entry that
//! does not decode is corruption, not a torn tail, and is a
//! [`JournalError::Format`]. The journal is never truncated except by a
//! checkpoint that first makes the store's own files durable
//! ([`CheckpointFlush`]) — `src/durability/hybrid.rs`'s discipline with
//! the batch as the unit and the `.mmap` files as the snapshot — so it
//! always holds exactly the batches applied since the last moment every
//! slot write was known to be on disk.

use super::protocol::{ErrorCode, TransactionOp};
use crate::durability::DurabilityError;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The journal file's magic.
pub const JOURNAL_MAGIC: &[u8; 8] = b"TXNJRNL\0";
/// The journal format this build writes and reads.
pub const JOURNAL_FORMAT_VERSION: u32 = 1;
/// After an append leaves the journal larger than this, the adapter
/// checkpoints: flushes the store, then truncates the journal
/// (`JRN-FR-004`). A constant chosen without measurement; the journaled
/// bench row in `benches/server.rs` is where it gets revisited.
pub const JOURNAL_CHECKPOINT_BYTES: u64 = 1 << 20;

const HEADER_LEN: u64 = 12;

/// Everything that can go wrong opening, appending to, or replaying a
/// batch journal.
#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    /// Not a batch journal, an unknown format version, an entry that is
    /// complete but does not decode, or a batch too large to frame.
    Format(String),
    Codec(bincode::Error),
    /// A journaled batch could not be re-applied on open — impossible
    /// for a batch the adapter validated before journaling, so this
    /// means the store's files are not the ones the journal belongs to.
    Replay {
        batch: usize,
        index: usize,
        code: ErrorCode,
    },
    /// The store's own flush failed during the checkpoint that precedes
    /// truncation on open.
    Durability(DurabilityError),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalError::Io(e) => write!(f, "batch journal I/O: {e}"),
            JournalError::Format(what) => write!(f, "batch journal format: {what}"),
            JournalError::Codec(e) => write!(f, "batch journal encoding: {e}"),
            JournalError::Replay { batch, index, code } => write!(
                f,
                "batch journal replay: batch {batch}, operation {index} failed with {code:?}"
            ),
            JournalError::Durability(e) => write!(f, "batch journal checkpoint: {e}"),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            JournalError::Io(e) => Some(e),
            JournalError::Codec(e) => Some(e),
            JournalError::Durability(e) => Some(e),
            JournalError::Format(_) | JournalError::Replay { .. } => None,
        }
    }
}

impl From<io::Error> for JournalError {
    fn from(e: io::Error) -> Self {
        JournalError::Io(e)
    }
}

impl From<bincode::Error> for JournalError {
    fn from(e: bincode::Error) -> Self {
        JournalError::Codec(e)
    }
}

impl From<DurabilityError> for JournalError {
    fn from(e: DurabilityError) -> Self {
        JournalError::Durability(e)
    }
}

/// What a checkpoint needs from the store an adapter holds exclusively:
/// force every slot write so far to disk, so the journal entries before
/// it can be dropped (`JRN-FR-004`). Implemented for exactly the three
/// exclusive store types the adapters reach through `with_exclusive` —
/// a server-layer trait, so no storage module changes.
pub trait CheckpointFlush {
    fn checkpoint_flush(&self) -> Result<(), DurabilityError>;
}

impl CheckpointFlush for crate::durability::mmap_store::MmapAgeStore {
    fn checkpoint_flush(&self) -> Result<(), DurabilityError> {
        self.flush()
    }
}

#[cfg(feature = "research")]
impl CheckpointFlush for crate::generic::order_customer::OrderProductionStack {
    fn checkpoint_flush(&self) -> Result<(), DurabilityError> {
        crate::generic::store::Flush::flush(self)
    }
}

#[cfg(feature = "research")]
impl CheckpointFlush for crate::generic_spike::employee_impl::EmployeeProductionStack {
    fn checkpoint_flush(&self) -> Result<(), DurabilityError> {
        crate::generic::store::Flush::flush(self)
    }
}

/// One adapter's journal file, held open for appends. See the module
/// docs for the format and the discipline.
pub(crate) struct BatchJournal {
    file: File,
    len: u64,
}

impl BatchJournal {
    /// Open (or create) the journal at `path` and return every complete
    /// entry in it, oldest first, for the caller to replay. A torn tail
    /// is dropped and truncated away; a bad magic or version, or a
    /// complete entry that does not decode, is an error (`JRN-FR-003`).
    pub(crate) fn open(path: &Path) -> Result<(Self, Vec<Vec<TransactionOp>>), JournalError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        if bytes.is_empty() {
            file.write_all(JOURNAL_MAGIC)?;
            file.write_all(&JOURNAL_FORMAT_VERSION.to_le_bytes())?;
            file.sync_all()?;
            return Ok((
                Self {
                    file,
                    len: HEADER_LEN,
                },
                Vec::new(),
            ));
        }
        if bytes.len() < HEADER_LEN as usize || &bytes[..8] != JOURNAL_MAGIC {
            return Err(JournalError::Format("not a batch journal".into()));
        }
        let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if version != JOURNAL_FORMAT_VERSION {
            return Err(JournalError::Format(format!(
                "format version {version}, this build reads {JOURNAL_FORMAT_VERSION}"
            )));
        }

        let mut entries = Vec::new();
        let mut pos = HEADER_LEN as usize;
        loop {
            let Some(len_bytes) = bytes.get(pos..pos + 4) else {
                break; // torn tail (or exactly the end)
            };
            let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
                as usize;
            let Some(payload) = bytes.get(pos + 4..pos + 4 + len) else {
                break; // torn tail: the length prefix landed, the payload did not
            };
            let batch: Vec<TransactionOp> = crate::codec::decode(payload).map_err(|e| {
                JournalError::Format(format!("entry at byte {pos} does not decode: {e}"))
            })?;
            entries.push(batch);
            pos += 4 + len;
        }
        let complete = pos as u64;
        if complete != bytes.len() as u64 {
            file.set_len(complete)?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::Start(complete))?;
        Ok((
            Self {
                file,
                len: complete,
            },
            entries,
        ))
    }

    /// Append one batch and force it to disk (`JRN-FR-002`): when this
    /// returns `Ok`, the batch is durable and the caller may apply it.
    pub(crate) fn append(&mut self, batch: &[TransactionOp]) -> Result<(), JournalError> {
        let payload = crate::codec::encode(batch)?;
        let len = u32::try_from(payload.len())
            .map_err(|_| JournalError::Format("batch too large to journal".into()))?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.file.sync_data()?;
        self.len += 4 + payload.len() as u64;
        Ok(())
    }

    /// Whether the journal has grown past [`JOURNAL_CHECKPOINT_BYTES`].
    pub(crate) fn needs_checkpoint(&self) -> bool {
        self.len > JOURNAL_CHECKPOINT_BYTES
    }

    /// Drop every entry — only after the store's own files are known
    /// durable (see [`CheckpointFlush`]).
    pub(crate) fn truncate(&mut self) -> Result<(), JournalError> {
        self.file.set_len(HEADER_LEN)?;
        self.file.seek(SeekFrom::Start(HEADER_LEN))?;
        self.file.sync_all()?;
        self.len = HEADER_LEN;
        Ok(())
    }

    /// The journal's size in bytes, header included.
    #[cfg(test)]
    pub(crate) fn len_bytes(&self) -> u64 {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::protocol::ScanValue;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn op(id: u128, age: u32) -> TransactionOp {
        TransactionOp {
            id: Uuid::from_u128(id),
            field: 1,
            value: ScanValue::U32(age),
        }
    }

    #[test]
    fn a_fresh_journal_is_a_header_and_replays_nothing() {
        let dir = fresh_temp_dir("journal_fresh").unwrap();
        let path = dir.join("txn.journal");
        let (journal, entries) = BatchJournal::open(&path).unwrap();
        assert!(entries.is_empty());
        assert_eq!(journal.len_bytes(), HEADER_LEN);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], JOURNAL_MAGIC);
        assert_eq!(&bytes[8..12], &JOURNAL_FORMAT_VERSION.to_le_bytes());
        assert_eq!(bytes.len(), HEADER_LEN as usize);
    }

    #[test]
    fn appended_batches_replay_in_order_and_a_torn_tail_is_dropped() {
        let dir = fresh_temp_dir("journal_replay").unwrap();
        let path = dir.join("txn.journal");
        {
            let (mut journal, _) = BatchJournal::open(&path).unwrap();
            journal.append(&[op(1, 30), op(2, 40)]).unwrap();
            journal.append(&[op(3, 50)]).unwrap();
        }
        // A crash mid-append: a length prefix promising more than landed.
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(&[0x40, 0x00, 0x00, 0x00, 0xaa, 0xbb])
                .unwrap();
        }
        let torn_len = std::fs::metadata(&path).unwrap().len();
        let (journal, entries) = BatchJournal::open(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].len(), 2);
        assert_eq!(entries[0][1].id, Uuid::from_u128(2));
        assert_eq!(entries[1][0].value, ScanValue::U32(50));
        // The torn tail was truncated away.
        assert!(journal.len_bytes() < torn_len);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), journal.len_bytes());
    }

    #[test]
    fn a_foreign_file_and_an_unknown_version_are_format_errors() {
        let dir = fresh_temp_dir("journal_format").unwrap();
        let foreign = dir.join("foreign.journal");
        std::fs::write(&foreign, b"DOGBLOB\0\x02\x00\x00\x00").unwrap();
        assert!(matches!(
            BatchJournal::open(&foreign).map(|_| ()),
            Err(JournalError::Format(_))
        ));
        let future = dir.join("future.journal");
        let mut bytes = JOURNAL_MAGIC.to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        std::fs::write(&future, bytes).unwrap();
        assert!(matches!(
            BatchJournal::open(&future).map(|_| ()),
            Err(JournalError::Format(_))
        ));
    }

    #[test]
    fn checkpoint_threshold_and_truncate() {
        let dir = fresh_temp_dir("journal_checkpoint").unwrap();
        let path = dir.join("txn.journal");
        let (mut journal, _) = BatchJournal::open(&path).unwrap();
        let big: Vec<TransactionOp> = (0..4096).map(|i| op(i as u128, i as u32)).collect();
        assert!(!journal.needs_checkpoint());
        while !journal.needs_checkpoint() {
            journal.append(&big).unwrap();
        }
        assert!(journal.len_bytes() > JOURNAL_CHECKPOINT_BYTES);
        journal.truncate().unwrap();
        assert!(!journal.needs_checkpoint());
        assert_eq!(journal.len_bytes(), HEADER_LEN);
        let (_, entries) = BatchJournal::open(&path).unwrap();
        assert!(entries.is_empty());
    }
}
