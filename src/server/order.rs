//! [`ConnectionStore`] adapter wrapping
//! [`crate::generic::production::GenericProductionStore<OrderProductionStack>`]
//! for `Order`/`Customer` — the second validation domain (a real directed
//! relation, via `Parent`/`Children`; no symmetric relation, so
//! `Neighbors` is unsupported here — the complementary case to
//! [`super::dog`]). Behind the `research` feature: `order_customer` itself
//! is research-gated reference material (see `crate::generic`'s own module
//! docs), so validating the server against it needs both `server` and
//! `research` enabled together.
//!
//! # `OrderProductionStack` only durably tracks `Status`/`Amount`
//!
//! `Order` has three `ScannableField`s in-memory (`Amount`, `CreatedAt`,
//! `DiscountCents`), but [`OrderProductionStack`] (the *durable*
//! production stack this adapter wraps) only carries `Status` (indexed)
//! and `Amount` (the one mmap-backed field) — `CreatedAt`/`DiscountCents`
//! aren't part of it at all (see `order_customer`'s own module docs on
//! why exactly one field is durable). So `ScanField`/`UpdateField` only
//! ever support `Amount` here; `GetById` still returns every field
//! (reconstructed from the caller-supplied records, the same write-through
//! guarantee every durable variant in this crate provides) since a
//! full-record read doesn't need a field to be independently scannable.
//!
//! # `Parent`/`Children` take differently-typed ids
//!
//! `Parent`'s `id` is an `Order` id (every order has exactly one
//! customer); `Children`'s `id` is a `Customer` id (a customer has zero or
//! more orders) — matching the real, asymmetric shape of a directed
//! relation (`docs/design/GENERIC-SCHEMA-DESIGN.md` §4.3), not a
//! convenience this adapter invents.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, RecordId,
    RelationCapabilities, ScanValue, TransactionOp, ValueKind,
};
use super::ConnectionStore;
use crate::generic::order_customer::{
    Amount, BelongsToCustomer, Customer, Order, OrderProductionStack, OrderStatus, Status,
};
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{GetById, UpdateField};
use std::path::Path;

pub const FIELD_AMOUNT: FieldRef = 0;
pub const FIELD_STATUS: FieldRef = 1;
pub const FIELD_CREATED_AT: FieldRef = 2;
pub const FIELD_DISCOUNT: FieldRef = 3;

/// `OrderStatus`'s wire encoding — a fixed discriminant, not `OrderStatus`
/// itself (the protocol's [`ScanValue`] enum stays domain-agnostic; this
/// mapping is this adapter's own concern, the same way the field tags
/// themselves are).
fn status_to_u32(status: OrderStatus) -> u32 {
    match status {
        OrderStatus::Pending => 0,
        OrderStatus::Shipped => 1,
        OrderStatus::Delivered => 2,
        OrderStatus::Cancelled => 3,
        OrderStatus::Refunded => 4,
    }
}

fn status_from_u32(value: u32) -> Option<OrderStatus> {
    match value {
        0 => Some(OrderStatus::Pending),
        1 => Some(OrderStatus::Shipped),
        2 => Some(OrderStatus::Delivered),
        3 => Some(OrderStatus::Cancelled),
        4 => Some(OrderStatus::Refunded),
        _ => None,
    }
}

pub struct OrderConnectionStore {
    store: GenericProductionStore<OrderProductionStack>,
    /// `JRN-FR-001` (ADR-0025) — see `DogConnectionStore::with_journal`.
    journal: Option<CommitGroup>,
}

impl OrderConnectionStore {
    pub fn new(store: GenericProductionStore<OrderProductionStack>) -> Self {
        Self {
            store,
            journal: None,
        }
    }

    /// The crash-atomic variant — see `DogConnectionStore::with_journal`
    /// for the contract; identical here.
    pub fn with_journal(
        store: GenericProductionStore<OrderProductionStack>,
        journal_path: &Path,
    ) -> Result<Self, JournalError> {
        let (journal, batches) = CommitGroup::open(journal_path)?;
        store.with_exclusive(|inner| -> Result<(), JournalError> {
            for (batch_index, batch) in batches.iter().enumerate() {
                Self::apply_batch(inner, batch).map_err(|(index, code)| JournalError::Replay {
                    batch: batch_index,
                    index,
                    code,
                })?;
            }
            inner.checkpoint_flush()?;
            journal.truncate()
        })?;
        Ok(Self {
            store,
            journal: Some(journal),
        })
    }

