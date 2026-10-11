//! `brix world <op>` — persistent world runtime CLI operations (ADR-0046, P5).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_kb::world::audit::{
    build_checkpoint_bundle_from_session, build_genesis_bundle_from_session,
    decode_world_audit_bundle_v1, ExecProfileV1, ProgramClosureV1, ScopeV1, WorldAuditDecodeLimits,
};
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyOptions};
use brix_kb::world::{OpenPolicy, TupleRecord, WorldBatch, WorldError, WorldKey, WorldSession};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use serde_json::json;

use crate::cli::WorldOp;
use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};

/// The JSON schema tag for `brix world` output (ADR-0046 P5).
pub const WORLD_JSON_SCHEMA: &str = "brix.cli.world-result@1";

/// Main entry point for `brix world` subcommand execution.
pub fn execute_world(op: &WorldOp, json_out: bool) -> u8 {
    match op {
        WorldOp::Init {
            dir,
            program,
            package_paths,
        } => execute_init(dir, program, package_paths, json_out),
        WorldOp::Batch {
            dir,
            batch_file,
            replay,
        } => execute_batch(dir, batch_file, *replay, json_out),
        WorldOp::Query {
            dir,
            relation,
            cursor,
            limit,
            replay,
        } => execute_query(
            dir,
            relation,
            cursor.as_deref(),
            limit.unwrap_or(50),
            *replay,
            json_out,
        ),
        WorldOp::Decide {
            dir,
            decide,
            entity,
            replay,
        } => execute_decide(dir, decide.as_deref(), entity.as_deref(), *replay, json_out),
        WorldOp::Show { dir, rev, replay } => execute_show(dir, *rev, *replay, json_out),
        WorldOp::OpenCheck {
            dir,
            verify_state,
            refresh_state,
            verify_store,
        } => execute_open_check(dir, *verify_state, *refresh_state, *verify_store, json_out),
        WorldOp::Gc {
            dir,
            dry_run,
            keep_revisions,
        } => execute_gc(dir, *dry_run, *keep_revisions, json_out),
        WorldOp::Explain {
            dir,
            entity,
            decide,
            rev,
            replay,
        } => execute_explain(dir, entity, decide.as_deref(), *rev, *replay, json_out),
        WorldOp::Audit {
            dir,
            out,
            from_checkpoint,
        } => execute_audit(dir, out, *from_checkpoint, json_out),
        WorldOp::Verify {
            expect_head,
            expect_program,
            trust_checkpoint,
            max_work,
            in_place,
            bundle,
        } => execute_verify(
            expect_head,
            expect_program,
            trust_checkpoint.as_deref(),
            *max_work,
            in_place.as_deref(),
            bundle.as_deref(),
            json_out,
        ),
        WorldOp::ImportKb { kb_dir, world_dir } => execute_import_kb(kb_dir, world_dir, json_out),
    }
}

