//! Benchmark harness comparing AoS, SoA, and UUID-canonical-store storage
//! layouts behind one [`store::DogStore`] trait.
//!
//! See `docs/charter/CHARTER.md` for the hypothesis under test and
//! `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for why
//! the three backends are compared this way.

pub mod bench_support;
pub mod generator;
pub mod record;
pub mod store;

pub use generator::{generate, GeneratorConfig};
pub use record::DogRecord;
pub use store::{DogStore, StoreError};
