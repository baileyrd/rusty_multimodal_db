//! The file mechanics behind every `GMMAPST\0` slot file, extracted from
//! `GenericMmapStore` so a second owner — the per-field `MmapScanned`
//! layer (`docs/decisions/ADR-0020-multi-field-mmap-durability-proposal.md`,
//! `STORAGE-017`) — can write and read exactly the same format without
//! duplicating it.
//!
//! A slot file is a memory-mapped column of fixed-width slots behind a
//! fixed header:
//!
//! ```text
//! [MAGIC: 8 bytes][SCHEMA_VERSION: u32 LE]   <- HEADER_LEN bytes
//! [id: Id::BYTE_WIDTH][value: V::BYTE_WIDTH][COMMITTED: 1]  <- slot 0
//! [id][value][COMMITTED]                                    <- slot 1
//! ...
//! ```
//!
//! Every design decision behind that layout — why each slot carries its
//! own id, why the trailing commit byte exists, why new slots are
//! appended through an `O_APPEND` handle rather than written at a
//! locally-computed position, why a torn slot reads as absent rather than
//! corrupt — is recorded in `GenericMmapStore`'s module docs, which
//! remain the format's reference. This module only moves the code; it
//! changes no byte of what is produced or accepted (`MFMD-FR-006`).
//!
//! What stays *outside* this type, deliberately: the reconciliation
//! policy (which records to reuse, append, or ignore) and the in-memory
//! `id -> position` index. Both owners keep those themselves, because the
//! policy is theirs (`MFMD-FR-004`) and the index is what their query
//! impls read. `SlotFile` answers only "what is committed in this file,"
//! "append these," "read/write the value at this position," and "flush."

