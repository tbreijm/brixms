//! brix-cli library surface.
//!
//! `main.rs` is a pre-G0 banner stub; the real groundwork for the
//! `new`/`build`/`run`/... verbs lives in these modules (argument parsing and
//! project scaffolding). Exposing them as a library target is what compiles
//! them, runs their unit tests, and puts them under `clippy` — the binary is a
//! thin front-end over this surface.

pub mod args;
pub mod scaffold;
