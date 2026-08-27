//! Deterministic synthetic `Source` dataset generation for
//! `benches/req_traceability_spike.rs` — the `Source` analogue of
//! [`super::rule_bench_support`], self-contained per this crate's
//! established convention.

use super::source::{Source, SourceKind};
use uuid::Uuid;

/// Build a linear parent chain of `depth` sources: source `0` is the root
/// (`parent_source_id: None`, carrying `root_tags`), source `i` (`i > 0`)
/// has source `i - 1` as its parent and empty `domain_tags` of its own —
/// same shape as `rule_bench_support::build_rule_chain`, purpose-built for
/// the root-lookup-depth benchmark: a straight line makes "how does the
/// walk-to-root cost scale with depth" the only variable.
pub fn build_source_chain(depth: usize, root_tags: Vec<String>) -> Vec<Source> {
    (0..depth)
        .map(|i| Source {
            id: Uuid::from_u128(i as u128),
            kind: SourceKind::OrgImplementation,
            parent_source_id: if i == 0 {
                None
            } else {
                Some(Uuid::from_u128((i - 1) as u128))
            },
            domain_tags: if i == 0 {
                root_tags.clone()
            } else {
                Vec::new()
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_source_chain_produces_a_linear_chain_with_tags_only_on_the_root() {
        let chain = build_source_chain(10, vec!["payments".into()]);
        assert_eq!(chain.len(), 10);
        assert_eq!(chain[0].parent_source_id, None);
        assert_eq!(chain[0].domain_tags, vec!["payments".to_string()]);
        for (i, source) in chain.iter().enumerate().skip(1) {
            assert_eq!(
                source.parent_source_id,
                Some(Uuid::from_u128((i - 1) as u128))
            );
            assert!(source.domain_tags.is_empty());
        }
    }
}
