//! Wall-clock Criterion suite for the durability prototypes
//! (`src/durability/*.rs`, STORAGE-008/009). Three groups, mirroring the
//! task's three requested metrics:
//!
//! - `durability_per_write`: `update_age` overhead vs. the existing
//!   non-durable baseline in `benches/workloads.rs`'s `update_age` group.
//! - `durability_checkpoint`: cost of an explicit checkpoint/flush call
//!   after 1,000 pre-applied writes.
//! - `durability_load`: cost of `open`ing a store from a path that already
//!   has 1,000 writes' worth of persisted state on disk (the
//!   "load/replay/startup" path each variant's `open` doc comment calls
//!   out as what this benchmark measures).
//!
//! # Two local traits, not `src/durability`'s own API
//!
//! The 8 variants' native `create`/`open`/`checkpoint`-or-`flush` methods
//! don't share one signature: [`SnapshotRebuildStore`]/[`SnapshotFullStore`]'s
//! `open` takes only a path (the persisted file *is* the canonical
//! records/edges, per their own module docs), while every other variant's
//! `open` takes `records`/`edges` alongside the path/dir; `checkpoint` and
//! `flush` are named differently across variants; [`RedbStore`] has neither,
//! since every `update_age` call is already its own committed transaction.
//! [`DurableVariant`]/[`Checkpointable`] below exist purely to give this
//! bench file one uniform shape to iterate over, the same way
//! `benches/workloads.rs` iterates over `S: DogStore + From<Vec<DogRecord>>`
//! — they are not part of this crate's public durability API, and adding
//! `DurableVariant`/`Checkpointable` impls elsewhere wouldn't do anything.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkGroup, BenchmarkId, Criterion,
};
use rusty_multimodal_db::bench_support::{build_dataset, fresh_temp_dir, Dataset, RoundRobin, SIZES};
use rusty_multimodal_db::durability::{
    DurabilityError, HybridStore, LsmStore, MmapAgeStore, RedbStore, SnapshotFullStore,
    SnapshotRebuildStore, WalBufferedStore, WalFsyncStore,
};
use rusty_multimodal_db::{DogRecord, DogStore};
use std::path::Path;
use uuid::Uuid;

/// Writes pre-applied before timing a checkpoint/flush or a load, in every
/// group that needs a non-trivial amount of prior state on disk. Matches
/// the task's own "batch of ~1000 writes" framing for the non-Criterion
/// recoverability comparison in `RESULTS.md`, so the same number backs both.
const DURABILITY_PREWRITE_COUNT: u32 = 1_000;

/// Uniform `create`/`open` shape across all 8 variants — see module docs
/// for why their native signatures don't already agree on one.
trait DurableVariant: DogStore + Sized {
    fn create_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError>;

    fn open_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError>;

    /// Guarantee everything written so far is durable on disk, for variants
    /// with a separate durability step beyond their per-write path. A no-op
    /// default covers variants (just [`RedbStore`]) where every write is
    /// already durable by the time `update_age` returns.
    fn finalize(&mut self) -> Result<(), DurabilityError> {
        Ok(())
    }
}

/// The subset of [`DurableVariant`]s with a standalone checkpoint/flush
/// operation worth benchmarking on its own — every variant except
/// [`RedbStore`] (see its module docs: no separate checkpoint exists to
/// benchmark). Not implementing this trait for `RedbStore` is what keeps
/// `bench_durability_checkpoint` from calling it — a compile-time omission,
/// not a runtime skip.
trait Checkpointable: DurableVariant {
    fn checkpoint_at(&mut self) -> Result<(), DurabilityError>;
}

macro_rules! impl_durable_variant_dir_based {
    ($ty:ty, checkpoint) => {
        impl DurableVariant for $ty {
            fn create_at(
                records: Vec<DogRecord>,
                edges: Vec<(Uuid, Uuid)>,
                path: &Path,
            ) -> Result<Self, DurabilityError> {
                Self::create(records, edges, path)
            }

            fn open_at(
                records: Vec<DogRecord>,
                edges: Vec<(Uuid, Uuid)>,
                path: &Path,
            ) -> Result<Self, DurabilityError> {
                Self::open(records, edges, path)
            }

            fn finalize(&mut self) -> Result<(), DurabilityError> {
                self.checkpoint()
            }
        }

        impl Checkpointable for $ty {
            fn checkpoint_at(&mut self) -> Result<(), DurabilityError> {
                self.checkpoint()
            }
        }
    };
}

