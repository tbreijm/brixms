//! The delta protocol (ADR-0002 §9.2 "Candidates are a materialized
//! incremental view"; `Build_Plan_v3_SOC.md` Step 6, E3):
//!
//! > Candidates are a materialized incremental view (semi-naive,
//! > delta-driven), never a re-run query. The regime trait is a dataflow
//! > operator: `footprint()` / `apply(delta) → candidate delta`.
//!
//! World/config identities are content-addressed [`Handle`]s, so a *change*
//! to the world is never an in-place mutation — it is
//! **remove-old-handle + add-new-handle**. A [`Delta`] is exactly that pair
//! of `BTreeSet`s over world-configuration handles; a [`CandidateDelta`] is
//! its image on the candidate view; and a [`Footprint`] is the declared
//! index domain a regime is sensitive to, so the incremental engine
//! ([`crate::engine`]) can **skip** any regime whose footprint does not
//! intersect a delta (that skip is what makes per-step cost `∝ |Δ|`, never
//! `∝ |world|` — ADR-0002 §9.1).
//!
//! **Determinism (Ring0 §0).** Every collection here is a `BTreeSet` — never
//! a `HashSet` — so a delta, a candidate delta, and a footprint each have one
//! canonical iteration order, and two engines fed identical deltas produce
//! byte-identical views.

use std::collections::BTreeSet;

use crate::intern::Handle;
use crate::regime::Candidate;

/// A change to the set of world-configuration handles between two states:
/// the handles that **entered** (`added`) and **left** (`removed`). Because
/// config identities are content-addressed, "editing" a configuration is
/// modelled as removing its old handle and adding the new one — there is no
/// third "modified" case.
///
/// `|Δ|` (the quantity ADR-0002 §9.1's invariant bounds per-step cost by) is
/// [`Delta::len`] = `|added| + |removed|`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Delta {
    /// World-config handles that entered the world.
    pub added: BTreeSet<Handle>,
    /// World-config handles that left the world.
    pub removed: BTreeSet<Handle>,
}

impl Delta {
    /// The empty delta — nothing entered or left.
    pub fn new() -> Self {
        Self::default()
    }

    /// A delta that only adds `handles`.
    pub fn of_added(handles: impl IntoIterator<Item = Handle>) -> Self {
        Delta {
            added: handles.into_iter().collect(),
            removed: BTreeSet::new(),
        }
    }

    /// A delta that only removes `handles`.
    pub fn of_removed(handles: impl IntoIterator<Item = Handle>) -> Self {
        Delta {
            added: BTreeSet::new(),
            removed: handles.into_iter().collect(),
        }
    }

    /// The world-config delta of one committed step whose world advanced from
    /// `before` to `after`: `after` entered, `before` left. A reflexive step
    /// (`before == after`) yields the empty delta — nothing actually changed.
    pub fn between_worlds(before: Handle, after: Handle) -> Self {
        if before == after {
            return Delta::new();
        }
        Delta {
            added: BTreeSet::from([after]),
            removed: BTreeSet::from([before]),
        }
    }

    /// `|Δ|` — the total number of handle changes (`|added| + |removed|`),
    /// the quantity ADR-0002 §9.1 bounds per-step cost by.
    pub fn len(&self) -> usize {
        self.added.len() + self.removed.len()
    }

    /// Whether this delta changes nothing.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// Every handle this delta touches — added first (ascending), then
    /// removed (ascending). Deterministic order (both halves are
    /// `BTreeSet`s). Used by the engine to route a delta to the regimes whose
    /// footprint intersects it.
    pub fn touched(&self) -> impl Iterator<Item = Handle> + '_ {
        self.added
            .iter()
            .copied()
            .chain(self.removed.iter().copied())
    }
}

/// The image of a world [`Delta`] on the materialized candidate view: the
/// [`Candidate`]s that entered (`added`) and left (`removed`) as a
/// consequence. This is the *only* thing a regime's
/// [`crate::engine::IncrementalRegime::apply`] returns — never the whole
/// view.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CandidateDelta {
    /// Candidates that entered the view.
    pub added: BTreeSet<Candidate>,
    /// Candidates that left the view.
    pub removed: BTreeSet<Candidate>,
}

impl CandidateDelta {
    /// The empty candidate delta.
    pub fn new() -> Self {
        Self::default()
    }

    /// A candidate delta that only adds `candidates`.
    pub fn of_added(candidates: impl IntoIterator<Item = Candidate>) -> Self {
        CandidateDelta {
            added: candidates.into_iter().collect(),
            removed: BTreeSet::new(),
        }
    }

    /// A candidate delta that only removes `candidates`.
    pub fn of_removed(candidates: impl IntoIterator<Item = Candidate>) -> Self {
        CandidateDelta {
            added: BTreeSet::new(),
            removed: candidates.into_iter().collect(),
        }
    }

    /// Total number of candidate changes (`|added| + |removed|`).
    pub fn len(&self) -> usize {
        self.added.len() + self.removed.len()
    }

    /// Whether this candidate delta changes nothing.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// Fold `other` into `self`: union the added and removed sets. Used by
    /// the engine to combine the candidate deltas of every regime a single
    /// world delta was routed to, before materializing them into the view.
    pub fn merge(&mut self, other: CandidateDelta) {
        self.added.extend(other.added);
        self.removed.extend(other.removed);
    }
}

