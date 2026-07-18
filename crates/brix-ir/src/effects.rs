//! Effect rows and the purity / determinism / non-divergence flags
//! (Part V §4-5, §8; Appendix E rule side-conditions `pure`, `det`,
//! `nondiverge`).
//!
//! An effect row is a *set* of kernel effect atoms plus a row-variable tail
//! (effects are "inferred and polymorphic," Part V §4). We store atoms in a
//! sorted, de-duplicated `Vec` rather than a `HashSet` — observable order must
//! be canon byte order and `HashSet` is clippy-denied in semantic paths
//! (CONTRIBUTING.md). Combination (`⊕`, the union used when sequencing/joining
//! sub-expressions) is set union of atoms and of tails.

use crate::ident::Ident;
use brix_canon::{CanonWriter, Canonical};
use core::fmt;

/// A kernel effect atom (Part V §4). Some atoms are scoped (`net<S>`,
/// `graph.read<S>`); the scope `S` is carried as an opaque scope identifier
/// because scope typing lives in the capability lane, not here.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Effect {
    /// `net<S>` — network access within scope `S`.
    Net(Scope),
    /// `fs<S>` — filesystem access within scope `S`.
    Fs(Scope),
    Clock,
    Random,
    Console,
    /// `graph.read<S>`.
    GraphRead(Scope),
    /// `graph.write<S>`.
    GraphWrite(Scope),
    /// Invariant-failure boundary effect (Part V §2).
    Panic,
    /// Non-termination capability. Banned in rule bodies (Part V §5).
    Diverge,
    /// `solver<S>`.
    Solver(Scope),
}

impl Effect {
    /// Ordinal for canonical ordering / encoding (declaration order is ABI,
    /// App. G enum rule). Scoped atoms order by ordinal first, then scope.
    fn ordinal(&self) -> u8 {
        match self {
            Effect::Net(_) => 0,
            Effect::Fs(_) => 1,
            Effect::Clock => 2,
            Effect::Random => 3,
            Effect::Console => 4,
            Effect::GraphRead(_) => 5,
            Effect::GraphWrite(_) => 6,
            Effect::Panic => 7,
            Effect::Diverge => 8,
            Effect::Solver(_) => 9,
        }
    }

    /// Whether this atom is a *side-effecting* atom for the `pure(B, H)` rule
    /// side-condition (Appendix E). `panic` and `diverge` are handled
    /// separately by `det`/`nondiverge`; every other atom is an impurity.
    pub fn is_impure(&self) -> bool {
        !matches!(self, Effect::Panic | Effect::Diverge)
    }
}

/// Digest-identity encoding (#15 PR3: `Ty::Fn` embeds an `EffectRow`, and
/// `Ty` is `Canonical` so `reflect::Fact` payloads that carry a function type
/// hash deterministically). Ordinal matches [`Effect::ordinal`].
impl Canonical for Effect {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Effect::Net(s) => w.write_enum(0, |w| s.canon_write(w)),
            Effect::Fs(s) => w.write_enum(1, |w| s.canon_write(w)),
            Effect::Clock => w.write_enum(2, |_| {}),
            Effect::Random => w.write_enum(3, |_| {}),
            Effect::Console => w.write_enum(4, |_| {}),
            Effect::GraphRead(s) => w.write_enum(5, |w| s.canon_write(w)),
            Effect::GraphWrite(s) => w.write_enum(6, |w| s.canon_write(w)),
            Effect::Panic => w.write_enum(7, |_| {}),
            Effect::Diverge => w.write_enum(8, |_| {}),
            Effect::Solver(s) => w.write_enum(9, |w| s.canon_write(w)),
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Net(s) => write!(f, "net<{s}>"),
            Effect::Fs(s) => write!(f, "fs<{s}>"),
            Effect::Clock => write!(f, "clock"),
            Effect::Random => write!(f, "random"),
            Effect::Console => write!(f, "console"),
            Effect::GraphRead(s) => write!(f, "graph.read<{s}>"),
            Effect::GraphWrite(s) => write!(f, "graph.write<{s}>"),
            Effect::Panic => write!(f, "panic"),
            Effect::Diverge => write!(f, "diverge"),
            Effect::Solver(s) => write!(f, "solver<{s}>"),
        }
    }
}

