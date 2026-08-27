//! Diagnosis, not a fix: what actually happens today when a file
//! persisted by `GenericMmapStore` under one version of `Order` is
//! reopened by code compiled against `Order` plus one new field — the
//! same kind of additive change `DiscountCents` was, tested here from the
//! persistence side rather than the trait-composition side. No production
//! code changes; this file exists purely to reproduce and record the
//! current behavior with real evidence, per this round's own task.
//!
//! See the module-level report this task asked for (delivered in chat,
//! not duplicated here) for the full severity assessment. Short version,
//! confirmed by the tests below: `GenericMmapStore`'s on-disk file is
//! *never* a serialization of the whole record — it is a raw, untagged
//! array of exactly one field's fixed-width values (`Amount`/`amount_cents`
//! here), addressed purely by the *array position* of the `records: Vec<R>`
//! the caller supplies fresh to every `create`/`open` call. Every other
//! field (`id`, `customer_id`, `status`, `created_at_unix_ms`,
//! `discount_cents`, and any newly-added one) is never written to this
//! file at all — which is exactly why `OrderV2` below can gain a field
//! and still open the same file cleanly: there is no on-disk layout for
//! that field to have shifted.

use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
use rusty_multimodal_db::generic::query::{GetById, UpdateField};
use rusty_multimodal_db::generic::store::Flush;
use rusty_multimodal_db::generic::traits::{IndexedField, Record, ScannableField};
use uuid::Uuid;

// ---- "Old schema": today's real `Order`, imported unchanged. ----
use rusty_multimodal_db::generic::order_customer::{Amount, Order, OrderStatus, Status};

// ---- "New schema": `Order` plus one additive field, the same kind of
// change `DiscountCents` was — a local test-only type, not a change to
// `src/generic/order_customer.rs` itself. ----
#[derive(Debug, Clone, PartialEq)]
struct OrderV2 {
    id: Uuid,
    customer_id: Uuid,
    amount_cents: i64,
    status: OrderStatus,
    created_at_unix_ms: i64,
    discount_cents: i64,
    /// The new field this round adds — deliberately never populated from
    /// anything persisted; every construction site below sets it directly,
    /// the same way a real caller's freshly-recompiled code would.
    fulfillment_notes: String,
}

impl Record for OrderV2 {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

impl IndexedField<Status> for OrderV2 {
    type IndexValue = OrderStatus;
    fn indexed_value(&self) -> &OrderStatus {
        &self.status
    }
}

impl ScannableField<Amount> for OrderV2 {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.amount_cents
    }
    fn set_scannable_value(&mut self, value: i64) {
        self.amount_cents = value;
    }
}

fn old_schema_sample() -> Vec<Order> {
    vec![
        Order {
            id: Uuid::from_u128(1),
            customer_id: Uuid::from_u128(100),
            amount_cents: 2_500,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 1_000,
            discount_cents: 0,
        },
        Order {
            id: Uuid::from_u128(2),
            customer_id: Uuid::from_u128(100),
            amount_cents: 4_200,
            status: OrderStatus::Pending,
            created_at_unix_ms: 2_000,
            discount_cents: 0,
        },
        Order {
            id: Uuid::from_u128(3),
            customer_id: Uuid::from_u128(200),
            amount_cents: 999,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 3_000,
            discount_cents: 0,
        },
    ]
}

/// The "new schema" reconstruction of the *same* dataset — same ids, same
/// order, same `amount_cents` (irrelevant on reopen anyway; the mmap file
/// is the source of truth for that field, not this Vec), just with the
/// new field populated. This is what a real caller's post-migration code
/// would naturally build: there is no source anywhere in this crate that
/// would hand it `fulfillment_notes` from disk, because nothing durable
/// ever stored it — see this file's own module docs.
fn new_schema_sample() -> Vec<OrderV2> {
    vec![
        OrderV2 {
            id: Uuid::from_u128(1),
            customer_id: Uuid::from_u128(100),
            amount_cents: 0, // overwritten by whatever the mmap file holds
            status: OrderStatus::Shipped,
            created_at_unix_ms: 1_000,
            discount_cents: 0,
            fulfillment_notes: String::new(),
        },
        OrderV2 {
            id: Uuid::from_u128(2),
            customer_id: Uuid::from_u128(100),
            amount_cents: 0,
            status: OrderStatus::Pending,
            created_at_unix_ms: 2_000,
            discount_cents: 0,
            fulfillment_notes: String::new(),
        },
        OrderV2 {
            id: Uuid::from_u128(3),
            customer_id: Uuid::from_u128(200),
            amount_cents: 0,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 3_000,
            discount_cents: 0,
            fulfillment_notes: "flagged for review".into(),
        },
    ]
}

