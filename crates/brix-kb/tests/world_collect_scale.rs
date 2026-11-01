//! A9-1: grouped `collect` persistence across every size boundary.
//!
//! Each group size N is declared with `collect(.., N)` and driven through: insert N, an
//! over-limit (N+1) batch carrying an unrelated write, an in-place member update, batched
//! retraction across the 257/256/255 boundary, retraction of the final child, and reinsert.
//! After EVERY batch (accepted or rejected) a fresh process runs the store audit, opens the
//! world, replays state, runs the oracle comparison and reads the derived rows; the parent
//! compares that against its own model and then reopens the world cold for the next batch.
use brix_kb::world::chunk_policy::FixedChunkPolicy;
use brix_kb::world::decision_codec::Value;
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "brix-a9-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn program(limit: usize) -> String {
    format!(
        "rel input record: {{ id: Str, kind: Str, ordinal: Int }} key id\n\
         rel input note: {{ id: Str, text: Str }} key id\n\
         rel derived o = select {{ kind: r.kind, ids: collect(r.id, r.ordinal, {limit}) }}\n  \
         from r in record group by r.kind\n\
         rel derived c = select {{ kind: r.kind, n: count() }}\n  \
         from r in record group by r.kind\n"
    )
}

fn id(i: usize) -> String {
    format!("r{i:05}")
}

fn rec(i: usize, kind: &str, ordinal: i64) -> WorldBatchOp {
    let mut r = TupleRecord::new();
    r.set_str("id", &id(i));
    r.set_str("kind", kind);
    r.set_str("ordinal", &ordinal.to_string());
    WorldBatchOp::Upsert {
        relation: "root::record".into(),
        key: WorldKey::from_str(&id(i)),
        tuple: r.to_tuple(),
    }
}

fn unrelated(text: &str) -> WorldBatchOp {
    let mut r = TupleRecord::new();
    r.set_str("id", "n1");
    r.set_str("text", text);
    WorldBatchOp::Upsert {
        relation: "root::note".into(),
        key: WorldKey::from_str("n1"),
        tuple: r.to_tuple(),
    }
}

fn del(i: usize) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: "root::record".into(),
        key: WorldKey::from_str(&id(i)),
    }
}

/// What a fresh process observed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    seq: u64,
    digest: String,
    typed_ok: bool,
    /// group kind -> (count from the `c` relation, collected ids)
    groups: BTreeMap<String, (u64, Vec<String>)>,
    notes: usize,
}

const CHILD_ENV: &str = "BRIX_A9_CHILD_DIR";

/// Re-entered in a fresh process by [`fresh_check`]; a no-op in a normal test run.
#[test]
fn a9_fresh_child() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    // `world open-check --verify-store`: audit every pack before opening.
    brix_kb::world::gc::verify_store(&dir).expect("verify_store");
    let session = WorldSession::open(&dir).expect("open");
    // On a cold-restored session verify_oracle first checks verify_state, then
    // compares a fresh replay with the restored network. Avoid replaying the
    // same state a second time here for the large-size lifecycle matrix.
    // The oracle comparison is a harness call, not a CLI flag.
    session.verify_oracle().expect("verify_oracle");

    let ty = brix_syntax::ast::Ty::App(
        "List".into(),
        vec![brix_syntax::ast::Ty::Named("Str".into())],
    );
    let mut typed_ok = true;
    let mut groups = String::new();
    let counts: BTreeMap<String, u64> = session
        .get_derived("root::c")
        .expect("c")
        .unwrap_or_default()
        .iter()
        .map(|r| {
            (
                r.get_str("kind").unwrap().to_string(),
                r.get_str("n").unwrap().parse().unwrap(),
            )
        })
        .collect();
    for row in session
        .get_derived("root::o")
        .expect("o")
        .unwrap_or_default()
    {
        let kind = row.get_str("kind").unwrap().to_string();
        let ids = match Value::from_typed_bytes(row.get("ids").unwrap(), &ty) {
            Ok(Value::List(items)) => items
                .into_iter()
                .map(|v| match v {
                    Value::Str(s) => s,
                    other => panic!("non-Str id {other:?}"),
                })
                .collect::<Vec<_>>(),
            Ok(other) => panic!("not a list: {other:?}"),
            Err(e) => {
                eprintln!("[a9_fresh_child] typed read of {kind} failed: {e:?}");
                typed_ok = false;
                Vec::new()
            }
        };
        groups.push_str(&format!(
            "{kind}={}={}\n",
            counts.get(&kind).copied().unwrap_or(0),
            ids.join(",")
        ));
    }
    let notes = session
        .query_page("root::note", None, 10)
        .expect("note page")
        .entries
        .len();
    println!(
        "A9|{}|{}|{}|{}|{}",
        session.current_revision(),
        session.current_revision_digest().unwrap().to_hex(),
        typed_ok,
        notes,
        groups.replace('\n', ";")
    );
}