/// Helper to load a program file and its imports into a source map closure.
pub fn load_program_and_sources(
    program: &Path,
    package_paths: &[PathBuf],
) -> Result<(String, BTreeMap<String, String>), WorldError> {
    let mut sources = BTreeMap::new();
    let root_src = fs::read_to_string(program).map_err(|e| {
        WorldError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read program file '{}': {e}", program.display()),
        ))
    })?;

    let root_stem = program
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("root")
        .to_string();

    sources.insert(root_stem.clone(), root_src);

    let prog_parent = program.parent().unwrap_or_else(|| Path::new("."));
    let mut search_dirs = vec![prog_parent.to_path_buf()];
    search_dirs.extend_from_slice(package_paths);

    // Read sibling .brix files into sources for import resolution
    for search_dir in &search_dirs {
        if let Ok(entries) = fs::read_dir(search_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("brix") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        if !sources.contains_key(stem) {
                            if let Ok(src) = fs::read_to_string(&p) {
                                sources.insert(stem.to_string(), src);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok((root_stem, sources))
}

fn execute_init(dir: &Path, program: &Path, package_paths: &[PathBuf], json_out: bool) -> u8 {
    let (root_name, sources) = match load_program_and_sources(program, package_paths) {
        Ok(res) => res,
        Err(e) => return print_world_error("world init", &e, json_out),
    };

    let loader = |name: &str| sources.get(name).cloned();
    let graph = match ModuleGraph::load(&root_name, &loader, ModuleLoaderLimits::default()) {
        Ok(g) => g,
        Err(e) => {
            let err = WorldError::NetworkError(format!("module load error: {e}"));
            return print_world_error("world init", &err, json_out);
        }
    };

    let linked = match graph.link() {
        Ok(l) => l,
        Err(e) => {
            let err = WorldError::NetworkError(format!("module link error: {e}"));
            return print_world_error("world init", &err, json_out);
        }
    };

    let session = match WorldSession::from_program_with_sources(dir, &linked.root_module, &sources)
    {
        Ok(s) => s,
        Err(e) => return print_world_error("world init", &e, json_out),
    };

    if json_out {
        let relations: Vec<String> = session.manifest.relations.keys().cloned().collect();
        let res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world init",
            "ok": true,
            "dir": dir.display().to_string(),
            "revision": session.current_revision,
            "digest": session.current_revision_digest.map(|d| d.to_hex()),
            "program_digest": session.manifest().program_digest.to_hex(),
            "relations": relations,
            "exec_profile": session.exec_profile.as_ref().map(|p| p.to_json()),
        });
        crate::json::emit_result_json(&res);
    } else {
        println!(
            "Initialized world at {} (revision {}, digest: {})",
            dir.display(),
            session.current_revision,
            session
                .current_revision_digest
                .map(|d| d.to_hex())
                .unwrap_or_default()
        );
        if let Some(profile) = &session.exec_profile {
            println!(
                "  Exec Profile: {} (evaluator: {}, crate: {})",
                brix_kb::world::EXEC_PROFILE_SCHEMA,
                profile.evaluator,
                profile.crate_version
            );
            println!(
                "    Module Loader Limits: depth={} modules={} module_bytes={} total_bytes={}",
                profile.module_loader_limits.depth,
                profile.module_loader_limits.modules,
                profile.module_loader_limits.module_bytes,
                profile.module_loader_limits.total_bytes
            );
            println!(
                "    Numeric Semantics: {}; Settlement: {}",
                profile.numeric_semantics, profile.settlement
            );
        }
    }
    EXIT_SUCCESS
}

fn execute_batch(dir: &Path, batch_file: &Path, replay: bool, json_out: bool) -> u8 {
    let mut session = match open_session(dir, replay) {
        Ok(s) => s,
        Err(e) => return print_world_error("world batch", &e, json_out),
    };

    let bytes = match fs::read(batch_file) {
        Ok(b) => b,
        Err(e) => {
            let err = WorldError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to read batch file '{}': {e}", batch_file.display()),
            ));
            return print_world_error("world batch", &err, json_out);
        }
    };

    let batch_val: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            let err = WorldError::Json(format!("invalid batch JSON: {e}"));
            return print_world_error("world batch", &err, json_out);
        }
    };

    let batch = match WorldBatch::from_json(&batch_val) {
        Ok(b) => b,
        Err(e) => return print_world_error("world batch", &e, json_out),
    };

    let receipt = match session.apply_batch(batch) {
        Ok(r) => r,
        Err(e) => return print_world_error("world batch", &e, json_out),
    };

    let decision_root = session
        .pin_revision(receipt.revision_seq)
        .ok()
        .and_then(|snap| snap.revision.decision_root.map(|d| d.to_hex()));

    if json_out {
        let mut res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world batch",
            "ok": true,
            "revision": receipt.revision_seq,
            "digest": receipt.revision_digest.to_hex(),
            "idempotency_key": receipt.idempotency_key,
            "is_idempotent_replay": receipt.is_idempotent_replay,
            "changed_keys_count": receipt.changed_keys_count,
            "objects_written": receipt.objects_written,
            "decision_root": decision_root,
        });
        add_open_info(&mut res, &session);
        crate::json::emit_result_json(&res);
    } else if receipt.is_idempotent_replay {
        println!(
            "Idempotent replay: revision {} (digest: {})",
            receipt.revision_seq,
            receipt.revision_digest.to_hex()
        );
    } else {
        println!(
            "Committed revision {} (digest: {}, changed: {}, objects: {})",
            receipt.revision_seq,
            receipt.revision_digest.to_hex(),
            receipt.changed_keys_count,
            receipt.objects_written
        );
    }
    EXIT_SUCCESS
}

fn execute_query(
    dir: &Path,
    relation: &str,
    cursor: Option<&str>,
    limit: usize,
    replay: bool,
    json_out: bool,
) -> u8 {
    let session = match open_session(dir, replay) {
        Ok(s) => s,
        Err(e) => return print_world_error("world query", &e, json_out),
    };

    let derived_known = match session.get_derived(relation) {
        Ok(rows) => rows.is_some(),
        Err(e) => return print_world_error("world query", &e, json_out),
    };
    let page = if session.relations.contains_key(relation) {
        session.query_page(relation, cursor, limit)
    } else if derived_known {
        session.query_derived_page(relation, cursor, limit)
    } else if let Some(qualified) = session
        .relations
        .keys()
        .find(|k| k.ends_with(&format!("::{relation}")))
        .cloned()
    {
        session.query_page(&qualified, cursor, limit)
    } else if let Some(qualified) = session
        .network
        .as_ref()
        .and_then(|n| {
            n.derived_relations
                .keys()
                .find(|k| k.ends_with(&format!("::{relation}")))
        })
        .cloned()
    {
        session.query_derived_page(&qualified, cursor, limit)
    } else {
        Err(WorldError::UnknownRelation(relation.to_string()))
    };

    let page = match page {
        Ok(p) => p,
        Err(e) => return print_world_error("world query", &e, json_out),
    };

    let entries_json: Vec<serde_json::Value> = page
        .entries
        .iter()
        .map(|(k, t)| {
            let key_str = std::str::from_utf8(k.as_bytes())
                .map(|s| s.to_string())
                .unwrap_or_else(|_| k.to_hex());
            if let Ok(rec) = TupleRecord::from_tuple(t) {
                let mut fields = serde_json::Map::new();
                for (fname, fbytes) in &rec.fields {
                    let s = std::str::from_utf8(fbytes)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|_| WorldKey::new(fbytes.clone()).to_hex());
                    fields.insert(fname.clone(), serde_json::Value::String(s));
                }
                json!({ "key": key_str, "tuple": fields })
            } else {
                json!({ "key": key_str, "tuple": t.to_hex() })
            }
        })
        .collect();

    if json_out {
        let mut res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world query",
            "ok": true,
            "relation": page.relation,
            "count": entries_json.len(),
            "has_more": page.has_more,
            "next_cursor": page.next_cursor,
            "entries": entries_json,
        });
        add_open_info(&mut res, &session);
        crate::json::emit_result_json(&res);
    } else {
        println!(
            "Query relation '{}' ({} entries):",
            page.relation,
            page.entries.len()
        );
        for (k, t) in &page.entries {
            let key_str = std::str::from_utf8(k.as_bytes())
                .map(|s| s.to_string())
                .unwrap_or_else(|_| k.to_hex());
            if let Ok(rec) = TupleRecord::from_tuple(t) {
                let formatted: Vec<String> = rec
                    .fields
                    .iter()
                    .map(|(f, v)| {
                        let s = std::str::from_utf8(v).unwrap_or("<bytes>");
                        format!("{f}={s}")
                    })
                    .collect();
                println!("  [{key_str}] {}", formatted.join(", "));
            } else {
                println!("  [{key_str}] {}", t.to_hex());
            }
        }
        if page.has_more {
            println!(
                "  ... more entries available (next_cursor: {:?})",
                page.next_cursor
            );
        }
    }
    EXIT_SUCCESS
}

