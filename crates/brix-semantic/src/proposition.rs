//! [`PropositionId`] — the content-addressed identity of a canonical
//! domain-specific *statement* (ADR-0001 §5.2): "e has type T", "this
//! authorization holds", "f runs in O(n²)", …
//!
//! Named **`Proposition`, not `Claim`** — `ClaimId` already means a *committed
//! transaction claim* in BrixMS; reusing it would fuse two unrelated
//! identities. The substrate does not fix a statement vocabulary: a
//! `PropositionId` is the id of whatever canonical value a domain (the type
//! checker, an authorization resolver, a complexity resolver) chooses to encode.
//! Build one with `PropositionId::of(&statement)`.

use crate::id::digest_id;

digest_id!(
    /// Identity of a canonical domain statement. Two byte-identical statements
    /// have the same `PropositionId`; that is the whole point (a proof about a
    /// proposition attaches to *the statement*, however it was phrased).
    PropositionId
);
