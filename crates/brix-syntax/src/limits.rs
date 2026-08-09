//! `ParseLimits` — hostile-input bounds for the source frontend
//! (ADR-0022 D6, implementation stage 3).
//!
//! # Why this exists
//!
//! ADR-0022 rules that an offline verifier re-derives an L3 settlement
//! manifest by re-running `parse → lower_l3_plan` over `.brix` source it was
//! given. That is a stronger result than trusting a signed authorization
//! (ADR-0021), because it checks that the rows *follow from* the source rather
//! than that someone said they should be used.
//!
//! It also means **the source frontend becomes part of that verifier's trusted
//! closure**, and its input is attacker-controlled. `PlanLimitsV1` bounds the
//! *lowered plan*; it does not bound the work done before a plan exists. A
//! 200 MB file, a million-token stream, or a thousand-deep nesting all consume
//! resources during lexing and recursive descent, before anything the existing
//! accounting can see.
//!
//! These limits are therefore **local resource policy, not canonical
//! artifacts** (ADR-0022 D6). They are never encoded, never hashed, and never
//! part of any id. A verifier may legitimately refuse a valid large program;
//! that refusal is a typed error, never acceptance under weaker limits.
//!
//! # The discipline
//!
//! Every bound is checked **before** the allocation it governs, not after. A
//! limit that measures a structure already built has not prevented the work it
//! was meant to prevent.

/// Bounds on the work the source frontend may perform for one parse
/// (ADR-0022 D6).
///
/// Noncanonical: this type is never written by a `CanonWriter`, never hashed,
/// and contributes to no artifact identity. Two verifiers running different
/// limits over the same accepted source produce the same `Module` and the same
/// downstream `ProgramIdV1`; they differ only in which inputs they refuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ParseLimits {
    /// Maximum source length in **bytes**, checked before UTF-8 conversion or
    /// tokenization. Bytes rather than chars because it is the only measure
    /// available before decoding.
    pub max_source_bytes: usize,
    /// Maximum number of tokens, accounted incrementally as they are produced
    /// rather than by measuring a finished vector.
    pub max_tokens: usize,
    /// Maximum recursive-descent nesting depth. Bounds stack consumption:
    /// exceeded *before* the recursive call, so a deep input is refused rather
    /// than overflowing the stack.
    pub max_nesting_depth: usize,
}

impl ParseLimits {
    /// Bounds generous enough for ordinary hand-written modules, and small
    /// enough that a hostile input is refused long before it exhausts a
    /// verifier.
    ///
    /// These numbers are policy, not semantics — a deployment may choose
    /// others. They exist so the strict entry point has a usable default
    /// rather than requiring every caller to invent one.
    pub const fn strict() -> Self {
        ParseLimits {
            max_source_bytes: 1 << 20, // 1 MiB
            max_tokens: 200_000,
            max_nesting_depth: 128,
        }
    }

    /// Bounds high enough not to disturb existing in-process callers, used by
    /// the unbounded-compatible [`crate::parse`] entry point.
    pub const fn generous() -> Self {
        ParseLimits {
            max_source_bytes: usize::MAX,
            max_tokens: usize::MAX,
            max_nesting_depth: usize::MAX,
        }
    }
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self::strict()
    }
}

/// Which bound a source input exceeded (ADR-0022 D8).
///
/// Every variant is a **refusal**: no partial module is produced, and no
/// caller may retry under weaker limits and treat the result as equivalent.
/// Noncanonical, like [`ParseLimits`] itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LimitExceeded {
    /// Source length exceeded `max_source_bytes`, refused before UTF-8
    /// conversion or tokenization.
    SourceBytes { limit: usize, found: usize },
    /// Token count exceeded `max_tokens` during lexing.
    Tokens { limit: usize },
    /// Recursive-descent depth would have exceeded `max_nesting_depth`.
    /// Refused before descending, so the stack is never at risk.
    NestingDepth { limit: usize },
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LimitExceeded::SourceBytes { limit, found } => {
                write!(f, "source is {found} bytes, limit is {limit}")
            }
            LimitExceeded::Tokens { limit } => write!(f, "token count exceeds limit {limit}"),
            LimitExceeded::NestingDepth { limit } => {
                write!(f, "nesting depth exceeds limit {limit}")
            }
        }
    }
}
