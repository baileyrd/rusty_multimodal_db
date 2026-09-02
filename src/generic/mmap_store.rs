//! The generic, durable storage core — the generic equivalent of
//! `src/durability/mmap_store.rs`'s `MmapAgeStore`, and the piece that
//! makes [`super::production::GenericProductionStore`] real durability
//! rather than a purely in-memory composed stack wrapped in a `RwLock`.
//!
//! # Hand-fused, like `MmapAgeStore`, not built from `BaseStore`/`Indexed`/`Scanned`
//!
//! `MmapAgeStore` doesn't wrap `CanonicalCachedStore` — it rebuilds the
//! same canonical-map/breed-index/position-index architecture directly,
//! with the mutable field backed by `MmapMut` instead of a plain `Vec`
//! (see `src/production.rs`'s own module docs for the full account of why:
//! `CanonicalCachedStore`'s private fields aren't reusable across
//! durability variants without either duplicating them or breaking
//! encapsulation). [`GenericMmapStore`] follows the identical precedent,
//! generically: it does not wrap [`super::store::BaseStore`]/
//! [`super::store::Indexed`]/[`super::store::Scanned`] — it rebuilds their
//! combined shape (one `IndexedField` + one `ScannableField`) directly,
//! with the scannable field's cache backed by `MmapMut`. Composable
//! capability layers (`Symmetric`, `Reversed`) can still be stacked *on
//! top* of a `GenericMmapStore` exactly as they stack on `BaseStore` —
//! their forwarding impls are generic over the inner store type, so they
//! don't care whether what's underneath is in-memory or mmap-backed. This
//! is what lets [`super::order_customer::OrderProductionStack`] reuse
//! `Reversed` completely unchanged.
//!
//! # Scoped to exactly one `IndexedField` and one `ScannableField`, by design
//!
//! `MmapAgeStore` only ever durably tracks one mutable field (`age`) —
//! `docs/decisions/ADR-0009-generic-schema-design-proposal.md` §4.2 is
//! explicit that generalizing mmap durability to more than one mutable
//! field is a real redesign (a string-heap/fixed-layout problem), not an
//! incremental extension, and out of scope for this round. `GenericMmapStore`
//! keeps that same one-durable-field scope, generically: it is parameterized
//! over exactly one `IndexedField` marker (mirroring `breed_index`,
//! immutable after construction) and exactly one `ScannableField` marker
//! (mirroring `age`, the one mutable, mmap-backed field). A domain that
//! wants more than one mutable durable field needs the redesign ADR-0009
//! already flagged as unscoped follow-up work — not attempted here.
//!
//! # A finding from wiring this up: write-through consistency needed a new trait method
//!
//! Building this surfaced something the in-memory spikes never exercised:
//! [`GetById::get`] has to return a record whose scannable field reflects
//! the *latest* `UpdateField::update` write, the same write-through
//! consistency every hand-written backend in this crate already has
//! (`CanonicalCachedStore::update_age` mutates both its canonical record
//! and its cache). The original design's `ScannableField` trait had no way
//! to write a new value back into a record it didn't already own the
//! layout of — `set_scannable_value` (`traits.rs`) was added specifically
//! to close this gap for the durable path. It is **not** yet threaded
//! through the in-memory `Scanned`/`BaseStore` composition (`store.rs`) —
//! see `traits.rs`'s doc comment on `set_scannable_value` for why that's a
//! separate, larger piece of work (the same O(N²) marker-pair problem
//! `forward_scannable_pairs!` already solves for `ScanField`/`UpdateField`
//! would need solving again for a record-mutating capability), not
//! attempted in this round.
//!
//! # Persisted slots are keyed by record identity, not array position
//!
//! **Fixed in a follow-up round** — the schema-evolution diagnosis that
//! motivated it found the real fragility in the original design: each
//! persisted slot held only the scannable value's raw bytes, addressed by
//! `position * BYTE_WIDTH`, where `position` was whatever index the
//! record happened to occupy in the caller-supplied `records: Vec<R>` *at
//! that specific `create`/`open` call* — nothing in the file itself
//! recorded which record a value belonged to. If the caller ever
//! supplied `records` in a different order between `create` and a later
//! `open` (a real possibility this crate's own convention invites,
//! externally-supplied `records` being rebuilt fresh every call — see
//! `crate::durability`'s own module docs), position N's persisted value
//! silently got attributed to whatever record now sat at position N: no
//! error, no panic, just wrong data under a real id.
//!
//! Each slot now holds `(id, value)`, both fixed-width
//! ([`MmapFieldValue`] extended to cover [`uuid::Uuid`] for exactly this
//! purpose): `[R::Id::BYTE_WIDTH bytes][R::ScanValue::BYTE_WIDTH bytes]`,
//! contiguous. [`GenericMmapStore::open`] reads every persisted `(id,
//! value)` pair up front and reconciles it against the caller-supplied
//! `records` **by id**, not position — reordering `records` between calls
//! now has no effect on which value a given id reads, which is exactly
//! the bug this closes. A `HashMap<Uuid, Value>`-shaped index built from
//! the file, same shape as every canonical-store index this project has
//! built from the start, not a novel idea — just finally applied to this
//! one path.
//!
//! ## Explicit behavior for the two mismatch cases
//!
//! The invariant that must hold, unconditionally: **a persisted value is
//! never attributed to an id other than the one it was written under.**
//! Given that, two mismatches between "what's in the file" and "what
//! `records` currently says exists" are possible, and each is handled
//! deliberately, not by accident:
//!
//! - **A persisted id has no matching record in the caller's current
//!   `records`** (stale — removed since the last write that included
//!   it). Its slot is simply never referenced: not added to
//!   [`GetById`]/[`FilterEq`]/[`ScanField`]'s visible state, which are
//!   all built from `records` the same way they always were. The bytes
//!   physically remain in the file (this round doesn't add compaction —
//!   a real, stated cost, not a silent one: the file can only grow, never
//!   shrink, across repeated `open` calls that omit previously-known
//!   ids), but are otherwise inert.
//! - **A record in the caller's current `records` has no persisted
//!   entry** (new — added since the last write). It's treated exactly
//!   the way [`GenericMmapStore::create`] treats every record: seeded
//!   from that record's own [`ScannableField::scannable_value`], and a
//!   new slot is appended to the file for it (growing the file, unmapping
//!   and remapping as needed) so it's durable from this point forward.
//!
//! Both are exercised directly in this module's tests, alongside a
//! reopen-with-reordered-records regression test confirming the original
//! silent-misattribution bug is gone.
//!
//! ## `scan` had to change too, for the same reason
//!
//! [`ScanField::scan`]'s old bulk `chunks_exact` read walked every byte
//! in the file — safe under the old design, where the file's size and
//! `records.len()` were always identical by construction. Once a stale
//! record's slot can outlive it (see above), that's no longer true: a
//! blind full-file scan would leak a removed record's value into
//! [`ScanField::scan`]'s result. `scan` now iterates only the positions
//! [`GenericMmapStore::position_index`] currently maps to a live id,
//! keeping the original bulk `chunks_exact` fast path when every slot in
//! the file is still live (the common case — no record has ever been
//! dropped between an `open` and the `records` now supplied), falling
//! back to per-position reads, sorted for locality, only when it isn't. A
//! real, measured cost of the fix in the general case; see this round's
//! own report for the numbers.
//!
//! # A versioned header, so a stale file is at least detectable
//!
//! **Fixed in a follow-up round.** The schema-evolution diagnosis's
//! headline finding was that nothing in *either* durability path's
//! persisted format carries a version marker — nothing would tell a
//! reader a file predates the code opening it. For `GenericMmapStore`
//! specifically, confirmed directly (not assumed) before designing
//! anything: the on-disk format described above genuinely has no header
//! of any kind — [`GenericMmapStore::create`] writes `records.len() *
//! slot_width()` bytes starting at file offset 0, and
//! [`GenericMmapStore::open`] reads them back from offset 0 the same way,
//! with nothing at any fixed offset a reader could check.
//!
//! Every file `GenericMmapStore::create` writes now begins with a fixed
//! [`HEADER_LEN`]-byte header — an 8-byte [`MAGIC`] constant, then
//! [`SCHEMA_VERSION`] as a little-endian `u32` (via the same
//! [`MmapFieldValue`] round-trip every id/value already uses) — followed
//! by the id+value slot layout, otherwise unchanged. [`GenericMmapStore::open`]
//! reads and checks this header *before* touching any slot data:
//!
//! - Magic bytes don't match (or the file is too short to even hold a
//!   header) → [`DurabilityError::InvalidMagic`] — not a
//!   `GenericMmapStore` file at all, full stop.
//! - Magic matches but the version doesn't →
//!   [`DurabilityError::SchemaVersionMismatch`], naming both the found
//!   and expected version. Nothing past the header is read in this case —
//!   no attempt to reconcile records against a slot layout this build
//!   doesn't actually know how to interpret.
//! - Both match → proceeds exactly as before this round.
//!
//! **Detection only, deliberately.** No migration path is built for a
//! version mismatch — the point of this round is giving a real migration
//! story something reliable to check *for*, not building that story
//! speculatively with no real old-to-new migration to test against yet.
//!
//! **One inherent, one-time limitation, stated plainly rather than
//! glossed over**: a file written by the *previous* round (id+value
//! slots, no header — the fix immediately before this one) has no magic
//! number at all, so reopening one now correctly fails, but as
//! `InvalidMagic`, not `SchemaVersionMismatch` — there is no way to
//! retroactively distinguish "an older version of this exact store" from
//! "an unrelated file" for a format that never had a version concept to
//! begin with. Every file written *from this round forward* gets the real
//! distinction; this one prior format simply predates the marker that
//! would have let it be told apart.
//!
//! # A trailing commit marker, so a crash can't produce a torn slot
//!
//! **Fixed in a follow-up round.** The crash-safety diagnosis
//! (subprocess `SIGKILL`, not a graceful drop — see that round's own
//! harness, `src/bin/crash_safety_harness.rs`) reproduced a real torn
//! write 8/8 times: a crash between a new slot's id write and its value
//! write leaves a slot with a *valid-looking id paired with a stale or
//! garbage value* — data that passes the schema-version/magic-number
//! check while being silently wrong. That diagnosis specifically
//! exercised slot **creation** (`create`, and `open`'s new-slot append
//! path, both of which write a slot's id then its value as two
//! independent, unsynchronized writes); this round's own diagnosis pass
//! (see `open`'s doc comment on `is_committed`) checked the **update**
//! path too, directly, rather than assuming the same fix automatically
//! covers it.
//!
//! Every slot now carries one extra trailing byte — [`COMMITTED`] — after
//! its id and value, written strictly *last*, once both of those are
//! fully in place: [`Self::write_slot_into`] is the single function that
//! performs a slot's id write, then its value write, then its marker
//! write, in that exact order, and both call sites that ever established a
//! slot's identity at the time (`create`'s per-record loop, and a
//! `write_slot` wrapper `open`'s new-slot path used) went through it —
//! one function, not two independently-maintained copies of the same
//! three-step order, so they couldn't drift apart the way the original
//! id/value split implicitly invited. (`write_slot` was later replaced by
//! [`Self::append_committed_slot`] — see this module's own "next free
//! slot" race section — which builds the identical three-field layout
//! but commits it through a different mechanism; `write_slot_into` itself
//! is unchanged and still the one place that byte layout is defined.)
//! [`Self::is_committed`] reads that byte back; [`Self::open`]'s
//! reconciliation pass skips any slot whose marker isn't set, exactly as
//! if that id had never been persisted at all — the record it belongs to
//! (if still current) falls into the ordinary "no persisted entry yet"
//! path and gets a fresh, properly-committed slot appended, while the
//! torn slot's bytes stay in the file, inert, the same documented,
//! already-accepted cost every other stale/orphaned slot has (no
//! compaction).
//!
//! **Why a single trailing byte, not "id written last" instead**: the id
//! field is 16 bytes ([`uuid::Uuid`]); relying on *that* write itself
//! being atomic would trade one unverified assumption for another — this
//! project has no guarantee a 16-byte `copy_from_slice` is atomic with
//! respect to a process-level crash across every platform/filesystem this
//! crate might run on. A single byte is the smallest unit this code can
//! write at all, which is about as close to a real atomicity guarantee as
//! this gets without much heavier machinery (a write-ahead log, or
//! copy-on-write) — stated explicitly, per this project's own convention
//! of naming its durability assumptions rather than leaving them
//! implicit, not proven beyond what "smallest possible write" implies.
//! [`Self::is_committed`]'s own doc comment covers the update path's
//! separate, narrower assumption in the same spirit.
//!
//! **Another one-time limitation, same shape as the header round's own**:
//! a file written by the *previous* round (id+value slots, header, no
//! commit marker) has a different [`SCHEMA_VERSION`] recorded in its
//! header, so reopening one now correctly fails as
//! [`DurabilityError::SchemaVersionMismatch`] rather than being misread
//! under the new, wider slot layout — the version bump this round makes
//! is exactly what the header round built the mechanism to catch.
//!
//! **Detection-and-repair, not detection-only this time**: unlike the
//! header round (which only detects a version mismatch, deliberately not
//! migrating), a torn slot found mid-reconciliation is actively repaired
//! in place, by the same "append a fresh slot for a record with no
//! persisted entry" path `open` already had — no separate migration
//! machinery needed, since a torn slot and a genuinely-new record are, by
//! construction, indistinguishable to `open`'s reconciliation pass once
//! the marker excludes the torn one.
//!
//! # The "next free slot" race — a second process's append can land on the exact same slot
//!
//! **Fixed in a follow-up round.** The multi-process diagnosis round (a
//! real two-*live*-process harness, `src/bin/multiprocess_harness.rs` —
//! contrast the crash-safety harness above, which kills one process
//! mid-work; this one lets both run to completion and race) reproduced
//! this directly, 24/24 trials: [`Self::open`]'s previous design decided
//! a new record's slot position purely from `existing_slot_count`, a
//! value read from *this process's own* memory-mapped view of the file's
//! length, before growing it. Two processes opening concurrently, each
//! with at least one record neither has a persisted slot for yet, both
//! read the same pre-growth length, both compute the identical
//! `existing_slot_count`, and both then write their own new record's
//! `(id, value, marker)` bytes starting at that same byte offset —
//! genuinely interleaved writes into the same slot, not just a logical
//! disagreement. Confirmed both by direct raw-byte inspection of the
//! collided slot (whichever process's write physically landed last wins;
//! the other's id is nowhere in the file, overwritten mid-air) and, more
//! directly, by a losing process's own very next read of its own
//! just-written record observing the *other* process's value.
//!
//! **The fix moves the position decision out of this process's own,
//! necessarily-stale read and into the kernel**, via `O_APPEND`.
//! [`Self::append_committed_slot`] opens a dedicated file handle with
//! the `append` flag set and performs exactly one `write_all` call per
//! missing record, each carrying that slot's fully-formed bytes — id,
//! then value, then the trailing [`COMMITTED`] marker, precomputed into
//! one buffer, so the write that lands the slot's identity *is* the
//! write that commits it, one syscall, not three separate mmap writes
//! racing anything. POSIX specifies that for a regular file opened with
//! `O_APPEND`, the repositioning to end-of-file and the write itself are
//! atomic with respect to other `O_APPEND` writers using *separate* open
//! file descriptions of the same file — which is exactly this shape:
//! two processes, two independently-`open()`ed handles, each blindly
//! `write_all`ing its own slot's bytes. Each write is placed by the
//! kernel past whatever any concurrent appender (this process's own
//! prior append, or another process's) has already written; two
//! processes can no longer choose the same byte offset because neither
//! of them is choosing it at all anymore.
//!
//! A new record's actual position is then read back from *that specific
//! write's own* resulting file offset (`stream_position`, an `lseek(..,
//! SEEK_CUR)` against the same handle that just wrote it) — not
//! recomputed from file length, which would just reintroduce the same
//! race one level up. This is safe to query without any coordination:
//! a file offset lives on the open file description the `append()` call
//! created, private to whichever process (and even which handle within
//! that process) owns it; no other process's append can perturb it.
//!
//! **Verified directly against the real two-process harness, not just
//! cited from the POSIX text** — this round's own report has the exact
//! trial count; the previously-100%-reproducible collision (raw slot
//! bytes belonging to only one of the two racing ids, the other
//! silently gone) no longer occurs at all, across repeated trials, and a
//! third-party reopen after the race sees both records, each at its own,
//! distinct position.
//!
//! **One accepted, explicit platform caveat, the same shape this
//! module's other durability assumptions get**: the atomicity POSIX
//! documents for `O_APPEND` is a *local filesystem* property (ext4,
//! xfs, btrfs, and similar all honor it) — NFS is a well-known,
//! explicitly-documented exception, where two NFS clients' `O_APPEND`
//! writes can still race each other. This crate has no NFS-backed
//! deployment target today; the caveat is named, not silently assumed
//! away, and not engineered around, matching this module's own stated
//! convention of naming its durability assumptions rather than leaving
//! them implicit.
//!
//! **Why `O_APPEND` over a cross-process file lock**: a lock (`fs2`,
//! `fd-lock`, or similar) wrapping the whole claim-and-write step would
//! also close this race, unconditionally, on every platform including
//! NFS, at the cost of a new dependency and serializing every
//! concurrent append against every other one for the lock's whole
//! critical section. `O_APPEND` closes the *identical* race using a
//! mechanism the kernel already provides for exactly this shape of
//! problem, no new dependency, and no serialization broader than what
//! each individual `write_all` call already implies — genuinely
//! concurrent appends from different processes can proceed without
//! blocking each other at all, they just can't land on the same bytes.
//! Given it demonstrably holds up under real, repeated cross-process
//! testing on this crate's actual target platforms, it's the simpler
//! mechanism for a real, verified guarantee here — not a case where this
//! project's general preference for the simple, universally-correct
//! default (documented in `src/production.rs`'s own `RwLock`-over-
//! sharding rationale) points the other way; that preference exists for
//! when the "clever" option's correctness is uncertain or costly to
//! verify, not as a rule to prefer more machinery once the simpler one
//! is confirmed to actually work.
//!
//! **No `SCHEMA_VERSION` bump** — the bytes a fixed slot ends up holding
//! are identical in shape and content to what the previous design wrote
//! (id, then value, then `COMMITTED`, at some slot position); only the
//! *mechanism* by which a new slot's position is chosen and its bytes
//! land changed, not the on-disk format itself. A file written by either
//! design opens identically under the other.
//!
//! **Deliberately unchanged**: `create`'s own initial-write race (two
//! processes calling `create` on the same path at once) is a separate,
//! already-diagnosed question — found to resolve cleanly in 24/24 trials
//! but accidentally, not by any actual guarantee (`create`'s
//! `.truncate(true)` still races another process's live mapping with
//! nothing in the code making it safe by construction). This round's fix
//! is scoped to the append/slot-claiming path `open` uses for records it
//! discovers have no persisted slot yet — the specific mechanism this
//! module's own diagnosis round named as a real, reproducible hazard.
//! `create`'s race is untouched here.
//!
//! # A companion record blob, so the files are portable on their own
//!
//! **Fixed in a follow-up round** (`STORAGE-015`, ADR-0017,
//! `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md`). Everything above
//! persists exactly one field per record — the mmap-backed
//! [`ScannableField`] — so a `.mmap` file alone could never rebuild the
//! records: [`GenericMmapStore::open`] has always needed the caller to
//! hand the full `Vec<R>` back in, which is the same one-durable-field gap
//! `ProductionStore` closed in `STORAGE-014`, and this round closes it the
//! same way. [`GenericMmapStore::create`] now also writes the complete
//! record set, `bincode`-serialized behind a fingerprinted header, to a
//! companion file at `<path>.records` (see `generic::record_blob`, which
//! shares its header layout, hash, and atomic write with the `Dog` blob);
//! [`GenericMmapStore::open`] checks that companion's 20-byte header
//! against a fingerprint of the records it was given and rewrites the
//! blob only when they differ — so a directory written before this round
//! (mmap file only) heals on its first `open`, and the steady-state
//! reopen with the same dataset costs one fingerprint pass and one small
//! read, never a file write. Two new constructors read the companion
//! back: [`GenericMmapStore::read_portable_records`] (the persisted
//! `Vec<R>`, in its original order — relationship layers built above the
//! store need that order to be deterministic) and
//! [`GenericMmapStore::open_portable`] (`open` fed from it, so the pair
//! of files is a complete, copyable store). The `.mmap` file's format,
//! header, slot layout, and reconciliation are untouched; the one visible
//! change to the type is the `Serialize + DeserializeOwned` bound on `R`,
//! which a record must satisfy for the blob to exist at all.

