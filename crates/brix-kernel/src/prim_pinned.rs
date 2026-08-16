//! Pinned foreign endpoint identities (ADR-0025 ⟨D-PINNED⟩, §6 Stage A).
//!
//! **What these are.** Thirty-two-byte literal constants naming values that
//! `soc-regimes` encodes: the six numeric `Ty::Con` atoms and the four
//! `ArithOp` atoms. They exist so a primitive relation can contain rows whose
//! endpoints are regime-encoded values, without the TCB reproducing the
//! regime's encoder.
//!
//! **Why that is not a second semantic encoder.** ADR-0023 §4.1 stated the
//! obstruction as "a registry row is matched by canonical bytes, so the kernel
//! must be able to author both endpoints". The obstruction was real; the stated
//! cause was not. A row is a pair of [`crate::Row`] `PropositionId`s and
//! `PrimRealizes` decides membership by exact id comparison, without decoding
//! either endpoint (ADR-0025 §1). **The kernel needs the digest, not the
//! encoder.** The first would be the second semantic encoder ADR-0015 §8.5
//! forbids; the second is a constant.
//!
//! **Why this is not caller-authorized facts (§8.3).** A caller *proposes*
//! `src` and `dst`; the kernel decides membership against rows a kernel release
//! authored. Nothing a caller sends can add, amend, or select a relation's
//! contents. Pinning changes what a row's endpoints may denote, not who
//! authorizes them.
//!
//! **Fail-closed is inherited, not added.** If `soc-regimes` ever changes its
//! encoding, these digests stop matching, membership fails, the leaf goes
//! unclosed, and the grade caps. Nothing is silently reinterpreted — ADR-0015
//! §7's discipline working as designed.
//!
//! **⟨D-REDERIVE⟩ is why you may trust them.** Each constant below is
//! accompanied by (1) a readable entry in
//! `vectors/pinned_endpoint_identities_v1.json` naming the value it digests,
//! and (2) a re-derivation test **in `soc-regimes`** — the crate that owns the
//! encoder, and the crate whose change would break it — asserting the constant
//! equals the digest recomputed from the source value. A pinned identity
//! shipped without (2) is a defect, not a shortcut: it would be the second
//! non-re-derivable element in the system, and ADR-0022 declined the first on
//! exactly that ground.
//!
//! **Nothing here is consulted yet.** Stage A pins the identities; Stages B–F
//! use them. No relation contains a pinned endpoint today, so this module moves
//! no grade and changes no verdict.

use brix_canon::Digest;
use brix_semantic::PropositionId;

/// The marker opening the pinned-identity manifest. Frozen v1 ABI.
pub const PINNED_ENDPOINT_MANIFEST_V1: &[u8] = b"brix.kernel.pinned-endpoint";

/// The manifest format version.
pub const PINNED_ENDPOINT_VERSION_V1: u64 = 1;

/// A numeric type atom the regime encodes as `Ty::Con(name)`.
///
/// Ordinals are **not** ABI here — this enum is a lookup key for kernel-side
/// code and never reaches a canonical preimage. What is frozen is each
/// variant's *digest*, below.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PinnedNumericTy {
    Nat,
    Int,
    Rat,
    Real,
    Complex,
    Float,
}

/// An arithmetic operator atom the regime encodes as `ArithOp`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PinnedArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// `Ty::Con("Nat")`.
pub const TY_CON_NAT: [u8; 32] =
    hexlit("7eee6973b1b330035c9ed76f7ac96bbe87bdb2f1ef81a15f2595555efe844af0");
/// `Ty::Con("Int")`.
pub const TY_CON_INT: [u8; 32] =
    hexlit("389d44164f1d264cc524fa8f12e62fc6eda07e04e549c411926abe230bb61cb1");
/// `Ty::Con("Rat")`.
pub const TY_CON_RAT: [u8; 32] =
    hexlit("a6ba9805cbb4fb5434e6776c4bf2b932fd782bb500aac9ef7b7d41040d7ec0a2");
/// `Ty::Con("Real")`.
pub const TY_CON_REAL: [u8; 32] =
    hexlit("3bf2a1beca7a3bd7a88899421fe99c3bc588505b0975045e0d5d8b4ee4c3ab19");
/// `Ty::Con("Complex")`.
pub const TY_CON_COMPLEX: [u8; 32] =
    hexlit("1d7016fb3a4053f26a0f5d9334dfb5f3e3cd285adf438dcf1d2dc2a48af18d3d");
/// `Ty::Con("Float")`.
pub const TY_CON_FLOAT: [u8; 32] =
    hexlit("97edda5c8bcde1bdc8d1afdaea131de5bd66cd90b8df65cf7db6ccc9f757c5e7");

/// `ArithOp::Add`.
pub const ARITH_OP_ADD: [u8; 32] =
    hexlit("c9c095a7d8969fb90d592c5c7926c2251d1fb04db1357d053ee7acf281b8d9ea");
/// `ArithOp::Sub`.
pub const ARITH_OP_SUB: [u8; 32] =
    hexlit("95219e8a7c7f068440471850f4053b35869f31e4ac0bfe1b4bbea775bc1099e6");
/// `ArithOp::Mul`.
pub const ARITH_OP_MUL: [u8; 32] =
    hexlit("504c618345de98f46f2937bfff284415d965de98a3ee544056eeeac7f1093a0d");