    /// Same validate-then-apply shape `server::dog`'s own uses —
    /// `Order`'s only mutable field over this protocol is `amount_cents`.
    /// Safe under one continuously held lock: see
    /// `docs/design/SERVER-TRANSACTION-DESIGN.md`'s own "no runtime
    /// deletion" invariant.
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_AMOUNT, ScanValue::I64(_)) => {
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_AMOUNT, _) => return Err((i, ErrorCode::Malformed)),
                (FIELD_STATUS | FIELD_CREATED_AT | FIELD_DISCOUNT, _) => {
                    return Err((i, ErrorCode::Unsupported))
                }
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    fn apply_batch(
        inner: &mut OrderProductionStack,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::I64(amount) = op.value {
                UpdateField::<Order, Amount>::update(inner, op.id, amount)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }
}

impl ConnectionStore for OrderConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Order>(id).map(|order| {
            vec![
                (FIELD_AMOUNT, ScanValue::I64(order.amount_cents)),
                (FIELD_STATUS, ScanValue::U32(status_to_u32(order.status))),
                (FIELD_CREATED_AT, ScanValue::I64(order.created_at_unix_ms)),
                (FIELD_DISCOUNT, ScanValue::I64(order.discount_cents)),
            ]
        })
    }

    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        match (field, value) {
            (FIELD_STATUS, ScanValue::U32(raw)) => match status_from_u32(*raw) {
                Some(status) => Ok(self.store.filter_eq::<Order, Status>(&status)),
                None => Err(ErrorCode::Malformed),
            },
            (FIELD_STATUS, _) => Err(ErrorCode::Malformed),
            (FIELD_AMOUNT | FIELD_CREATED_AT | FIELD_DISCOUNT, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_AMOUNT => Ok(self
                .store
                .scan::<Order, Amount>()
                .into_iter()
                .map(ScanValue::I64)
                .collect()),
            FIELD_STATUS | FIELD_CREATED_AT | FIELD_DISCOUNT => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn update_field(
        &self,
        id: RecordId,
        field: FieldRef,
        value: ScanValue,
    ) -> Result<bool, ErrorCode> {
        match (field, value) {
            (FIELD_AMOUNT, ScanValue::I64(amount)) => {
                match self.store.update::<Order, Amount>(id, amount) {
                    Ok(()) => Ok(true),
                    Err(_not_found) => Ok(false),
                }
            }
            (FIELD_AMOUNT, _) => Err(ErrorCode::Malformed),
            (FIELD_STATUS | FIELD_CREATED_AT | FIELD_DISCOUNT, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn parent(&self, id: RecordId) -> Result<ParentLookup, ErrorCode> {
        match self.store.parent::<Order, BelongsToCustomer>(id) {
            Ok(Some(customer_id)) => Ok(ParentLookup::Parent(customer_id)),
            Ok(None) => Ok(ParentLookup::NoParent),
            Err(_not_found) => Ok(ParentLookup::ChildNotFound),
        }
    }

    fn children(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(self
            .store
            .children::<Customer, Order, BelongsToCustomer>(id))
    }

    fn neighbors(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        // Order/Customer has no SymmetricRelation.
        Err(ErrorCode::Unsupported)
    }

    /// `STV-FR-002`: `validate_batch` on this one operation, with the
    /// same per-call existence read the journaled path uses.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode> {
        Self::validate_batch(std::slice::from_ref(op), |id| {
            self.store.get::<Order>(id).is_some()
        })
        .map_err(|(_, code)| code)
    }

    fn describe(&self) -> DomainSchema {
        let read_only = |value_kind: ValueKind, name: &str, tag: FieldRef| FieldDescriptor {
            tag,
            name: name.into(),
            value_kind,
            capabilities: FieldCapabilities {
                filter_eq: false,
                scan: false,
                update: false,
            },
        };
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: FIELD_AMOUNT,
                    name: "amount_cents".into(),
                    value_kind: ValueKind::I64,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_STATUS,
                    name: "status".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                read_only(ValueKind::I64, "created_at_unix_ms", FIELD_CREATED_AT),
                read_only(ValueKind::I64, "discount_cents", FIELD_DISCOUNT),
            ],
            relations: RelationCapabilities {
                parent_children: true,
                neighbors: false,
            },
        }
    }

    fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)> {
        // See `DogConnectionStore::apply_transaction` for the two paths
        // (`GRP-FR-001`–`005`); identical here.
        match &self.journal {
            None => self.store.with_exclusive(|inner| {
                Self::validate_batch(updates, |id| GetById::<Order>::get(inner, id).is_some())?;
                Self::apply_batch(inner, updates)
            }),
            Some(journal) => {
                Self::validate_batch(updates, |id| self.store.get::<Order>(id).is_some())?;
                journal
                    .commit(updates, |turn| {
                        self.store.with_exclusive(|inner| {
                            Self::apply_batch(inner, updates)?;
                            Ok(turn.checkpoint_due && inner.checkpoint_flush().is_ok())
                        })
                    })
                    .map_err(|e| match e {
                        CommitError::Journal(_) => (0, ErrorCode::Journal),
                        CommitError::Apply(e) => e,
                    })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::order_customer::{create_order_production_stack, OrderStatus};
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> OrderConnectionStore {
        let dir = fresh_temp_dir("server_order_adapter").unwrap();
        let path = dir.join("amount.mmap");
        let orders = vec![
            crate::generic::order_customer::Order {
                id: Uuid::from_u128(1),
                customer_id: Uuid::from_u128(100),
                amount_cents: 2_500,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 1_000,
                discount_cents: 0,
            },
            crate::generic::order_customer::Order {
                id: Uuid::from_u128(2),
                customer_id: Uuid::from_u128(100),
                amount_cents: 4_200,
                status: OrderStatus::Pending,
                created_at_unix_ms: 2_000,
                discount_cents: 0,
            },
        ];
        let stack = create_order_production_stack(orders, &path).unwrap();
        OrderConnectionStore::new(GenericProductionStore::new(stack))
    }

    #[test]
    fn get_returns_every_field() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap(),
            vec![
                (FIELD_AMOUNT, ScanValue::I64(2_500)),
                (FIELD_STATUS, ScanValue::U32(1)),
                (FIELD_CREATED_AT, ScanValue::I64(1_000)),
                (FIELD_DISCOUNT, ScanValue::I64(0)),
            ]
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn filter_eq_by_status_and_unsupported_fields() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.filter_eq(FIELD_STATUS, &ScanValue::U32(1)),
            Ok(vec![Uuid::from_u128(1)])
        );
        assert_eq!(
            adapter.filter_eq(FIELD_AMOUNT, &ScanValue::I64(0)),
            Err(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn scan_and_update_amount_only() {
        let adapter = sample_adapter();
        let mut amounts = adapter.scan_field(FIELD_AMOUNT).unwrap();
        amounts.sort_by_key(|v| match v {
            ScanValue::I64(n) => *n,
            _ => 0,
        });
        assert_eq!(amounts, vec![ScanValue::I64(2_500), ScanValue::I64(4_200)]);
        assert_eq!(
            adapter.scan_field(FIELD_DISCOUNT),
            Err(ErrorCode::Unsupported)
        );

        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_AMOUNT, ScanValue::I64(9_000)),
            Ok(true)
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap()[0],
            (FIELD_AMOUNT, ScanValue::I64(9_000))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_AMOUNT, ScanValue::I64(1)),
            Ok(false)
        );
    }

    #[test]
    fn parent_and_children_reflect_belongs_to_customer() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.parent(Uuid::from_u128(1)),
            Ok(ParentLookup::Parent(Uuid::from_u128(100)))
        );
        assert_eq!(
            adapter.parent(Uuid::from_u128(99)),
            Ok(ParentLookup::ChildNotFound)
        );

        let mut children = adapter.children(Uuid::from_u128(100)).unwrap();
        children.sort();
        assert_eq!(children, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
    }

    #[test]
    fn describe_names_all_four_fields_and_reports_parent_children_only() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 4);
        let amount = schema
            .fields
            .iter()
            .find(|f| f.name == "amount_cents")
            .unwrap();
        assert!(
            amount.capabilities.scan
                && amount.capabilities.update
                && !amount.capabilities.filter_eq
        );
        let status = schema.fields.iter().find(|f| f.name == "status").unwrap();
        assert!(
            status.capabilities.filter_eq
                && !status.capabilities.scan
                && !status.capabilities.update
        );
        for read_only_name in ["created_at_unix_ms", "discount_cents"] {
            let field = schema
                .fields
                .iter()
                .find(|f| f.name == read_only_name)
                .unwrap();
            assert!(
                !field.capabilities.filter_eq
                    && !field.capabilities.scan
                    && !field.capabilities.update
            );
        }
        assert!(schema.relations.parent_children);
        assert!(!schema.relations.neighbors);
    }

    #[test]
    fn neighbors_is_unsupported() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.neighbors(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
    }
}
