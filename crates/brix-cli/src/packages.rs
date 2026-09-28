//! Package loading and bounded source reading, defined once in
//! `brix_lower::packages` and shared with `brix-kb`.

pub use brix_lower::packages::{
    make_package_loader, read_source_bounded, EMBEDDED_BRIX_SOC, MAX_SOURCE_BYTES,
};
