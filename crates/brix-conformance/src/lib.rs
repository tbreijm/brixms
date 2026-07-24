//! brix-conformance — Fixture format, CONF runner, differential harness, random program generator.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! # Scope (issue #26)
//!
//! This crate currently implements the oracle-side half only: the fixture
//! format ([`fixture`]) and its runner ([`oracle_engine`]), plus fixtures
//! pinning the ruled errata's oracle-observable behavior ([`fixtures`]).
//! The compiled-engine runner and the digest-for-digest differential
//! harness against it are #11 — [`fixture::Engine`] is the seam that lets
//! that drop in later without reshaping anything here.
//!
//! ## A note on `spec/errata/0001-estimate-f64-canonical-in-value-domain.md`
//!
//! Not pinned here, deliberately. `brix_oracle::value::Value` has no float
//! variant by design (its own module doc: "Floats are absent by
//! construction... the oracle's kernel proof does not need [the
//! strict-IEEE ops module] at all"). The ruling's substance — an
//! `Estimate<F64>` row is `Canonical` in the *value* domain but not the
//! *key* domain — is a `brix-ir` static type-checking concern with no
//! oracle-runtime component to build a [`fixture::Fixture`] against: there
//! is no `Estimate<F64>` value, no policy/`candidatesDigest` mechanism,
//! and no key-canonical type check anywhere in `brix-oracle`. It is
//! already pinned by `brix-ir/src/types.rs`'s own test,
//! `estimate_f64_is_value_canonical_but_not_key_canonical`.

pub mod compiled_engine;
pub mod fixture;
pub mod fixtures;
pub mod oracle_engine;
pub mod translate;
pub mod typecorpus;

pub use compiled_engine::CompiledEngine;
pub use fixture::{Engine, Fixture, RevDump, RunOutput};
pub use oracle_engine::OracleEngine;
