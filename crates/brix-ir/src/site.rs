//! Stable `SiteId` assignment for `?` / `partial` failure sites (Part III §9,
//! Part V §5).
//!
//! `RuleError(rule, site: SiteId, partialMatch, error, atRevision)` needs a
//! "stable compiler-assigned expression-site identity, so two failing sites in
//! one rule never collide" (Part III §9). Stability across compiler runs is a
//! conformance property (Appendix I.2 deterministic identity), so a `SiteId`
//! must be a *function of the rule and the site's position within it*, not a
//! global mutable counter whose value depends on visitation order across
//! rules.
//!
//! We therefore derive `SiteId = Hash(Value domain, rule-name ++ ordinal)`
//! where `ordinal` is the site's index in a deterministic left-to-right,
//! depth-first walk of the rule body. Two rules with the same site count get
//! disjoint ids because the rule name is mixed in; renumbering one rule never
//! perturbs another.

use brix_canon::{CanonWriter, Digest, Domain};
use core::fmt;

use crate::ident::Ident;

/// A stable per-rule expression-site identity for `?`/`partial` failure sites.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SiteId(Digest);

impl SiteId {
    /// Derive the id from a rule name and the site's ordinal within that rule.
    /// Ordinal is the DFS index produced by [`SiteAssigner`], so it is a pure
    /// function of the rule's structure.
    pub fn derive(rule: &Ident, ordinal: u32) -> Self {
        let mut w = CanonWriter::new();
        // rule name then ordinal: length-prefixed ident cannot collide with
        // the varint ordinal, so (rule="a", ord=…) and (rule="", ord=…) differ.
        w.write_ident(rule.as_str());
        w.write_uint(ordinal as u64);
        SiteId(w.digest(Domain::Value))
    }

    pub fn digest(&self) -> Digest {
        self.0
    }
}

impl fmt::Display for SiteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short prefix of the hex digest, enough to disambiguate in diagnostics
        // and `Display` snapshots while staying readable.
        write!(f, "site:{}", &self.0.to_hex()[..12])
    }
}

/// Assigns stable ordinals (and thus [`SiteId`]s) to `?` sites within one
/// rule, in deterministic DFS order. The IR builder calls [`Self::next`] each
/// time it lowers a `?` postfix or a `partial` call site, in source-encounter
/// order; because the IR is built by a deterministic traversal of the AST, the
/// ordinals are stable across runs.
#[derive(Debug)]
pub struct SiteAssigner {
    rule: Ident,
    next_ordinal: u32,
}

impl SiteAssigner {
    pub fn new(rule: Ident) -> Self {
        SiteAssigner {
            rule,
            next_ordinal: 0,
        }
    }

    /// Mint the next site id for this rule.
    pub fn next_site(&mut self) -> SiteId {
        let id = SiteId::derive(&self.rule, self.next_ordinal);
        self.next_ordinal += 1;
        id
    }

    /// How many sites have been assigned so far.
    pub fn count(&self) -> u32 {
        self.next_ordinal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_within_a_rule_are_distinct_and_stable() {
        let rule = Ident::new("FromComputed");
        let mut a = SiteAssigner::new(rule.clone());
        let s0 = a.next_site();
        let s1 = a.next_site();
        assert_ne!(s0, s1, "two sites in one rule must not collide");

        // Re-deriving with the same ordinals reproduces the same ids (stable
        // across compiler runs — Appendix I.2).
        let mut b = SiteAssigner::new(rule);
        assert_eq!(b.next_site(), s0);
        assert_eq!(b.next_site(), s1);
    }

    #[test]
    fn same_ordinal_different_rule_does_not_collide() {
        let s_a = SiteId::derive(&Ident::new("RuleA"), 0);
        let s_b = SiteId::derive(&Ident::new("RuleB"), 0);
        assert_ne!(
            s_a, s_b,
            "renumbering is per-rule; rule name must separate the id spaces"
        );
    }

    #[test]
    fn display_is_a_stable_short_hex() {
        let s = SiteId::derive(&Ident::new("R"), 3);
        let shown = s.to_string();
        assert!(shown.starts_with("site:"));
        assert_eq!(shown.len(), "site:".len() + 12);
    }
}
