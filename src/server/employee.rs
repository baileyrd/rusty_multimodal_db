//! [`ConnectionStore`] adapter wrapping
//! [`crate::generic::production::GenericProductionStore<EmployeeProductionStack>`]
//! for `Employee` — the third validation domain, and the first where
//! `Parent`/`Children` *and* `Neighbors` are all real (no domain-shaped
//! `ErrorCode::Unsupported`): `reports_to` (`ChildOf`, self-referential)
//! and `collaborates_with` (`SymmetricRelation`, self-referential) both
//! target `Employee` itself. See `crate::generic_spike::employee_impl`'s
//! own module doc comment for the real, load-bearing gap this combination
//! found and fixed directly in `crate::generic::{store,production}`
//! (`Reversed` never forwarded `Neighbors`; `GenericProductionStore` had
//! no `neighbors` method) before this adapter could even be written.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, RecordId,
    RelationCapabilities, ScanValue, TransactionOp, ValueKind,
};
use super::ConnectionStore;
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{GetById, UpdateField};
use crate::generic_spike::employee_impl::{
    CollaboratesWith, Department, DepartmentField, Employee, EmployeeProductionStack, ReportsTo,
    SalaryCents,
};
use std::path::Path;

pub const FIELD_NAME: FieldRef = 0;
pub const FIELD_DEPARTMENT: FieldRef = 1;
pub const FIELD_SALARY: FieldRef = 2;

/// `Department`'s wire encoding — a fixed discriminant, the same pattern
/// `server::order`'s `status_to_u32`/`status_from_u32` already established
/// for an enum `IndexedField`.
fn department_to_u32(department: Department) -> u32 {
    match department {
        Department::Engineering => 0,
        Department::Sales => 1,
        Department::Support => 2,
    }
}

fn department_from_u32(value: u32) -> Option<Department> {
    match value {
        0 => Some(Department::Engineering),
        1 => Some(Department::Sales),
        2 => Some(Department::Support),
        _ => None,
    }
}

pub struct EmployeeConnectionStore {
    store: GenericProductionStore<EmployeeProductionStack>,
    /// `JRN-FR-001` (ADR-0025) — see `DogConnectionStore::with_journal`.
    journal: Option<CommitGroup>,
}

impl EmployeeConnectionStore {
    pub fn new(store: GenericProductionStore<EmployeeProductionStack>) -> Self {
        Self {
            store,
            journal: None,
        }
    }

