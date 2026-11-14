//! Reproducible Atlas-shaped Alpha 9 world scale harness.
//!
//! This measures real `WorldSession`, incremental network, persistence, cold-open,
//! and scoped named-query paths. It is intentionally not a microbenchmark.
//!
//! Full-run modes:
//!   alpha9_scale_bench --full --facts 8000 --batches 10 --seed 0 --world-dir DIR
//!   alpha9_scale_bench --full --facts 8000 --batches 1 --seed 0 --world-dir DIR
//!
//! Fresh-process modes:
//!   alpha9_scale_bench --child-open DIR
//!   alpha9_scale_bench --child-query DIR
//!
//! The parent runner owns repetitions, timeouts, peak-RSS collection, and cleanup.

use brix_canon::{CanonWriter, Digest, Domain};
use brix_kb::world::decision_codec::Value;
use brix_kb::world::scoped_query::world_revision_artifact_preimage_v1;
use brix_kb::world::state_store::DEFAULT_NODE_CACHE_BYTES;
use brix_kb::world::{
    gc, AuthorizedHostScope, HostContextExpectationV2, HostQueryLimitsV1, HostScopeProfileV1,
    NamedQueryArgsV1, QueryValueV1, ScopedQueryPageRequestV1, TupleRecord, WorldBatch,
    WorldBatchOp, WorldKey, WorldSession, DEFAULT_APPLY_HYDRATED_BYTES,
    HOST_SCOPE_PROFILE_PARTITION_V1,
};
use brix_semantic::{
    encode_context_model_v2, ArtifactRefV2, AssumptionValidatorV2, ContextDimensionsV2, ContextId,
    ContextModelV2, ContextResolverV2, ModelLimits, ScenarioModeV2,
};
use serde_json::{json, Value as JsonValue};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EXAMPLE_SCHEMA: &str = "brix.alpha9.scale-bench@1";
const FIXTURE_SCHEMA: &str = "brix.alpha9.public-fixture@1";
const FIXTURE_FILE: &str = "alpha9-fixture.json";
const TARGET_FACTS: [usize; 3] = [8_000, 80_000, 800_000];
const QUERY_NAME: &str = "catalogue_page";
const QUERY_TENANT: &str = "tenant-0";
const QUERY_KIND: &str = "book";
const QUERY_PAGE_ROWS: usize = 32;
const RELATION_PAGE_ROWS: usize = 1024;
const QUERIED_DERIVED_RELATIONS: [&str; 4] = [
    "root::activity_pages",
    "root::all_entities",
    "root::fulfillment",
    "root::decision_value_join",
];

const PROGRAM_ROOT: &str = r#"
config Product = { sku: Str }
config Page = Page(List<Str>, Bool)
config DispatchResult = { b: Str }

rel input events: { id: Str, tenant: Str, parent: Str, sort: Int, value: Str } key id
rel input orders: { id: Str, tenant: Str, product: Product, qty: Int } key id
rel input inventory: { id: Str, sku: Str, available: Int } key id
rel input entity_a: { row_key: Str, id: Str, tenant: Str, kind: Str } key row_key
rel input entity_b: { row_key: Str, id: Str, tenant: Str, kind: Str } key row_key
rel input catalogue: { row_key: Str, tenant: Str, sku: Str, kind: Str, label: Str } key row_key
rel input decision_labels: { result: Str, label: Str } key result

rel derived activity_pages = select {
    parent: e.parent,
    page: collect(e.value, e.sort).take(8)
} from e in events group by e.parent

rel derived all_entities = union distinct entity_a, entity_b

rel derived fulfillment = select {
    order_id: o.id,
    tenant: o.tenant,
    sku: o.product.sku,
    qty: o.qty,
    available: s.available
} from o in orders, s in inventory where o.product.sku == s.sku

rel derived decision_value_join = select {
    order_id: d.order_id,
    result: d.value.b,
    label: l.label
} from d in dispatch, l in decision_labels where d.value.b == l.result

decide activity_summary for p in activity_pages per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}

decide entity_review for e in all_entities per id {
    propose approve priority 1 when e.kind == "book" = "approved"
    propose review priority 2 when e.kind != "book" = "review"
}

decide dispatch for f in fulfillment per order_id {
    propose ship priority 1 when f.qty <= f.available = DispatchResult { b: "ship" }
    propose hold priority 2 when f.qty > f.available = DispatchResult { b: "hold" }
}

query catalogue_page(kind: Str) = select {
    sku: c.sku,
    label: c.label
} from c in catalogue where c.kind == kind
"#;

#[derive(Clone, Copy)]
struct FixtureCounts {
    events: usize,
    orders: usize,
    inventory: usize,
    entity_a: usize,
    entity_b: usize,
    catalogue: usize,
    decision_labels: usize,
}

impl FixtureCounts {
    fn new(facts: usize) -> Self {
        let catalogue = 128;
        let decision_labels = 2;
        let inventory = 64;
        let orders = facts / 10;
        let entities = facts / 10;
        let entity_a = entities / 2;
        let entity_b = entities - entity_a;
        let events = facts - catalogue - decision_labels - inventory - orders - entity_a - entity_b;
        Self {
            events,
            orders,
            inventory,
            entity_a,
            entity_b,
            catalogue,
            decision_labels,
        }
    }

    fn source_json(self) -> JsonValue {
        json!({
            "root::events": self.events,
            "root::orders": self.orders,
            "root::inventory": self.inventory,
            "root::entity_a": self.entity_a,
            "root::entity_b": self.entity_b,
            "root::catalogue": self.catalogue,
            "root::decision_labels": self.decision_labels,
        })
    }
}

#[derive(Clone, Debug)]
struct Cli {
    mode: Mode,
}

