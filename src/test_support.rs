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

/// Format bytes as a ready-to-paste Rust byte-array literal body
/// (`0x01, 0x02, …`), so a golden-vector test that fails prints the
/// bytes in the exact form the pinned constant is written in.
#[cfg(test)]
pub(crate) fn hex_literal(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("0x{b:02x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The golden-vector check every `BINENC-FR-004` test makes: `value`
/// encodes to exactly `golden`, and `golden` decodes to a value that
/// encodes to `golden` again (the decode half stated without needing
/// `PartialEq` — `Request` has none). `label` names the vector in the
/// failure message, which also prints the actual bytes as a literal.
#[cfg(test)]
#[track_caller]
pub(crate) fn assert_golden<T>(label: &str, value: &T, golden: &[u8])
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let encoded = crate::codec::encode(value).unwrap();
    assert_eq!(
        encoded,
        golden,
        "{label}: encoding drifted from the pinned bytes; actual: [{}]",
        hex_literal(&encoded)
    );
    let decoded: T = crate::codec::decode(golden).unwrap();
    assert_eq!(
        crate::codec::encode(&decoded).unwrap(),
        golden,
        "{label}: the pinned bytes did not decode to a value that re-encodes to them"
    );
}

/// [`assert_golden`] plus `decode(golden) == value` for types that can
/// say so directly.
#[cfg(test)]
#[track_caller]
pub(crate) fn assert_golden_eq<T>(label: &str, value: &T, golden: &[u8])
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_golden(label, value, golden);
    let decoded: T = crate::codec::decode(golden).unwrap();
    assert_eq!(
        &decoded, value,
        "{label}: the pinned bytes decode to a different value"
    );
}