fn fresh_check(dir: &Path) -> Observed {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a9_fresh_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, dir)
        .output()
        .expect("spawn fresh process");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "fresh process failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout
        .lines()
        .find_map(|l| l.find("A9|").map(|at| &l[at..]))
        .unwrap_or_else(|| panic!("no observation in:\n{stdout}"));
    let mut p = line.splitn(6, '|').skip(1);
    let seq = p.next().unwrap().parse().unwrap();
    let digest = p.next().unwrap().to_string();
    let typed_ok = p.next().unwrap() == "true";
    let notes = p.next().unwrap().parse().unwrap();
    let mut groups = BTreeMap::new();
    for g in p.next().unwrap().split(';').filter(|g| !g.is_empty()) {
        let mut f = g.splitn(3, '=');
        let kind = f.next().unwrap().to_string();
        let n: u64 = f.next().unwrap().parse().unwrap();
        let ids = f
            .next()
            .unwrap()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        groups.insert(kind, (n, ids));
    }
    Observed {
        seq,
        digest,
        typed_ok,
        groups,
        notes,
    }
}

/// The driver: a model of the source rows, a cold-reopened session per step.
struct Driver {
    dir: TestDir,
    _limit: usize,
    chunk: Option<usize>,
    model: BTreeMap<String, BTreeMap<usize, i64>>, // kind -> member -> ordinal
    notes: usize,
}

impl Driver {
    fn new(label: &str, limit: usize, chunk: Option<usize>) -> Self {
        let dir = TestDir::new(label);
        let sources = BTreeMap::from([("root".into(), program(limit))]);
        let s = WorldSession::from_program_with_sources(&dir.0, "root", &sources).unwrap();
        drop(s);
        let d = Self {
            dir,
            _limit: limit,
            chunk,
            model: BTreeMap::new(),
            notes: 0,
        };
        d.check("genesis");
        d
    }

    fn open(&self) -> WorldSession {
        let mut s = WorldSession::open(&self.dir.0).expect("cold open for next batch");
        if let Some(k) = self.chunk {
            s.chunk_policy = Arc::new(FixedChunkPolicy(k));
        }
        s
    }

    fn expected_groups(&self) -> BTreeMap<String, (u64, Vec<String>)> {
        self.model
            .iter()
            .filter(|(_, m)| !m.is_empty())
            .map(|(k, m)| {
                let mut v: Vec<(&i64, &usize)> = m.iter().map(|(i, o)| (o, i)).collect();
                v.sort();
                (
                    k.clone(),
                    (m.len() as u64, v.into_iter().map(|(_, i)| id(*i)).collect()),
                )
            })
            .collect()
    }

    fn check(&self, what: &str) -> Observed {
        let seen = fresh_check(&self.dir.0);
        assert!(seen.typed_ok, "[{what}] typed read of collected ids failed");
        assert_eq!(seen.groups, self.expected_groups(), "[{what}] groups");
        assert_eq!(seen.notes, self.notes, "[{what}] unrelated rows");
        seen
    }