use super::mmap_field::MmapFieldValue;
use super::query::{FilterEq, GetById, ScanField, UpdateField};
use super::record_blob::{self, blob_path, GenericRecordBlob};
use super::store::Flush;
use super::traits::{IndexedField, Record, ScannableField};
use super::NotFound;
use crate::durability::DurabilityError;
use memmap2::MmapMut;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// Identifies a file as one [`GenericMmapStore`] wrote, at all — 8
/// arbitrary-but-fixed bytes, ASCII-readable purely for the convenience of
/// anyone inspecting a file by hand (`hexdump`/`xxd`), not a meaningful
/// abbreviation the code relies on. One global constant, not per-`R` —
/// the header identifies "a `GenericMmapStore`-shaped file," not which
/// domain it belongs to; nothing in this format encodes that today, on
/// either side of this round.
const MAGIC: [u8; 8] = *b"GMMAPST\0";

/// Bumped whenever [`GenericMmapStore`]'s on-disk *slot* layout changes in
/// a way that would make an old file misread under a new build — bumped
/// this round for the trailing per-slot [`COMMITTED`] marker byte (see
/// this module's own doc comment), the same way the record-identity-
/// keying round would have bumped it for the id+value slot layout, had
/// this marker existed yet at that point.
const SCHEMA_VERSION: u32 = 2;