use super::mmap_field::MmapFieldValue;
use crate::durability::DurabilityError;
use memmap2::MmapMut;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::hash::Hash;
use std::io::{Seek, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// Identifies a file as a slot file at all — 8 arbitrary-but-fixed bytes,
/// ASCII-readable purely for the convenience of anyone inspecting a file
/// by hand (`hexdump`/`xxd`), not a meaningful abbreviation the code
/// relies on. One global constant, not per record type — the header
/// identifies "a slot-file-shaped file," not which domain or which field
/// it belongs to; nothing in this format encodes that (the companion
/// record blob's schema tag does, for the stack as a whole).
pub(crate) const MAGIC: [u8; 8] = *b"GMMAPST\0";

/// Bumped whenever the on-disk *slot* layout changes in a way that would
/// make an old file misread under a new build — last bumped for the
/// trailing per-slot `COMMITTED` marker byte.
pub(crate) const SCHEMA_VERSION: u32 = 2;

/// `MAGIC` followed by `SCHEMA_VERSION` as a little-endian `u32`.
pub(crate) const HEADER_LEN: usize = MAGIC.len() + 4;

/// The value a slot's trailing marker byte holds once the slot's id and
/// value are both fully in place. Any other byte value — including `0`,
/// what a freshly-extended file's zero-filled bytes already are before
/// anything writes to them — means "not committed," so a slot that was
/// never reached at all and a slot whose marker write was interrupted by
/// a crash both read the same safe way: absent, not corrupted.
pub(crate) const COMMITTED: u8 = 1;

/// One memory-mapped `GMMAPST\0` slot file: a column of
/// `(Id, V, COMMITTED)` slots behind the fixed header. Owns the mapping
/// and the path; knows nothing about records.
pub(crate) struct SlotFile<Id, V> {
    mmap: MmapMut,
    path: PathBuf,
    _marker: PhantomData<(Id, V)>,
}

impl<Id, V> SlotFile<Id, V>
where
    Id: MmapFieldValue + Eq + Hash,
    V: MmapFieldValue,
{
    /// Bytes per persisted slot: the id prefix, the value, and the
    /// trailing `COMMITTED` marker byte.
    pub(crate) fn slot_width() -> usize {
        Id::BYTE_WIDTH + V::BYTE_WIDTH + 1
    }

    /// Byte offset of slot `position`'s first byte — every slot sits after
    /// the fixed `HEADER_LEN`-byte header, not at file offset 0 directly.
    pub(crate) fn slot_offset(position: usize) -> usize {
        HEADER_LEN + position * Self::slot_width()
    }

    /// The value stored in slot `position`. The caller guarantees
    /// `position` is a slot this file holds (it came from
    /// [`Self::committed_pairs`] or [`Self::append_committed_slots`]).
    pub(crate) fn read_value(&self, position: usize) -> V {
        let start = Self::slot_offset(position) + Id::BYTE_WIDTH;
        V::read_le(&self.mmap[start..start + V::BYTE_WIDTH])
    }

    /// Overwrite only slot `position`'s value bytes — never its id or its
    /// marker — with one bounded in-place copy: no allocation, no
    /// syscall (`MFMD-FR-001`). The crash-safety argument for why this
    /// single fixed-width copy is not given a second commit mechanism is
    /// in `GenericMmapStore`'s docs (the in-place update path was
    /// checked separately from slot creation and reproduced empirically
    /// by `src/bin/crash_safety_harness.rs`).
    pub(crate) fn write_value(&mut self, position: usize, value: V) {
        let start = Self::slot_offset(position) + Id::BYTE_WIDTH;
        value.write_le(&mut self.mmap[start..start + V::BYTE_WIDTH]);
    }

    /// Every byte after the header — the slot column itself, for a bulk
    /// `chunks_exact(slot_width())` scan. Only meaningful when the caller
    /// has established every slot is live (see [`Self::is_gapless`]);
    /// otherwise a stale or uncommitted slot's value would leak into the
    /// result.
    pub(crate) fn slot_bytes(&self) -> &[u8] {
        &self.mmap[HEADER_LEN..]
    }

    /// Number of whole slots the file currently has room for, header
    /// excluded — committed or not. A trailing partial slot is not
    /// counted; see [`Self::trailing_partial_bytes`].
    pub(crate) fn slot_count(&self) -> usize {
        (self.mmap.len() - HEADER_LEN) / Self::slot_width()
    }

    /// Bytes after the header that don't make up a whole slot: `0` for
    /// every file this type wrote and left alone. Non-zero means either a
    /// file written for a *different* `(Id, V)` width — another record
    /// shape's column, in a directory this stack doesn't own — or a file
    /// truncated mid-slot. `GenericMmapStore` ignores the remainder (the
    /// permissive-truncation convention this crate's WAL reader also
    /// follows); `MmapScanned` refuses the file (`MFMD-FR-009`'s slot-width
    /// check). Both policies are the owners' to choose, which is why this
    /// only reports.
    pub(crate) fn trailing_partial_bytes(&self) -> usize {
        (self.mmap.len() - HEADER_LEN) % Self::slot_width()
    }

    /// True once `live_count` slots account for every slot in the file —
    /// i.e. no persisted slot is stale or uncommitted. Since positions are
    /// unique and never reused, a live count equal to the file's whole-slot
    /// count is sufficient to prove every slot `0..slot_count()` is live
    /// (pigeonhole): an uncommitted or stale slot is never in the owner's
    /// position index, so if one existed the live count would be short by
    /// exactly that slot.
    pub(crate) fn is_gapless(&self, live_count: usize) -> bool {
        live_count * Self::slot_width() == self.mmap.len() - HEADER_LEN
    }

    /// The path this file was created at or opened from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Force the mapping to physical disk (`msync`).
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the `msync` fails.
    pub(crate) fn flush(&self) -> Result<(), DurabilityError> {
        self.mmap.flush()?;
        Ok(())
    }

    /// Write a full slot at `position` directly into `mmap`: id, then
    /// value, then the trailing `COMMITTED` marker — strictly in that
    /// order, the marker only once both halves of the slot's identity are
    /// fully in place. Used by [`Self::create`]'s per-slot loop, before
    /// `Self` exists yet; every later new slot goes through
    /// [`Self::append_committed_slot`] instead.
    fn write_slot_into(mmap: &mut MmapMut, position: usize, id: Id, value: V) {
        let id_width = Id::BYTE_WIDTH;
        let value_width = V::BYTE_WIDTH;
        let start = Self::slot_offset(position);
        id.write_le(&mut mmap[start..start + id_width]);
        value.write_le(&mut mmap[start + id_width..start + id_width + value_width]);
        mmap[start + id_width + value_width] = COMMITTED;
    }

    /// Whether slot `position`'s trailing marker byte reads back as
    /// `COMMITTED` — the one thing [`Self::committed_pairs`] trusts before
    /// treating a slot's id/value bytes as real data at all.
    fn is_committed(mmap: &MmapMut, position: usize) -> bool {
        let marker_offset = Self::slot_offset(position) + Id::BYTE_WIDTH + V::BYTE_WIDTH;
        mmap[marker_offset] == COMMITTED
    }

    /// Atomically append one fully-formed, already-committed slot — id,
    /// then value, then the trailing `COMMITTED` marker, precomputed into a
    /// single buffer — to `appender`'s file via one `write_all` call, and
    /// return the position that slot landed at. The position is read back
    /// from this specific write's own resulting file offset rather than
    /// recomputed from the file's length, which is what closes the
    /// "next free slot" race `GenericMmapStore`'s docs describe.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the write or the subsequent
    /// offset query fails.
    fn append_committed_slot(
        appender: &mut File,
        id: Id,
        value: V,
    ) -> Result<usize, DurabilityError> {
        let id_width = Id::BYTE_WIDTH;
        let value_width = V::BYTE_WIDTH;
        let slot_width = Self::slot_width();

        let mut buffer = vec![0u8; slot_width];
        id.write_le(&mut buffer[..id_width]);
        value.write_le(&mut buffer[id_width..id_width + value_width]);
        buffer[id_width + value_width] = COMMITTED;

        appender.write_all(&buffer)?;
        // Safe without any coordination: this file offset lives on the
        // open file description `appender` owns privately — no other
        // process's own append can perturb what this read reports.
        let end_offset = appender.stream_position()?;
        let position = (end_offset as usize - HEADER_LEN) / slot_width - 1;
        Ok(position)
    }

    /// Write the fixed header (`MAGIC` + `SCHEMA_VERSION`) at the very
    /// start of `mmap` — called once, by [`Self::create`] only. `open`
    /// never writes a header; it only ever reads and validates one an
    /// earlier `create` already wrote.
    fn write_header(mmap: &mut MmapMut) {
        mmap[0..MAGIC.len()].copy_from_slice(&MAGIC);
        SCHEMA_VERSION.write_le(&mut mmap[MAGIC.len()..HEADER_LEN]);
    }

    /// Read and validate the header at the start of `mmap`. A file that
    /// fails this check has none of its slot data touched at all.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::InvalidMagic`] if `mmap` is shorter than
    /// `HEADER_LEN` or its first `MAGIC.len()` bytes don't match, or
    /// [`DurabilityError::SchemaVersionMismatch`] if the magic matches but
    /// the recorded version doesn't. The two are kept distinct because
    /// they mean different things: "not one of these files at all" versus
    /// "one of these files, from a build with a different slot layout."
    fn read_header(mmap: &MmapMut) -> Result<(), DurabilityError> {
        if mmap.len() < HEADER_LEN || mmap[0..MAGIC.len()] != MAGIC {
            return Err(DurabilityError::InvalidMagic);
        }
        let found = u32::read_le(&mmap[MAGIC.len()..HEADER_LEN]);
        if found != SCHEMA_VERSION {
            return Err(DurabilityError::SchemaVersionMismatch {
                found,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    /// Build fresh: create a new `HEADER_LEN + slot_width() * slots.len()`
    /// byte file at `path` — the versioned header first, then one
    /// committed slot per `(id, value)` in `slots`' own order, so slot
    /// `i` holds the `i`th pair — memory-map it, and flush it once. Any
    /// existing file at `path` is truncated. `path`'s parent directory is
    /// created if missing.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the parent can't be created, the
    /// file can't be created/sized, the mapping fails, or the flush fails.
    pub(crate) fn create<I>(path: &Path, slots: I) -> Result<Self, DurabilityError>
    where
        I: ExactSizeIterator<Item = (Id, V)>,
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((HEADER_LEN + slots.len() * Self::slot_width()) as u64)?;

        // SAFETY: this process holds exclusive read/write access to the
        // freshly-created file at `path` for the lifetime of the mapping;
        // nothing else concurrently truncates or writes to it out from
        // under the mapping — the single-process-exclusive-access
        // assumption every mmap store in this crate documents.
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        Self::write_header(&mut mmap);
        for (position, (id, value)) in slots.enumerate() {
            Self::write_slot_into(&mut mmap, position, id, value);
        }
        mmap.flush()?;

        Ok(Self {
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }

    /// Map the existing file at `path` read/write and validate its header.
    /// Nothing past the header is read; the caller decides what to do
    /// with the slots via [`Self::committed_pairs`],
    /// [`Self::trailing_partial_bytes`], and
    /// [`Self::append_committed_slots`].
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist or can't be
    /// mapped; [`DurabilityError::InvalidMagic`] or
    /// [`DurabilityError::SchemaVersionMismatch`] if the header doesn't
    /// check out (see [`Self::read_header`]).
    pub(crate) fn open(path: &Path) -> Result<Self, DurabilityError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // SAFETY: see `create` — same single-process exclusive-access
        // assumption.
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Self::read_header(&mmap)?;
        Ok(Self {
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }

    /// Every *committed* `(id, value)` pair in the file, keyed by id, each
    /// paired with the slot position it was found at — the input to the
    /// owner's reconciliation pass. A slot whose marker byte isn't
    /// `COMMITTED` (never reached, or a crash landed before its marker
    /// write) is skipped entirely, so it falls through reconciliation
    /// exactly as if it had never been persisted, rather than handing a
    /// torn id/value pair to a caller. A trailing partial slot is likewise
    /// never read (it isn't a slot at all — [`Self::slot_count`]).
    pub(crate) fn committed_pairs(&self) -> HashMap<Id, (usize, V)> {
        let id_width = Id::BYTE_WIDTH;
        let value_width = V::BYTE_WIDTH;
        let slot_count = self.slot_count();
        let mut persisted = HashMap::with_capacity(slot_count);
        for position in 0..slot_count {
            if !Self::is_committed(&self.mmap, position) {
                continue;
            }
            let start = Self::slot_offset(position);
            let id = Id::read_le(&self.mmap[start..start + id_width]);
            let value = V::read_le(&self.mmap[start + id_width..start + id_width + value_width]);
            persisted.insert(id, (position, value));
        }
        persisted
    }

    /// Append one committed slot per `(id, value)` in `slots`, in that
    /// order, through an `O_APPEND` handle — one `write_all` per slot —
    /// and return the position each landed at, in the same order. The
    /// mapping is then re-established at the file's new length so the
    /// appended slots are readable and writable through `self`.
    ///
    /// The previous mapping stays alive while the appends happen. That is
    /// sound: an append only ever writes bytes *past* the old mapping's
    /// extent, never a byte the old mapping covers, and no borrow of the
    /// old mapping is outstanding across this call (`&mut self`). The
    /// re-map replaces it; `memmap2` unmaps the old one on drop.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the append handle can't be
    /// opened, any append fails, or the re-map fails. On an append
    /// failure part-way, the slots already appended are on disk and
    /// committed but *not* reported — the next `open` finds them by id
    /// through [`Self::committed_pairs`], which is the same recovery a
    /// crash mid-append gets.
    pub(crate) fn append_committed_slots<I>(
        &mut self,
        slots: I,
    ) -> Result<Vec<usize>, DurabilityError>
    where
        I: IntoIterator<Item = (Id, V)>,
    {
        let mut appender = OpenOptions::new().append(true).open(&self.path)?;
        let mut positions = Vec::new();
        for (id, value) in slots {
            positions.push(Self::append_committed_slot(&mut appender, id, value)?);
        }
        // Re-map at the file's current length — reflecting the appends
        // just made through a *different* handle than the mapping's, but
        // the same underlying file, so its on-disk length is already
        // correct by the time this mapping is established.
        let file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        // SAFETY: see `create`.
        self.mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(positions)
    }
}