fn execute_decide(
    dir: &Path,
    decide_filter: Option<&str>,
    entity_filter: Option<&str>,
    replay: bool,
    json_out: bool,
) -> u8 {
    let session = match open_session(dir, replay) {
        Ok(s) => s,
        Err(e) => return print_world_error("world decide", &e, json_out),
    };

    let mut settlements = match session.all_settlements() {
        Ok(s) => s,
        Err(e) => return print_world_error("world decide", &e, json_out),
    };
    if let Some(dec) = decide_filter {
        settlements.retain(|k, _| k == dec || k.ends_with(&format!("::{dec}")));
    }
    if let Some(ent) = entity_filter {
        for entity_map in settlements.values_mut() {
            entity_map.retain(|k, _| k == ent);
        }
        settlements.retain(|_, map| !map.is_empty());
    }

    if json_out {
        let mut settlements_json = serde_json::Map::new();
        for (dec_name, entity_map) in settlements {
            let mut emap = serde_json::Map::new();
            for (eid, s) in entity_map {
                emap.insert(eid, s.to_json());
            }
            settlements_json.insert(dec_name, serde_json::Value::Object(emap));
        }
        let mut res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world decide",
            "ok": true,
            "settlements": settlements_json,
        });
        add_open_info(&mut res, &session);
        crate::json::emit_result_json(&res);
    } else {
        println!("Settled decisions:");
        if settlements.is_empty() {
            println!("  (no active decisions settled)");
        } else {
            for (dec_name, entity_map) in &settlements {
                println!("  Decide '{}':", dec_name);
                for (eid, dec) in entity_map {
                    println!(
                        "    entity '{}' -> candidate '{}' (priority {}, value: {})",
                        eid, dec.candidate_name, dec.priority, dec.value
                    );
                }
            }
        }
    }
    EXIT_SUCCESS
}

fn execute_show(dir: &Path, rev_opt: Option<u64>, replay: bool, json_out: bool) -> u8 {
    let session = match open_session(dir, replay) {
        Ok(s) => s,
        Err(e) => return print_world_error("world show", &e, json_out),
    };

    let seq = rev_opt.unwrap_or(session.current_revision);
    let snap = match session.pin_revision(seq) {
        Ok(sn) => sn,
        Err(e) => return print_world_error("world show", &e, json_out),
    };

    if json_out {
        let mut res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world show",
            "ok": true,
            "revision": snap.revision.to_json(),
        });
        add_open_info(&mut res, &session);
        crate::json::emit_result_json(&res);
    } else {
        let rev = &snap.revision;
        println!("World Revision {} ({})", rev.seq, rev.timestamp);
        println!("  Digest: {}", rev.revision_digest.to_hex());
        println!("  Idempotency Key: {}", rev.idempotency_key);
        if let Some(ref d) = rev.batch_digest {
            println!("  Batch Digest: {}", d.to_hex());
        }
        if let Some(ref d) = rev.decision_root {
            println!("  Decision Root: {}", d.to_hex());
        }
        println!("  Relations ({}):", rev.relation_roots.len());
        for (rel, root) in &rev.relation_roots {
            let card = rev.relation_cardinalities.get(rel).copied().unwrap_or(0);
            println!("    - {rel}: {card} tuples (root: {})", root.to_hex());
        }
    }
    EXIT_SUCCESS
}