/// [`MAGIC`] followed by [`SCHEMA_VERSION`] as a little-endian `u32` — see
/// this module's doc comment for the full header design.
const HEADER_LEN: usize = MAGIC.len() + 4;

/// The value [`GenericMmapStore::is_committed`] looks for in a slot's
/// trailing marker byte. Any other byte value — including `0`, what a
/// freshly-extended file's zero-filled bytes already are before anything
/// writes to them — means "not committed," so a slot that was never
/// reached at all (file merely grown, e.g. by `open`'s `file.set_len`
/// ahead of writing it) and a slot whose marker write was interrupted by
/// a crash both read the same safe way: absent, not corrupted.
const COMMITTED: u8 = 1;

/// The generic, durable storage core: owns every record, one equality
/// index (`IndexMarker`), and one mmap-backed scannable field
/// (`ScanMarker`). See module docs for why it's hand-fused, not composed,
/// scoped to exactly one field of each kind, and why each persisted slot
/// now carries its own record id rather than being addressed by array
/// position alone.
pub struct GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    records: HashMap<R::Id, R>,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
    /// `id` -> that id's *current* slot position in `mmap` — built by
    /// matching persisted ids against `records`, not by array index. See
    /// module docs for the two mismatch cases this reconciliation has to
    /// decide between.
    position_index: HashMap<R::Id, usize>,
    mmap: MmapMut,
    #[allow(dead_code)] // kept for symmetry with MmapAgeStore's Self; not read again
    path: PathBuf,
    _marker: PhantomData<(IndexMarker, ScanMarker)>,
}