impl_durable_variant_dir_based!(WalFsyncStore, checkpoint);
impl_durable_variant_dir_based!(WalBufferedStore, checkpoint);
impl_durable_variant_dir_based!(HybridStore, checkpoint);

// Variants 3/4: `open` takes only a path — the persisted file is the
// canonical source of truth, not an externally-supplied base dataset (see
// each module's own docs). `open_at` ignores the supplied records/edges to
// present the same three-argument shape every other variant uses.

impl DurableVariant for SnapshotRebuildStore {
    fn create_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::create(records, edges, path)
    }

    fn open_at(
        _records: Vec<DogRecord>,
        _edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::open(path)
    }

    fn finalize(&mut self) -> Result<(), DurabilityError> {
        self.checkpoint()
    }
}

impl Checkpointable for SnapshotRebuildStore {
    fn checkpoint_at(&mut self) -> Result<(), DurabilityError> {
        self.checkpoint()
    }
}

impl DurableVariant for SnapshotFullStore {
    fn create_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::create(records, edges, path)
    }

    fn open_at(
        _records: Vec<DogRecord>,
        _edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::open(path)
    }

    fn finalize(&mut self) -> Result<(), DurabilityError> {
        self.checkpoint()
    }
}

impl Checkpointable for SnapshotFullStore {
    fn checkpoint_at(&mut self) -> Result<(), DurabilityError> {
        self.checkpoint()
    }
}

impl DurableVariant for MmapAgeStore {
    fn create_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::create(records, edges, path)
    }

    fn open_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::open(records, edges, path)
    }

    fn finalize(&mut self) -> Result<(), DurabilityError> {
        self.flush()
    }
}

impl Checkpointable for MmapAgeStore {
    fn checkpoint_at(&mut self) -> Result<(), DurabilityError> {
        self.flush()
    }
}

impl DurableVariant for LsmStore {
    fn create_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::create(records, edges, path)
    }

    fn open_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::open(records, edges, path)
    }

    fn finalize(&mut self) -> Result<(), DurabilityError> {
        self.flush()
    }
}

impl Checkpointable for LsmStore {
    fn checkpoint_at(&mut self) -> Result<(), DurabilityError> {
        self.flush()
    }
}

// RedbStore: DurableVariant only, no Checkpointable impl — see module docs.
impl DurableVariant for RedbStore {
    fn create_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::create(records, edges, path)
    }

    fn open_at(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Self::open(records, edges, path)
    }
}

/// Per-`update_age`-call overhead: store built once (outside the timed
/// region, same convention as `benches/workloads.rs`'s `run_update_age`),
/// rotating target ids and ages across iterations.
fn run_per_write<S: DurableVariant>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) {
    let Ok(dir) = fresh_temp_dir(&format!("durability_write_{name}_{n}")) else {
        return;
    };
    let path = dir.join("store");
    let Ok(mut store) = S::create_at(dataset.records.clone(), dataset.edges.clone(), &path)
    else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };

    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    let mut next_age: u32 = 0;
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            next_age = next_age.wrapping_add(1) % 21;
            let _ = black_box(store.update_age(black_box(id), black_box(next_age)));
        });
    });

    let _ = std::fs::remove_dir_all(&dir);
}

/// Cost of one checkpoint/flush call, after [`DURABILITY_PREWRITE_COUNT`]
/// writes have already been applied (untimed).
fn run_checkpoint<S: Checkpointable>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) {
    let Ok(dir) = fresh_temp_dir(&format!("durability_checkpoint_{name}_{n}")) else {
        return;
    };
    let path = dir.join("store");
    let Ok(mut store) = S::create_at(dataset.records.clone(), dataset.edges.clone(), &path)
    else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };

    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    let mut next_age: u32 = 0;
    for _ in 0..DURABILITY_PREWRITE_COUNT {
        let id = dataset.sample_ids[cursor.advance()];
        next_age = next_age.wrapping_add(1) % 21;
        let _ = store.update_age(id, next_age);
    }

    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let _ = black_box(store.checkpoint_at());
        });
    });

    let _ = std::fs::remove_dir_all(&dir);
}