fn execute_explain(
    dir: &Path,
    entity: &str,
    decide_filter: Option<&str>,
    rev: Option<u64>,
    replay: bool,
    json_out: bool,
) -> u8 {
    let session = match open_session(dir, replay) {
        Ok(s) => s,
        Err(e) => return print_world_error("world explain", &e, json_out),
    };

    // A past revision (ADR-0046 P6 G2, decided 2026-10-04): read the real
    // persisted historical decision from the node store, never a replay.
    // Only the live head gets the richer in-memory deliberation below (full
    // candidate list, contributing base facts) — a past revision's candidate
    // frontier is not persisted, only its settled winners.
    if let Some(seq) = rev {
        if seq != session.current_revision {
            let found = match session.historical_settlement(seq, decide_filter, entity) {
                Ok(f) => f,
                Err(e) => return print_world_error("world explain", &e, json_out),
            };
            let Some((decide_name, settlement)) = found else {
                let err = WorldError::NetworkError(format!(
                    "no settled decision found for entity '{entity}' at revision {seq}"
                ));
                return print_world_error("world explain", &err, json_out);
            };
            if json_out {
                let mut res = json!({
                    "schema": WORLD_JSON_SCHEMA,
                    "command": "world explain",
                    "ok": true,
                    "authority": "derived",
                    "revision": seq,
                    "explanation": {
                        "entity_id": settlement.entity_id,
                        "decide_name": decide_name,
                        "winning_candidate": settlement.candidate_name,
                        "value": settlement.value.to_string(),
                        "priority": settlement.priority,
                        "phase": settlement.phase,
                    },
                });
                add_open_info(&mut res, &session);
                crate::json::emit_result_json(&res);
            } else {
                println!("Historical decision at revision {seq} for entity '{entity}':");
                println!("  authority: Derived");
                println!("  Decide Block: {decide_name}");
                println!(
                    "  Winner: {} (priority {}, value: {})",
                    settlement.candidate_name, settlement.priority, settlement.value
                );
                println!(
                    "  (read from the persisted decision trie at revision {seq}; full candidate \
                     deliberation is only available at the current head)"
                );
            }
            return EXIT_SUCCESS;
        }
    }

    let explanation = match decide_filter {
        Some(d) => match session.explain_decision_for(d, entity) {
            Ok(Some(exp)) => Ok(Some(exp)),
            Ok(None) => {
                let qualified: Vec<String> = session
                    .network
                    .as_ref()
                    .map(|n| {
                        n.decides
                            .keys()
                            .filter(|k| k.ends_with(&format!("::{d}")))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                let mut found = Ok(None);
                for q in &qualified {
                    match session.explain_decision_for(q, entity) {
                        Ok(None) => continue,
                        other => {
                            found = other;
                            break;
                        }
                    }
                }
                found
            }
            Err(e) => Err(e),
        },
        None => session.explain_decision(entity),
    };
    let explanation = match explanation {
        Ok(e) => e,
        Err(e) => return print_world_error("world explain", &e, json_out),
    };

    let explanation = match explanation {
        Some(exp) => exp,
        None => {
            let err = WorldError::NetworkError(format!(
                "no settled decision or candidate frontier found for entity '{entity}'"
            ));
            return print_world_error("world explain", &err, json_out);
        }
    };

    if json_out {
        let mut res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world explain",
            "ok": true,
            "explanation": explanation.to_json(),
        });
        add_open_info(&mut res, &session);
        crate::json::emit_result_json(&res);
    } else {
        println!(
            "Decision explanation for entity '{}':",
            explanation.entity_id
        );
        println!("  Decide Block: {}", explanation.decide_name);
        println!(
            "  Winner: {} (priority {:?}, value: {:?})",
            explanation.winning_candidate.as_deref().unwrap_or("<none>"),
            explanation.priority.unwrap_or(0),
            explanation
                .value
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default()
        );
        println!("  Candidates ({} evaluated):", explanation.candidates.len());
        for c in &explanation.candidates {
            let win_mark = if c.winning { " [SELECTED]" } else { "" };
            println!(
                "    - '{}' (priority {}, value: {}, supports: {}){}",
                c.name, c.priority, c.value, c.supports_count, win_mark
            );
        }
        println!(
            "  Contributing Base Facts ({}):",
            explanation.contributing_facts.len()
        );
        for (rel, key) in &explanation.contributing_facts {
            let key_str = std::str::from_utf8(key.as_bytes())
                .map(|s| s.to_string())
                .unwrap_or_else(|_| key.to_hex());
            println!("    - relation '{rel}', key '{key_str}'");
        }
    }
    EXIT_SUCCESS
}