/// The concrete scenario the task asked for: build fresh under today's
/// `Order`, write, flush, drop; recompile (simulated here by using
/// `OrderV2`, a distinct Rust type) with one new field; reopen the *same*
/// file. Confirms this "just works," but for a reason worth being
/// precise about: not because any compatibility mechanism recognized and
/// tolerated the new field, but because the file never had a place the
/// new field could have occupied in the first place — it holds only
/// `Amount`'s raw bytes, positionally.
#[test]
fn reopening_an_old_mmap_file_under_a_schema_with_an_added_field_reads_the_correct_amounts() {
    let dir = rusty_multimodal_db::bench_support::fresh_temp_dir("schema_evolution_mmap").unwrap();
    let path = dir.join("amount.mmap");

    // Old schema: build, write, update one record, flush, drop cleanly.
    {
        let mut store =
            GenericMmapStore::<Order, Status, Amount>::create(old_schema_sample(), &path).unwrap();
        UpdateField::update(&mut store, Uuid::from_u128(2), 9_999).unwrap();
        Flush::flush(&store).unwrap();
    }

    // New schema: reopen the same file with `OrderV2` (one added field).
    let reopened =
        GenericMmapStore::<OrderV2, Status, Amount>::open(new_schema_sample(), &path).unwrap();

    // The durable field (Amount) round-trips correctly, including the
    // update made under the old schema.
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(1))
            .unwrap()
            .amount_cents,
        2_500
    );
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(2))
            .unwrap()
            .amount_cents,
        9_999,
        "the update made under the OLD schema must survive the reopen under the NEW one"
    );
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(3))
            .unwrap()
            .amount_cents,
        999
    );

    // The new field is exactly what the caller supplied fresh — never
    // read from the file, because it was never written to the file.
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(3))
            .unwrap()
            .fulfillment_notes,
        "flagged for review"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The real danger this architecture *does* have, distinct from "a field
/// was added": since the file's bytes are addressed purely by the
/// *position* of the record within the `records: Vec<R>` the caller
/// supplies (not by id, not by any marker in the file), a caller that
/// reopens with records in a *different order* than they were created
/// silently reads another record's `Amount` value under the wrong id —
/// no error, no panic, wrong data. Demonstrated here to make the actual
/// risk shape precise: **the risk is positional-drift between the create-
/// time and open-time record orderings, not additive schema evolution**.
#[test]
fn reopening_with_records_in_a_different_order_silently_swaps_which_id_gets_which_amount() {
    let dir =
        rusty_multimodal_db::bench_support::fresh_temp_dir("schema_evolution_reorder").unwrap();
    let path = dir.join("amount.mmap");

    {
        let store =
            GenericMmapStore::<Order, Status, Amount>::create(old_schema_sample(), &path).unwrap();
        Flush::flush(&store).unwrap();
    }

    // Reopen with the same three records, but positions 1 and 2 swapped.
    let mut reordered = old_schema_sample();
    reordered.swap(0, 1);
    let reopened = GenericMmapStore::<Order, Status, Amount>::open(reordered, &path).unwrap();

    // No error anywhere above — `open` succeeded silently. But order 1
    // (originally 2_500) now reads order 2's on-disk value (4_200), and
    // vice versa: a real, silent data-integrity failure with no signal
    // that anything went wrong, entirely unrelated to the schema/field
    // question this round asked about, but real evidence of exactly how
    // fragile the positional coupling between `create` and `open` is.
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(1))
            .unwrap()
            .amount_cents,
        4_200,
        "order 1 silently reads order 2's persisted amount once record order drifts"
    );
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(2))
            .unwrap()
            .amount_cents,
        2_500,
        "and vice versa — no error, no panic, just swapped data"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