/// `ArithOp::Div`.
pub const ARITH_OP_DIV: [u8; 32] =
    hexlit("655296fff7ee570dcc092f34bec47de49e42958abadcb156678dabe7c1ca3d6d");

/// Decode a 64-character lowercase hex literal at compile time.
///
/// Const rather than runtime so a malformed constant is a build failure, and
/// so the literals above read as the hex a reviewer sees in the vector file
/// rather than as byte arrays nobody can compare by eye.
const fn hexlit(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() == 64, "a pinned identity is 64 hex characters");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = nibble(b[i * 2]) << 4 | nibble(b[i * 2 + 1]);
        i += 1;
    }
    out
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("a pinned identity is lowercase hex"),
    }
}

impl PinnedNumericTy {
    /// Every pinned numeric type, in declaration order.
    pub const ALL: [PinnedNumericTy; 6] = [
        PinnedNumericTy::Nat,
        PinnedNumericTy::Int,
        PinnedNumericTy::Rat,
        PinnedNumericTy::Real,
        PinnedNumericTy::Complex,
        PinnedNumericTy::Float,
    ];

    /// The `Ty::Con` name this atom is the identity of. Part of the readable
    /// manifest, and what the `soc-regimes` re-derivation test rebuilds from.
    pub const fn lattice_name(self) -> &'static str {
        match self {
            PinnedNumericTy::Nat => "Nat",
            PinnedNumericTy::Int => "Int",
            PinnedNumericTy::Rat => "Rat",
            PinnedNumericTy::Real => "Real",
            PinnedNumericTy::Complex => "Complex",
            PinnedNumericTy::Float => "Float",
        }
    }

    /// The pinned identity.
    pub fn proposition_id(self) -> PropositionId {
        PropositionId(Digest::from_bytes(match self {
            PinnedNumericTy::Nat => TY_CON_NAT,
            PinnedNumericTy::Int => TY_CON_INT,
            PinnedNumericTy::Rat => TY_CON_RAT,
            PinnedNumericTy::Real => TY_CON_REAL,
            PinnedNumericTy::Complex => TY_CON_COMPLEX,
            PinnedNumericTy::Float => TY_CON_FLOAT,
        }))
    }
}

impl PinnedArithOp {
    /// Every pinned operator, in declaration order.
    pub const ALL: [PinnedArithOp; 4] = [
        PinnedArithOp::Add,
        PinnedArithOp::Sub,
        PinnedArithOp::Mul,
        PinnedArithOp::Div,
    ];

    /// The operator's name in the regime's vocabulary.
    pub const fn op_name(self) -> &'static str {
        match self {
            PinnedArithOp::Add => "Add",
            PinnedArithOp::Sub => "Sub",
            PinnedArithOp::Mul => "Mul",
            PinnedArithOp::Div => "Div",
        }
    }

    /// The pinned identity.
    pub fn proposition_id(self) -> PropositionId {
        PropositionId(Digest::from_bytes(match self {
            PinnedArithOp::Add => ARITH_OP_ADD,
            PinnedArithOp::Sub => ARITH_OP_SUB,
            PinnedArithOp::Mul => ARITH_OP_MUL,
            PinnedArithOp::Div => ARITH_OP_DIV,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every pinned identity is distinct. A collision would silently merge two
    /// endpoints, and a copy-paste in the table above is the likely cause.
    #[test]
    fn every_pinned_identity_is_distinct() {
        let mut seen = BTreeSet::new();
        for t in PinnedNumericTy::ALL {
            assert!(seen.insert(t.proposition_id()), "duplicate: {t:?}");
        }
        for op in PinnedArithOp::ALL {
            assert!(seen.insert(op.proposition_id()), "duplicate: {op:?}");
        }
        assert_eq!(seen.len(), 10);
    }

    /// The hex decoder round-trips, so a constant reads as the hex printed in
    /// the manifest.
    #[test]
    fn pinned_constants_render_as_their_hex() {
        assert_eq!(
            PinnedNumericTy::Int.proposition_id().digest().to_hex(),
            "389d44164f1d264cc524fa8f12e62fc6eda07e04e549c411926abe230bb61cb1"
        );
        assert_eq!(
            PinnedArithOp::Div.proposition_id().digest().to_hex(),
            "655296fff7ee570dcc092f34bec47de49e42958abadcb156678dabe7c1ca3d6d"
        );
    }

    /// **Nothing consults a pinned identity yet.** Stage A pins; Stages B–F
    /// use. If a relation ever contains one, this test should be replaced by
    /// one that says which relation and why — not deleted.
    #[test]
    fn no_relation_contains_a_pinned_endpoint_yet() {
        let pinned: BTreeSet<PropositionId> = PinnedNumericTy::ALL
            .iter()
            .map(|t| t.proposition_id())
            .chain(PinnedArithOp::ALL.iter().map(|o| o.proposition_id()))
            .collect();

        let relation = crate::resolve_primitive_relation(&crate::typing_arith_v2())
            .expect("the arithmetic relation resolves");
        for (src, dst) in &relation.rows {
            assert!(
                !pinned.contains(src) && !pinned.contains(dst),
                "a pinned endpoint reached a relation before Stage B"
            );
        }
    }
}