    /// Apply one accepted batch, then verify it in a fresh process.
    fn apply(&mut self, what: &str, ops: Vec<WorldBatchOp>) -> Observed {
        let mut s = self.open();
        let base = s.current_revision();
        s.apply_batch(WorldBatch::new(base, what, ops))
            .unwrap_or_else(|e| {
                panic!("[{what}] batch refused: {e}");
            });
        drop(s);
        self.check(what)
    }

    /// Apply a batch that must be refused; HEAD and every observable must be unchanged.
    fn reject(&mut self, what: &str, ops: Vec<WorldBatchOp>) {
        let before = self.check(&format!("{what}: before"));
        let mut s = self.open();
        let base = s.current_revision();
        let err = s
            .apply_batch(WorldBatch::new(base, what, ops))
            .expect_err("over-limit batch must be refused");
        assert!(
            err.to_string().contains("maximum cardinality limit"),
            "[{what}] unexpected error: {err}"
        );
        assert_eq!(s.current_revision(), before.seq, "[{what}] HEAD seq moved");
        drop(s);
        let after = self.check(&format!("{what}: after"));
        assert_eq!(before, after, "[{what}] a rejected batch changed state");
        // The next batch still works on the same world (checked by the caller's next step).
    }

    fn insert_members_with_prefix(
        &mut self,
        prefix: &str,
        kind: &str,
        members: std::ops::Range<usize>,
        rows_per_batch: usize,
    ) {
        let all: Vec<usize> = members.collect();
        for part in all.chunks(rows_per_batch.max(1)) {
            for i in part {
                self.model
                    .entry(kind.into())
                    .or_default()
                    .insert(*i, *i as i64);
            }
            let ops = part.iter().map(|i| rec(*i, kind, *i as i64)).collect();
            self.apply(&format!("{prefix} {kind} {}..", part[0]), ops);
        }
    }

    fn insert_members(
        &mut self,
        kind: &str,
        members: std::ops::Range<usize>,
        rows_per_batch: usize,
    ) {
        self.insert_members_with_prefix("insert", kind, members, rows_per_batch);
    }

    fn remove_members(&mut self, kind: &str, members: &[usize]) {
        for i in members {
            self.model.get_mut(kind).unwrap().remove(i);
        }
        let ops = members.iter().map(|i| del(*i)).collect();
        let label = members
            .first()
            .map(|f| format!("retract {} of {kind} from r{f}", members.len()))
            .unwrap_or_else(|| format!("retract 0 of {kind}"));
        self.apply(&label, ops);
    }
}

/// The full per-size lifecycle from the A9-1 contract.
fn lifecycle(n: usize, rows_per_batch: usize, chunk: Option<usize>) {
    let label = format!("n{n}-b{rows_per_batch}-c{}", chunk.unwrap_or(0));
    let mut d = Driver::new(&label, n, chunk);

    // 1. Insert N.
    d.insert_members("g", 0..n, rows_per_batch);

    // 2. N+1 is refused as a whole, even with an unrelated write riding along.
    d.reject(
        "overflow",
        vec![unrelated("must not land"), rec(n, "g", n as i64)],
    );
    // The rejected child is absent and the unrelated write did not apply.
    assert!(!d.expected_groups()["g"].1.contains(&id(n)));

    // An unrelated write on its own still commits afterwards (the world is not wedged).
    d.notes = 1;
    d.apply("unrelated", vec![unrelated("lands")]);

    // 3. Replace a member without changing cardinality.
    d.model.get_mut("g").unwrap().insert(0, 1_000_000);
    d.apply("replace member", vec![rec(0, "g", 1_000_000)]);

    // 4. Retract N-1 members (all but the last), crossing 257/256/255 for the large sizes.
    if n > 1 {
        let keep = n - 1; // member n-1 survives
        let mut victims: Vec<usize> = (0..n).filter(|i| *i != keep).collect();
        if n >= 260 {
            // Bulk down to 259 members, then one member at a time through 258..=254.
            let bulk: Vec<usize> = victims.drain(..n - 259).collect();
            d.remove_members("g", &bulk);
            for v in victims.drain(..5) {
                d.remove_members("g", &[v]);
            }
        }
        if !victims.is_empty() && n < 260 {
            d.remove_members("g", &victims);
        }
        // Whatever is left beyond the final child goes in one batch.
        let rest: Vec<usize> = d.model["g"]
            .keys()
            .copied()
            .filter(|i| *i != keep)
            .collect();
        if !rest.is_empty() {
            d.remove_members("g", &rest);
        }
        assert_eq!(d.model["g"].len(), 1);
    }

    // 5. Retract the final child: an ordinary group disappears.
    let last: Vec<usize> = d.model["g"].keys().copied().collect();
    d.remove_members("g", &last);
    assert!(d.expected_groups().is_empty());

    // 6. Reinsert.
    d.insert_members_with_prefix("reinsert", "g", 0..n, rows_per_batch);
}

