//! `Employee`: the third domain, purpose-built for one specific untested
//! combination this project's own status tracking had named explicitly —
//! `docs/PROJECT-STATUS.md`'s "a third, structurally different domain
//! (e.g. one needing `SymmetricRelation` *and* `ChildOf` together)" —
//! rather than a domain motivated by an external reference shape the way
//! `Order`/`Customer` (a real e-commerce shape) or `Rule`/`RuleRelation`
//! (a real requirements-traceability shape) were. `Dog` has only
//! `SymmetricRelation` (`littermate_of`); `Order`/`Customer` has only
//! `ChildOf` (`belongs_to`); neither combination was ever exercised on
//! one record type before this spike, at either the in-memory or the
//! durable-production layer.
//!
//! `reports_to` (`ChildOf<ReportsTo>`, self-referential — an employee's
//! manager is another employee, and a manager at the top has none) and
//! `collaborates_with` (`SymmetricRelation<CollaboratesWith>`,
//! self-referential — a peer-to-peer edge, the same shape as
//! `littermate_of`) both target `Employee` itself, so `R = P = C =
//! Employee` throughout — the specific shape that surfaced a real, load-
//! bearing gap: [`super::super::generic::store::Reversed`] never forwarded
//! [`super::super::generic::query::Neighbors`] (nothing had ever stacked a
//! `Symmetric` layer underneath a `Reversed` one before), and
//! [`super::super::generic::production::GenericProductionStore`] had no
//! `neighbors` method at all (no domain wrapped in it had ever needed
//! one). Both fixed in `crate::generic::{store,production}` directly, not
//! worked around here — see those modules' own doc comments.

