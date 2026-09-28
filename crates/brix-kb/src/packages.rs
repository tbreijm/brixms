//! Package loading and bounded source reading, defined once in
//! `brix_lower::packages` and shared with the `brix` CLI, so `kb` replay
//! resolves every `use` exactly as `brix run` does for the same program.

pub use brix_lower::packages::{make_package_loader, read_source_bounded};