#[test]
fn collect_size_1() {
    lifecycle(1, usize::MAX, None);
}
#[test]
fn collect_size_255() {
    lifecycle(255, usize::MAX, None);
}
#[test]
fn collect_size_256() {
    lifecycle(256, usize::MAX, None);
}
#[test]
fn collect_size_257() {
    lifecycle(257, usize::MAX, None);
}
#[test]
fn collect_size_1000() {
    lifecycle(1000, usize::MAX, None);
}
#[test]
fn collect_size_5000() {
    lifecycle(5000, usize::MAX, None);
}

#[test]
fn collect_size_257_in_50_row_batches() {
    lifecycle(257, 50, None);
}
#[test]
fn collect_size_300_in_six_batches_of_50() {
    // The downstream reproduction: declared limit 300, six batches of 50.
    lifecycle(300, 50, None);
}
#[test]
fn collect_size_256_forced_chunk_policy() {
    lifecycle(256, usize::MAX, Some(7));
}
#[test]
fn collect_size_257_forced_chunk_policy() {
    lifecycle(257, usize::MAX, Some(64));
}

/// Controls from the report: 700 members over ten groups, and `count()` over one group of 700.
#[test]
fn controls_ten_groups_and_big_count() {
    let mut d = Driver::new("controls", 1000, None);
    for k in 0..10 {
        let kind = format!("k{k}");
        let ids: Vec<usize> = (k * 70..(k + 1) * 70).collect();
        for i in &ids {
            d.model
                .entry(kind.clone())
                .or_default()
                .insert(*i, *i as i64);
        }
        let ops = ids.iter().map(|i| rec(*i, &kind, *i as i64)).collect();
        d.apply(&format!("group {kind}"), ops);
    }
    let mut big = Driver::new("count700", 1000, None);
    big.insert_members("g", 0..700, usize::MAX);
    assert_eq!(big.check("count700").groups["g"].0, 700);
}

/// Equivalent histories under different internal chunk policies and batch shapes agree on
/// every observable value (revision digests may legitimately differ across batch shapes).
#[test]
fn chunk_policy_and_batch_shape_do_not_change_observables() {
    let run = |rows: usize, chunk: Option<usize>| {
        let mut d = Driver::new(&format!("equiv-{rows}-{}", chunk.unwrap_or(0)), 400, chunk);
        d.insert_members("g", 0..257, rows);
        d.model.get_mut("g").unwrap().insert(3, 9_999);
        d.apply("replace", vec![rec(3, "g", 9_999)]);
        d.remove_members("g", &[10, 11, 12]);
        d.check("final").groups
    };
    let whole = run(usize::MAX, None);
    assert_eq!(whole, run(50, None));
    assert_eq!(whole, run(usize::MAX, Some(5)));
    assert_eq!(whole, run(50, Some(13)));
}