    /// The crash-atomic variant — see `DogConnectionStore::with_journal`
    /// for the contract; identical here.
    pub fn with_journal(
        store: GenericProductionStore<EmployeeProductionStack>,
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
    /// `Employee`'s only mutable field over this protocol is
    /// `salary_cents`. Safe under one continuously held lock: see
    /// `docs/design/SERVER-TRANSACTION-DESIGN.md`'s own "no runtime
    /// deletion" invariant.
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_SALARY, ScanValue::I64(_)) => {
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_SALARY, _) => return Err((i, ErrorCode::Malformed)),
                (FIELD_NAME | FIELD_DEPARTMENT, _) => return Err((i, ErrorCode::Unsupported)),
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    fn apply_batch(
        inner: &mut EmployeeProductionStack,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::I64(salary) = op.value {
                UpdateField::<Employee, SalaryCents>::update(inner, op.id, salary)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }
}

impl ConnectionStore for EmployeeConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Employee>(id).map(|employee| {
            vec![
                (FIELD_NAME, ScanValue::Str(employee.name)),
                (
                    FIELD_DEPARTMENT,
                    ScanValue::U32(department_to_u32(employee.department)),
                ),
                (FIELD_SALARY, ScanValue::I64(employee.salary_cents)),
            ]
        })
    }

    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        match (field, value) {
            (FIELD_DEPARTMENT, ScanValue::U32(raw)) => match department_from_u32(*raw) {
                Some(department) => Ok(self
                    .store
                    .filter_eq::<Employee, DepartmentField>(&department)),
                None => Err(ErrorCode::Malformed),
            },
            (FIELD_DEPARTMENT, _) => Err(ErrorCode::Malformed),
            (FIELD_NAME | FIELD_SALARY, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_SALARY => Ok(self
                .store
                .scan::<Employee, SalaryCents>()
                .into_iter()
                .map(ScanValue::I64)
                .collect()),
            FIELD_NAME | FIELD_DEPARTMENT => Err(ErrorCode::Unsupported),
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
            (FIELD_SALARY, ScanValue::I64(salary)) => {
                match self.store.update::<Employee, SalaryCents>(id, salary) {
                    Ok(()) => Ok(true),
                    Err(_not_found) => Ok(false),
                }
            }
            (FIELD_SALARY, _) => Err(ErrorCode::Malformed),
            (FIELD_NAME | FIELD_DEPARTMENT, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn parent(&self, id: RecordId) -> Result<ParentLookup, ErrorCode> {
        match self.store.parent::<Employee, ReportsTo>(id) {
            Ok(Some(manager_id)) => Ok(ParentLookup::Parent(manager_id)),
            Ok(None) => Ok(ParentLookup::NoParent),
            Err(_not_found) => Ok(ParentLookup::ChildNotFound),
        }
    }

    fn children(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(self.store.children::<Employee, Employee, ReportsTo>(id))
    }

    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(self.store.neighbors::<Employee, CollaboratesWith>(id))
    }

    /// `STV-FR-002`: `validate_batch` on this one operation, with the
    /// same per-call existence read the journaled path uses.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode> {
        Self::validate_batch(std::slice::from_ref(op), |id| {
            self.store.get::<Employee>(id).is_some()
        })
        .map_err(|(_, code)| code)
    }

    fn describe(&self) -> DomainSchema {
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: FIELD_NAME,
                    name: "name".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_DEPARTMENT,
                    name: "department".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_SALARY,
                    name: "salary_cents".into(),
                    value_kind: ValueKind::I64,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
            ],
            // The first domain where both are true — Dog has neighbors
            // only, Order/Customer has parent_children only.
            relations: RelationCapabilities {
                parent_children: true,
                neighbors: true,
            },
        }
    }

    fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)> {
        // See `DogConnectionStore::apply_transaction` for the two paths
        // (`GRP-FR-001`–`005`); identical here.
        match &self.journal {
            None => self.store.with_exclusive(|inner| {
                Self::validate_batch(updates, |id| GetById::<Employee>::get(inner, id).is_some())?;
                Self::apply_batch(inner, updates)
            }),
            Some(journal) => {
                Self::validate_batch(updates, |id| self.store.get::<Employee>(id).is_some())?;
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
    use crate::generic_spike::employee_impl::create_employee_production_stack;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> EmployeeConnectionStore {
        let dir = fresh_temp_dir("server_employee_adapter").unwrap();
        let path = dir.join("salary.mmap");
        let employees = vec![
            Employee {
                id: Uuid::from_u128(1),
                name: "Alex".into(),
                department: Department::Engineering,
                salary_cents: 1_200_000,
                manager_id: None,
            },
            Employee {
                id: Uuid::from_u128(2),
                name: "Bel".into(),
                department: Department::Engineering,
                salary_cents: 950_000,
                manager_id: Some(Uuid::from_u128(1)),
            },
            Employee {
                id: Uuid::from_u128(3),
                name: "Cas".into(),
                department: Department::Sales,
                salary_cents: 800_000,
                manager_id: Some(Uuid::from_u128(1)),
            },
        ];
        let edges = vec![(Uuid::from_u128(2), Uuid::from_u128(3))];
        let stack = create_employee_production_stack(employees, &edges, &path).unwrap();
        EmployeeConnectionStore::new(GenericProductionStore::new(stack))
    }

    #[test]
    fn get_returns_every_field() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.get(Uuid::from_u128(2)).unwrap(),
            vec![
                (FIELD_NAME, ScanValue::Str("Bel".into())),
                (FIELD_DEPARTMENT, ScanValue::U32(0)),
                (FIELD_SALARY, ScanValue::I64(950_000)),
            ]
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn filter_by_department_and_unsupported_fields() {
        let adapter = sample_adapter();
        let mut engineers = adapter
            .filter_eq(FIELD_DEPARTMENT, &ScanValue::U32(0))
            .unwrap();
        engineers.sort();
        assert_eq!(engineers, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        assert_eq!(
            adapter.filter_eq(FIELD_SALARY, &ScanValue::I64(0)),
            Err(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn scan_and_update_salary_only() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.update_field(Uuid::from_u128(2), FIELD_SALARY, ScanValue::I64(1_000_000)),
            Ok(true)
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(2)).unwrap()[2],
            (FIELD_SALARY, ScanValue::I64(1_000_000))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_SALARY, ScanValue::I64(1)),
            Ok(false)
        );
        assert_eq!(adapter.scan_field(FIELD_NAME), Err(ErrorCode::Unsupported));
    }

    #[test]
    fn parent_children_and_neighbors_are_all_real_for_the_first_time() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.parent(Uuid::from_u128(2)),
            Ok(ParentLookup::Parent(Uuid::from_u128(1)))
        );
        assert_eq!(
            adapter.parent(Uuid::from_u128(1)),
            Ok(ParentLookup::NoParent)
        );

        let mut reports = adapter.children(Uuid::from_u128(1)).unwrap();
        reports.sort();
        assert_eq!(reports, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);

        assert_eq!(
            adapter.neighbors(Uuid::from_u128(2)),
            Ok(vec![Uuid::from_u128(3)])
        );
    }

    #[test]
    fn describe_reports_both_relation_kinds_as_supported() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 3);
        assert!(schema.relations.parent_children);
        assert!(schema.relations.neighbors);
    }
}
