//! One module per pinned erratum fixture (issue #26).
//!
//! `spec/errata/0001-estimate-f64-canonical-in-value-domain.md` is
//! deliberately **not** represented here — see the crate-level doc comment
//! in `lib.rs` for why.

pub mod edge_identity_domain;
pub mod entity_key_conflict;
pub mod matchdigest_supportref;
pub mod predicate_level_condensation;
