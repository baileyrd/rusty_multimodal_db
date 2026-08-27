//! A fresh, empty, uniquely-named directory under the OS temp dir — the
//! crate's one shared scratch-directory helper.
//!
//! Lives here, unconditionally compiled (not gated behind the `research`
//! feature the way [`crate::bench_support`] is), because it's needed by
//! genuinely front-door code: [`crate::production::ProductionStore`]'s
//! own infallible constructors (`ConcurrentStore::new`, both `From` impls
//! — see that module's own docs for why those can't take a caller-
//! supplied path) call it directly, not just tests/benches. Re-exported
//! as `bench_support::fresh_temp_dir` (research-gated) for the many
//! existing call sites written against that path, so this split is
//! invisible to them.

/// See module docs.
///
/// Uniqueness (PID + atomic counter, not just `label`) matters because
/// Rust's test harness runs tests concurrently by default — two tests
/// both naming themselves the same label must not collide on the same
/// on-disk files.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the directory can't be
/// created (e.g. no space, no permission on the OS temp dir).
pub fn fresh_temp_dir(label: &str) -> std::io::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rusty_multimodal_db_{label}_{}_{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
