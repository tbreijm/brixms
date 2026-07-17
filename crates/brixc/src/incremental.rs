//! Incremental front-end compilation for long-lived callers.
//!
//! The CLI's on-disk artifact cache is intentionally process-independent. An
//! editor, language server, or future `brix serve` loop needs a different
//! boundary: retain the last source revision and reuse the parsed and lowered
//! artifacts while that revision is unchanged. This module provides that
//! small, explicit state machine without global mutable state or filesystem
//! timestamps.

use brix_ast::{parse_file, Diagnostics, File};
use brix_canon::{Digest, Domain};

use crate::lower::{lower_file, Lowered};

/// Observable reuse information for one incremental compilation request.
/// Consumers can use this for editor telemetry without inferring behavior
/// from timings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncrementalProgress {
    pub parsed_reused: bool,
    pub lowered_reused: bool,
}

/// Borrowed result of compiling the current source revision.
pub struct IncrementalUnit<'a> {
    pub source_digest: Digest,
    pub file: &'a File,
    pub parse_diagnostics: &'a Diagnostics,
    pub lowered: &'a Lowered,
    pub progress: IncrementalProgress,
}

/// A one-document compilation state machine.
///
/// `Empty -> Parsed -> Lowered` is advanced lazily by [`Self::lower`]. A
/// source-digest change replaces the revision and invalidates every later
/// stage; an identical digest reuses the complete current state. Keeping the
/// owner explicit makes the cache deterministic and prevents stale artifacts
/// crossing package/workspace boundaries.
#[derive(Default)]
pub struct IncrementalCompiler {
    revision: Option<Revision>,
}

struct Revision {
    source_digest: Digest,
    file: File,
    parse_diagnostics: Diagnostics,
    lowered: Option<Lowered>,
}

impl IncrementalCompiler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse and lower `source`, reusing the current revision when its exact
    /// content digest is unchanged. The returned references remain valid until
    /// the next mutable call on this compiler session.
    pub fn lower(&mut self, source: &str) -> IncrementalUnit<'_> {
        let source_digest = Digest::of(Domain::Value, source.as_bytes());
        let parsed_reused = self
            .revision
            .as_ref()
            .is_some_and(|revision| revision.source_digest == source_digest);

        if !parsed_reused {
            let (file, parse_diagnostics) = parse_file(source);
            self.revision = Some(Revision {
                source_digest,
                file,
                parse_diagnostics,
                lowered: None,
            });
        }

        let revision = self.revision.as_mut().expect("revision was just installed");
        let lowered_reused = revision.lowered.is_some();
        if revision.lowered.is_none() {
            revision.lowered = Some(lower_file(&revision.file, &revision.parse_diagnostics));
        }

        IncrementalUnit {
            source_digest,
            file: &revision.file,
            parse_diagnostics: &revision.parse_diagnostics,
            lowered: revision
                .lowered
                .as_ref()
                .expect("lowering was just installed"),
            progress: IncrementalProgress {
                parsed_reused,
                lowered_reused,
            },
        }
    }

    /// Drop the current revision explicitly, for example when a document is
    /// closed. This is the only cache eviction policy; it never depends on
    /// ambient time or memory-pressure heuristics.
    pub fn clear(&mut self) {
        self.revision = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_A: &str = "package demo @ 1.0.0\nrel Input { value: Int } key(value)\n";
    const SOURCE_B: &str = "package demo @ 1.0.0\nrel Input { value: String } key(value)\n";

    #[test]
    fn unchanged_revision_reuses_parse_and_lowering() {
        let mut compiler = IncrementalCompiler::new();
        let first = compiler.lower(SOURCE_A);
        assert_eq!(
            first.progress,
            IncrementalProgress {
                parsed_reused: false,
                lowered_reused: false,
            }
        );
        let first_digest = first.source_digest;
        let second = compiler.lower(SOURCE_A);
        assert_eq!(second.source_digest, first_digest);
        assert_eq!(
            second.progress,
            IncrementalProgress {
                parsed_reused: true,
                lowered_reused: true,
            }
        );
    }

    #[test]
    fn source_change_invalidates_every_downstream_stage() {
        let mut compiler = IncrementalCompiler::new();
        let first_digest = compiler.lower(SOURCE_A).source_digest;
        let changed = compiler.lower(SOURCE_B);
        assert_ne!(changed.source_digest, first_digest);
        assert_eq!(
            changed.progress,
            IncrementalProgress {
                parsed_reused: false,
                lowered_reused: false,
            }
        );
    }

    #[test]
    fn clear_forces_a_fresh_revision() {
        let mut compiler = IncrementalCompiler::new();
        compiler.lower(SOURCE_A);
        compiler.clear();
        assert_eq!(
            compiler.lower(SOURCE_A).progress,
            IncrementalProgress {
                parsed_reused: false,
                lowered_reused: false,
            }
        );
    }
}