/// Ordinary groups disappear after their final child, but parent-anchored empty-list
/// constructions retain their parent with [] across cold restore and oracle checks.
#[test]
fn parent_anchored_empty_list_retains_parent_with_empty_list_under_cold_restore() {
    let program = r#"
config Plan = AskBack(List<Str>) | Done

fn no_slots(): List<Str> = []

rel input parents: { id: Str } key id
rel input slots: { id: Str, parent: Str, slot: Str, prio: Int } key id

rel derived nonempty_slots = select {
    parent: s.parent,
    items: collect(s.slot, s.prio, 500)
} from s in slots group by s.parent

rel derived empty_slots = select {
    parent: p.id,
    items: no_slots()
} from p in parents where not exists s in slots where s.parent == p.id

rel derived parent_slots = union nonempty_slots, empty_slots key parent

decide parent_plan for p in parent_slots per parent {
    propose winner priority 1 when true = Plan::AskBack(p.items)
}
"#;
    let dir = TestDir::new("parent-anchored-cold");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let parent_upsert = |id: &str| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        WorldBatchOp::Upsert {
            relation: "root::parents".into(),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };
    let slot_upsert = |id: &str, parent: &str, slot: &str, prio: i64| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        r.set_str("parent", parent);
        r.set_str("slot", slot);
        r.set_str("prio", &prio.to_string());
        WorldBatchOp::Upsert {
            relation: "root::slots".into(),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };
    let slot_remove = |id: &str| WorldBatchOp::Remove {
        relation: "root::slots".into(),
        key: WorldKey::from_str(id),
    };

    // 1. Insert parent with no children: settlement produces AskBack([])
    session
        .apply_batch(WorldBatch::new(
            0,
            "create parent",
            vec![parent_upsert("p1")],
        ))
        .expect("batch 0");
    drop(session);

    // Fresh audit & cold reopen:
    brix_kb::world::gc::verify_store(&dir.0).expect("verify store");
    let s1 = WorldSession::open(&dir.0).expect("open 1");
    s1.verify_state().expect("verify state 1");
    s1.verify_oracle().expect("verify oracle 1");
    let settlements = s1.all_settlements().expect("settlements 1");
    assert_eq!(
        settlements["root::parent_plan"]["p1"].value,
        Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![])],
        }
    );
    drop(s1);

    // 2. Add 300 children (crossing 257 boundary)
    let mut session = WorldSession::open(&dir.0).expect("open for 300 children");
    let base = session.current_revision();
    let ops: Vec<_> = (0..300)
        .map(|i| slot_upsert(&format!("s{i:03}"), "p1", &format!("slot_{i:03}"), i as i64))
        .collect();
    session
        .apply_batch(WorldBatch::new(base, "add 300 slots", ops))
        .expect("batch 300");
    drop(session);

    // Fresh audit & cold reopen:
    brix_kb::world::gc::verify_store(&dir.0).expect("verify store");
    let s2 = WorldSession::open(&dir.0).expect("open 2");
    s2.verify_state().expect("verify state 2");
    s2.verify_oracle().expect("verify oracle 2");
    let settlements = s2.all_settlements().expect("settlements 2");
    match &settlements["root::parent_plan"]["p1"].value {
        Value::Variant { args, .. } => match &args[0] {
            Value::List(items) => assert_eq!(items.len(), 300),
            other => panic!("expected list: {other:?}"),
        },
        other => panic!("expected AskBack: {other:?}"),
    }
    drop(s2);

    // 3. Retract all 300 children: parent settlement is retained with []
    let mut session = WorldSession::open(&dir.0).expect("open for retract");
    let base = session.current_revision();
    let ops: Vec<_> = (0..300).map(|i| slot_remove(&format!("s{i:03}"))).collect();
    session
        .apply_batch(WorldBatch::new(base, "retract 300 slots", ops))
        .expect("retract batch");
    drop(session);

    // Fresh audit & cold reopen:
    brix_kb::world::gc::verify_store(&dir.0).expect("verify store");
    let s3 = WorldSession::open(&dir.0).expect("open 3");
    s3.verify_state().expect("verify state 3");
    s3.verify_oracle().expect("verify oracle 3");
    let settlements = s3.all_settlements().expect("settlements 3");
    assert_eq!(
        settlements["root::parent_plan"]["p1"].value,
        Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![])],
        },
        "retracting all children retains parent with empty list []"
    );
}