/// An opaque effect/capability scope identifier (the `<S>` on scoped atoms).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Scope(pub Ident);

impl Canonical for Scope {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.0.canon_write(w);
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A row variable standing for "some further, not-yet-inferred effects."
/// Effects are polymorphic (Part V §4); a real inferencer would unify these.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EffectVar(pub u32);

impl Canonical for EffectVar {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0 as u64);
    }
}

impl fmt::Display for EffectVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?e{}", self.0)
    }
}

/// An effect row: a canonical-ordered, de-duplicated set of atoms plus an
/// optional polymorphic tail. The empty closed row is *pure* (Part V §4:
/// "the empty row is pure").
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct EffectRow {
    atoms: Vec<Effect>,
    tail: Option<EffectVar>,
}

impl EffectRow {
    /// The pure, closed, empty row.
    pub fn empty() -> Self {
        EffectRow::default()
    }

    /// Build a closed row from an atom iterator; atoms are sorted and
    /// de-duplicated into canonical order.
    pub fn from_atoms(atoms: impl IntoIterator<Item = Effect>) -> Self {
        let mut row = EffectRow::empty();
        for a in atoms {
            row.insert(a);
        }
        row
    }

    /// Open a row with a polymorphic tail variable.
    pub fn with_tail(mut self, tail: EffectVar) -> Self {
        self.tail = Some(tail);
        self
    }

    /// Insert one atom, preserving sorted-dedup canonical order.
    pub fn insert(&mut self, e: Effect) {
        match self.atoms.binary_search_by(|a| cmp_effect(a, &e)) {
            Ok(_) => {}
            Err(pos) => self.atoms.insert(pos, e),
        }
    }

    pub fn atoms(&self) -> &[Effect] {
        &self.atoms
    }

    pub fn tail(&self) -> Option<EffectVar> {
        self.tail
    }

    /// `pure` in the sense of Appendix E's `pure(B, H)` side-condition: no
    /// *impure* effect atom present, and no open tail (an open tail could
    /// unify to an impure atom, so a provably-pure row must be closed).
    pub fn is_pure(&self) -> bool {
        self.tail.is_none() && !self.atoms.iter().any(Effect::is_impure)
    }

    /// Whether `diverge` is present — the `nondiverge(B, H)` side-condition is
    /// its negation (Appendix E; Part V §5 "diverge-capable functions remain
    /// banned in rules").
    pub fn may_diverge(&self) -> bool {
        self.atoms.contains(&Effect::Diverge)
    }

    /// The `⊕` combination used when an expression's effect is the union of
    /// its sub-expressions' effects (sequencing, application, join). Set union
    /// of atoms; tail is kept if either side is open (a real unifier would
    /// merge the two tail variables — here we keep the left, or the right if
    /// the left is closed, and note that as a stub).
    pub fn combine(&self, other: &EffectRow) -> EffectRow {
        let mut out = self.clone();
        for a in &other.atoms {
            out.insert(a.clone());
        }
        out.tail = self.tail.or(other.tail);
        out
    }
}

fn cmp_effect(a: &Effect, b: &Effect) -> core::cmp::Ordering {
    a.ordinal().cmp(&b.ordinal()).then_with(|| a.cmp(b))
}

impl Canonical for EffectRow {
    fn canon_write(&self, w: &mut CanonWriter) {
        // `atoms` is already sorted/deduped canonical order (see `insert`),
        // so a plain sequence write is order-faithful, same reasoning as
        // `Dimension`'s `Vec` encoding.
        w.write_list(self.atoms.iter().map(|a| a.canon_bytes()));
        self.tail.canon_write(w);
    }
}