/// The caller-derived, file-independent pieces of [`GenericMmapStore`]'s
/// state — everything built purely from the `records: Vec<R>` argument,
/// with no reference to what (if anything) is already on disk. Factored
/// into its own struct (rather than a tuple) purely for readability at
/// the `create`/`open` call sites, mirroring
/// `src/durability/mmap_store.rs`'s own `Indexes` struct. Deliberately
/// does *not* include `position_index` any more — unlike `records`/
/// `index`, that now depends on what's actually persisted (see module
/// docs), so `create`/`open` each compute it themselves.
struct Indexes<R, IndexMarker>
where
    R: IndexedField<IndexMarker>,
{
    records: HashMap<R::Id, R>,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
}

/// Every `(id, value)` pair read from an existing file during
/// [`GenericMmapStore::open`]'s reconciliation pass, keyed by id, each
/// paired with the slot position it was found at.
type PersistedSlots<R, ScanMarker> =
    HashMap<<R as Record>::Id, (usize, <R as ScannableField<ScanMarker>>::ScanValue)>;

impl<R, IndexMarker, ScanMarker> GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker>
        + ScannableField<ScanMarker>
        + Clone
        + Serialize
        + DeserializeOwned,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Bytes per persisted slot: the id prefix, the scannable value, and
    /// the trailing [`COMMITTED`] marker byte — see module docs for why
    /// neither addition is optional.
    fn slot_width() -> usize {
        R::Id::BYTE_WIDTH + R::ScanValue::BYTE_WIDTH + 1
    }

    /// Byte offset of slot `position`'s first byte — every slot sits after
    /// the fixed [`HEADER_LEN`]-byte header, not at file offset 0 directly.
    fn slot_offset(position: usize) -> usize {
        HEADER_LEN + position * Self::slot_width()
    }

    fn read_value(&self, position: usize) -> R::ScanValue {
        let id_width = R::Id::BYTE_WIDTH;
        let start = Self::slot_offset(position) + id_width;
        R::ScanValue::read_le(&self.mmap[start..start + R::ScanValue::BYTE_WIDTH])
    }

    fn write_value(&mut self, position: usize, value: R::ScanValue) {
        let id_width = R::Id::BYTE_WIDTH;
        let start = Self::slot_offset(position) + id_width;
        value.write_le(&mut self.mmap[start..start + R::ScanValue::BYTE_WIDTH]);
    }

    /// Write a full slot at `position` directly into `mmap`: id, then
    /// value, then the trailing [`COMMITTED`] marker — strictly in that
    /// order, the marker only once both halves of the slot's identity are
    /// fully in place. Used by `create`'s per-record loop, before `Self`
    /// even exists yet. `open`'s new-slot append path used to go through
    /// this too (via a since-removed `write_slot` wrapper); it now goes
    /// through [`Self::append_committed_slot`] instead, which builds the
    /// identical three-field byte layout but commits it via one
    /// `O_APPEND` `write_all` call rather than three separate mmap writes
    /// at a locally-computed position — see module docs' "next free
    /// slot" race section for why. `read_value`/`write_value` above never
    /// need to touch the id or marker half of a slot again once it's
    /// written, since `position_index` already captures the id ->
    /// position mapping in memory.
    fn write_slot_into(mmap: &mut MmapMut, position: usize, id: R::Id, value: R::ScanValue) {
        let id_width = R::Id::BYTE_WIDTH;
        let value_width = R::ScanValue::BYTE_WIDTH;
        let start = Self::slot_offset(position);
        id.write_le(&mut mmap[start..start + id_width]);
        value.write_le(&mut mmap[start + id_width..start + id_width + value_width]);
        mmap[start + id_width + value_width] = COMMITTED;
    }

    /// Whether slot `position`'s trailing marker byte reads back as
    /// [`COMMITTED`] — the one thing [`Self::open`]'s reconciliation pass
    /// trusts before treating a slot's id/value bytes as real data at
    /// all. A slot that fails this check (marker byte anything other than
    /// `COMMITTED`) is skipped entirely during reconciliation, exactly as
    /// if it had never been persisted — see module docs for the full
    /// mechanism and why a single trailing byte, not id-write-ordering,
    /// is what this round's fix relies on.
    ///
    /// **Scoped to slot *creation* only, deliberately** — this crate's own
    /// crash-safety diagnosis (this round's own follow-up pass, not the
    /// original diagnosis round) checked the in-place *update* path
    /// separately, directly, rather than assuming this fix covers it too:
    /// [`UpdateField::update`]/[`Self::write_value`] overwrite only a
    /// slot's already-committed value bytes, never its id or marker, via
    /// one `copy_from_slice` call of `R::ScanValue::BYTE_WIDTH` bytes. A
    /// process-level `SIGKILL` can only land at an instruction boundary,
    /// never inside one CPU store instruction, and that single fixed-
    /// width, compile-time-sized copy is exactly the shape LLVM is
    /// expected to lower to one (or a small, fixed number of) store
    /// instructions rather than a byte-wise loop — so an in-place value
    /// update tearing mid-write was expected to be far harder to trigger
    /// than the original id/value gap. Reproduced empirically: many
    /// rapid, back-to-back, unsynchronized updates alternating between
    /// two maximally-distinguishable byte patterns, killed at
    /// uncontrolled points relative to individual writes, across repeated
    /// trials (`src/bin/crash_safety_harness.rs`'s `trial_torn_update`) —
    /// every single trial read back exactly one of the two known
    /// patterns, never a mix. That result is evidence for, not proof of,
    /// this remaining safe on every platform/compiler this crate might
    /// ever run on — it depends on codegen this project doesn't pin or
    /// verify per-target, a real, narrower, explicitly-accepted residual
    /// assumption, not a second commit-marker mechanism. Closing that gap
    /// for certain would mean paying the same append-and-repoint cost
    /// `create`/`write_slot` pay for a *new* slot on every single
    /// in-place `update` too — turning every mutation into unbounded,
    /// log-structured file growth — which is exactly the "much heavier
    /// machinery" this round's own module doc says a single-byte marker
    /// is meant to avoid building without a real design needing it first.
    fn is_committed(mmap: &MmapMut, position: usize) -> bool {
        let start = Self::slot_offset(position);
        let marker_offset = start + R::Id::BYTE_WIDTH + R::ScanValue::BYTE_WIDTH;
        mmap[marker_offset] == COMMITTED
    }

    /// Atomically append one fully-formed, already-committed slot — id,
    /// then value, then the trailing [`COMMITTED`] marker, precomputed
    /// into a single buffer — to `appender`'s file via one `write_all`
    /// call, and return the position that slot landed at. See this
    /// module's doc comment (the "next free slot" race section) for why
    /// this closes the race the previous, length-derived position
    /// calculation had, and why the position is read back from this
    /// specific write's own resulting file offset rather than recomputed
    /// from the file's length.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the write or the subsequent
    /// offset query fails.
    fn append_committed_slot(
        appender: &mut File,
        id: R::Id,
        value: R::ScanValue,
    ) -> Result<usize, DurabilityError> {
        let id_width = R::Id::BYTE_WIDTH;
        let value_width = R::ScanValue::BYTE_WIDTH;
        let slot_width = Self::slot_width();

        let mut buffer = vec![0u8; slot_width];
        id.write_le(&mut buffer[..id_width]);
        value.write_le(&mut buffer[id_width..id_width + value_width]);
        buffer[id_width + value_width] = COMMITTED;

        appender.write_all(&buffer)?;
        // Safe without any coordination: this file offset lives on the
        // open file description `appender` owns privately (see module
        // docs) — no other process's own append can perturb what this
        // read reports.
        let end_offset = appender.stream_position()?;
        let position = (end_offset as usize - HEADER_LEN) / slot_width - 1;
        Ok(position)
    }

    /// Write the fixed header ([`MAGIC`] + [`SCHEMA_VERSION`]) at the very
    /// start of `mmap` — called once, by [`Self::create`] only. `open`
    /// never writes a header; it only ever reads and validates one an
    /// earlier `create` already wrote.
    fn write_header(mmap: &mut MmapMut) {
        mmap[0..MAGIC.len()].copy_from_slice(&MAGIC);
        SCHEMA_VERSION.write_le(&mut mmap[MAGIC.len()..HEADER_LEN]);
    }

    /// Read and validate the header at the start of `mmap` — see this
    /// module's own doc comment for exactly what each failure means and
    /// why they're kept distinct. Called by [`Self::open`] before any
    /// slot data is read; a file that fails this check has none of its
    /// record data touched at all.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::InvalidMagic`] if `mmap` is shorter than
    /// [`HEADER_LEN`] or its first [`MAGIC`]`.len()` bytes don't match,
    /// or [`DurabilityError::SchemaVersionMismatch`] if the magic matches
    /// but the recorded version doesn't.
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

    /// True once every slot currently in the file maps to a live record in
    /// `position_index` — i.e. no record has ever been dropped between an
    /// `open` and the `records` this store was actually built from. Since
    /// positions are unique and never reused, `position_index.len()`
    /// matching the file's total slot count (the header excluded) is
    /// sufficient to prove every slot 0..total is covered (pigeonhole —
    /// see module docs' `scan` section for why this matters).
    fn is_gapless(&self) -> bool {
        self.position_index.len() * Self::slot_width() == self.mmap.len() - HEADER_LEN
    }

    fn build_indexes(records: &[R]) -> Indexes<R, IndexMarker> {
        let mut index: HashMap<R::IndexValue, Vec<R::Id>> = HashMap::new();
        for record in records {
            index
                .entry(record.indexed_value().clone())
                .or_default()
                .push(record.id());
        }
        let records_map = records.iter().cloned().map(|r| (r.id(), r)).collect();
        Indexes {
            records: records_map,
            index,
        }
    }

    /// Build fresh: create a new `HEADER_LEN + slot_width() * records.len()`-byte
    /// file at `path` — the versioned header first, then one `(id, value)`
    /// slot per record in `records`' own order — and memory-map it.
    /// Mirrors `MmapAgeStore::create`'s overall shape, generically, with
    /// the header and id prefix this module's own docs describe. Also
    /// writes the full record set to the companion blob at
    /// `<path>.records` (see module docs, "A companion record blob") —
    /// encoded before the mmap file is touched, installed after it is
    /// complete, so a failure in either step never leaves a blob that
    /// describes a store which doesn't exist (`STORAGE-015-FR-002`).
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, the file can't be created/sized, the mapping fails, or
    /// the companion blob can't be written; [`DurabilityError::Serde`] if
    /// `records` can't be serialized.
    pub fn create(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encoded_blob = GenericRecordBlob::new(&records).encode()?;
        let slot_width = Self::slot_width();
        let indexes = Self::build_indexes(&records);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((HEADER_LEN + records.len() * slot_width) as u64)?;

        // SAFETY: this process holds exclusive read/write access to the
        // freshly-created file at `path` for the lifetime of the mapping;
        // nothing else concurrently truncates or writes to it out from
        // under the mapping — the same single-process-exclusive-access
        // assumption `MmapAgeStore::create` documents.
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        Self::write_header(&mut mmap);
        let mut position_index = HashMap::with_capacity(records.len());
        for (position, record) in records.iter().enumerate() {
            Self::write_slot_into(&mut mmap, position, record.id(), record.scannable_value());
            position_index.insert(record.id(), position);
        }
        mmap.flush()?;
        encoded_blob.write(&blob_path(path))?;

        Ok(Self {
            records: indexes.records,
            index: indexes.index,
            position_index,
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }

    /// Reopen `path`, reconciling its persisted `(id, value)` slots
    /// against the externally-supplied `records` **by id**, not by array
    /// position — see module docs for why, and for the explicit,
    /// deliberate behavior of the two mismatch cases this reconciliation
    /// can hit (a persisted id no longer in `records`; a record in
    /// `records` with no persisted slot yet). A record in the second case
    /// gets a freshly-appended slot, seeded from its own
    /// [`ScannableField::scannable_value`] — appended via
    /// [`Self::append_committed_slot`], not written at a locally-computed
    /// position; see module docs' "next free slot" race section for why.
    ///
    /// Also keeps the companion blob at `<path>.records` current with
    /// `records`: its header fingerprint is compared against `records`
    /// first, and only if they differ (a changed dataset, a missing or
    /// foreign file, a directory written before the blob existed) is the
    /// blob re-encoded and — after the mmap file has opened and
    /// reconciled successfully — rewritten in place. The common reopen
    /// with the same dataset never writes (`STORAGE-015-FR-003`).
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist, can't be
    /// mapped, a new slot can't be appended, or a stale companion blob
    /// can't be rewritten; [`DurabilityError::InvalidMagic`]
    /// or [`DurabilityError::SchemaVersionMismatch`] if the file's header
    /// doesn't check out — see this module's own doc comment. Either
    /// header failure returns before any slot data is read.
    /// [`DurabilityError::Serde`] if a stale blob's records can't be
    /// serialized.
    pub fn open(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError> {
        let companion = blob_path(path);
        let stale_blob = {
            let blob = GenericRecordBlob::new(&records);
            if blob.is_current_at(&companion) {
                None
            } else {
                Some(blob.encode()?)
            }
        };
        let indexes = Self::build_indexes(&records);
        let slot_width = Self::slot_width();
        let id_width = R::Id::BYTE_WIDTH;

        let file = OpenOptions::new().read(true).write(true).open(path)?;

        // First pass: check the header, then (only once it checks out)
        // read every *committed* (id, value) pair, keyed by id — the
        // reconciliation step the record-identity-keying fix added. A
        // slot whose marker byte isn't `COMMITTED` (never reached, or a
        // crash landed before its marker write) is skipped here entirely
        // — see module docs — so it falls through reconciliation below
        // exactly as if it had never been persisted, rather than handing
        // a torn id/value pair to a caller. A trailing partial slot
        // (fewer than `slot_width` bytes left) is ignored, the same
        // permissive-truncation convention this crate's WAL reader
        // (`durability::read_wal_entries`) already follows.
        let persisted: PersistedSlots<R, ScanMarker> = {
            // SAFETY: see `create` — same single-process exclusive-access
            // assumption for the *mapping* itself. This mapping is read,
            // then dropped before any append below; `memmap2` unmaps on
            // drop.
            let mmap = unsafe { MmapMut::map_mut(&file)? };
            Self::read_header(&mmap)?;
            let existing_slot_count = (mmap.len() - HEADER_LEN) / slot_width;
            let value_width = R::ScanValue::BYTE_WIDTH;
            let mut persisted = HashMap::with_capacity(existing_slot_count);
            for position in 0..existing_slot_count {
                if !Self::is_committed(&mmap, position) {
                    continue;
                }
                let start = Self::slot_offset(position);
                let id = R::Id::read_le(&mmap[start..start + id_width]);
                let value =
                    R::ScanValue::read_le(&mmap[start + id_width..start + id_width + value_width]);
                persisted.insert(id, (position, value));
            }
            persisted
        };

        // Reconcile: every record in `records` either already has a
        // persisted slot (reuse its position) or doesn't (append a fresh
        // one for it, in `records`' own order — deterministic, not
        // HashMap-iteration-order-dependent). A persisted id with no
        // matching record in `records` is simply never added to
        // `position_index` — see module docs' "stale" case. Unlike the
        // previous design, a missing record's position is *not* computed
        // here at all — it's whatever `append_committed_slot` reports
        // back from the write that actually landed it (see module docs'
        // "next free slot" race section for why that distinction is the
        // entire fix).
        let mut position_index = HashMap::with_capacity(records.len());
        let mut missing: Vec<&R> = Vec::new();
        for record in &records {
            match persisted.get(&record.id()) {
                Some(&(position, _)) => {
                    position_index.insert(record.id(), position);
                }
                None => missing.push(record),
            }
        }

        if !missing.is_empty() {
            let mut appender = OpenOptions::new().append(true).open(path)?;
            for record in missing {
                let position = Self::append_committed_slot(
                    &mut appender,
                    record.id(),
                    record.scannable_value(),
                )?;
                position_index.insert(record.id(), position);
            }
        }

        // Re-map at the file's current length — reflecting any appends
        // just made above, through a *different* handle than `mmap`
        // (already dropped, block-scoped, before those appends) but the
        // same underlying file, so its on-disk length is already correct
        // by the time this mapping is established.
        // SAFETY: see `create`.
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        // Only now that the mmap file has opened and reconciled cleanly:
        // an error above must never replace a valid blob with one
        // describing a store this call failed to produce.
        if let Some(encoded) = stale_blob {
            encoded.write(&companion)?;
        }

        Ok(Self {
            records: indexes.records,
            index: indexes.index,
            position_index,
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }

    /// The record set persisted in the companion blob at `<path>.records`,
    /// in the order it was written — the `Vec<R>` a later [`Self::open`]
    /// (or a relationship layer built above the store, which needs that
    /// order to be deterministic) would otherwise have had to be handed
    /// by the caller. Reads only the blob; never touches the mmap file,
    /// never writes anything.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::RecordBlobUnreadable`], naming the
    /// companion path, if the blob is missing, isn't one (wrong magic —
    /// a `ProductionStore` companion included), was written by an
    /// incompatible version, doesn't decode, or doesn't match its own
    /// header fingerprint (`STORAGE-015-FR-005`).
    pub fn read_portable_records(path: &Path) -> Result<Vec<R>, DurabilityError> {
        record_blob::read(&blob_path(path))
    }

    /// Reopen a store from its two files alone — exactly
    /// `open(read_portable_records(path)?, path)`. Because the records
    /// come from the blob itself, `open`'s currency check always passes
    /// and nothing is rewritten (`STORAGE-015-FR-004`).
    ///
    /// # Errors
    ///
    /// Everything [`Self::read_portable_records`] and [`Self::open`] can
    /// return.
    pub fn open_portable(path: &Path) -> Result<Self, DurabilityError> {
        Self::open(Self::read_portable_records(path)?, path)
    }
}

impl<R, IndexMarker, ScanMarker> GetById<R> for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker>
        + ScannableField<ScanMarker>
        + Clone
        + Serialize
        + DeserializeOwned,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Write-through consistent with `UpdateField::update`: the returned
    /// record's scannable field always reflects the live mapped value, not
    /// whatever `records` held at construction time — see this module's
    /// doc comment on why `set_scannable_value` exists.
    fn get(&self, id: R::Id) -> Option<R> {
        let mut record = self.records.get(&id)?.clone();
        let position = *self.position_index.get(&id)?;
        record.set_scannable_value(self.read_value(position));
        Some(record)
    }
}

impl<R, IndexMarker, ScanMarker> FilterEq<R, IndexMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker>
        + ScannableField<ScanMarker>
        + Clone
        + Serialize
        + DeserializeOwned,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.index.get(value).cloned().unwrap_or_default()
    }
}

impl<R, IndexMarker, ScanMarker> ScanField<R, ScanMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker>
        + ScannableField<ScanMarker>
        + Clone
        + Serialize
        + DeserializeOwned,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// The bulk `chunks_exact` fast path `MmapAgeStore::scan_ages`'s own
    /// `PRODUCTION-DEFAULT` diagnosis established (25-32x over one
    /// `read_value` call per position, `RESULTS.md`'s `## Production
    /// recommendation` section) still applies whenever every slot in the
    /// file is live (`is_gapless`) — the common case, and the only case
    /// the original benchmark measured. Once a stale record's slot can
    /// outlive it (see module docs), a blind full-file scan would leak
    /// that removed record's value into the result, so the gapped case
    /// falls back to reading only the positions `position_index` says are
    /// actually live, sorted first for some locality rather than following
    /// `HashMap` iteration order — see this round's own report for the
    /// measured cost of that fallback path.
    fn scan(&self) -> Vec<R::ScanValue> {
        let id_width = R::Id::BYTE_WIDTH;
        let value_width = R::ScanValue::BYTE_WIDTH;
        let slot_width = Self::slot_width();
        if self.is_gapless() {
            // Skip the header — everything from here on is slot data.
            // `is_gapless` (see its own doc comment) already guarantees
            // every slot in this range is committed, via the same
            // pigeonhole argument: an uncommitted slot is never in
            // `position_index`, so if every slot *were* accounted for
            // here despite one being uncommitted, `position_index` would
            // be short by exactly that slot and this fast path wouldn't
            // have been taken at all. `[id_width..id_width + value_width]`
            // stops short of the trailing marker byte — it's never part
            // of a value.
            return self.mmap[HEADER_LEN..]
                .chunks_exact(slot_width)
                .map(|slot| R::ScanValue::read_le(&slot[id_width..id_width + value_width]))
                .collect();
        }
        let mut positions: Vec<usize> = self.position_index.values().copied().collect();
        positions.sort_unstable();
        positions
            .into_iter()
            .map(|position| self.read_value(position))
            .collect()
    }
}

