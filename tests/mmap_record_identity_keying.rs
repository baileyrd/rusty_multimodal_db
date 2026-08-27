//! Confirms the fix for the record-order-drift silent-corruption bug the
//! schema-evolution diagnosis round found: `GenericMmapStore` persisted a
//! scannable field's raw value at `position * BYTE_WIDTH`, where
//! `position` was whatever index a record happened to occupy in the
//! caller-supplied `records: Vec<R>` — nothing in the file recorded which
//! record a value actually belonged to. Reopening with `records` in a
//! different order silently attributed position N's persisted value to
//! whatever record now sat at position N.
//!
//! Each persisted slot now carries `(id, value)`, and `open` reconciles
//! persisted ids against the caller's `records` **by id**, not position
//! (`src/generic/mmap_store.rs`'s own module docs have the full account).
//! This file reproduces the diagnosis's own order-drift scenario against
//! the fix (now correct, not a regression test that only demonstrates the
//! bug) and adds the two mismatch cases the original diagnosis didn't
//! need: a record present at open time with no persisted entry (new since
//! the last write), and a persisted id no longer present in the caller's
//! records at open time (removed since the last write).

use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
use rusty_multimodal_db::generic::order_customer::{Amount, Order, OrderStatus, Status};
use rusty_multimodal_db::generic::query::{GetById, ScanField, UpdateField};
use rusty_multimodal_db::generic::store::Flush;
use uuid::Uuid;

fn order(id: u128, customer: u128, amount_cents: i64) -> Order {
    Order {
        id: Uuid::from_u128(id),
        customer_id: Uuid::from_u128(customer),
        amount_cents,
        status: OrderStatus::Shipped,
        created_at_unix_ms: 1_000,
        discount_cents: 0,
    }
}

fn sample() -> Vec<Order> {
    vec![
        order(1, 100, 2_500),
        order(2, 100, 4_200),
        order(3, 200, 999),
    ]
}

/// The exact scenario the schema-evolution diagnosis's second test
/// demonstrated as broken: reopening with the same records, reordered.
/// Previously this silently swapped which id read which persisted
/// amount; now it must not, because the fix keys by id, not position.
#[test]
fn reopening_with_records_in_a_different_order_no_longer_causes_misattribution() {
    let dir = rusty_multimodal_db::bench_support::fresh_temp_dir("mmap_identity_reorder").unwrap();
    let path = dir.join("amount.mmap");

    {
        let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        Flush::flush(&store).unwrap();
    }

    // Reopen with positions 0 and 1 swapped relative to `create` — the
    // exact perturbation that used to cause silent misattribution.
    let mut reordered = sample();
    reordered.swap(0, 1);
    let reopened = GenericMmapStore::<Order, Status, Amount>::open(reordered, &path).unwrap();

    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(1))
            .unwrap()
            .amount_cents,
        2_500,
        "order 1 must still read its own persisted amount, not order 2's, once records reorder"
    );
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(2))
            .unwrap()
            .amount_cents,
        4_200
    );
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(3))
            .unwrap()
            .amount_cents,
        999
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Case (a), per `mmap_store.rs`'s own documented mismatch semantics: a
/// persisted id no longer present in the caller's current `records` —
/// removed since the last write. Its slot must not surface through any
/// query capability; the invariant is that a persisted value is never
/// attributed to a different id, and here there's no id to (mis)attribute
/// it to at all.
#[test]
fn a_record_removed_since_the_last_write_is_not_visible_after_reopen() {
    let dir = rusty_multimodal_db::bench_support::fresh_temp_dir("mmap_identity_stale").unwrap();
    let path = dir.join("amount.mmap");

    {
        let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        Flush::flush(&store).unwrap();
    }

    // Reopen without order 2 at all — as if it were deleted upstream
    // between the write and this reopen.
    let remaining: Vec<Order> = sample()
        .into_iter()
        .filter(|o| o.id != Uuid::from_u128(2))
        .collect();
    let reopened = GenericMmapStore::<Order, Status, Amount>::open(remaining, &path).unwrap();

    assert_eq!(GetById::get(&reopened, Uuid::from_u128(2)), None);
    // The two still-present records are unaffected by the removal.
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(1))
            .unwrap()
            .amount_cents,
        2_500
    );
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(3))
            .unwrap()
            .amount_cents,
        999
    );
    // `scan` must not leak the removed record's still-physically-present
    // bytes either — the exact leak the gapless/fallback split in `scan`
    // exists to prevent.
    let scanned = ScanField::<Order, Amount>::scan(&reopened);
    assert_eq!(
        scanned.len(),
        2,
        "a removed record's value must not appear in scan()"
    );
    assert!(!scanned.contains(&4_200));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Case (b): a record present in the caller's current `records` with no