/// Cost of `open`ing a store whose path already has
/// [`DURABILITY_PREWRITE_COUNT`] writes' worth of persisted state (built and
/// finalized once, untimed, outside `b.iter_batched`). Uses
/// `iter_batched`/`PerIteration` rather than plain `b.iter` because `open`
/// takes `records`/`edges` by value — cloning the dataset per iteration must
/// stay in the untimed setup closure, or every measurement would be
/// dominated by clone cost rather than open/replay cost.
fn run_load<S: DurableVariant>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) {
    let Ok(dir) = fresh_temp_dir(&format!("durability_load_{name}_{n}")) else {
        return;
    };
    let path = dir.join("store");
    {
        let Ok(mut store) = S::create_at(dataset.records.clone(), dataset.edges.clone(), &path)
        else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        let mut cursor = RoundRobin::new(dataset.sample_ids.len());
        let mut next_age: u32 = 0;
        for _ in 0..DURABILITY_PREWRITE_COUNT {
            let id = dataset.sample_ids[cursor.advance()];
            next_age = next_age.wrapping_add(1) % 21;
            let _ = store.update_age(id, next_age);
        }
        let _ = store.finalize();
        // `store` dropped here — the on-disk files at `path` are what
        // `b.iter_batched` below repeatedly opens.
    }

    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter_batched(
            || (dataset.records.clone(), dataset.edges.clone()),
            |(records, edges)| black_box(S::open_at(records, edges, &path)),
            BatchSize::PerIteration,
        );
    });

    let _ = std::fs::remove_dir_all(&dir);
}

fn bench_durability_per_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("durability_per_write");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_per_write::<WalFsyncStore>(&mut group, "wal_fsync", n, &dataset);
        run_per_write::<WalBufferedStore>(&mut group, "wal_buffered", n, &dataset);
        run_per_write::<SnapshotRebuildStore>(&mut group, "snapshot_rebuild", n, &dataset);
        run_per_write::<SnapshotFullStore>(&mut group, "snapshot_full", n, &dataset);
        run_per_write::<HybridStore>(&mut group, "hybrid", n, &dataset);
        run_per_write::<MmapAgeStore>(&mut group, "mmap", n, &dataset);
        run_per_write::<LsmStore>(&mut group, "lsm", n, &dataset);
        run_per_write::<RedbStore>(&mut group, "redb", n, &dataset);
    }
    group.finish();
}

fn bench_durability_checkpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("durability_checkpoint");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_checkpoint::<WalFsyncStore>(&mut group, "wal_fsync", n, &dataset);
        run_checkpoint::<WalBufferedStore>(&mut group, "wal_buffered", n, &dataset);
        run_checkpoint::<SnapshotRebuildStore>(&mut group, "snapshot_rebuild", n, &dataset);
        run_checkpoint::<SnapshotFullStore>(&mut group, "snapshot_full", n, &dataset);
        run_checkpoint::<HybridStore>(&mut group, "hybrid", n, &dataset);
        run_checkpoint::<MmapAgeStore>(&mut group, "mmap", n, &dataset);
        run_checkpoint::<LsmStore>(&mut group, "lsm", n, &dataset);
        // RedbStore intentionally omitted: it doesn't implement
        // `Checkpointable` (no separate checkpoint operation exists — every
        // `update_age` is already its own committed transaction). See
        // RESULTS.md's durability section / Open Questions.
    }
    group.finish();
}

fn bench_durability_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("durability_load");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_load::<WalFsyncStore>(&mut group, "wal_fsync", n, &dataset);
        run_load::<WalBufferedStore>(&mut group, "wal_buffered", n, &dataset);
        run_load::<SnapshotRebuildStore>(&mut group, "snapshot_rebuild", n, &dataset);
        run_load::<SnapshotFullStore>(&mut group, "snapshot_full", n, &dataset);
        run_load::<HybridStore>(&mut group, "hybrid", n, &dataset);
        run_load::<MmapAgeStore>(&mut group, "mmap", n, &dataset);
        run_load::<LsmStore>(&mut group, "lsm", n, &dataset);
        run_load::<RedbStore>(&mut group, "redb", n, &dataset);
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_durability_per_write,
    bench_durability_checkpoint,
    bench_durability_load
);
criterion_main!(benches);
