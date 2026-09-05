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

/// `ECO-FR-005`/`006` (ADR-0043): the machine-readable conformance
/// fixture, `tests/fixtures/wire-vectors.txt` — one `name<TAB>version<TAB>
/// hex` line per wire golden vector, plain text, no dependency. The
/// golden-vector tests in `src/server/protocol.rs` route every vector
/// through [`wire_fixture::check`], so the file cannot drift from the
/// Rust pins without a red `cargo test`: a missing or differing line is a
/// failure that names the vector and the one command that regenerates
/// the file. Regeneration is explicit — `RMDB_REGENERATE_VECTORS=1 cargo
/// test --features client --lib protocol::` rewrites the file from the
/// pins, and a second, plain run verifies it — never implicit, so a
/// stale file on `main` is impossible. `SERVER-002` names this file as a
/// foreign implementation's conformance test.
#[cfg(test)]
pub(crate) mod wire_fixture {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    pub const PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wire-vectors.txt"
    );
    pub const REGENERATE_VAR: &str = "RMDB_REGENERATE_VECTORS";
    const HEADER: &str = "# Wire conformance vectors (SERVER-002 §9, ECO-FR-005) — GENERATED, do not edit.\n\
# One line per golden vector in src/server/protocol.rs's tests: name<TAB>introduced-at-protocol-version<TAB>hex payload bytes.\n\
# A foreign implementation is conformant at version N when it encodes every Request/* line and decodes every Response/* line with version <= N byte-for-byte.\n\
# Regenerate: RMDB_REGENERATE_VECTORS=1 cargo test --features client --lib protocol::   (then run the tests again without the variable to verify)\n";

    static LOCK: Mutex<()> = Mutex::new(());

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn read_all() -> BTreeMap<String, (u32, String)> {
        let mut out = BTreeMap::new();
        let Ok(text) = std::fs::read_to_string(PATH) else {
            return out;
        };
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(name), Some(version), Some(hex)) = (parts.next(), parts.next(), parts.next())
            else {
                panic!("{PATH}: malformed line {line:?}");
            };
            let version: u32 = version
                .parse()
                .unwrap_or_else(|_| panic!("{PATH}: bad version in {line:?}"));
            out.insert(name.to_string(), (version, hex.to_string()));
        }
        out
    }

    /// Verify (or, under [`REGENERATE_VAR`], record) one vector.
    #[track_caller]
    pub fn check(name: &str, version: u32, bytes: &[u8]) {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let actual = hex(bytes);
        if std::env::var_os(REGENERATE_VAR).is_some() {
            let mut all = read_all();
            all.insert(name.to_string(), (version, actual));
            let mut text = String::from(HEADER);
            for (n, (v, h)) in &all {
                text.push_str(&format!("{n}\t{v}\t{h}\n"));
            }
            if let Some(dir) = std::path::Path::new(PATH).parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(PATH, text).unwrap();
            return;
        }
        let all = read_all();
        match all.get(name) {
            None => panic!(
                "{name}: not in {PATH}. Regenerate with {REGENERATE_VAR}=1 cargo test --features client --lib protocol:: then re-run."
            ),
            Some((v, h)) => {
                assert_eq!(
                    *v, version,
                    "{name}: {PATH} says protocol {v}, the test says {version}; regenerate"
                );
                assert_eq!(
                    *h, actual,
                    "{name}: {PATH} disagrees with the pinned bytes; regenerate after a deliberate wire change"
                );
            }
        }
    }
}
