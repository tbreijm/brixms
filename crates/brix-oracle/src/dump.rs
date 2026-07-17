//! Canonical settled-dump encoding — the bytes the differential harness
//! compares bit-for-bit (Appendix I.1: "the incrementally settled view at
//! every revision is bit-identical to whole-world recomputation — including
//! error edges, KeyConflicts, masks, and provenance answers").
//!
//! Everything here goes through **brix-canon** and nothing else (OWNER.md /
//! Ring0 §1.7: "Serialize dumps only via brix-canon"). The dump is a single
//! canonical byte string over: every relation's live extent (in canonical
//! relation-name then canonical-row order — the `BTreeMap`s already give
//! this), then the sealed provenance edges (Support, Claim, Masked,
//! KeyConflict, RuleError, Violation), each family in canonical order. Two
//! settled revisions are equal iff their dump bytes are equal; the digest of
//! the dump is the one number a harness needs to compare.

use brix_canon::{CanonWriter, Canonical, Digest, Domain};

use crate::eval::Settled;
use crate::provenance::Provenance;

/// Append the full canonical encoding of `settled` to `w`. Field order here
/// is this module's ABI for the dump format; it never needs to match any
/// user-facing schema, only to be stable and total.
pub fn write_dump(settled: &Settled, w: &mut CanonWriter) {
    w.write_uint(settled.at_revision);

    // Live extents, relation by relation (BTreeMap → canonical name order),
    // row by row (Extent keyed by canonical row bytes → canonical row order).
    w.write_uint(settled.extents.len() as u64);
    for (rel, extent) in &settled.extents {
        w.write_tag(rel);
        w.write_uint(extent.len() as u64);
        for record in extent.values() {
            record.row.canon_write(w);
            // Provenance counts make support dynamics (Appendix I.3)
            // visible in the dump: an edge kept alive by two supports vs.
            // one is a different settled state.
            w.write_uint(record.claims.len() as u64);
            w.write_uint(record.supports.len() as u64);
        }
    }

    write_provenance(&settled.provenance, w);
}

fn write_provenance(p: &Provenance, w: &mut CanonWriter) {
    // Each family is emitted in its stored order. The evaluator produces
    // supports/masked/etc. in a deterministic order (phase order, then
    // sorted rule ids, then canonical-row iteration), so this is already
    // canonical; we sort-normalize the ones whose production order is not
    // obviously canonical to be safe against future evaluator changes.
    let mut supports = p.supports.clone();
    supports.sort();
    w.write_uint(supports.len() as u64);
    for s in &supports {
        w.write_bytes(s.edge.as_bytes());
        w.write_tag(&s.relation);
        w.write_tag(&s.rule);
        w.write_bytes(s.match_digest.as_bytes());
        w.write_uint(s.at_revision);
    }

    let mut claims = p.claims.clone();
    claims.sort();
    w.write_uint(claims.len() as u64);
    for c in &claims {
        w.write_bytes(c.edge.as_bytes());
        w.write_tag(&c.relation);
        w.write_bytes(c.claim.digest().as_bytes());
        w.write_uint(c.at_revision);
    }

    let mut masked = p.masked.clone();
    masked.sort();
    w.write_uint(masked.len() as u64);
    for m in &masked {
        w.write_bytes(m.target.digest().as_bytes());
        w.write_bytes(m.by.digest().as_bytes());
        w.write_tag(&m.relation);
        w.write_tag(&m.rule);
        w.write_uint(m.at_phase as u64);
        w.write_uint(m.at_revision);
    }

    let mut conflicts = p.key_conflicts.clone();
    conflicts.sort();
    w.write_uint(conflicts.len() as u64);
    for k in &conflicts {
        w.write_tag(&k.relation);
        w.write_bytes(&k.key);
        w.write_uint(k.candidates.len() as u64);
        for cand in &k.candidates {
            w.write_bytes(cand.as_bytes());
        }
        w.write_uint(k.at_revision);
    }

    let mut errors = p.rule_errors.clone();
    errors.sort();
    w.write_uint(errors.len() as u64);
    for e in &errors {
        w.write_tag(&e.rule);
        w.write_tag(&e.site);
        w.write_bytes(e.partial_match.as_bytes());
        e.error.canon_write(w);
        w.write_uint(e.at_revision);
    }

    let mut violations = p.violations.clone();
    violations.sort();
    w.write_uint(violations.len() as u64);
    for v in &violations {
        w.write_tag(&v.constraint);
        w.write_bytes(v.match_digest.as_bytes());
        w.write_uint(v.at_revision);
    }
}

/// The full canonical byte string of a settled revision.
pub fn dump_bytes(settled: &Settled) -> Vec<u8> {
    let mut w = CanonWriter::new();
    write_dump(settled, &mut w);
    w.finish()
}

/// The digest of a settled revision's canonical dump — the single value a
/// differential harness compares (a mismatch localizes to a revision;
/// bytes-vs-bytes then localizes within it).
pub fn dump_digest(settled: &Settled) -> Digest {
    Digest::of(Domain::Value, &dump_bytes(settled))
}

/// A human-/snapshot-readable rendering of a settled revision. **Not**
/// canonical bytes (that is [`dump_bytes`]) — this is for `insta` snapshots
/// and `brix run` output, so it renders values structurally. It is derived
/// deterministically from the same `BTreeMap` orders, so it is stable.
pub fn render(settled: &Settled) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "revision {}", settled.at_revision);
    for (rel, extent) in &settled.extents {
        if extent.is_empty() {
            continue;
        }
        let _ = writeln!(out, "  {rel}:");
        for record in extent.values() {
            let _ = writeln!(out, "    {}", render_row(&record.row));
        }
    }
    let p = &settled.provenance;
    if !p.masked.is_empty() {
        let _ = writeln!(out, "  Masked: {}", p.masked.len());
    }
    if !p.key_conflicts.is_empty() {
        let _ = writeln!(out, "  KeyConflict:");
        for k in &p.key_conflicts {
            let _ = writeln!(
                out,
                "    {} ({} candidates)",
                k.relation,
                k.candidates.len()
            );
        }
    }
    if !p.rule_errors.is_empty() {
        let _ = writeln!(out, "  RuleError:");
        for e in &p.rule_errors {
            let _ = writeln!(out, "    {} @ {}: {:?}", e.rule, e.site, e.error);
        }
    }
    if !p.violations.is_empty() {
        let _ = writeln!(out, "  Violation:");
        for v in &p.violations {
            let _ = writeln!(out, "    {}", v.constraint);
        }
    }
    out
}

fn render_row(row: &crate::row::Row) -> String {
    use std::fmt::Write;
    let mut s = String::from("{ ");
    for (i, (role, val)) in row.0.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{role}: {}", render_value(val));
    }
    s.push_str(" }");
    s
}

fn render_value(v: &crate::value::Value) -> String {
    use crate::value::Value::*;
    match v {
        Nat(n) => n.to_string(),
        Int(n) => n.to_string(),
        Bool(b) => b.to_string(),
        Str(s) => format!("{s:?}"),
        Node(id) => format!("Node({})", short(&id.digest())),
        Edge(id) => format!("Edge({})", short(&id.digest())),
        Claim(id) => format!("Claim({})", short(&id.digest())),
        Enum { name, .. } => (*name).to_string(),
        Unit => "()".to_string(),
    }
}

fn short(d: &Digest) -> String {
    d.to_hex()[..8].to_string()
}