impl<R, IndexMarker, ScanMarker> UpdateField<R, ScanMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker>
        + ScannableField<ScanMarker>
        + Clone
        + Serialize
        + DeserializeOwned,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    fn update(&mut self, id: R::Id, value: R::ScanValue) -> Result<(), NotFound<R::Id>> {
        let position = *self.position_index.get(&id).ok_or(NotFound(id))?;
        self.write_value(position, value);
        Ok(())
    }
}

impl<R, IndexMarker, ScanMarker> Flush for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker>
        + ScannableField<ScanMarker>
        + Clone
        + Serialize
        + DeserializeOwned,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Force the mapped scannable field to physical disk (`msync`) —
    /// mirrors `MmapAgeStore::flush` exactly.
    fn flush(&self) -> Result<(), DurabilityError> {
        self.mmap.flush()?;
        Ok(())
    }
}

// Uses `order_customer::{Order, ...}` as its concrete test fixture (the
// generic machinery under test here has no domain of its own) — gated
// behind `research` the same way that module is, so a default (research
// off) `cargo test` still compiles cleanly; `cargo test --features
// research` (or `--all-features`) runs these.
#[cfg(all(test, feature = "research"))]
mod tests {
    use super::*;
    use crate::generic::order_customer::{Amount, Order, OrderStatus, Status};

