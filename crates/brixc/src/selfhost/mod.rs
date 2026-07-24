//! Self-hosted `brix.type` checker support.
//!
//! [`typefacts`] is the fact-export/decode bridge between `brix_ir`'s
//! reflective type-checker report ([`brix_ir::reflect`]) and the flat
//! token/row shape the self-hosted checker consumes and emits. It was
//! moved here from `brix-conformance` (Track A) so a later slice can call
//! it directly from the compiler's real pipeline instead of only from
//! test harnesses; this move changes no logic.
pub mod typefacts;