/// `WorldError::NetworkError` carries the stringified module-load/link and
/// relational-lowering errors (`ModuleLinkError`, `RelationalLowerError`,
/// world-expression lowering), which are not its own `WorldError` variants.
/// Classify their messages into the P5 status taxonomy (ADR-0046 §4 P5:
/// missing inputs / unsupported operators / exhaustion / cached prior
/// results must be distinguishable) so a missing import, a bounded-loader
/// limit refusal, and a genuinely unsupported relational construct are not
/// all flattened into one generic "network-error" bucket.
fn classify_network_error(msg: &str) -> (&'static str, u8) {
    if msg.contains("was not found") || msg.contains("unresolved symbol") {
        // A missing module/package import, or a reference to a symbol that
        // does not exist in the resolved closure: a missing input, exactly
        // like `WorldError::UnknownRelation`/`ManifestNotFound`.
        ("missing-input", EXIT_REJECTED_OR_UNKNOWN)
    } else if msg.contains("exhaust") || msg.contains("exceeded limit") {
        // `ModuleLinkError::{ImportDepthExceeded,ModuleCountExceeded,
        // ModuleBytesExceeded,TotalBytesExceeded}`: a bounded-loader refusal
        // of insufficiently provisioned work (ADR-0046 §3.8), the world
        // profile's operational-exhaustion case.
        ("resource-exhaustion", EXIT_REJECTED_OR_UNKNOWN)
    } else if msg.contains("no settled decision") {
        ("missing-decision", EXIT_REJECTED_OR_UNKNOWN)
    } else if msg.contains("unsupported relational profile")
        || msg.contains("not admitted in profile")
        || msg.contains("is not exported")
        || msg.contains("cycle detected")
        || msg.contains("Unsupported(")
        || msg.contains("world expression lowering")
    {
        // `RelationalLowerError::{RecursiveRelationCycle,UnstratifiedNegation,
        // UnsupportedNegation}`, `ModuleLinkError::{Cycle,NonExportedAccess}`,
        // and world-expression lowering refusals: the program asks for a
        // construct this profile does not admit, distinct from a missing
        // input or an exhausted budget.
        ("unsupported-operator", EXIT_USAGE_OR_IO)
    } else {
        ("network-error", EXIT_REJECTED_OR_UNKNOWN)
    }
}

/// Open a world, restoring by default or forcing a replay of the base facts (`--replay`).
fn open_session(dir: &Path, replay: bool) -> Result<WorldSession, WorldError> {
    if replay {
        WorldSession::open_replay(dir)
    } else {
        WorldSession::open(dir)
    }
}

/// The fields describing how a session was opened (R0 M4).
fn open_info(session: &WorldSession) -> serde_json::Value {
    json!({
        "open_ms": session.open_ms,
        "open_mode": session.open_mode.as_str(),
        "replay_reason": session.replay_reason,
        "state_source": session.state_source,
        "state_refreshed": session.state_refreshed,
        "tail_batches": session.tail_batches,
    })
}

fn add_open_info(res: &mut serde_json::Value, session: &WorldSession) {
    if let (Some(obj), serde_json::Value::Object(info)) = (res.as_object_mut(), open_info(session))
    {
        obj.extend(info);
    }
}

fn execute_open_check(
    dir: &Path,
    verify_state: bool,
    refresh_state: bool,
    verify_store: bool,
    json_out: bool,
) -> u8 {
    const CMD: &str = "world open-check";
    // The pack audit runs first: damage it names is a typed corrupt-state error, not whatever
    // the open happens to trip over.
    let store_report = if verify_store {
        match brix_kb::world::gc::verify_store(dir) {
            Ok(r) => Some(r),
            Err(e) => return print_world_error(CMD, &e, json_out),
        }
    } else {
        None
    };
    // Refreshing is explicit here, not a side effect of the open.
    let policy = OpenPolicy {
        force_replay: false,
        refresh_after_replay: false,
    };
    let mut session = match WorldSession::open_with_policy(dir, policy) {
        Ok(s) => s,
        Err(e) => return print_world_error(CMD, &e, json_out),
    };
    if verify_state {
        if let Err(e) = session.verify_state() {
            return print_world_error(CMD, &e, json_out);
        }
    }
    let mut checkpoint_entries = None;
    if verify_state {
        match session.verify_settlement_checkpoint() {
            Ok(n) => checkpoint_entries = n,
            Err(e) => return print_world_error(CMD, &e, json_out),
        }
    }
    let mut refreshed = None;
    if refresh_state {
        refreshed = Some(match session.refresh_state_cache() {
            Ok(()) => Ok(()),
            Err(e) => Err(e.to_string()),
        });
        // The settlement checkpoint (decisions read cache) is refreshed with the state cache;
        // a world with no decide blocks has none to write.
        if session
            .network
            .as_ref()
            .is_some_and(|n| !n.decides.is_empty())
        {
            match session.refresh_settlement_checkpoint() {
                Ok(n) => checkpoint_entries = Some(n),
                Err(e) => refreshed = Some(Err(e.to_string())),
            }
        }
    }
    let refresh_error = refreshed.as_ref().and_then(|r| r.as_ref().err().cloned());
    if json_out {
        let mut res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": CMD,
            "ok": true,
            "revision": session.current_revision,
            "network_state_root": session.network_state_root.map(|d| d.to_hex()),
            "verified_state": verify_state,
            "verified_store": store_report.map(|r| json!({
                "packs": r.packs,
                "nodes": r.nodes,
                "body_bytes": r.body_bytes,
                "legacy_nodes": r.legacy_nodes,
            })),
            "refreshed_state": refreshed.as_ref().map(|r| r.is_ok()),
            "refresh_error": refresh_error,
            "settlement_checkpoint_entries": checkpoint_entries,
        });
        add_open_info(&mut res, &session);
        crate::json::emit_result_json(&res);
    } else {
        println!(
            "revision {}: opened {} in {} ms{}{}",
            session.current_revision,
            session.open_mode.as_str(),
            session.open_ms,
            session
                .replay_reason
                .map(|r| format!(" (replay: {r})"))
                .unwrap_or_default(),
            session
                .state_source
                .map(|s| format!(" (state: {s})"))
                .unwrap_or_default(),
        );
        if verify_state {
            println!("state verified against a replay of the base facts");
        }
        if let Some(r) = store_report {
            println!(
                "store verified: {} packs, {} nodes, {} body bytes, {} legacy objects",
                r.packs, r.nodes, r.body_bytes, r.legacy_nodes
            );
        }
        match refreshed {
            Some(Ok(())) => println!("state cache refreshed"),
            Some(Err(e)) => println!("state cache not refreshed: {e}"),
            None => {}
        }
    }
    if refresh_error.is_some() {
        EXIT_REJECTED_OR_UNKNOWN
    } else {
        EXIT_SUCCESS
    }
}