#[derive(Clone, Debug)]
enum Mode {
    Full {
        facts: usize,
        batches: usize,
        seed: u64,
        world_dir: PathBuf,
    },
    ChildLoad(PathBuf),
    ChildOpen(PathBuf),
    ChildQuery {
        world_dir: PathBuf,
        warm: bool,
    },
    ChildVerify(PathBuf),
    ChildEdit {
        world_dir: PathBuf,
        action: EditAction,
    },
}

#[derive(Clone, Copy, Debug)]
enum EditAction {
    Add,
    Retract,
}

fn parse_cli() -> Result<Cli, String> {
    let mut args = env::args().skip(1);
    let Some(first) = args.next() else {
        return Err(usage());
    };
    match first.as_str() {
        "--child-load" => Ok(Cli {
            mode: Mode::ChildLoad(PathBuf::from(args.next().ok_or_else(usage)?)),
        }),
        "--child-open" => Ok(Cli {
            mode: Mode::ChildOpen(PathBuf::from(args.next().ok_or_else(usage)?)),
        }),
        "--child-query" => Ok(Cli {
            mode: Mode::ChildQuery {
                world_dir: PathBuf::from(args.next().ok_or_else(usage)?),
                warm: false,
            },
        }),
        "--child-query-warm" => Ok(Cli {
            mode: Mode::ChildQuery {
                world_dir: PathBuf::from(args.next().ok_or_else(usage)?),
                warm: true,
            },
        }),
        "--child-verify" => Ok(Cli {
            mode: Mode::ChildVerify(PathBuf::from(args.next().ok_or_else(usage)?)),
        }),
        "--child-edit" => {
            let world_dir = PathBuf::from(args.next().ok_or_else(usage)?);
            let action = match args.next().as_deref() {
                Some("add") => EditAction::Add,
                Some("retract") => EditAction::Retract,
                _ => return Err(usage()),
            };
            Ok(Cli {
                mode: Mode::ChildEdit { world_dir, action },
            })
        }
        "--full" => {
            let mut facts = None;
            let mut batches = None;
            let mut seed = 0_u64;
            let mut world_dir = None;
            while let Some(flag) = args.next() {
                let value = args.next().ok_or_else(usage)?;
                match flag.as_str() {
                    "--facts" => facts = Some(value.parse().map_err(|_| usage())?),
                    "--batches" => batches = Some(value.parse().map_err(|_| usage())?),
                    "--seed" => seed = value.parse().map_err(|_| usage())?,
                    "--world-dir" => world_dir = Some(PathBuf::from(value)),
                    _ => return Err(usage()),
                }
            }
            let facts = facts.ok_or_else(usage)?;
            let batches = batches.ok_or_else(usage)?;
            if !TARGET_FACTS.contains(&facts) || ![1, 10].contains(&batches) || facts % batches != 0
            {
                return Err("facts must be 8000/80000/800000; batches must be 1 or 10".into());
            }
            Ok(Cli {
                mode: Mode::Full {
                    facts,
                    batches,
                    seed,
                    world_dir: world_dir.ok_or_else(usage)?,
                },
            })
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: alpha9_scale_bench --full --facts {8000|80000|800000} --batches {1|10} --seed N --world-dir DIR | --child-load DIR | --child-open DIR | --child-query DIR | --child-query-warm DIR | --child-verify DIR | --child-edit DIR add|retract".into()
}

fn main() {
    let main_entry_unix_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let result = parse_cli().and_then(|cli| match cli.mode {
        Mode::Full {
            facts,
            batches,
            seed,
            world_dir,
        } => run_full(facts, batches, seed, &world_dir),
        Mode::ChildLoad(dir) => run_child_load(&dir),
        Mode::ChildOpen(dir) => run_child_open(&dir),
        Mode::ChildQuery { world_dir, warm } => run_child_query(&world_dir, warm),
        Mode::ChildVerify(dir) => run_child_verify(&dir),
        Mode::ChildEdit { world_dir, action } => run_child_edit(&world_dir, action),
    });
    match result {
        Ok(mut report) => {
            report["main_entry_unix_ns"] = json!(main_entry_unix_ns);
            println!("{}", serde_json::to_string(&report).expect("JSON report"));
        }
        Err(error) => {
            eprintln!("alpha9_scale_bench: {error}");
            std::process::exit(1);
        }
    }
}

fn run_full(
    facts: usize,
    batches: usize,
    seed: u64,
    world_dir: &Path,
) -> Result<JsonValue, String> {
    if world_dir.join("world.json").exists() || world_dir.join("HEAD").exists() {
        return Err(format!(
            "world directory already contains a world: {}",
            world_dir.display()
        ));
    }
    let counts = FixtureCounts::new(facts);
    let init_started = Instant::now();
    let sources = BTreeMap::from([("root".to_string(), PROGRAM_ROOT.to_string())]);
    let _session = WorldSession::from_program_with_sources(world_dir, "root", &sources)
        .map_err(|error| format!("world initialization: {error}"))?;
    let init_ms = ms(init_started.elapsed());
    let metadata = json!({
        "schema": FIXTURE_SCHEMA,
        "facts": facts,
        "batches": batches,
        "seed": seed,
        "fixture_digest": JsonValue::Null,
        "digest_algorithm": "brix-canon-blake3-chain-v1",
        "source_counts": counts.source_json(),
        "expected_query_rows": counts.catalogue / 2,
        "query_name": QUERY_NAME,
        "query_tenant": QUERY_TENANT,
        "query_kind": QUERY_KIND,
        "payload_distribution": payload_distribution(counts.events),
        "source_text": PROGRAM_ROOT,
    });
    fs::write(
        world_dir.join(FIXTURE_FILE),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .map_err(|error| format!("write fixture metadata: {error}"))?;

    Ok(json!({
        "schema": EXAMPLE_SCHEMA,
        "mode": "full",
        "world_dir": world_dir,
        "fixture": fixture_report(facts, batches, seed, String::new()),
        "build": build_report(),
        "storage_settings": storage_settings(&_session),
        "source_counts": counts.source_json(),
        "derived_counts": JsonValue::Null,
        "load": JsonValue::Null,
        "query": JsonValue::Null,
        "edit": JsonValue::Null,
        "storage": storage_report(world_dir)?,
        "correctness": JsonValue::Null,
        "timings": {
            "init_ms": init_ms,
            "load_ms": null,
            "edit_ms": null,
            "correctness_ms": null,
            "fresh_process_startup_ms": null,
            "world_open_ms": null,
            "first_page_ms": null,
            "complete_query_ms": null,
        },
        "payload_distribution": payload_distribution(counts.events),
    }))
}

fn run_child_load(world_dir: &Path) -> Result<JsonValue, String> {
    let mut metadata = read_fixture_metadata(world_dir)?;
    let facts = metadata["facts"].as_u64().ok_or("fixture facts missing")? as usize;
    let batches = metadata["batches"]
        .as_u64()
        .ok_or("fixture batches missing")? as usize;
    let seed = metadata["seed"].as_u64().ok_or("fixture seed missing")?;
    let counts = FixtureCounts::new(facts);
    let open_started = Instant::now();
    let mut session =
        WorldSession::open(world_dir).map_err(|error| format!("open empty load world: {error}"))?;
    let world_open_ms = ms(open_started.elapsed());
    if session.current_revision() != 0 {
        return Err(format!(
            "child-load expected revision 0, found {}",
            session.current_revision()
        ));
    }
    let mut hasher = FixtureHasher::new(seed, facts);
    let batch_rows = facts / batches;
    let started = Instant::now();
    let mut commit_profiles = Vec::with_capacity(batches);
    let mut objects_written = 0_u64;
    let mut source_tuple_bytes = 0_u64;
    let mut source_key_bytes = 0_u64;
    let mut commit_ms = 0.0_f64;
    let mut generation_ms = 0.0_f64;
    for batch_index in 0..batches {
        let begin = batch_index * batch_rows;
        let generation_started = Instant::now();
        let mut operations = Vec::with_capacity(batch_rows);
        for index in begin..begin + batch_rows {
            let operation = fixture_op(index, counts, seed);
            if let WorldBatchOp::Upsert { key, tuple, .. } = &operation {
                source_tuple_bytes += tuple.as_bytes().len() as u64;
                source_key_bytes += key.0.len() as u64;
            }
            hasher.operation(&operation);
            operations.push(operation);
        }
        generation_ms += ms(generation_started.elapsed());
        let commit_started = Instant::now();
        let receipt = session
            .apply_batch(WorldBatch::new(
                session.current_revision(),
                format!("alpha9-scale-batch-{batch_index:02}"),
                operations,
            ))
            .map_err(|error| format!("load batch {batch_index}: {error}"))?;
        commit_ms += ms(commit_started.elapsed());
        objects_written += receipt.objects_written as u64;
        commit_profiles.push(commit_counters(&session, &receipt));
    }
    let load_ms = ms(started.elapsed());
    if session.current_revision() != batches as u64 {
        return Err(format!(
            "expected {batches} load revisions, got {}",
            session.current_revision()
        ));
    }
    let digest = hasher.finish();
    metadata["fixture_digest"] = json!(digest);
    fs::write(
        world_dir.join(FIXTURE_FILE),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .map_err(|error| format!("write final fixture metadata: {error}"))?;
    let storage = storage_report(world_dir)?;
    Ok(json!({
        "schema": EXAMPLE_SCHEMA,
        "mode": "child-load",
        "world_dir": world_dir,
        "fixture": fixture_report(facts, batches, seed, digest),
        "build": build_report(),
        "storage_settings": storage_settings(&session),
        "source_counts": counts.source_json(),
        "derived_counts": JsonValue::Null,
        "source_payload": {"tuple_bytes": source_tuple_bytes, "key_bytes": source_key_bytes, "physical_storage": "shared CAS packs; payload counts are non-additive and exclude trie/framing/history overhead"},
        "load": {"batch_count": batches, "objects_written": objects_written, "per_batch_commit_counters": commit_profiles},
        "storage": storage,
        "correctness": JsonValue::Null,
        "timings": {"init_ms": null, "load_ms": load_ms, "commit_ms": commit_ms, "fixture_generation_ms": generation_ms, "edit_ms": null, "correctness_ms": null, "fresh_process_startup_ms": null, "world_open_ms": world_open_ms, "first_page_ms": null, "complete_query_ms": null},
        "payload_distribution": payload_distribution(counts.events),
    }))
}

fn run_child_open(world_dir: &Path) -> Result<JsonValue, String> {
    let metadata = read_fixture_metadata(world_dir)?;
    let started = Instant::now();
    let session = WorldSession::open(world_dir).map_err(|error| format!("cold open: {error}"))?;
    let world_open_ms = ms(started.elapsed());
    Ok(json!({
        "schema": EXAMPLE_SCHEMA,
        "mode": "child-open",
        "world_dir": world_dir,
        "fixture": fixture_report_from_metadata(&metadata),
        "build": build_report(),
        "storage_settings": storage_settings(&session),
        "source_counts": metadata["source_counts"],
        "derived_counts": JsonValue::Null,
        "query": JsonValue::Null,
        "edit": JsonValue::Null,
        "storage": JsonValue::Null,
        "correctness": JsonValue::Null,
        "timings": {
            "init_ms": null,
            "load_ms": null,
            "edit_ms": null,
            "correctness_ms": null,
            "fresh_process_startup_ms": null,
            "world_open_ms": world_open_ms,
            "first_page_ms": null,
            "complete_query_ms": null,
        },
        "open": {
            "open_mode": session.open_mode.as_str(),
            "replay_reason": session.replay_reason,
            "state_source": session.state_source,
            "state_refreshed": session.state_refreshed,
        }
    }))
}

fn run_child_verify(world_dir: &Path) -> Result<JsonValue, String> {
    let session = WorldSession::open(world_dir).map_err(|error| format!("verify open: {error}"))?;
    let metadata = read_fixture_metadata(world_dir)?;
    let facts = metadata["facts"].as_u64().ok_or("fixture facts missing")? as usize;
    let counts = FixtureCounts::new(facts);
    let started = Instant::now();
    let store = gc::verify_store(world_dir).map_err(|error| format!("verify store: {error}"))?;
    session
        .verify_oracle()
        .map_err(|error| format!("verify oracle: {error}"))?;
    let derived = derived_counts(&session)?;
    let expected = BTreeMap::from([
        (
            "root::activity_pages".to_string(),
            activity_group_count(counts.events),
        ),
        (
            "root::all_entities".to_string(),
            counts.entity_a.min(counts.entity_b),
        ),
        ("root::fulfillment".to_string(), counts.orders),
        ("root::decision_value_join".to_string(), counts.orders),
    ]);
    if derived != expected {
        return Err(format!(
            "derived count mismatch: expected {expected:?}, got {derived:?}"
        ));
    }
    for relation in QUERIED_DERIVED_RELATIONS {
        let mut materialized = session
            .get_derived(relation)
            .map_err(|error| format!("verify derived relation {relation}: {error}"))?
            .ok_or_else(|| format!("missing derived relation {relation}"))?
            .into_iter()
            .map(|row| row.to_tuple().as_bytes().to_vec())
            .collect::<Vec<_>>();
        materialized.sort();
        let paged = collect_derived_relation_rows(&session, session.current_revision(), relation)?;
        if materialized != paged {
            return Err(format!(
                "paged relation result differs from materialized content: {relation}"
            ));
        }
    }
    let scope = build_scope(&session)?;
    let expected_rows = metadata["expected_query_rows"]
        .as_u64()
        .ok_or("expected query rows missing")? as usize;
    let (query, _, _, _) = execute_query(&session, &scope, expected_rows, true)?;
    let correctness_ms = ms(started.elapsed());
    Ok(json!({
        "schema": EXAMPLE_SCHEMA,
        "mode": "child-verify",
        "world_dir": world_dir,
        "fixture": fixture_report_from_metadata(&metadata),
        "build": build_report(),
        "storage_settings": storage_settings(&session),
        "source_counts": metadata["source_counts"],
        "derived_counts": derived,
        "query": query,
        "correctness": {"passed": true, "store_nodes": store.nodes, "oracle": "passed", "derived_counts_match": true, "named_query_rows_match": true},
        "timings": {"init_ms": null, "load_ms": null, "edit_ms": null, "correctness_ms": correctness_ms, "fresh_process_startup_ms": null, "world_open_ms": null, "first_page_ms": null, "complete_query_ms": null},
        "storage": JsonValue::Null,
    }))
}

fn run_child_edit(world_dir: &Path, action: EditAction) -> Result<JsonValue, String> {
    let mut session =
        WorldSession::open(world_dir).map_err(|error| format!("open edit world: {error}"))?;
    let before_revision = session.current_revision();
    let metadata = read_fixture_metadata(world_dir)?;
    let seed = metadata["seed"].as_u64().ok_or("fixture seed missing")?;
    let (name, operations) = match action {
        EditAction::Add => (
            "alpha9-scale-one-row-add",
            vec![event_edit_op("bench-edit-00000000", seed)],
        ),
        EditAction::Retract => (
            "alpha9-scale-one-row-retract",
            vec![WorldBatchOp::Remove {
                relation: "root::events".into(),
                key: WorldKey::from_str("bench-edit-00000000"),
            }],
        ),
    };
    let started = Instant::now();
    let receipt = session
        .apply_batch(WorldBatch::new(before_revision, name, operations))
        .map_err(|error| format!("one-row {:?}: {error}", action))?;
    let edit_ms = ms(started.elapsed());
    Ok(json!({
        "schema": EXAMPLE_SCHEMA,
        "mode": "child-edit",
        "action": match action { EditAction::Add => "add", EditAction::Retract => "retract" },
        "world_dir": world_dir,
        "fixture": fixture_report_from_metadata(&metadata),
        "build": build_report(),
        "storage_settings": storage_settings(&session),
        "edit": {"before_revision": before_revision, "after_revision": receipt.revision_seq, "elapsed_ms": edit_ms, "counters": commit_counters(&session, &receipt)},
        "timings": {"init_ms": null, "load_ms": null, "edit_ms": edit_ms, "correctness_ms": null, "fresh_process_startup_ms": null, "world_open_ms": null, "first_page_ms": null, "complete_query_ms": null},
        "correctness": JsonValue::Null,
        "storage": JsonValue::Null,
    }))
}

fn run_child_query(world_dir: &Path, warm: bool) -> Result<JsonValue, String> {
    let startup = Instant::now();
    let session =
        WorldSession::open(world_dir).map_err(|error| format!("query world open: {error}"))?;
    let world_open_ms = ms(startup.elapsed());
    let metadata = read_fixture_metadata(world_dir)?;
    let expected_rows = metadata["expected_query_rows"]
        .as_u64()
        .ok_or_else(|| "fixture metadata lacks expected_query_rows".to_string())?
        as usize;
    let scope = build_scope(&session)?;
    if warm {
        let _ = execute_query(&session, &scope, expected_rows, false)?;
        let _ = execute_relation_queries(&session, session.current_revision())?;
    }
    let (query, first_page_ms, complete_query_ms, _correctness_ms) =
        execute_query(&session, &scope, expected_rows, false)?;
    let derived_relations = execute_relation_queries(&session, session.current_revision())?;
    Ok(json!({
        "schema": EXAMPLE_SCHEMA,
            "mode": if warm { "child-query-warm" } else { "child-query" },
        "world_dir": world_dir,
        "fixture": fixture_report_from_metadata(&metadata),
        "build": build_report(),
        "source_counts": metadata["source_counts"],
        "derived_counts": JsonValue::Null,
        "query": query,
        "derived_relations": derived_relations,
        "edit": JsonValue::Null,
        "storage": JsonValue::Null,
        "correctness": JsonValue::Null,
        "timings": {
            "init_ms": null,
            "load_ms": null,
            "edit_ms": null,
            "correctness_ms": null,
            "fresh_process_startup_ms": null,
            "world_open_ms": world_open_ms,
            "first_page_ms": first_page_ms,
            "complete_query_ms": complete_query_ms,
        }
    }))
}

fn execute_relation_queries(session: &WorldSession, revision: u64) -> Result<JsonValue, String> {
    let mut output = serde_json::Map::new();
    for relation in QUERIED_DERIVED_RELATIONS {
        let mut rows = Vec::<Vec<u8>>::new();
        let mut row_bytes = 0_u64;
        let mut pages = 0_u64;
        let total_started = Instant::now();
        let first_started = Instant::now();
        let first = session
            .query_derived_page_at(revision, relation, None, RELATION_PAGE_ROWS)
            .map_err(|error| format!("query derived relation {relation}: {error}"))?;
        let first_page_ms = ms(first_started.elapsed());
        let first_rows = first.entries.len();
        let first_bytes = first
            .entries
            .iter()
            .map(|(_, tuple)| tuple.as_bytes().len() as u64)
            .sum::<u64>();
        pages += 1;
        for (_, tuple) in first.entries {
            row_bytes += tuple.as_bytes().len() as u64;
            rows.push(tuple.as_bytes().to_vec());
        }
        let mut cursor = first.next_cursor;
        while let Some(token) = cursor {
            let page = session
                .query_derived_page_at(revision, relation, Some(&token), RELATION_PAGE_ROWS)
                .map_err(|error| {
                    format!("query derived relation {relation} continuation: {error}")
                })?;
            pages += 1;
            for (_, tuple) in page.entries {
                row_bytes += tuple.as_bytes().len() as u64;
                rows.push(tuple.as_bytes().to_vec());
            }
            cursor = page.next_cursor;
        }
        let complete_query_ms = ms(total_started.elapsed());
        rows.sort();
        let mut writer = CanonWriter::new();
        writer.write_tag("brix.alpha9.derived-result@1");
        writer.write_str(relation);
        writer.write_uint(rows.len() as u64);
        for row in &rows {
            writer.write_bytes(row);
        }
        output.insert(
            relation.to_string(),
            json!({
                "revision": revision,
                "page_limit": RELATION_PAGE_ROWS,
                "first_page_ms": first_page_ms,
                "first_page_rows": first_rows,
                "first_page_bytes": first_bytes,
                "complete_query_ms": complete_query_ms,
                "pages": pages,
                "rows": rows.len(),
                "bytes": row_bytes,
                "canonical_content_digest": writer.digest(Domain::Value).to_hex(),
            }),
        );
    }
    Ok(JsonValue::Object(output))
}

fn collect_derived_relation_rows(
    session: &WorldSession,
    revision: u64,
    relation: &str,
) -> Result<Vec<Vec<u8>>, String> {
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = session
            .query_derived_page_at(revision, relation, cursor.as_deref(), RELATION_PAGE_ROWS)
            .map_err(|error| format!("verify paged relation {relation}: {error}"))?;
        rows.extend(
            page.entries
                .into_iter()
                .map(|(_, tuple)| tuple.as_bytes().to_vec()),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    rows.sort();
    Ok(rows)
}

fn execute_query(
    session: &WorldSession,
    scope: &AuthorizedHostScope,
    expected_rows: usize,
    verify_results: bool,
) -> Result<(JsonValue, f64, f64, f64), String> {
    let request = |cursor: Option<String>| ScopedQueryPageRequestV1 {
        query_name: QUERY_NAME.into(),
        arguments: NamedQueryArgsV1(BTreeMap::from([(
            "kind".into(),
            QueryValueV1::Str(QUERY_KIND.into()),
        )])),
        revision: Some(session.current_revision()),
        limit: QUERY_PAGE_ROWS,
        max_input_rows: 1_000,
        max_input_bytes: 1_000_000,
        max_eval_work: 1_000_000,
        max_eval_matches: 100_000,
        max_output_rows: 1_000,
        cursor,
    };
    let full_started = Instant::now();
    let first_started = Instant::now();
    let first = session
        .query_named_scoped_page(&request(None), scope)
        .map_err(|error| format!("first scoped page: {error}"))?;
    let first_page_ms = ms(first_started.elapsed());
    let first_page_rows = first.typed_rows.len();
    let first_page_bytes = first
        .rows
        .iter()
        .map(|row| row.as_bytes().len() as u64)
        .sum::<u64>();
    let mut typed_rows = first.typed_rows.clone();
    let mut result_bytes: u64 = first
        .rows
        .iter()
        .map(|row| row.as_bytes().len() as u64)
        .sum();
    let mut work_units = first.work_units;
    let mut matches = first.matches;
    let mut expressions_evaluated = first.expressions_evaluated;
    let mut cursor = first.next_cursor.clone();
    let mut page_count = 1usize;
    while let Some(token) = cursor {
        let page = session
            .query_named_scoped_page(&request(Some(token)), scope)
            .map_err(|error| format!("scoped query continuation: {error}"))?;
        result_bytes += page
            .rows
            .iter()
            .map(|row| row.as_bytes().len() as u64)
            .sum::<u64>();
        work_units += page.work_units;
        matches += page.matches;
        expressions_evaluated += page.expressions_evaluated;
        typed_rows.extend(page.typed_rows);
        cursor = page.next_cursor;
        page_count += 1;
    }
    let complete_query_ms = ms(full_started.elapsed());
    let correctness_ms = if verify_results {
        let correctness_started = Instant::now();
        let mut expected = expected_catalogue_rows(expected_rows);
        let mut actual = typed_rows.clone();
        canonical_sort_json_rows(&mut expected);
        canonical_sort_json_rows(&mut actual);
        if actual != expected {
            return Err(format!(
                "scoped query result mismatch: expected {} rows, got {}",
                expected.len(),
                typed_rows.len()
            ));
        }
        if typed_rows.len() != expected_rows {
            return Err(format!(
                "expected {expected_rows} query rows, found {}",
                typed_rows.len()
            ));
        }
        ms(correctness_started.elapsed())
    } else {
        0.0
    };
    Ok((
        json!({
            "name": QUERY_NAME,
            "kind": QUERY_KIND,
            "limit": QUERY_PAGE_ROWS,
            "first_page_rows": first_page_rows,
            "first_page_bytes": first_page_bytes,
            "rows": typed_rows.len(),
            "bytes": result_bytes,
            "work_units": work_units,
            "matches": matches,
            "expressions_evaluated": expressions_evaluated,
            "page_count": page_count,
            "mode": "fresh-process-unmutated-session",
        }),
        first_page_ms,
        complete_query_ms,
        correctness_ms,
    ))
}

fn canonical_sort_json_rows(rows: &mut [JsonValue]) {
    rows.sort_by(|left, right| {
        serde_json::to_vec(left)
            .unwrap_or_default()
            .cmp(&serde_json::to_vec(right).unwrap_or_default())
    });
}

fn expected_catalogue_rows(count: usize) -> Vec<JsonValue> {
    (0..count)
        .map(|ordinal| {
            let index = ordinal * 2;
            json!({
                "sku": format!("sku-{index:08}"),
                "label": format!("label-{index:010}"),
            })
        })
        .collect()
}

fn build_scope(session: &WorldSession) -> Result<AuthorizedHostScope, String> {
    let profile = HostScopeProfileV1 {
        profile_id: HOST_SCOPE_PROFILE_PARTITION_V1.into(),
        partition_fields: BTreeMap::from([("root::catalogue".into(), "tenant".into())]),
        global_relations: BTreeSet::new(),
        query_limits: HostQueryLimitsV1 {
            page_rows: QUERY_PAGE_ROWS,
            max_input_rows: 1_000,
            max_input_bytes: 1_000_000,
            max_eval_work: 1_000_000,
            max_eval_matches: 100_000,
            max_output_rows: 1_000,
        },
    };
    let revision = session
        .pin_revision(session.current_revision())
        .map_err(|error| format!("pin query revision: {error}"))?;
    let program_bytes = session
        .program_artifact_preimage()
        .map_err(|error| format!("program artifact: {error}"))?;
    let world_bytes = world_revision_artifact_preimage_v1(
        &session.manifest.world_id,
        session.current_revision(),
        revision.revision_digest(),
        session.manifest.program_digest,
    );
    let program_ref_digest = Digest::of(Domain::Value, &program_bytes);
    let world_ref_digest = Digest::of(Domain::Value, &world_bytes);
    let dimensions = ContextDimensionsV2 {
        parent: None,
        program: Some(ArtifactRefV2 {
            kind: "program".into(),
            digest: program_ref_digest,
        }),
        world: Some(ArtifactRefV2 {
            kind: "world.revision@1".into(),
            digest: world_ref_digest,
        }),
        namespace: "alpha9-benchmark".into(),
        tenant: QUERY_TENANT.into(),
        principal: "alpha9-benchmark-host".into(),
        capability_view: profile
            .context_capability_view()
            .map_err(|error| format!("scope capability view: {error:?}"))?,
        scenario_mode: ScenarioModeV2::Production,
        branch: None,
        valid_time_start: 0,
        valid_time_end: u64::MAX,
        transaction_time: None,
        correction_time: None,
        simulation_time: None,
        observation_profile: "none@1".into(),
        redaction_profile: "none@1".into(),
        kernel_profile: "none".into(),
        regime_profile: "none".into(),
        axiom_profile: "none".into(),
        validity_start: 0,
        validity_end: u64::MAX,
        step_budget: 1_000_000,
        depth_budget: 32,
    };
    let model = ContextModelV2::new(dimensions.clone(), vec![])
        .map_err(|error| format!("benchmark context: {error:?}"))?;
    let context_bytes = encode_context_model_v2(&model);
    let resolver = BenchmarkResolver(BTreeMap::from([
        (program_ref_digest, program_bytes),
        (world_ref_digest, world_bytes),
    ]));
    AuthorizedHostScope::authorize(
        profile,
        HostContextExpectationV2 {
            bytes: &context_bytes,
            id: model.id(),
            dimensions: &dimensions,
            limits: ModelLimits::default(),
        },
        &resolver,
        &NoAssumptions,
    )
    .map_err(|error| format!("authorize benchmark query context: {error:?}"))
}

struct BenchmarkResolver(BTreeMap<Digest, Vec<u8>>);
impl ContextResolverV2 for BenchmarkResolver {
    fn context_bytes(&self, _: &ContextId) -> Option<Vec<u8>> {
        None
    }
    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>> {
        self.0.get(&reference.digest).cloned()
    }
}

struct NoAssumptions;
impl AssumptionValidatorV2 for NoAssumptions {
    fn validate(&self, _: &str, _: &[u8], _: &mut usize) -> bool {
        false
    }
}

fn fixture_op(index: usize, c: FixtureCounts, seed: u64) -> WorldBatchOp {
    let mut offset = index;
    if offset < c.catalogue {
        let i = offset;
        let mut row = TupleRecord::new();
        row.set_str("row_key", &format!("catalogue-{i:08}"));
        row.set_str("tenant", QUERY_TENANT);
        row.set_str("sku", &format!("sku-{i:08}"));
        row.set_str("kind", if i.is_multiple_of(2) { "book" } else { "tool" });
        row.set_str("label", &format!("label-{i:010}"));
        return upsert("root::catalogue", &format!("catalogue-{i:08}"), row);
    }
    offset -= c.catalogue;
    if offset < c.decision_labels {
        let (result, label) = if offset == 0 {
            ("ship", "shipped")
        } else {
            ("hold", "held")
        };
        let mut row = TupleRecord::new();
        row.set_str("result", result);
        row.set_str("label", label);
        return upsert("root::decision_labels", result, row);
    }
    offset -= c.decision_labels;
    if offset < c.inventory {
        let i = offset;
        let mut row = TupleRecord::new();
        row.set_str("id", &format!("inventory-{i:08}"));
        row.set_str("sku", &format!("sku-{:08}", i % 64));
        row.set_str("available", "20");
        return upsert("root::inventory", &format!("inventory-{i:08}"), row);
    }
    offset -= c.inventory;
    if offset < c.orders {
        let i = offset;
        let mut row = TupleRecord::new();
        row.set_str("id", &format!("order-{i:08}"));
        row.set_str("tenant", QUERY_TENANT);
        row.set("product", product_value(i % 64).to_bytes());
        row.set_str("qty", if i.is_multiple_of(17) { "25" } else { "5" });
        return upsert("root::orders", &format!("order-{i:08}"), row);
    }
    offset -= c.orders;
    if offset < c.entity_a {
        let i = offset;
        let key = format!("entity-{i:08}");
        let mut row = TupleRecord::new();
        row.set_str("row_key", &key);
        row.set_str("id", &key);
        row.set_str("tenant", QUERY_TENANT);
        row.set_str("kind", if i.is_multiple_of(2) { "book" } else { "tool" });
        return upsert("root::entity_a", &key, row);
    }
    offset -= c.entity_a;
    if offset < c.entity_b {
        let i = offset;
        let key = format!("entity-{i:08}");
        let mut row = TupleRecord::new();
        row.set_str("row_key", &key);
        row.set_str("id", &key);
        row.set_str("tenant", QUERY_TENANT);
        row.set_str("kind", if i.is_multiple_of(2) { "book" } else { "tool" });
        return upsert("root::entity_b", &key, row);
    }
    offset -= c.entity_b;
    if offset >= c.events {
        unreachable!("fixture count formula must cover exactly every source fact");
    }
    event_op(offset, seed)
}

fn event_op(index: usize, seed: u64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    let id = format!("event-{index:08}");
    row.set_str("id", &id);
    row.set_str("tenant", QUERY_TENANT);
    let parent = if index < 5_000 {
        "hot".to_string()
    } else {
        format!("group-{:05}", (index - 5_000) / 64)
    };
    row.set_str("parent", &parent);
    row.set_str(
        "sort",
        &((stable_mix(seed, index as u64) % 64) as usize).to_string(),
    );
    row.set_str("value", &format!("value-{index:010}"));
    upsert("root::events", &id, row)
}

fn event_edit_op(id: &str, seed: u64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("tenant", QUERY_TENANT);
    row.set_str("parent", "hot");
    row.set_str("sort", &(stable_mix(seed, u64::MAX) % 64).to_string());
    row.set_str("value", "edit-value-000000");
    upsert("root::events", id, row)
}

fn product_value(index: usize) -> Value {
    Value::Record {
        nominal: "root::Product".into(),
        fields: BTreeMap::from([("sku".into(), Value::Str(format!("sku-{index:08}")))]),
    }
}

fn upsert(relation: &str, key: &str, row: TupleRecord) -> WorldBatchOp {
    WorldBatchOp::Upsert {
        relation: relation.into(),
        key: WorldKey::from_str(key),
        tuple: row.to_tuple(),
    }
}

fn stable_mix(seed: u64, value: u64) -> u64 {
    let mut x = value ^ seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

struct FixtureHasher(Digest);
impl FixtureHasher {
    fn new(seed: u64, facts: usize) -> Self {
        let mut writer = CanonWriter::new();
        writer.write_tag("brix.alpha9.fixture-seed@1");
        writer.write_tag(EXAMPLE_SCHEMA);
        writer.write_tag(FIXTURE_SCHEMA);
        writer.write_uint(seed);
        writer.write_uint(facts as u64);
        Self(writer.digest(Domain::Value))
    }
    fn operation(&mut self, op: &WorldBatchOp) {
        if let WorldBatchOp::Upsert {
            relation,
            key,
            tuple,
        } = op
        {
            let mut writer = CanonWriter::new();
            writer.write_tag("brix.alpha9.fixture-row@1");
            writer.write_bytes(self.0.as_bytes());
            writer.write_str(relation);
            writer.write_bytes(key.as_bytes());
            writer.write_bytes(tuple.as_bytes());
            self.0 = writer.digest(Domain::Value);
        }
    }
    fn finish(self) -> String {
        self.0.to_hex()
    }
}

fn derived_counts(session: &WorldSession) -> Result<BTreeMap<String, usize>, String> {
    let mut counts = BTreeMap::new();
    for relation in [
        "root::activity_pages",
        "root::all_entities",
        "root::fulfillment",
        "root::decision_value_join",
    ] {
        let rows = session
            .get_derived(relation)
            .map_err(|error| format!("read derived relation {relation}: {error}"))?
            .ok_or_else(|| format!("missing derived relation {relation}"))?;
        counts.insert(relation.to_string(), rows.len());
    }
    Ok(counts)
}

fn activity_group_count(event_count: usize) -> usize {
    if event_count <= 5_000 {
        1
    } else {
        1 + (event_count - 5_000).div_ceil(64)
    }
}

fn commit_counters(session: &WorldSession, receipt: &brix_kb::world::RevisionReceipt) -> JsonValue {
    let profile = session.last_commit_profile.as_ref();
    json!({
        "revision": receipt.revision_seq,
        "changed_keys": receipt.changed_keys_count,
        "objects_written": receipt.objects_written,
        "state_node_reads": profile.map(|p| p.state_node_reads),
        "state_bytes_read": profile.map(|p| p.state_bytes_read),
        "state_bytes_written": profile.map(|p| p.state_bytes_written),
        "chunks": profile.map(|p| p.chunks),
        "operator_work": profile.map(|p| p.operator_work),
        "matches": profile.map(|p| p.matches),
        "expressions_evaluated": profile.map(|p| p.expressions_evaluated),
    })
}

fn storage_settings(session: &WorldSession) -> JsonValue {
    let configured_node_cache_bytes = session.manifest.node_cache_bytes;
    let effective_node_cache_bytes =
        configured_node_cache_bytes.unwrap_or(DEFAULT_NODE_CACHE_BYTES as u64);
    let configured_apply_hydrated_bytes = session.manifest.apply_hydrated_bytes;
    let effective_apply_hydrated_bytes =
        configured_apply_hydrated_bytes.unwrap_or(DEFAULT_APPLY_HYDRATED_BYTES);
    json!({
        "node_cache_bytes": {
            "configured": configured_node_cache_bytes,
            "effective": effective_node_cache_bytes,
            "default": DEFAULT_NODE_CACHE_BYTES,
        },
        "apply_hydrated_bytes": {
            "configured": configured_apply_hydrated_bytes,
            "effective": effective_apply_hydrated_bytes,
            "default": DEFAULT_APPLY_HYDRATED_BYTES,
            "chunking_enabled": effective_apply_hydrated_bytes > 0,
        },
        "changed_for_benchmark": false,
    })
}

fn storage_report(root: &Path) -> Result<JsonValue, String> {
    let mut components = BTreeMap::new();
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("measure world root {}: {error}", root.display()))?;
    if !root_metadata.is_dir() {
        return Err(format!("world root is not a directory: {}", root.display()));
    }
    components.insert("root_metadata", (0_u64, allocated_bytes(&root_metadata)));
    let entries = fs::read_dir(root)
        .map_err(|error| format!("read world directory {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read world directory entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect world path {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing to skip symlink in world storage report: {}",
                path.display()
            ));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unrecognized");
        let bucket = match file_name {
            "objects" => "objects_source_state_and_indexes",
            "revisions" => "revision_journal_and_decisions",
            "receipts" => "receipts",
            "staging" => "staging_scratch",
            _ if metadata.is_dir() => "other_required_files",
            _ => "root_metadata",
        };
        let measured = dir_size(&path)?;
        let total = components.entry(bucket).or_insert((0_u64, 0_u64));
        total.0 += measured.0;
        total.1 += measured.1;
    }
    let logical = components.values().map(|v| v.0).sum::<u64>();
    let allocated = components.values().map(|v| v.1).sum::<u64>();
    let component_json: BTreeMap<_, _> = components
        .into_iter()
        .map(|(name, (logical, allocated))| {
            (
                name,
                json!({"logical_bytes": logical, "allocated_bytes": allocated}),
            )
        })
        .collect();
    Ok(json!({
        "logical_bytes": logical,
        "allocated_bytes": allocated,
        "components": component_json,
        "includes_fixture_metadata_file": true,
        "includes_directory_blocks": true,
    }))
}

fn dir_size(path: &Path) -> Result<(u64, u64), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect storage path {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to skip symlink in storage report: {}",
            path.display()
        ));
    }
    if metadata.is_file() {
        return Ok((metadata.len(), allocated_bytes(&metadata)));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "unsupported filesystem object in storage report: {}",
            path.display()
        ));
    }
    let mut total = (0_u64, 0_u64);
    total.1 += allocated_bytes(&metadata);
    let entries = fs::read_dir(path)
        .map_err(|error| format!("read storage directory {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read storage directory entry: {error}"))?;
        let next = dir_size(&entry.path())?;
        total.0 += next.0;
        total.1 += next.1;
    }
    Ok(total)
}