    fn sample() -> Vec<Order> {
        vec![
            Order {
                id: uuid::Uuid::from_u128(1),
                customer_id: uuid::Uuid::from_u128(100),
                amount_cents: 2_500,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 1_000,
                discount_cents: 0,
            },
            Order {
                id: uuid::Uuid::from_u128(2),
                customer_id: uuid::Uuid::from_u128(100),
                amount_cents: 4_200,
                status: OrderStatus::Pending,
                created_at_unix_ms: 2_000,
                discount_cents: 0,
            },
        ]
    }

    #[test]
    fn create_then_read_and_write() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_basic").unwrap();
        let path = dir.join("amount.mmap");
        let mut store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();

        assert_eq!(
            GetById::get(&store, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500
        );
        UpdateField::update(&mut store, uuid::Uuid::from_u128(1), 9_999).unwrap();
        assert_eq!(
            GetById::get(&store, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            9_999
        );
        assert!(ScanField::scan(&store).contains(&9_999));

        assert!(matches!(
            UpdateField::update(&mut store, uuid::Uuid::from_u128(99), 1),
            Err(NotFound(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_then_reopen_sees_the_written_value() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_roundtrip").unwrap();
        let path = dir.join("amount.mmap");

        {
            let mut store =
                GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            UpdateField::update(&mut store, uuid::Uuid::from_u128(1), 77_000).unwrap();
            Flush::flush(&store).unwrap();
        }

        let reopened = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            77_000
        );
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(2))
                .unwrap()
                .amount_cents,
            4_200
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_eq_works_on_the_index_field() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_index").unwrap();
        let path = dir.join("amount.mmap");
        let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();

        assert_eq!(
            FilterEq::filter_eq(&store, &OrderStatus::Shipped),
            vec![uuid::Uuid::from_u128(1)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Baseline: a file written at [`SCHEMA_VERSION`] opens cleanly — the
    /// header check must not get in the way of the ordinary case.
    #[test]
    fn opening_a_file_at_the_current_schema_version_succeeds() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_header_baseline").unwrap();
        let path = dir.join("amount.mmap");
        {
            let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            Flush::flush(&store).unwrap();
        }

        let reopened = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path);
        assert!(
            reopened.is_ok(),
            "a file written at the current schema version must open cleanly, got {:?}",
            reopened.err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Poking a different version number into an otherwise-valid file's
    /// header is exactly the on-disk shape a real older (or newer)
    /// `SCHEMA_VERSION` would have produced — a reader can't tell the
    /// difference between "an old build wrote this" and this test's
    /// direct byte edit, which is the point: it proves the *detection*
    /// mechanism, not any particular history of the constant.
    #[test]
    fn opening_a_file_with_a_mismatched_schema_version_fails_distinctly() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_header_version").unwrap();
        let path = dir.join("amount.mmap");
        {
            let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            Flush::flush(&store).unwrap();
        }

        let bogus_version: u32 = SCHEMA_VERSION.wrapping_add(1);
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            // SAFETY: same single-process exclusive-access assumption as
            // every other mapping in this module — this is a test-only
            // corruption of a file this same test just wrote and owns.
            let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            bogus_version.write_le(&mut mmap[MAGIC.len()..HEADER_LEN]);
            mmap.flush().unwrap();
        }

        let result = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path);
        match result.err() {
            Some(DurabilityError::SchemaVersionMismatch { found, expected }) => {
                assert_eq!(found, bogus_version);
                assert_eq!(expected, SCHEMA_VERSION);
            }
            other => panic!("expected SchemaVersionMismatch, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other failure mode, kept distinct from a version mismatch: the
    /// magic bytes themselves don't match at all — not "an older version
    /// of this store," but "not this store's file to begin with." Only
    /// the magic is corrupted here, leaving the (already-correct) version
    /// bytes untouched — if this were ever misread as a version mismatch
    /// instead, that confusion is exactly what this test exists to catch.
    #[test]
    fn opening_a_file_with_the_wrong_magic_number_fails_distinctly_from_a_version_mismatch() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_header_magic").unwrap();
        let path = dir.join("amount.mmap");
        {
            let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            Flush::flush(&store).unwrap();
        }

        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            // SAFETY: see the version-mismatch test above.
            let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            mmap[0..MAGIC.len()].copy_from_slice(&[0xFFu8; MAGIC.len()]);
            mmap.flush().unwrap();
        }

        let result = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path);
        assert!(
            matches!(result, Err(DurabilityError::InvalidMagic)),
            "expected InvalidMagic, got {:?}",
            result.err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file too short to even contain a header must fail the same way a
    /// wrong-magic file does (there's no magic to compare at all) — a
    /// clean, typed error, not an out-of-bounds panic on the header slice.
    #[test]
    fn a_file_shorter_than_the_header_fails_as_invalid_magic_not_a_panic() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_header_short").unwrap();
        let path = dir.join("garbage.mmap");
        std::fs::write(&path, [0u8; 4]).unwrap(); // shorter than HEADER_LEN

        let result = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path);
        assert!(
            matches!(result, Err(DurabilityError::InvalidMagic)),
            "expected InvalidMagic, got {:?}",
            result.err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A slot whose trailing marker byte isn't [`COMMITTED`] — exactly
    /// what the crash-safety diagnosis reproduced (a crash between a new
    /// slot's id write and its value write, or between the value and
    /// marker writes this round adds) — must be treated as if it had
    /// never been persisted at all, not read as whatever stale/garbage
    /// bytes happen to sit there. The corrupted value is deliberately set
    /// to something `sample()` never produces, so a passing assertion
    /// here can only mean the marker check actually excluded the slot,
    /// not a coincidence of matching bytes.
    #[test]
    fn a_slot_with_an_unset_commit_marker_is_treated_as_never_persisted() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_torn_slot").unwrap();
        let path = dir.join("amount.mmap");
        {
            let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            Flush::flush(&store).unwrap();
        }

        let corrupted_position = 1; // sample()[1] is id 2, amount_cents 4_200
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            // SAFETY: same single-process exclusive-access assumption as
            // every other mapping in this module — a test-only corruption
            // of a file this same test just wrote and owns.
            let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            let start = GenericMmapStore::<Order, Status, Amount>::slot_offset(corrupted_position);
            let id_width = uuid::Uuid::BYTE_WIDTH;
            let value_width = i64::BYTE_WIDTH;
            // Id bytes stay untouched — a real torn write always keeps a
            // valid-looking id. Value is corrupted to a sentinel
            // `sample()` never writes; marker is cleared.
            (-1i64).write_le(&mut mmap[start + id_width..start + id_width + value_width]);
            mmap[start + id_width + value_width] = 0;
            mmap.flush().unwrap();
        }

        let reopened = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(2))
                .unwrap()
                .amount_cents,
            4_200,
            "an uncommitted slot must be re-seeded from the supplied record, not read as the \
             corrupted stale value"
        );
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500,
            "an untouched slot must be unaffected by a neighboring slot's corruption"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- companion record blob / portability (STORAGE-015) ----

    #[test]
    fn create_writes_the_companion_blob_and_open_portable_round_trips_every_field() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_portable").unwrap();
        let path = dir.join("amount.mmap");
        {
            let mut store =
                GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            UpdateField::update(&mut store, uuid::Uuid::from_u128(1), 77_000).unwrap();
            Flush::flush(&store).unwrap();
        }
        assert!(
            blob_path(&path).is_file(),
            "create must write <path>.records"
        );

        // Records come back in creation order, every field intact.
        let records =
            GenericMmapStore::<Order, Status, Amount>::read_portable_records(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, uuid::Uuid::from_u128(1));
        assert_eq!(records[1].id, uuid::Uuid::from_u128(2));
        assert_eq!(records[1].status, OrderStatus::Pending);
        assert_eq!(records[1].created_at_unix_ms, 2_000);

        let reopened = GenericMmapStore::<Order, Status, Amount>::open_portable(&path).unwrap();
        let first = GetById::get(&reopened, uuid::Uuid::from_u128(1)).unwrap();
        // The mmap-backed field reads from the mmap file (the update
        // survived), the non-durable fields from the blob.
        assert_eq!(first.amount_cents, 77_000);
        assert_eq!(first.status, OrderStatus::Shipped);
        assert_eq!(first.customer_id, uuid::Uuid::from_u128(100));
        assert_eq!(first.created_at_unix_ms, 1_000);
        assert_eq!(
            FilterEq::filter_eq(&reopened, &OrderStatus::Pending),
            vec![uuid::Uuid::from_u128(2)]
        );
        let mut scanned = ScanField::scan(&reopened);
        scanned.sort_unstable();
        assert_eq!(scanned, vec![4_200, 77_000]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_both_files_to_a_fresh_directory_reopens_portably() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_portable_copy").unwrap();
        let path = dir.join("amount.mmap");
        {
            let mut store =
                GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            UpdateField::update(&mut store, uuid::Uuid::from_u128(2), 1).unwrap();
            Flush::flush(&store).unwrap();
        }

        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let copied = elsewhere.join("renamed.mmap");
        std::fs::copy(&path, &copied).unwrap();
        std::fs::copy(blob_path(&path), blob_path(&copied)).unwrap();

        let reopened = GenericMmapStore::<Order, Status, Amount>::open_portable(&copied).unwrap();
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(2))
                .unwrap()
                .amount_cents,
            1
        );
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(1))
                .unwrap()
                .status,
            OrderStatus::Shipped
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_companion_is_unreadable_naming_its_path_and_plain_open_heals_it() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_portable_missing").unwrap();
        let path = dir.join("amount.mmap");
        GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        let companion = blob_path(&path);
        std::fs::remove_file(&companion).unwrap();