impl fmt::Display for EffectRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "!{{")?;
        for (i, a) in self.atoms.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{a}")?;
        }
        if let Some(tail) = self.tail {
            if self.atoms.is_empty() {
                write!(f, "{tail}")?;
            } else {
                write!(f, " | {tail}")?;
            }
        }
        write!(f, "}}")
    }
}

/// The three rule side-condition flags of Appendix E, computed over a rule
/// body's combined effect row (and the determinism judgment, which brix-ir
/// tracks as a flag because full `det` per Part V §8 needs the numeric-ops
/// analysis that is stubbed here).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RuleEffectFlags {
    /// `pure(B, H)` — no effect atoms in `ε(B ∪ H)`.
    pub pure: bool,
    /// `det(B, H)` — deterministic per Part V §8. STUB: brix-ir currently
    /// equates determinism with "no `random`/`clock`/`net`/`fs` atom" (the
    /// obvious non-determinism sources); the full strict-IEEE / aggregate
    /// row-order analysis is later work in the numerics module.
    pub det: bool,
    /// `nondiverge(B, H)` — no `diverge` in any called fn.
    pub nondiverge: bool,
}

impl EffectRow {
    /// Compute the Appendix E rule side-condition flags for a body whose
    /// combined effect row is `self`.
    pub fn rule_flags(&self) -> RuleEffectFlags {
        let nondeterministic = self.atoms.iter().any(|a| {
            matches!(
                a,
                Effect::Random | Effect::Clock | Effect::Net(_) | Effect::Fs(_) | Effect::Solver(_)
            )
        }) || self.tail.is_some();
        RuleEffectFlags {
            pure: self.is_pure(),
            det: !nondeterministic,
            nondiverge: !self.may_diverge(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(s: &str) -> Scope {
        Scope(Ident::new(s))
    }

    #[test]
    fn empty_row_is_pure_and_displays_as_empty_bang_braces() {
        let e = EffectRow::empty();
        assert!(e.is_pure());
        assert_eq!(e.to_string(), "!{}");
        let flags = e.rule_flags();
        assert!(flags.pure && flags.det && flags.nondiverge);
    }

    #[test]
    fn atoms_are_canonicalized_sorted_and_deduped() {
        let e = EffectRow::from_atoms([
            Effect::Console,
            Effect::Clock,
            Effect::Console,
            Effect::Net(scope("S")),
        ]);
        // net(0) < clock(2) < console(4) by ordinal; console deduped.
        assert_eq!(e.atoms().len(), 3);
        assert_eq!(e.to_string(), "!{net<S>, clock, console}");
    }

    #[test]
    fn open_tail_is_never_provably_pure() {
        let e = EffectRow::empty().with_tail(EffectVar(0));
        assert!(!e.is_pure());
        assert_eq!(e.to_string(), "!{?e0}");
    }

    #[test]
    fn diverge_flips_nondiverge_but_not_purity_directly() {
        let e = EffectRow::from_atoms([Effect::Diverge]);
        // diverge is not an "impure" atom, but it is not pure because... it is
        // the only atom and is_impure(diverge) == false, so is_pure() is true.
        // Purity and non-divergence are orthogonal judgments (Appendix E lists
        // them separately); this test pins that separation.
        assert!(e.is_pure());
        assert!(e.may_diverge());
        let flags = e.rule_flags();
        assert!(flags.pure);
        assert!(!flags.nondiverge);
    }

    #[test]
    fn combine_is_set_union() {
        let a = EffectRow::from_atoms([Effect::Clock]);
        let b = EffectRow::from_atoms([Effect::Console, Effect::Clock]);
        let c = a.combine(&b);
        assert_eq!(c.atoms().len(), 2);
        assert_eq!(c.to_string(), "!{clock, console}");
    }

    #[test]
    fn random_makes_a_row_nondeterministic() {
        let e = EffectRow::from_atoms([Effect::Random]);
        assert!(!e.rule_flags().det);
    }
}