#[cfg(unix)]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

fn read_fixture_metadata(world_dir: &Path) -> Result<JsonValue, String> {
    let path = world_dir.join(FIXTURE_FILE);
    let bytes = fs::read(&path)
        .map_err(|error| format!("read fixture metadata {}: {error}", path.display()))?;
    let value: JsonValue = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if value.get("schema").and_then(JsonValue::as_str) != Some(FIXTURE_SCHEMA) {
        return Err("unsupported fixture metadata schema".into());
    }
    Ok(value)
}

fn fixture_report(facts: usize, batches: usize, seed: u64, digest: String) -> JsonValue {
    json!({
        "rows": facts,
        "retained_revisions": batches,
        "seed": seed,
        "digest": if digest.is_empty() { JsonValue::Null } else { json!(digest) },
        "payload_distribution": payload_distribution(FixtureCounts::new(facts).events),
    })
}

fn fixture_report_from_metadata(metadata: &JsonValue) -> JsonValue {
    json!({
        "rows": metadata["facts"],
        "retained_revisions": metadata["batches"],
        "seed": metadata["seed"],
        "digest": metadata["fixture_digest"],
        "payload_distribution": metadata["payload_distribution"],
    })
}

fn payload_distribution(event_count: usize) -> JsonValue {
    json!({
        "version": "fixed-width-atlas-shaped-v2",
        "actual_atlas_data": false,
        "id_bytes": 14,
        "tenant_bytes": 8,
        "event_value_bytes": 16,
        "catalogue_label_bytes": 16,
        "events_hot_group_children": 5_000,
        "events_other_groups": event_count.saturating_sub(5_000).div_ceil(64),
        "events_other_group_target_children": 64,
        "catalogue_rows": 128,
        "entity_union_overlap": "entity_b duplicates entity_a row values and keys",
    })
}

fn build_report() -> JsonValue {
    let binary = env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let rustc = Command::new("rustc")
        .arg("-V")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    json!({
        "binary": binary,
        "embedded_build_commit": option_env!("BRIX_BENCH_BUILD_COMMIT"),
        "program_source_digest": Digest::of(Domain::Value, PROGRAM_ROOT.as_bytes()).to_hex(),
        "rustc": rustc,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "package_version": env!("CARGO_PKG_VERSION"),
    })
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