fn execute_gc(dir: &Path, dry_run: bool, keep_revisions: Option<u64>, json_out: bool) -> u8 {
    const CMD: &str = "world gc";
    let opts = brix_kb::world::gc::GcOptions {
        dry_run,
        keep_revisions,
        ..Default::default()
    };
    let r = match brix_kb::world::gc::gc_world(dir, &opts) {
        Ok(r) => r,
        Err(e) => return print_world_error(CMD, &e, json_out),
    };
    if json_out {
        crate::json::emit_result_json(&json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": CMD,
            "ok": true,
            "dry_run": r.dry_run,
            "head_revision": r.head_revision,
            "revisions_total": r.revisions_total,
            "revisions_retained": r.revisions_retained,
            "roots": r.roots,
            "live_nodes": r.live_nodes,
            "live_bytes": r.live_bytes,
            "nodes_before": r.nodes_before,
            "dead_nodes": r.dead_nodes,
            "bytes_before": r.bytes_before,
            "bytes_after": r.bytes_after,
            "packs_before": r.packs_before,
            "packs_after": r.packs_after,
            "files_swept": r.files_swept,
            "elapsed_ms": r.elapsed_ms,
        }));
    } else {
        println!(
            "{}: {} of {} nodes live ({} dead), {} revisions retained of {}",
            if r.dry_run { "dry run" } else { "gc" },
            r.live_nodes,
            r.nodes_before,
            r.dead_nodes,
            r.revisions_retained,
            r.revisions_total
        );
        println!(
            "store {} -> {} bytes, {} -> {} packs ({} ms)",
            r.bytes_before, r.bytes_after, r.packs_before, r.packs_after, r.elapsed_ms
        );
    }
    EXIT_SUCCESS
}

fn print_world_error(cmd: &str, err: &WorldError, json_out: bool) -> u8 {
    let (status, exit_code) = match err {
        WorldError::UnknownRelation(_)
        | WorldError::ManifestNotFound
        | WorldError::RevisionNotFound(_) => ("missing-input", EXIT_REJECTED_OR_UNKNOWN),
        WorldError::InvalidSchema(_) | WorldError::InvalidIndexDeclaration(_) => {
            ("unsupported-operator", EXIT_USAGE_OR_IO)
        }
        WorldError::BatchConflict { .. } | WorldError::IdempotencyConflict { .. } => {
            ("batch-conflict", EXIT_REJECTED_OR_UNKNOWN)
        }
        WorldError::StaleBaseRevision { .. } => ("stale-base-revision", EXIT_REJECTED_OR_UNKNOWN),
        // Another writer (or an operator command that writes) holds the world: retry.
        WorldError::WorldLocked(_) => ("world-locked", EXIT_REJECTED_OR_UNKNOWN),
        WorldError::StaleSession(_) => ("stale-session", EXIT_REJECTED_OR_UNKNOWN),
        // History dropped on purpose by `gc --keep-revisions`, not damage.
        WorldError::RevisionCompacted(_) => ("revision-compacted", EXIT_REJECTED_OR_UNKNOWN),
        WorldError::Io(_)
        | WorldError::Json(_)
        | WorldError::CorruptedHead(_)
        | WorldError::CorruptedRevision { .. }
        | WorldError::CorruptedObject(_) => ("io-or-corruption", EXIT_USAGE_OR_IO),
        WorldError::NetworkError(msg) => classify_network_error(msg),
        WorldError::CorruptState(_) | WorldError::State(_) => ("corrupt-state", EXIT_USAGE_OR_IO),
        WorldError::MissingState(_) => ("missing-state", EXIT_USAGE_OR_IO),
        WorldError::ValidityMismatch(_) | WorldError::StateMismatch(_) => {
            ("state-mismatch", EXIT_REJECTED_OR_UNKNOWN)
        }
        _ => ("unknown-error", EXIT_REJECTED_OR_UNKNOWN),
    };

    if json_out {
        let res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": cmd,
            "ok": false,
            "status": status,
            "errors": [err.to_string()],
        });
        crate::json::emit_result_json(&res);
    } else {
        eprintln!("brix {cmd}: {status}: {err}");
    }
    exit_code
}

fn parse_digest_hex(hex: &str) -> Result<Digest, String> {
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        let chunk = &hex[i * 2..i * 2 + 2];
        *byte = u8::from_str_radix(chunk, 16).map_err(|e| format!("invalid hex '{chunk}': {e}"))?;
    }
    Ok(Digest::from_bytes(bytes))
}