        // Both portable entry points fail distinctly, without touching the
        // mmap file, and without a panic.
        match GenericMmapStore::<Order, Status, Amount>::read_portable_records(&path) {
            Err(DurabilityError::RecordBlobUnreadable { path: p, .. }) => assert_eq!(p, companion),
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        assert!(matches!(
            GenericMmapStore::<Order, Status, Amount>::open_portable(&path),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));
        assert!(!companion.exists(), "a failed read must not create a blob");

        // The caller-supplied path (the pre-feature contract) heals it.
        GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        assert!(companion.is_file());
        let healed = GenericMmapStore::<Order, Status, Amount>::open_portable(&path).unwrap();
        assert_eq!(
            GetById::get(&healed, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_rewrites_the_companion_only_when_the_record_set_changed() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_portable_stale").unwrap();
        let path = dir.join("amount.mmap");
        GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        let companion = blob_path(&path);
        let before = std::fs::read(&companion).unwrap();
        let mtime_before = std::fs::metadata(&companion).unwrap().modified().unwrap();

        // Same dataset: no write at all — bytes and mtime unchanged.
        GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        GenericMmapStore::<Order, Status, Amount>::open_portable(&path).unwrap();
        assert_eq!(std::fs::read(&companion).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&companion).unwrap().modified().unwrap(),
            mtime_before
        );

        // A changed non-durable field: rewritten, and the new value is what
        // `open_portable` sees afterwards.
        let mut changed = sample();
        changed[0].status = OrderStatus::Refunded;
        GenericMmapStore::<Order, Status, Amount>::open(changed, &path).unwrap();
        assert_ne!(std::fs::read(&companion).unwrap(), before);
        let reopened = GenericMmapStore::<Order, Status, Amount>::open_portable(&path).unwrap();
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(1))
                .unwrap()
                .status,
            OrderStatus::Refunded
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_production_store_blob_at_the_companion_path_is_a_magic_error() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_portable_dog_blob").unwrap();
        let path = dir.join("amount.mmap");
        GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        let companion = blob_path(&path);
        crate::durability::record_blob::RecordBlob {
            records: crate::durability::test_support::sample_records(),
            edges: crate::durability::test_support::sample_edges(),
        }
        .write(&companion)
        .unwrap();

        match GenericMmapStore::<Order, Status, Amount>::read_portable_records(&path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(cause.starts_with("magic number mismatch"), "{cause}");
            }
            other => panic!("expected RecordBlobUnreadable, got {other:?}"),
        }
        // A foreign file counts as stale: plain `open` replaces it.
        GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        GenericMmapStore::<Order, Status, Amount>::open_portable(&path).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stray_temp_file_is_not_a_companion_and_the_mmap_file_alone_is_not_portable() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_portable_mmap_only").unwrap();
        let path = dir.join("amount.mmap");
        GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        let companion = blob_path(&path);
        std::fs::remove_file(&companion).unwrap();
        // The mmap file is still a perfectly good mmap file on its own...
        GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        // ...and now has a blob again; the mmap file's own header errors are
        // untouched by all of this (a truncated mmap file is still InvalidMagic).
        std::fs::write(&path, [0u8; 4]).unwrap();
        assert!(matches!(
            GenericMmapStore::<Order, Status, Amount>::open_portable(&path),
            Err(DurabilityError::InvalidMagic)
        ));
        assert!(matches!(
            GenericMmapStore::<Order, Status, Amount>::open(sample(), &path),
            Err(DurabilityError::InvalidMagic)
        ));
        // ...and the failed `open` did not touch the (still-current) blob.
        assert!(
            GenericMmapStore::<Order, Status, Amount>::read_portable_records(&path).is_ok(),
            "an mmap-file failure must leave a valid companion untouched"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