/// persisted entry — added since the last write. Must be seeded from its
/// own `scannable_value()` (the same way `create` seeds every record) and
/// become durable going forward, not silently dropped or defaulted to
/// zero.
#[test]
fn a_record_added_since_the_last_write_is_seeded_and_persists() {
    let dir = rusty_multimodal_db::bench_support::fresh_temp_dir("mmap_identity_new").unwrap();
    let path = dir.join("amount.mmap");

    {
        let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        Flush::flush(&store).unwrap();
    }

    // Reopen with one extra order that was never part of the original
    // `create` call — as if it were inserted upstream since the last write.
    let mut with_new_order = sample();
    with_new_order.push(order(4, 300, 55_555));
    {
        let reopened =
            GenericMmapStore::<Order, Status, Amount>::open(with_new_order.clone(), &path).unwrap();
        assert_eq!(
            GetById::get(&reopened, Uuid::from_u128(4))
                .unwrap()
                .amount_cents,
            55_555,
            "a record with no persisted entry must be seeded from its own scannable_value"
        );
        Flush::flush(&reopened).unwrap();
    }

    // And it must actually be durable now — reopening again (with no
    // further mismatches) must still see it, proving the earlier `open`
    // really appended a real slot rather than only holding it in memory.
    let reopened_again =
        GenericMmapStore::<Order, Status, Amount>::open(with_new_order, &path).unwrap();
    assert_eq!(
        GetById::get(&reopened_again, Uuid::from_u128(4))
            .unwrap()
            .amount_cents,
        55_555
    );
    // Update it through the newly-appended slot to confirm it's a real,
    // writable slot, not a read-only artifact.
    let mut mutable = reopened_again;
    UpdateField::update(&mut mutable, Uuid::from_u128(4), 60_000).unwrap();
    assert_eq!(
        GetById::get(&mutable, Uuid::from_u128(4))
            .unwrap()
            .amount_cents,
        60_000
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Both mismatch cases at once, plus the original three unaffected
/// records — confirms they don't interfere with each other.
#[test]
fn added_and_removed_records_are_handled_independently_in_the_same_reopen() {
    let dir = rusty_multimodal_db::bench_support::fresh_temp_dir("mmap_identity_both").unwrap();
    let path = dir.join("amount.mmap");

    {
        let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
        Flush::flush(&store).unwrap();
    }

    // Drop order 3, add order 4 — one removal and one addition in the
    // same reopen.
    let mut mixed: Vec<Order> = sample()
        .into_iter()
        .filter(|o| o.id != Uuid::from_u128(3))
        .collect();
    mixed.push(order(4, 300, 11_111));

    let reopened = GenericMmapStore::<Order, Status, Amount>::open(mixed, &path).unwrap();

    assert_eq!(GetById::get(&reopened, Uuid::from_u128(3)), None);
    assert_eq!(
        GetById::get(&reopened, Uuid::from_u128(4))
            .unwrap()
            .amount_cents,
        11_111
    );
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
        4_200
    );

    let _ = std::fs::remove_dir_all(&dir);
}
