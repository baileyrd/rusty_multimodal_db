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

use crate::generic::edge_blob::edges_path;
use crate::generic::mmap_store::GenericMmapStore;
use crate::generic::store::{BaseStore, Indexed, Reversed, Scanned, Symmetric};
use crate::generic::traits::{
    ChildOf, IndexedField, Record, ScannableField, SchemaTag, SymmetricRelation,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// `Serialize`/`Deserialize`: required by `GenericMmapStore`'s companion
// record blob (`STORAGE-015-FR-006`); nothing else about them changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Department {
    Engineering,
    Sales,
    Support,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// The name written into every `Employee` companion blob's header —
// `<path>.records` and `<path>.edges` alike (`SCHTAG-FR-007`). Part of
// the on-disk format; see `Order`'s impl for the same caveat.
impl SchemaTag for Employee {
    const SCHEMA_TAG: &'static str = "employee::Employee";
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
/// durable field) -> `Symmetric<.., CollaboratesWith>` (in-memory
/// adjacency, rebuilt from the caller-supplied edges at every `open`;
/// since `STORAGE-016` the edge list itself is also persisted to
/// `<path>.edges` so the stack can be rebuilt from its files alone — see
/// [`open_employee_production_stack_portable`]) ->
/// `Reversed<.., Employee, Employee, ReportsTo>` (rebuilt from the
/// records, as every `Reversed` layer is). The `Reversed`-outside-
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
/// Writes three files: `path` (the mmap file), `<path>.records` (the
/// record blob, via [`GenericMmapStore::create`]), and `<path>.edges`
/// (the collaboration edge list, via `Symmetric::create` —
/// `SYMPORT-FR-007`). All three must travel together for
/// [`open_employee_production_stack_portable`].
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::create`] does, or if the edge blob
/// can't be written; [`crate::durability::DurabilityError::Serde`] if
/// the records or edges can't be serialized.
pub fn create_employee_production_stack(
    employees: Vec<Employee>,
    collaboration_edges: &[(Uuid, Uuid)],
    path: &std::path::Path,
) -> Result<EmployeeProductionStack, crate::durability::DurabilityError> {
    let core = GenericMmapStore::<Employee, DepartmentField, SalaryCents>::create(
        employees.clone(),
        path,
    )?;
    let symmetric = Symmetric::<_, Employee, CollaboratesWith>::create(
        core,
        collaboration_edges,
        &edges_path(path),
    )?;
    Ok(Reversed::<_, Employee, Employee, ReportsTo>::new(
        symmetric, &employees,
    ))
}

/// Reopen an existing durable production store for `Employee` at `path` —
/// the generic analogue of `ProductionStore::open`/`open_order_production_stack`.
/// Keeps both companions current with the caller's arguments: the record
/// blob via [`GenericMmapStore::open`], the edge blob via
/// `Symmetric::open` — each rewritten only when its content changed, and
/// a pre-`STORAGE-016` directory (no `<path>.edges` yet) gains the file
/// on this call (`SYMPORT-FR-004`/`FR-007`).
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::open`] does, or if a stale edge blob
/// can't be rewritten; [`crate::durability::DurabilityError::Serde`] if
/// a stale companion's content can't be serialized.
pub fn open_employee_production_stack(
    employees: Vec<Employee>,
    collaboration_edges: &[(Uuid, Uuid)],
    path: &std::path::Path,
) -> Result<EmployeeProductionStack, crate::durability::DurabilityError> {
    let core =
        GenericMmapStore::<Employee, DepartmentField, SalaryCents>::open(employees.clone(), path)?;
    let symmetric = Symmetric::<_, Employee, CollaboratesWith>::open(
        core,
        collaboration_edges,
        &edges_path(path),
    )?;
    Ok(Reversed::<_, Employee, Employee, ReportsTo>::new(
        symmetric, &employees,
    ))
}

/// Reopen the whole stack from its three files alone — `path`,
/// `<path>.records`, and `<path>.edges` — with no `employees` or
/// `collaboration_edges` argument (`SYMPORT-FR-007`). The record blob is
/// read once and serves both [`GenericMmapStore::open`] and the
/// `Reversed` layer (as `open_order_production_stack_portable` does); the
/// edge blob is read once by `Symmetric::open_portable`. Because both
/// companions' content comes from the files themselves, neither currency
/// check finds anything stale and nothing is rewritten. `neighbors`
/// results, order included, match the stack the files were written from
/// (`SYMPORT-FR-008`).
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::RecordBlobUnreadable`]
/// naming whichever companion is missing or invalid — `<path>.records`
/// from [`GenericMmapStore::read_portable_records`], `<path>.edges` from
/// `Symmetric::read_portable_edges` — so a directory copied without its
/// `.edges` file is a typed error naming that file, never a stack with
/// silently empty adjacency; [`open_employee_production_stack`] on the
/// same directory heals it. Otherwise everything
/// [`GenericMmapStore::open`] can return.
pub fn open_employee_production_stack_portable(
    path: &std::path::Path,
) -> Result<EmployeeProductionStack, crate::durability::DurabilityError> {
    let employees =
        GenericMmapStore::<Employee, DepartmentField, SalaryCents>::read_portable_records(path)?;
    let core =
        GenericMmapStore::<Employee, DepartmentField, SalaryCents>::open(employees.clone(), path)?;
    let symmetric =
        Symmetric::<_, Employee, CollaboratesWith>::open_portable(core, &edges_path(path))?;
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

    // ---- STORAGE-016: edge-list portability, one test per acceptance
    // criterion in docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md ----

    /// A richer edge list than `sample_collaboration_edges`, so that
    /// per-id `neighbors` order (which follows edge order) is actually
    /// observable: 2 sees [3, 1], 1 sees [2, 3].
    fn portability_edges() -> Vec<(Uuid, Uuid)> {
        vec![
            (Uuid::from_u128(2), Uuid::from_u128(3)),
            (Uuid::from_u128(1), Uuid::from_u128(2)),
            (Uuid::from_u128(1), Uuid::from_u128(3)),
        ]
    }

    /// Every query the acceptance criterion names, in one comparable
    /// snapshot — including `neighbors` per id, order preserved.
    #[derive(Debug, PartialEq)]
    struct Snapshot {
        records: Vec<Option<Employee>>,
        engineering: Vec<Uuid>,
        salaries: Vec<i64>,
        parents: Vec<Result<Option<Uuid>, crate::generic::NotFound<Uuid>>>,
        reports_to_1: Vec<Uuid>,
        neighbors: Vec<Vec<Uuid>>,
    }

    fn snapshot(store: &GenericProductionStore<EmployeeProductionStack>) -> Snapshot {
        let ids: Vec<Uuid> = (1..=3).map(Uuid::from_u128).collect();
        let mut engineering =
            store.filter_eq::<Employee, DepartmentField>(&Department::Engineering);
        engineering.sort();
        let mut reports_to_1 = store.children::<Employee, Employee, ReportsTo>(Uuid::from_u128(1));
        reports_to_1.sort();
        Snapshot {
            records: ids.iter().map(|&id| store.get::<Employee>(id)).collect(),
            engineering,
            salaries: store.scan::<Employee, SalaryCents>(),
            parents: ids
                .iter()
                .map(|&id| store.parent::<Employee, ReportsTo>(id))
                .collect(),
            reports_to_1,
            neighbors: ids
                .iter()
                .map(|&id| store.neighbors::<Employee, CollaboratesWith>(id))
                .collect(),
        }
    }

    #[test]
    fn portable_reopen_answers_every_query_identically_with_no_arguments() {
        let dir = crate::bench_support::fresh_temp_dir("employee_portable_identical").unwrap();
        let path = dir.join("salary.mmap");
        let expected = {
            let stack =
                create_employee_production_stack(sample(), &portability_edges(), &path).unwrap();
            let store = GenericProductionStore::new(stack);
            store
                .update::<Employee, SalaryCents>(Uuid::from_u128(2), 1_000_000)
                .unwrap();
            store.flush().unwrap();
            snapshot(&store)
        };
        assert_eq!(
            expected.neighbors,
            vec![
                vec![Uuid::from_u128(2), Uuid::from_u128(3)],
                vec![Uuid::from_u128(3), Uuid::from_u128(1)],
                vec![Uuid::from_u128(2), Uuid::from_u128(1)],
            ],
            "the fixture must make edge order observable"
        );

        let portable =
            GenericProductionStore::new(open_employee_production_stack_portable(&path).unwrap());
        let actual = snapshot(&portable);
        assert_eq!(actual, expected);
        // And the portable stack is fully functional, not read-only.
        portable
            .update::<Employee, SalaryCents>(Uuid::from_u128(1), 1_250_000)
            .unwrap();
        assert_eq!(
            portable
                .get::<Employee>(Uuid::from_u128(1))
                .unwrap()
                .salary_cents,
            1_250_000
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_three_files_copied_together_reopen_elsewhere() {
        let dir = crate::bench_support::fresh_temp_dir("employee_portable_copy_src").unwrap();
        let copied_dir =
            crate::bench_support::fresh_temp_dir("employee_portable_copy_dst").unwrap();
        let path = dir.join("salary.mmap");
        let copied = copied_dir.join("elsewhere.mmap");
        let expected = {
            let stack =
                create_employee_production_stack(sample(), &portability_edges(), &path).unwrap();
            let store = GenericProductionStore::new(stack);
            store.flush().unwrap();
            snapshot(&store)
        };

        std::fs::copy(&path, &copied).unwrap();
        std::fs::copy(
            crate::generic::record_blob::blob_path(&path),
            crate::generic::record_blob::blob_path(&copied),
        )
        .unwrap();
        std::fs::copy(edges_path(&path), edges_path(&copied)).unwrap();

        let portable =
            GenericProductionStore::new(open_employee_production_stack_portable(&copied).unwrap());
        assert_eq!(snapshot(&portable), expected);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&copied_dir);
    }

    #[test]
    fn a_missing_edges_file_is_a_typed_error_that_the_existing_open_heals() {
        let dir = crate::bench_support::fresh_temp_dir("employee_portable_missing_edges").unwrap();
        let path = dir.join("salary.mmap");
        let expected = {
            let stack =
                create_employee_production_stack(sample(), &portability_edges(), &path).unwrap();
            let store = GenericProductionStore::new(stack);
            store.flush().unwrap();
            snapshot(&store)
        };
        let edge_file = edges_path(&path);
        std::fs::remove_file(&edge_file).unwrap();

        // The pre-STORAGE-016 directory shape: mmap + records, no edges.
        match open_employee_production_stack_portable(&path) {
            Err(crate::durability::DurabilityError::RecordBlobUnreadable { path: p, cause }) => {
                assert_eq!(
                    p, edge_file,
                    "the error must name the edge blob, not the records"
                );
                assert!(cause.starts_with("cannot read file"), "{cause}");
            }
            Err(other) => panic!("expected RecordBlobUnreadable, got {other:?}"),
            Ok(_) => panic!("expected RecordBlobUnreadable, got a stack"),
        }

        let healed = GenericProductionStore::new(
            open_employee_production_stack(sample(), &portability_edges(), &path).unwrap(),
        );
        assert_eq!(snapshot(&healed), expected);
        assert!(edge_file.is_file(), "open must write the missing edge blob");
        let portable =
            GenericProductionStore::new(open_employee_production_stack_portable(&path).unwrap());
        assert_eq!(snapshot(&portable), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_rewrites_the_edge_blob_only_when_the_edge_list_changed() {
        let dir = crate::bench_support::fresh_temp_dir("employee_portable_rewrite").unwrap();
        let path = dir.join("salary.mmap");
        let _ = create_employee_production_stack(sample(), &portability_edges(), &path).unwrap();
        let edge_file = edges_path(&path);
        let bytes_before = std::fs::read(&edge_file).unwrap();
        let mtime_before = std::fs::metadata(&edge_file).unwrap().modified().unwrap();

        // Same edges, same order: nothing written.
        let _ = open_employee_production_stack(sample(), &portability_edges(), &path).unwrap();
        assert_eq!(std::fs::read(&edge_file).unwrap(), bytes_before);
        assert_eq!(
            std::fs::metadata(&edge_file).unwrap().modified().unwrap(),
            mtime_before
        );

        // Same edges, different order: counts as changed and is
        // observable through neighbors after a portable reopen.
        let mut reordered = portability_edges();
        reordered.reverse();
        let _ = open_employee_production_stack(sample(), &reordered, &path).unwrap();
        assert_ne!(std::fs::read(&edge_file).unwrap(), bytes_before);
        let portable =
            GenericProductionStore::new(open_employee_production_stack_portable(&path).unwrap());
        assert_eq!(
            portable.neighbors::<Employee, CollaboratesWith>(Uuid::from_u128(1)),
            vec![Uuid::from_u128(3), Uuid::from_u128(2)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_blob_at_the_edges_path_is_a_magic_error_not_a_decode_attempt() {
        let dir = crate::bench_support::fresh_temp_dir("employee_portable_wrong_blob").unwrap();
        let path = dir.join("salary.mmap");
        let _ = create_employee_production_stack(sample(), &portability_edges(), &path).unwrap();
        let edge_file = edges_path(&path);
        // Overwrite the edge blob with the (valid) record blob.
        std::fs::copy(crate::generic::record_blob::blob_path(&path), &edge_file).unwrap();

        match open_employee_production_stack_portable(&path) {
            Err(crate::durability::DurabilityError::RecordBlobUnreadable { path: p, cause }) => {
                assert_eq!(p, edge_file);
                assert!(cause.starts_with("magic number mismatch"), "{cause}");
            }
            Err(other) => panic!("expected RecordBlobUnreadable, got {other:?}"),
            Ok(_) => panic!("expected RecordBlobUnreadable, got a stack"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