/// A declared limit the persisted state cannot represent is refused at lowering, not after
/// commit.
#[test]
fn declared_limit_above_persistable_bound_is_refused_up_front() {
    let dir = TestDir::new("toobig");
    let sources = BTreeMap::from([("root".into(), program(65_537))]);
    let err = WorldSession::from_program_with_sources(&dir.0, "root", &sources)
        .err()
        .expect("limit above the persistable bound must be rejected")
        .to_string();
    assert!(
        err.contains("exceeds maximum supported cardinality limit"),
        "{err}"
    );
}

/// Damaged pack nodes on a 257-member collect world fail closed as `CorruptState`, are never
/// reinterpreted as an empty group, and leave the damaged world untouched. Verified journal
/// revision deltas can be replayed into a separate destination directory to recover the state.
#[test]
fn damaged_world_fails_closed_and_verified_journal_replays_into_separate_destination() {
    let mut d = Driver::new("damage-and-replay", 500, None);
    d.insert_members("g", 0..257, 50);
    let expected = d.check("before-damage");

    // 1. Reconstruct and replay verified revision deltas from the source world into a separate
    //    destination directory before damaging the pack file, proving separate-destination
    //    journal recovery works.
    let src_session = WorldSession::open(&d.dir.0).expect("open source");
    let head_seq = src_session.current_revision();
    let mut replay_batches = Vec::new();
    for seq in 1..=head_seq {
        let prev = src_session.pin_revision(seq - 1).expect("pin prev");
        let curr = src_session.pin_revision(seq).expect("pin curr");
        let mut ops = Vec::new();
        for (relation, keys) in &curr.revision.changed_keys {
            for key in keys {
                match curr.get(relation, key).expect("get curr") {
                    Some(tuple) => ops.push(WorldBatchOp::Upsert {
                        relation: relation.clone(),
                        key: key.clone(),
                        tuple,
                    }),
                    None => {
                        if prev.get(relation, key).expect("get prev").is_some() {
                            ops.push(WorldBatchOp::Remove {
                                relation: relation.clone(),
                                key: key.clone(),
                            });
                        }
                    }
                }
            }
        }
        replay_batches.push(WorldBatch::new(
            seq - 1,
            curr.revision.idempotency_key.clone(),
            ops,
        ));
    }
    drop(src_session);

    let replay_dir = TestDir::new("replay-dest");
    let sources = BTreeMap::from([("root".into(), program(500))]);
    let mut dst_session = WorldSession::from_program_with_sources(&replay_dir.0, "root", &sources)
        .expect("create replay destination");
    for batch in replay_batches {
        dst_session.apply_batch(batch).expect("replay batch");
    }
    drop(dst_session);

    brix_kb::world::gc::verify_store(&replay_dir.0).expect("verify replayed store");
    let replayed = WorldSession::open(&replay_dir.0).expect("open replayed");
    replayed.verify_state().expect("verify replayed state");
    replayed.verify_oracle().expect("verify replayed oracle");
    assert_eq!(replayed.current_revision(), expected.seq);
    drop(replayed);

    // 2. Corrupt a pack body byte in the original world. Verify that `verify_store` fails
    //    closed as `CorruptState` without silently rewriting the damaged world or returning
    //    an empty group.
    let objects_dir = brix_kb::world::WorldPaths::new(&d.dir.0).objects_dir();
    let pack = std::fs::read_dir(&objects_dir)
        .expect("read objects dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("pack"))
        .expect("at least one pack file");
    let mut bytes = std::fs::read(&pack).expect("read pack");
    bytes[8 + 3] ^= 0x40;
    std::fs::write(&pack, &bytes).expect("write damaged pack");

    let err = brix_kb::world::gc::verify_store(&d.dir.0)
        .expect_err("damaged pack must fail verify_store");
    assert!(
        matches!(err, brix_kb::world::WorldError::CorruptState(_)),
        "expected CorruptState, got: {err:?}"
    );
    // Damaged pack bytes on disk remain untouched (no silent rewrite).
    assert_eq!(
        std::fs::read(&pack).expect("re-read damaged pack"),
        bytes,
        "damaged world must never be silently rewritten"
    );
}