fn load_program_closure_from_dir(
    dir: &Path,
    default_manifest_digest: Digest,
) -> Result<(ProgramClosureV1, ExecProfileV1), WorldError> {
    let program_path = dir.join("program.json");
    if program_path.exists() {
        let bytes = fs::read(&program_path)?;
        let val: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let root_mod = val
            .get("root_module")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        let mut sources = Vec::new();
        if let Some(src_map) = val.get("sources").and_then(|x| x.as_object()) {
            for (k, v) in src_map {
                if let Some(s) = v.as_str() {
                    sources.push((k.clone(), s.to_string()));
                }
            }
        }
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        let program_manifest_digest =
            if let Some(hex) = val.get("program_manifest_digest").and_then(|x| x.as_str()) {
                parse_digest_hex(hex).map_err(|e| {
                    WorldError::InvalidSchema(format!("invalid program_manifest_digest: {e}"))
                })?
            } else {
                default_manifest_digest
            };
        let ep = if let Some(p) = val.get("exec_profile") {
            ExecProfileV1::from_json(p)?
        } else {
            ExecProfileV1::default()
        };
        Ok((
            ProgramClosureV1 {
                root_module: root_mod,
                sources,
                program_manifest_digest,
            },
            ep,
        ))
    } else {
        Ok((
            ProgramClosureV1 {
                root_module: String::new(),
                sources: Vec::new(),
                program_manifest_digest: default_manifest_digest,
            },
            ExecProfileV1::default(),
        ))
    }
}

fn execute_audit(
    dir: &Path,
    bundle_out: &Path,
    from_checkpoint: Option<u64>,
    json_out: bool,
) -> u8 {
    let session = match WorldSession::open(dir) {
        Ok(s) => s,
        Err(e) => return print_world_error("world audit", &e, json_out),
    };

    let (program_closure, exec_profile) =
        match load_program_closure_from_dir(dir, session.manifest().program_digest) {
            Ok(p) => p,
            Err(e) => return print_world_error("world audit", &e, json_out),
        };

    let bundle = if let Some(cp_seq) = from_checkpoint {
        match build_checkpoint_bundle_from_session(&session, cp_seq, program_closure, exec_profile)
        {
            Ok(b) => b,
            Err(e) => return print_world_error("world audit", &e, json_out),
        }
    } else {
        match build_genesis_bundle_from_session(&session, program_closure, exec_profile) {
            Ok(b) => b,
            Err(e) => return print_world_error("world audit", &e, json_out),
        }
    };

    let limits = WorldAuditDecodeLimits::default();
    let encoded_bytes = match bundle.encode(&limits) {
        Ok(b) => b,
        Err(e) => {
            return print_world_error(
                "world audit",
                &WorldError::NetworkError(e.to_string()),
                json_out,
            )
        }
    };

    if let Some(parent) = bundle_out.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    if let Err(e) = fs::write(bundle_out, &encoded_bytes) {
        return print_world_error("world audit", &WorldError::Io(e), json_out);
    }

    let scope_str = match &bundle.scope {
        ScopeV1::Genesis => "complete-from-genesis".to_string(),
        ScopeV1::Checkpoint { seq, .. } => format!("checkpoint-suffix ({seq}..HEAD)"),
    };

    let bundle_id = bundle.id();
    if json_out {
        let res = json!({
            "schema": WORLD_JSON_SCHEMA,
            "command": "world audit",
            "ok": true,
            "dir": dir.display().to_string(),
            "bundle": bundle_out.display().to_string(),
            "bundle_id": bundle_id.to_hex(),
            "head_seq": bundle.head.seq,
            "head_digest": bundle.head.revision_digest.to_hex(),
            "scope": scope_str,
            "revisions_count": bundle.revisions.len(),
        });
        crate::json::emit_result_json(&res);
    } else {
        println!("dir: {}", dir.display());
        println!("bundle: {}", bundle_out.display());
        println!("bundle_id: {}", bundle_id.to_hex());
        println!("head_seq: {}", bundle.head.seq);
        println!("head_digest: {}", bundle.head.revision_digest.to_hex());
        println!("scope: {scope_str}");
        println!("revisions: {}", bundle.revisions.len());
    }
    EXIT_SUCCESS
}

