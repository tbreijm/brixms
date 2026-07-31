//! `brix-syntax` — the Brix surface language (SOC paradigm): AST + lexer +
//! parser for `.brix` files (ADR-0010, L1).
//!
//! Brix is the language; **SOC is the paradigm** it realizes. This crate is a
//! *fresh* front-end — deliberately not the legacy `soc_regimes::native`
//! `NExpr`/`NTy` mirror, which was shaped by the (now-deleted) brix-ir parity
//! goal. No external dependencies: a hand-written lexer + recursive-descent
//! parser, per the workspace's no-new-deps discipline.
//!
//! Pipeline (this crate): source text → [`lexer`] tokens → [`parser`] → [`ast`]
//! [`ast::Module`]. Lowering (AST → configurations/generators) is L2, a later
//! crate/slice.

pub mod ast;
pub mod lexer;
pub mod parser;

pub use ast::Module;
pub use parser::{parse, ParseError};