use crate::generic::mmap_store::GenericMmapStore;
use crate::generic::store::{BaseStore, Indexed, Reversed, Scanned, Symmetric};
use crate::generic::traits::{ChildOf, IndexedField, Record, ScannableField, SymmetricRelation};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Department {
    Engineering,
    Sales,
    Support,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Employee {
    pub id: Uuid,
    pub name: String,
    pub department: Department,
    pub salary_cents: i64,
    /// `None` for whoever sits at the top of the reporting chain — the
    /// same optional-parent shape `Rule`'s tree root already established
    /// (`crate::generic_spike::rule_trace`), reused here rather than
    /// reinvented.
    pub manager_id: Option<Uuid>,
}

impl Record for Employee {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

pub struct DepartmentField;
impl IndexedField<DepartmentField> for Employee {
    type IndexValue = Department;
    fn indexed_value(&self) -> &Department {
        &self.department
    }
}

pub struct SalaryCents;
impl ScannableField<SalaryCents> for Employee {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.salary_cents
    }
    fn set_scannable_value(&mut self, value: i64) {
        self.salary_cents = value;
    }
}

/// The directed relation: an employee reports to their manager, another
/// `Employee`. Self-referential, unlike `Order belongs_to Customer`'s
/// two-type shape — the same self-referential pattern `Rule`'s
/// `chain_to_root` already used, but paired here with a real symmetric
/// relation on the same type, which `Rule` was not.
pub struct ReportsTo;
impl ChildOf<ReportsTo> for Employee {
    type ParentId = Uuid;
    fn parent_id(&self) -> Option<Uuid> {
        self.manager_id
    }
}

/// The symmetric relation: two employees who collaborate, the same
/// undirected-edge shape as `littermate_of` (`crate::generic_spike::dog_impl`).
pub struct CollaboratesWith;
impl SymmetricRelation<CollaboratesWith> for Employee {}

/// The full in-memory composed stack: `BaseStore` -> `Indexed<..,
/// DepartmentField>` -> `Scanned<.., SalaryCents>` -> `Symmetric<..,
/// CollaboratesWith>` -> `Reversed<.., Employee, Employee, ReportsTo>`.
/// See [`EmployeeProductionStack`] for the durable analogue.
pub type EmployeeGenericStore = Reversed<
    Symmetric<
        Scanned<Indexed<BaseStore<Employee>, Employee, DepartmentField>, Employee, SalaryCents>,
        Employee,
        CollaboratesWith,
    >,
    Employee,
    Employee,
    ReportsTo,
>;

pub fn build_employee_generic_store(
    employees: &[Employee],
    collaboration_edges: &[(Uuid, Uuid)],
) -> EmployeeGenericStore {
    let base = BaseStore::new(employees.to_vec());
    let indexed = Indexed::<_, Employee, DepartmentField>::new(base, employees);
    let scanned = Scanned::<_, Employee, SalaryCents>::new(indexed, employees);
    let symmetric = Symmetric::<_, Employee, CollaboratesWith>::new(scanned, collaboration_edges);
    Reversed::<_, Employee, Employee, ReportsTo>::new(symmetric, employees)
}

/// The durable production stack: [`GenericMmapStore`] (owns records, the
/// `DepartmentField` index, and `SalaryCents` — the one mmap-backed
/// durable field) -> `Symmetric<.., CollaboratesWith>` (in-memory,
/// rebuilt from the caller-supplied edges at every `open`, same
/// convention every relation layer in this crate already follows) ->
/// `Reversed<.., Employee, Employee, ReportsTo>`. The `Reversed`-outside-
/// `Symmetric` ordering is deliberate, not arbitrary: it's what requires
/// `Reversed` to forward `Neighbors` (the gap this spike found) rather
/// than the reverse ordering, which would have needed `Symmetric` to
/// forward `Children` instead — either ordering surfaces a real gap;
/// this one was fixed first because it matches `EmployeeGenericStore`'s
/// own layer order above.
pub type EmployeeProductionStack = Reversed<
    Symmetric<GenericMmapStore<Employee, DepartmentField, SalaryCents>, Employee, CollaboratesWith>,
    Employee,
    Employee,
    ReportsTo,
>;

/// Build a fresh, durable production store for `Employee` at `path` — the
/// generic analogue of `ProductionStore::create`/`create_order_production_stack`.
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::create`] does.
pub fn create_employee_production_stack(
    employees: Vec<Employee>,
    collaboration_edges: &[(Uuid, Uuid)],
    path: &std::path::Path,
) -> Result<EmployeeProductionStack, crate::durability::DurabilityError> {
    let core = GenericMmapStore::<Employee, DepartmentField, SalaryCents>::create(
        employees.clone(),
        path,
    )?;
    let symmetric = Symmetric::<_, Employee, CollaboratesWith>::new(core, collaboration_edges);
    Ok(Reversed::<_, Employee, Employee, ReportsTo>::new(
        symmetric, &employees,
    ))
}

/// Reopen an existing durable production store for `Employee` at `path` —
/// the generic analogue of `ProductionStore::open`/`open_order_production_stack`.
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::open`] does.
pub fn open_employee_production_stack(
    employees: Vec<Employee>,
    collaboration_edges: &[(Uuid, Uuid)],
    path: &std::path::Path,
) -> Result<EmployeeProductionStack, crate::durability::DurabilityError> {
    let core =
        GenericMmapStore::<Employee, DepartmentField, SalaryCents>::open(employees.clone(), path)?;
    let symmetric = Symmetric::<_, Employee, CollaboratesWith>::new(core, collaboration_edges);
    Ok(Reversed::<_, Employee, Employee, ReportsTo>::new(
        symmetric, &employees,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::production::GenericProductionStore;
    use crate::generic::query::{Children, FilterEq, GetById, Neighbors, Parent};

    fn sample() -> Vec<Employee> {
        vec![
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
        ]
    }

    fn sample_collaboration_edges() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(2), Uuid::from_u128(3))]
    }

    #[test]
    fn in_memory_stack_supports_both_relations_and_the_scannable_field() {
        let store = build_employee_generic_store(&sample(), &sample_collaboration_edges());

        assert_eq!(
            GetById::<Employee>::get(&store, Uuid::from_u128(2))
                .unwrap()
                .salary_cents,
            950_000
        );
        assert_eq!(
            FilterEq::<Employee, DepartmentField>::filter_eq(&store, &Department::Engineering)
                .len(),
            2
        );
        assert_eq!(
            Parent::<Employee, ReportsTo>::parent(&store, Uuid::from_u128(2)),
            Ok(Some(Uuid::from_u128(1)))
        );
        assert_eq!(
            Parent::<Employee, ReportsTo>::parent(&store, Uuid::from_u128(1)),
            Ok(None)
        );
        let mut reports =
            Children::<Employee, Employee, ReportsTo>::children(&store, Uuid::from_u128(1));
        reports.sort();
        assert_eq!(reports, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        assert_eq!(
            Neighbors::<Employee, CollaboratesWith>::neighbors(&store, Uuid::from_u128(2)),
            vec![Uuid::from_u128(3)]
        );
    }

    #[test]
    fn durable_production_stack_supports_both_relations_together() {
        let dir = crate::bench_support::fresh_temp_dir("employee_production_basic").unwrap();
        let path = dir.join("salary.mmap");
        let stack =
            create_employee_production_stack(sample(), &sample_collaboration_edges(), &path)
                .unwrap();
        let store = GenericProductionStore::new(stack);

        assert_eq!(
            store
                .get::<Employee>(Uuid::from_u128(2))
                .unwrap()
                .salary_cents,
            950_000
        );
        assert_eq!(
            store
                .filter_eq::<Employee, DepartmentField>(&Department::Engineering)
                .len(),
            2
        );

        store
            .update::<Employee, SalaryCents>(Uuid::from_u128(2), 1_000_000)
            .unwrap();
        assert_eq!(
            store
                .get::<Employee>(Uuid::from_u128(2))
                .unwrap()
                .salary_cents,
            1_000_000
        );

        assert_eq!(
            store.parent::<Employee, ReportsTo>(Uuid::from_u128(2)),
            Ok(Some(Uuid::from_u128(1)))
        );
        let mut reports = store.children::<Employee, Employee, ReportsTo>(Uuid::from_u128(1));
        reports.sort();
        assert_eq!(reports, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);

        // The gap this spike found and fixed: neighbors() through a
        // Reversed-outermost stack, requiring the new forwarding impl in
        // crate::generic::store plus the new inherent method on
        // GenericProductionStore.
        assert_eq!(
            store.neighbors::<Employee, CollaboratesWith>(Uuid::from_u128(2)),
            vec![Uuid::from_u128(3)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_then_reopen_sees_the_written_value_and_both_relations() {
        let dir = crate::bench_support::fresh_temp_dir("employee_production_roundtrip").unwrap();
        let path = dir.join("salary.mmap");

        {
            let stack =
                create_employee_production_stack(sample(), &sample_collaboration_edges(), &path)
                    .unwrap();
            let store = GenericProductionStore::new(stack);
            store
                .update::<Employee, SalaryCents>(Uuid::from_u128(3), 850_000)
                .unwrap();
            store.flush().unwrap();
        }

        let reopened_stack =
            open_employee_production_stack(sample(), &sample_collaboration_edges(), &path).unwrap();
        let reopened = GenericProductionStore::new(reopened_stack);
        assert_eq!(
            reopened
                .get::<Employee>(Uuid::from_u128(3))
                .unwrap()
                .salary_cents,
            850_000
        );
        assert_eq!(
            reopened.neighbors::<Employee, CollaboratesWith>(Uuid::from_u128(3)),
            vec![Uuid::from_u128(2)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