fn execute_verify(
    expect_head_hex: &str,
    expect_program_hex: &str,
    trust_checkpoint_hex: Option<&str>,
    max_work: Option<u64>,
    in_place: Option<&Path>,
    bundle_path: Option<&Path>,
    json_out: bool,
) -> u8 {
    let expect_head = match parse_digest_hex(expect_head_hex) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("brix world verify: invalid --expect-head hex: {e}");
            return EXIT_USAGE_OR_IO;
        }
    };
    let expect_program = match parse_digest_hex(expect_program_hex) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("brix world verify: invalid --expect-program hex: {e}");
            return EXIT_USAGE_OR_IO;
        }
    };
    let trust_checkpoint = if let Some(hex) = trust_checkpoint_hex {
        match parse_digest_hex(hex) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("brix world verify: invalid --trust-checkpoint hex: {e}");
                return EXIT_USAGE_OR_IO;
            }
        }
    } else {
        None
    };

    let limits = WorldAuditDecodeLimits::default();
    let options = VerifyOptions {
        expect_head,
        expect_program,
        trust_checkpoint,
        limits,
        max_work,
    };

    let bundle = if let Some(dir) = in_place {
        let session = match WorldSession::open(dir) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("brix world verify: failed to open world for in-place verification: {e}");
                return EXIT_USAGE_OR_IO;
            }
        };
        let (program_closure, exec_profile) =
            match load_program_closure_from_dir(dir, session.manifest().program_digest) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("brix world verify: failed to load program closure: {e}");
                    return EXIT_USAGE_OR_IO;
                }
            };
        match build_genesis_bundle_from_session(&session, program_closure, exec_profile) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("brix world verify: failed to build in-place audit bundle: {e}");
                return EXIT_USAGE_OR_IO;
            }
        }
    } else if let Some(path) = bundle_path {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("brix world verify: failed to read bundle file: {e}");
                return EXIT_USAGE_OR_IO;
            }
        };
        match decode_world_audit_bundle_v1(&bytes, &limits) {
            Ok(b) => b,
            Err(e) => {
                let refusal = format!("Unknown(bundle-decode-error:{e})");
                if json_out {
                    let res = json!({
                        "schema": "brix.cli.world-verify-result@1",
                        "command": "world verify",
                        "ok": false,
                        "status": "unknown",
                        "errors": [refusal],
                    });
                    crate::json::emit_result_json(&res);
                } else {
                    println!("{refusal}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
        }
    } else {
        eprintln!("brix world verify: either <bundle> or --in-place <dir> is required");
        return EXIT_USAGE_OR_IO;
    };

    match verify_world_audit_bundle(&bundle, &options) {
        Ok(report) => {
            if json_out {
                let settlements_json: Vec<serde_json::Value> = report
                    .verified_settlements
                    .iter()
                    .map(|(seq, decide, ent, cand)| {
                        json!({
                            "seq": seq,
                            "decide": decide,
                            "entity": ent,
                            "candidate": cand,
                            "status": "audited",
                        })
                    })
                    .collect();
                let res = json!({
                    "schema": "brix.cli.world-verify-result@1",
                    "command": "world verify",
                    "ok": true,
                    "status": "audited",
                    "scope": report.scope,
                    "head_revision": report.head_revision,
                    "head_digest": report.head_digest.to_hex(),
                    "program_digest": report.program_digest.to_hex(),
                    "work": {
                        "tuples_decoded": report.work.tuples_decoded,
                        "trie_nodes_built": report.work.trie_nodes_built,
                        "index_rows_rebuilt": report.work.index_rows_rebuilt,
                        "candidates_evaluated": report.work.candidates_evaluated,
                        "settlements_computed": report.work.settlements_computed,
                        "revisions_replayed": report.work.revisions_replayed,
                    },
                    "settlements": settlements_json,
                });
                crate::json::emit_result_json(&res);
            } else {
                println!("scope: {}", report.scope);
                println!("status: audited");
                println!("head_revision: {}", report.head_revision);
                println!("head_digest: {}", report.head_digest.to_hex());
                println!("program_digest: {}", report.program_digest.to_hex());
                println!(
                    "work: tuples_decoded={} trie_nodes_built={} index_rows_rebuilt={} candidates_evaluated={} settlements_computed={} revisions_replayed={}",
                    report.work.tuples_decoded,
                    report.work.trie_nodes_built,
                    report.work.index_rows_rebuilt,
                    report.work.candidates_evaluated,
                    report.work.settlements_computed,
                    report.work.revisions_replayed,
                );
                println!("settlements: {} audited", report.verified_settlements.len());
            }
            EXIT_SUCCESS
        }
        Err(err) => {
            let refusal = err.to_string();
            if json_out {
                let res = json!({
                    "schema": "brix.cli.world-verify-result@1",
                    "command": "world verify",
                    "ok": false,
                    "status": "unknown",
                    "errors": [refusal],
                });
                crate::json::emit_result_json(&res);
            } else {
                println!("{refusal}");
            }
            EXIT_REJECTED_OR_UNKNOWN
        }
    }
}

fn execute_import_kb(kb_dir: &Path, world_dir: &Path, json_out: bool) -> u8 {
    match brix_kb::world::import_kb::import_kb(kb_dir, world_dir) {
        Ok(report) => {
            if json_out {
                let res = json!({
                    "schema": WORLD_JSON_SCHEMA,
                    "command": "world import-kb",
                    "ok": true,
                    "kb_dir": kb_dir.display().to_string(),
                    "world_dir": world_dir.display().to_string(),
                    "kb_head_seq": report.kb_head_seq,
                    "world_head_seq": report.world_head_seq,
                    "provenance_path": report.provenance_path.display().to_string(),
                });
                crate::json::emit_result_json(&res);
            } else {
                println!("imported KB v1 directory into world:");
                println!("  kb_dir: {}", kb_dir.display());
                println!("  world_dir: {}", world_dir.display());
                println!("  kb_head_seq: {}", report.kb_head_seq);
                println!("  world_head_seq: {}", report.world_head_seq);
                println!("  provenance: {}", report.provenance_path.display());
            }
            EXIT_SUCCESS
        }
        Err(err) => {
            let refusal = err.to_string();
            if json_out {
                let res = json!({
                    "schema": WORLD_JSON_SCHEMA,
                    "command": "world import-kb",
                    "ok": false,
                    "status": "unknown",
                    "errors": [refusal],
                });
                crate::json::emit_result_json(&res);
            } else {
                println!("{refusal}");
            }
            EXIT_REJECTED_OR_UNKNOWN
        }
    }
}