/// The index/key domain a regime declares itself sensitive to — the feed the
/// engine consults to decide whether a regime must be re-run for a given
/// delta (ADR-0002 §9.1/§9.2). Coarse is acceptable for v1 ([`AllConfigs`]),
/// but a footprint MUST be *declared*: an undeclared "sensitive to
/// everything" default would silently defeat the O(Δ) skip and re-introduce
/// the v1 recompute-the-world cost this whole step exists to forbid.
///
/// [`AllConfigs`]: Footprint::AllConfigs
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Footprint {
    /// Sensitive to *every* configuration — the coarsest footprint, and the
    /// one the engine can **never** skip (any non-empty delta intersects it).
    /// Valid for v1 but pays full fan-in on every delta; a regime that can
    /// name the configs it cares about should return [`Footprint::Configs`]
    /// instead.
    AllConfigs,
    /// Sensitive only to this explicit set of configuration handles. The
    /// empty set (`Configs(∅)`) is a genuinely **inert** regime: no delta
    /// ever intersects it, so the engine never re-runs it — the exact shape
    /// the O(Δ) gate scales as ballast.
    Configs(BTreeSet<Handle>),
}

impl Footprint {
    /// A footprint over an explicit set of configuration handles.
    pub fn configs(handles: impl IntoIterator<Item = Handle>) -> Self {
        Footprint::Configs(handles.into_iter().collect())
    }

    /// The empty (inert) footprint — no delta ever intersects it.
    pub fn empty() -> Self {
        Footprint::Configs(BTreeSet::new())
    }

    /// Whether this footprint intersects `delta` — i.e. whether `delta`
    /// touches at least one configuration this regime is sensitive to.
    ///
    /// - [`Footprint::AllConfigs`] intersects any *non-empty* delta (and, as
    ///   for every footprint, never intersects the empty delta — nothing
    ///   changed, so nothing to re-run).
    /// - [`Footprint::Configs`] intersects iff a touched handle is in the set;
    ///   the empty config set never intersects anything.
    pub fn intersects(&self, delta: &Delta) -> bool {
        if delta.is_empty() {
            return false;
        }
        match self {
            Footprint::AllConfigs => true,
            Footprint::Configs(set) => delta.touched().any(|h| set.contains(&h)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Interner;
    use brix_canon::{Digest, Domain};

    fn handles(n: usize) -> (Interner, Vec<Handle>) {
        let mut i = Interner::new();
        let hs = (0..n)
            .map(|k| i.intern(Digest::of(Domain::Value, format!("h{k}").as_bytes())))
            .collect();
        (i, hs)
    }

    #[test]
    fn delta_len_is_added_plus_removed() {
        let (_i, h) = handles(3);
        let d = Delta {
            added: BTreeSet::from([h[0], h[1]]),
            removed: BTreeSet::from([h[2]]),
        };
        assert_eq!(d.len(), 3);
        assert!(!d.is_empty());
    }

    #[test]
    fn between_worlds_reflexive_is_empty() {
        let (_i, h) = handles(1);
        assert!(Delta::between_worlds(h[0], h[0]).is_empty());
    }

    #[test]
    fn between_worlds_advancing_removes_old_adds_new() {
        let (_i, h) = handles(2);
        let d = Delta::between_worlds(h[0], h[1]);
        assert_eq!(d.added, BTreeSet::from([h[1]]));
        assert_eq!(d.removed, BTreeSet::from([h[0]]));
    }

    #[test]
    fn touched_yields_added_then_removed() {
        let (_i, h) = handles(2);
        let d = Delta {
            added: BTreeSet::from([h[1]]),
            removed: BTreeSet::from([h[0]]),
        };
        let touched: Vec<_> = d.touched().collect();
        assert_eq!(touched, vec![h[1], h[0]]);
    }

    #[test]
    fn empty_footprint_never_intersects() {
        let (_i, h) = handles(2);
        let fp = Footprint::empty();
        let d = Delta::of_added([h[0], h[1]]);
        assert!(!fp.intersects(&d), "an inert regime is never re-run");
    }

    #[test]
    fn all_configs_intersects_any_non_empty_delta_but_not_the_empty_one() {
        let (_i, h) = handles(1);
        assert!(Footprint::AllConfigs.intersects(&Delta::of_added([h[0]])));
        assert!(!Footprint::AllConfigs.intersects(&Delta::new()));
    }

    #[test]
    fn configs_intersects_only_on_a_touched_member() {
        let (_i, h) = handles(3);
        let fp = Footprint::configs([h[0]]);
        assert!(fp.intersects(&Delta::of_added([h[0]])));
        assert!(fp.intersects(&Delta::of_removed([h[0]])));
        assert!(
            !fp.intersects(&Delta::of_added([h[1], h[2]])),
            "a delta touching only non-member configs must not intersect"
        );
    }

    #[test]
    fn candidate_delta_merge_unions_both_halves() {
        let (mut i, _h) = handles(0);
        let mut mk = |tag: &str| {
            let r = i.intern(Digest::of(Domain::Value, format!("r{tag}").as_bytes()));
            let w = i.intern(Digest::of(Domain::Value, format!("w{tag}").as_bytes()));
            let s = i.intern(Digest::of(Domain::Value, format!("s{tag}").as_bytes()));
            Candidate {
                regime: r,
                witness: w,
                successor: s,
            }
        };
        let a = mk("a");
        let b = mk("b");
        let mut cd = CandidateDelta::of_added([a]);
        cd.merge(CandidateDelta::of_added([b]));
        assert_eq!(cd.added, BTreeSet::from([a, b]));
        assert!(cd.removed.is_empty());
    }
}
