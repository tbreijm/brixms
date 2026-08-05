//! Frozen L3 Stage A canonical-identity vectors (ADR-0012 §3.1–§3.3, §9 Stage
//! A).
//!
//! One small plan — one config, one `let`, one rule that publishes it — is
//! lowered by [`brix_lower::lower_l3_plan`], run through every production
//! encoder in `brix_lower::l3_canon`, and frozen in
//! `vectors/l3_plan_v1.json` together with every pinned preimage and
//! identity: the plan (`ProgramIdV1`), the rule (`RuleId`), the fact payload
//! (`L3ValueId`), the initial and terminal worlds (`ConfigId`), the one
//! generator/witness pair (`GeneratorId`/`WitnessId`), the fact (`ConfigId`),
//! the policy (`ConfigId`), the run context (`ContextId`), and the
//! presentation revision (`PresentationIdV1`).
//!
//! Two consumers guard the manifest, following
//! `crates/brix-kernel/tests/certificate_vectors.rs`'s rule:
//!
//! 1. `l3_plan_vectors_are_frozen` — the production encoders in
//!    `brix_lower::l3_canon` must keep reproducing the committed bytes
//!    (regenerate with `BLESS_VECTORS=1`, and review the hex diff by hand);
//! 2. `l3_plan_vectors_reproduced_by_primitive_canon_writes` — a second
//!    construction path that spells out every pinned field with primitive
//!    `brix_canon::CanonWriter` operations and **repeats the frozen
//!    literals** (marker bytes, tag strings, enum ordinals) rather than
//!    importing `l3_canon`'s constants, and never calls a production encoder
//!    function. A vector that only exercised the first consumer could be
//!    vacuously satisfied by a typo'd constant agreeing with itself; this is
//!    the consumer that actually guards the byte layout.
//!
//! After the freeze this manifest is append-only: an existing case may never
//! change without a new format version for the affected encoder (mirroring
//! ADR-0013 §7's evolution rule, which this ADR's §3.1 explicitly asks
//! encoders to follow).

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Digest, Domain};
use brix_lower::{
    context_id, fact_id, l3_generator_id, l3_value_id, l3_witness_id, lower_l3_plan, policy_id,
    program_id, rule_id, world_id, FactChainIdV1, FactV1, L3ConfigBody, L3PlanItem, L3PlanV1,
    L3PolicyV1, L3TypeRef, L3ValueV1, L3WorldV1, PendingIdV1, PlanLimitsV1, PresentationIdV1,
    RunContextV1, L3_PROFILE_MARKER_V1,
};
use brix_semantic::RegimeId;
use brix_syntax::parse;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

const FIXTURE_SOURCE: &str = r#"
    config Item = { name: Str, base: Int }
    let widget = Item { name: "widget", base: 10 }
    rule publish() = widget
"#;

/// A textually different, structurally equivalent source (whitespace,
/// comments, and int/string spelling all vary) — used only to double-check
/// that this fixture's `ProgramIdV1` is source-shape-insensitive, alongside
/// the dedicated unit tests in `l3_canon`.
const FIXTURE_SOURCE_EQUIVALENT: &str =
    "config Item={name:Str,base:Int}\n// widget config\nlet widget=Item{base:010,name:\"widget\"} // the widget\nrule publish()=widget\n";

fn fixture_limits() -> PlanLimitsV1 {
    PlanLimitsV1 {
        max_selected_rules: 8,
        max_config_nodes: 64,
        max_total_value_nodes: 256,
        max_total_value_bytes: 4096,
        max_value_depth: 16,
    }
}

fn fixture_plan(source: &str) -> L3PlanV1 {
    let module = parse(source).unwrap_or_else(|e| panic!("parse failed: {e}"));
    lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &fixture_limits())
        .unwrap_or_else(|e| panic!("lowering failed: {e:?}"))
}

/// The normalized value of both `widget` (the `let`) and `publish`'s rule
/// body — they are the same closed `L3ValueV1` (a `let` reference resolves to
/// its already-closed value, ADR-0012 §3.1), so there is exactly one payload
/// value in this fixture.
fn fixture_value(plan: &L3PlanV1) -> L3ValueV1 {
    match &plan.items[1] {
        L3PlanItem::Let { value, .. } => value.clone(),
        other => panic!("expected Let at index 1, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The independent construction path.
//
// Every marker byte string, tag string, and enum ordinal below is a literal,
// re-typed rather than imported from `brix_lower::l3_canon`, per this
// manifest's module doc. Only `brix_canon`'s own primitive `CanonWriter`
// operations and `Digest::of` are used — never a `brix_lower::l3_canon`
// encoder function.
// ---------------------------------------------------------------------------

fn independent_type_ref_bytes(w: &mut CanonWriter, ty: &L3TypeRef) {
    match ty {
        L3TypeRef::Int => w.write_uint(0),
        L3TypeRef::Str => w.write_uint(1),
        L3TypeRef::Config(name) => {
            w.write_uint(2);
            w.write_ident(name);
        }
    }
}

fn independent_config_body_bytes(w: &mut CanonWriter, body: &L3ConfigBody) {
    match body {
        L3ConfigBody::Record(fields) => {
            w.write_uint(0);
            w.write_list(fields.iter().map(|(name, ty)| {
                let mut fw = CanonWriter::new();
                fw.write_ident(name);
                independent_type_ref_bytes(&mut fw, ty);
                fw.finish()
            }));
        }
        L3ConfigBody::Sum(variants) => {
            w.write_uint(1);
            w.write_list(variants.iter().map(|(name, params)| {
                let mut vw = CanonWriter::new();
                vw.write_ident(name);
                vw.write_list(params.iter().map(|ty| {
                    let mut tw = CanonWriter::new();
                    independent_type_ref_bytes(&mut tw, ty);
                    tw.finish()
                }));
                vw.finish()
            }));
        }
    }
}

fn independent_value_bytes(w: &mut CanonWriter, value: &L3ValueV1) {
    match value {
        L3ValueV1::Int(i) => {
            w.write_uint(0);
            w.write_int(*i);
        }
        L3ValueV1::Str(s) => {
            w.write_uint(1);
            w.write_str(s);
        }
        L3ValueV1::Record {
            nominal_config,
            fields,
        } => {
            w.write_uint(2);
            w.write_ident(nominal_config);
            w.write_list(fields.iter().map(|(name, value)| {
                let mut fw = CanonWriter::new();
                fw.write_ident(name);
                independent_value_bytes(&mut fw, value);
                fw.finish()
            }));
        }
        L3ValueV1::NullaryVariant {
            nominal_sum,
            variant,
        } => {
            w.write_uint(3);
            w.write_ident(nominal_sum);
            w.write_ident(variant);
        }
    }
}

fn independent_value_preimage(value: &L3ValueV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.value");
    w.write_uint(1);
    independent_value_bytes(&mut w, value);
    w.finish()
}

fn independent_value_id(value: &L3ValueV1) -> Digest {
    Digest::of(Domain::Value, &independent_value_preimage(value))
}

fn independent_item_bytes(item: &L3PlanItem) -> Vec<u8> {
    let mut w = CanonWriter::new();
    match item {
        L3PlanItem::Config(decl) => {
            w.write_uint(0);
            w.write_ident(&decl.name);
            independent_config_body_bytes(&mut w, &decl.body);
        }
        L3PlanItem::Let { name, value } => {
            w.write_uint(1);
            w.write_ident(name);
            w.write_bytes(independent_value_id(value).as_bytes());
        }
        L3PlanItem::Rule {
            ordinal,
            name,
            value,
        } => {
            w.write_uint(2);
            w.write_uint(*ordinal);
            w.write_ident(name);
            w.write_bytes(independent_value_id(value).as_bytes());
        }
    }
    w.finish()
}

fn independent_program_preimage(plan: &L3PlanV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.plan");
    w.write_uint(1);
    w.write_str("brix.l3.rule-agenda-saturated@1");
    w.write_list(plan.items.iter().map(independent_item_bytes));
    w.finish()
}

fn independent_program_id(plan: &L3PlanV1) -> Digest {
    Digest::of(Domain::Value, &independent_program_preimage(plan))
}

fn independent_rule_preimage(program: Digest, ordinal: u64, name: &str) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.rule@1");
    w.write_bytes(program.as_bytes());
    w.write_uint(ordinal);
    w.write_ident(name);
    w.finish()
}

fn independent_rule_id(program: Digest, ordinal: u64, name: &str) -> Digest {
    Digest::of(
        Domain::Value,
        &independent_rule_preimage(program, ordinal, name),
    )
}

fn independent_generator_preimage(
    program: Digest,
    rule: Digest,
    src: Digest,
    dst: Digest,
) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.generator@1");
    w.write_bytes(program.as_bytes());
    w.write_bytes(rule.as_bytes());
    w.write_bytes(src.as_bytes());
    w.write_bytes(dst.as_bytes());
    w.finish()
}

fn independent_generator_id(program: Digest, rule: Digest, src: Digest, dst: Digest) -> Digest {
    Digest::of(
        Domain::Value,
        &independent_generator_preimage(program, rule, src, dst),
    )
}

fn independent_pending_empty() -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.pending");
    w.write_uint(1);
    w.write_uint(0);
    Digest::of(Domain::Value, &w.finish())
}

fn independent_pending_cons(rule: Digest, tail: Digest) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.pending");
    w.write_uint(1);
    w.write_uint(1);
    w.write_bytes(rule.as_bytes());
    w.write_bytes(tail.as_bytes());
    Digest::of(Domain::Value, &w.finish())
}

fn independent_fact_chain_genesis() -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.fact-chain");
    w.write_uint(1);
    w.write_uint(0);
    Digest::of(Domain::Value, &w.finish())
}

fn independent_fact_chain_append(prior: Digest, fact: Digest) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.fact-chain");
    w.write_uint(1);
    w.write_uint(1);
    w.write_bytes(prior.as_bytes());
    w.write_bytes(fact.as_bytes());
    Digest::of(Domain::Value, &w.finish())
}

fn independent_fact_bytes(rule: Digest, payload: Digest) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(rule.as_bytes());
    w.write_bytes(payload.as_bytes());
    w.finish()
}

fn independent_fact_id(rule: Digest, payload: Digest) -> Digest {
    Digest::of(Domain::Value, &independent_fact_bytes(rule, payload))
}

fn independent_world_bytes(
    program: Digest,
    pending: Digest,
    facts: Digest,
    fact_count: u64,
) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.world");
    w.write_uint(1);
    w.write_bytes(program.as_bytes());
    w.write_bytes(pending.as_bytes());
    w.write_bytes(facts.as_bytes());
    w.write_uint(fact_count);
    w.finish()
}

fn independent_world_id(
    program: Digest,
    pending: Digest,
    facts: Digest,
    fact_count: u64,
) -> Digest {
    Digest::of(
        Domain::Value,
        &independent_world_bytes(program, pending, facts, fact_count),
    )
}

fn independent_policy_bytes(plan: Digest, profile: &str, regime: Digest) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(plan.as_bytes());
    w.write_str(profile);
    w.write_bytes(regime.as_bytes());
    w.finish()
}

fn independent_policy_id(plan: Digest, profile: &str, regime: Digest) -> Digest {
    Digest::of(
        Domain::Value,
        &independent_policy_bytes(plan, profile, regime),
    )
}

#[allow(clippy::too_many_arguments)]
fn independent_run_context_bytes(
    program: Digest,
    initial_world: Digest,
    policy: Digest,
    profile: &str,
    limits: &PlanLimitsV1,
) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_uint(1); // format version — no leading marker (ADR-0012 §3.3's pinned field list starts at "format version")
    w.write_bytes(program.as_bytes());
    w.write_bytes(initial_world.as_bytes());
    w.write_bytes(policy.as_bytes());
    w.write_str(profile);
    w.write_uint(limits.max_selected_rules);
    w.write_uint(limits.max_config_nodes);
    w.write_uint(limits.max_total_value_nodes);
    w.write_uint(limits.max_total_value_bytes);
    w.write_uint(limits.max_value_depth);
    w.finish()
}

#[allow(clippy::too_many_arguments)]
fn independent_run_context_id(
    program: Digest,
    initial_world: Digest,
    policy: Digest,
    profile: &str,
    limits: &PlanLimitsV1,
) -> Digest {
    Digest::of(
        Domain::Value,
        &independent_run_context_bytes(program, initial_world, policy, profile, limits),
    )
}

// ---------------------------------------------------------------------------
// Manifest rendering. Hand-built ASCII JSON, following
// `certificate_vectors.rs`'s precedent: `brix-lower` carries no dependencies
// beyond what it already needs, and gains none for its tests.
// ---------------------------------------------------------------------------

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).expect("nibble is a hex digit"));
        s.push(char::from_digit((b & 0xf) as u32, 16).expect("nibble is a hex digit"));
    }
    s
}

fn json_str(value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 2);
    s.push('"');
    for c in value.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            other => s.push(other),
        }
    }
    s.push('"');
    s
}

/// Everything the manifest needs, computed once through the production
/// encoders (`brix_lower::l3_canon`, re-exported at the crate root).
struct Computed {
    plan: L3PlanV1,
    program: brix_lower::ProgramIdV1,
    rule: brix_lower::RuleId,
    value: L3ValueV1,
    src_world: L3WorldV1,
    dst_world: L3WorldV1,
    fact: FactV1,
    generator: brix_semantic::GeneratorId,
    witness: brix_semantic::WitnessId,
    policy: L3PolicyV1,
    context: RunContextV1,
    presentation: PresentationIdV1,
}

fn compute() -> Computed {
    let plan = fixture_plan(FIXTURE_SOURCE);
    let program = program_id(&plan);
    let rule = rule_id(program, 0, "publish");
    let value = fixture_value(&plan);
    let payload = l3_value_id(&value);

    let pending0 = brix_lower::build_pending(&[rule]);
    let facts0 = FactChainIdV1::genesis();
    let src_world = L3WorldV1 {
        program,
        pending: pending0,
        facts: facts0,
        fact_count: 0,
    };
    let src = world_id(&src_world);

    let fact = FactV1 { rule, payload };
    let fact_cid = fact_id(&fact);
    let pending1 = PendingIdV1::empty();
    let facts1 = FactChainIdV1::append(facts0, fact_cid);
    let dst_world = L3WorldV1 {
        program,
        pending: pending1,
        facts: facts1,
        fact_count: 1,
    };
    let dst = world_id(&dst_world);

    let generator = l3_generator_id(program, rule, src, dst);
    let witness = l3_witness_id(generator);

    let policy = L3PolicyV1 {
        plan: program,
        profile: L3_PROFILE_MARKER_V1.to_string(),
        regime: RegimeId::named(L3_PROFILE_MARKER_V1),
    };

    let context = RunContextV1 {
        program,
        initial_world: src,
        policy: policy_id(&policy),
        profile: L3_PROFILE_MARKER_V1.to_string(),
        limits: fixture_limits(),
    };

    let presentation = PresentationIdV1::of_program(program);

    Computed {
        plan,
        program,
        rule,
        value,
        src_world,
        dst_world,
        fact,
        generator,
        witness,
        policy,
        context,
        presentation,
    }
}

fn build_manifest() -> String {
    let c = compute();

    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.l3.plan\",\n");
    out.push_str("  \"plan_format_version\": 1,\n");
    out.push_str(&format!(
        "  \"profile\": {},\n",
        json_str(L3_PROFILE_MARKER_V1)
    ));
    out.push_str(
        "  \"note\": \"Frozen L3 Stage A canonical-identity vectors (ADR-0012 \
S3.1-S3.3, S9 Stage A). Append-only: an existing case may never change \
without a new encoder format version. Regenerate with BLESS_VECTORS=1.\",\n",
    );
    out.push_str("  \"limits\": {\n");
    out.push_str(&format!(
        "    \"max_selected_rules\": {},\n",
        c.context.limits.max_selected_rules
    ));
    out.push_str(&format!(
        "    \"max_config_nodes\": {},\n",
        c.context.limits.max_config_nodes
    ));
    out.push_str(&format!(
        "    \"max_total_value_nodes\": {},\n",
        c.context.limits.max_total_value_nodes
    ));
    out.push_str(&format!(
        "    \"max_total_value_bytes\": {},\n",
        c.context.limits.max_total_value_bytes
    ));
    out.push_str(&format!(
        "    \"max_value_depth\": {}\n",
        c.context.limits.max_value_depth
    ));
    out.push_str("  },\n");
    out.push_str("  \"cases\": [\n");
    out.push_str("    {\n");
    out.push_str("      \"name\": \"one_rule_publish\",\n");
    out.push_str(
        "      \"description\": \"one config, one let, one rule publishing it: \
the smallest fixture exercising a plan, rule, initial/terminal world, \
generator, witness, fact, policy, and run context together\",\n",
    );
    out.push_str(&format!(
        "      \"source\": {},\n",
        json_str(FIXTURE_SOURCE)
    ));
    out.push_str(&format!(
        "      \"program_preimage_hex\": \"{}\",\n",
        to_hex(&brix_lower::program_preimage(&c.plan))
    ));
    out.push_str(&format!(
        "      \"program_id\": \"{}\",\n",
        c.program.to_hex()
    ));
    out.push_str("      \"rule\": {\n");
    out.push_str("        \"name\": \"publish\",\n");
    out.push_str("        \"ordinal\": 0,\n");
    out.push_str(&format!(
        "        \"preimage_hex\": \"{}\",\n",
        to_hex(&brix_lower::rule_preimage(c.program, 0, "publish"))
    ));
    out.push_str(&format!("        \"rule_id\": \"{}\"\n", c.rule.to_hex()));
    out.push_str("      },\n");
    out.push_str("      \"fact_payload\": {\n");
    out.push_str("        \"shape\": \"Record(Item, [name: Str(widget), base: Int(10)])\",\n");
    out.push_str(&format!(
        "        \"preimage_hex\": \"{}\",\n",
        to_hex(&brix_lower::l3_value_preimage(&c.value))
    ));
    out.push_str(&format!(
        "        \"value_id\": \"{}\"\n",
        l3_value_id(&c.value).to_hex()
    ));
    out.push_str("      },\n");
    out.push_str("      \"initial_world\": {\n");
    out.push_str(&format!(
        "        \"fact_count\": {},\n",
        c.src_world.fact_count
    ));
    out.push_str(&format!(
        "        \"world_id\": \"{}\"\n",
        world_id(&c.src_world).to_hex()
    ));
    out.push_str("      },\n");
    out.push_str("      \"terminal_world\": {\n");
    out.push_str(&format!(
        "        \"fact_count\": {},\n",
        c.dst_world.fact_count
    ));
    out.push_str(&format!(
        "        \"world_id\": \"{}\"\n",
        world_id(&c.dst_world).to_hex()
    ));
    out.push_str("      },\n");
    out.push_str("      \"generator\": {\n");
    out.push_str(&format!(
        "        \"preimage_hex\": \"{}\",\n",
        to_hex(&l3_generator_preimage_for_manifest(&c))
    ));
    out.push_str(&format!(
        "        \"generator_id\": \"{}\"\n",
        c.generator.to_hex()
    ));
    out.push_str("      },\n");
    out.push_str(&format!(
        "      \"witness_id\": \"{}\",\n",
        c.witness.to_hex()
    ));
    out.push_str(&format!(
        "      \"fact_id\": \"{}\",\n",
        fact_id(&c.fact).to_hex()
    ));
    out.push_str("      \"policy\": {\n");
    out.push_str(&format!(
        "        \"regime_id\": \"{}\",\n",
        c.policy.regime.to_hex()
    ));
    out.push_str(&format!(
        "        \"policy_id\": \"{}\"\n",
        policy_id(&c.policy).to_hex()
    ));
    out.push_str("      },\n");
    out.push_str(&format!(
        "      \"run_context_id\": \"{}\",\n",
        context_id(&c.context).to_hex()
    ));
    out.push_str(&format!(
        "      \"presentation_id\": \"{}\"\n",
        c.presentation.to_hex()
    ));
    out.push_str("    }\n");
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

/// The production `l3_generator_preimage` call, isolated so `build_manifest`
/// reads linearly.
fn l3_generator_preimage_for_manifest(c: &Computed) -> Vec<u8> {
    brix_lower::l3_generator_preimage(
        c.program,
        c.rule,
        world_id(&c.src_world),
        world_id(&c.dst_world),
    )
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("l3_plan_v1.json")
}

#[test]
fn l3_plan_vectors_are_frozen() {
    let generated = build_manifest();
    let path = manifest_path();
    let committed = std::fs::read_to_string(&path).unwrap_or_default();

    if generated == committed {
        return;
    }

    if std::env::var_os("BLESS_VECTORS").is_some() {
        std::fs::write(&path, &generated).expect("vector manifest is writable");
        return;
    }

    panic!(
        "L3 plan vectors drifted from {}.\n\
         These identities are frozen ABI — this is a compatibility break, not \
         a refresh. If the change is intended and versioned, regenerate with \
         BLESS_VECTORS=1 and review the diff by hand.",
        path.display()
    );
}

#[test]
fn l3_plan_vectors_reproduced_by_primitive_canon_writes() {
    let c = compute();

    // Program.
    let independent_program = independent_program_id(&c.plan);
    assert_eq!(
        to_hex(independent_program.as_bytes()),
        c.program.to_hex(),
        "ProgramIdV1 differs from the independent reproduction"
    );
    assert_eq!(
        to_hex(&independent_program_preimage(&c.plan)),
        to_hex(&brix_lower::program_preimage(&c.plan)),
        "program preimage bytes differ from the independent reproduction"
    );

    // Rule.
    let independent_rule = independent_rule_id(independent_program, 0, "publish");
    assert_eq!(
        to_hex(independent_rule.as_bytes()),
        c.rule.to_hex(),
        "RuleId differs from the independent reproduction"
    );

    // Fact payload value.
    let independent_payload = independent_value_id(&c.value);
    assert_eq!(
        to_hex(independent_payload.as_bytes()),
        l3_value_id(&c.value).to_hex(),
        "L3ValueId differs from the independent reproduction"
    );

    // Initial world.
    let independent_pending0 =
        independent_pending_cons(independent_rule, independent_pending_empty());
    let independent_facts0 = independent_fact_chain_genesis();
    let independent_src = independent_world_id(
        independent_program,
        independent_pending0,
        independent_facts0,
        0,
    );
    assert_eq!(
        to_hex(independent_src.as_bytes()),
        world_id(&c.src_world).to_hex(),
        "initial world id differs from the independent reproduction"
    );

    // Terminal world.
    let independent_fact = independent_fact_id(independent_rule, independent_payload);
    assert_eq!(
        to_hex(independent_fact.as_bytes()),
        fact_id(&c.fact).to_hex(),
        "fact id differs from the independent reproduction"
    );
    let independent_pending1 = independent_pending_empty();
    let independent_facts1 = independent_fact_chain_append(independent_facts0, independent_fact);
    let independent_dst = independent_world_id(
        independent_program,
        independent_pending1,
        independent_facts1,
        1,
    );
    assert_eq!(
        to_hex(independent_dst.as_bytes()),
        world_id(&c.dst_world).to_hex(),
        "terminal world id differs from the independent reproduction"
    );

    // Generator / witness.
    let independent_generator = independent_generator_id(
        independent_program,
        independent_rule,
        independent_src,
        independent_dst,
    );
    assert_eq!(
        to_hex(independent_generator.as_bytes()),
        c.generator.to_hex(),
        "GeneratorId differs from the independent reproduction"
    );
    // A generator is a primitive witness: same digest, different wrapper —
    // reproduced here by comparing raw hex rather than calling
    // `GeneratorId::witness_id`.
    assert_eq!(
        to_hex(independent_generator.as_bytes()),
        c.witness.to_hex(),
        "WitnessId differs from the independent reproduction"
    );

    // Policy.
    let independent_regime = {
        // `RegimeId::named` is a reused, already-frozen `brix-semantic`
        // encoder (ADR-0012 §3.1 explicitly reuses it), so it is called
        // directly here rather than reproduced byte-by-byte — precisely the
        // same treatment `certificate_vectors.rs` gives `PropositionId`'s
        // already-frozen `Canonical` impl.
        RegimeId::named(L3_PROFILE_MARKER_V1)
    };
    let independent_policy = independent_policy_id(
        independent_program,
        L3_PROFILE_MARKER_V1,
        independent_regime.digest(),
    );
    assert_eq!(
        to_hex(independent_policy.as_bytes()),
        policy_id(&c.policy).to_hex(),
        "policy id differs from the independent reproduction"
    );

    // Run context.
    let independent_context = independent_run_context_id(
        independent_program,
        independent_src,
        independent_policy,
        L3_PROFILE_MARKER_V1,
        &fixture_limits(),
    );
    assert_eq!(
        to_hex(independent_context.as_bytes()),
        context_id(&c.context).to_hex(),
        "run context id differs from the independent reproduction"
    );

    // Presentation.
    assert_eq!(
        to_hex(independent_program.as_bytes()),
        c.presentation.to_hex(),
        "PresentationIdV1 differs from the independent reproduction"
    );
}

#[test]
fn presentation_id_equals_presentation_id_of_program_digest() {
    let c = compute();
    assert_eq!(c.presentation, PresentationIdV1::of_program(c.program));
    assert_eq!(c.presentation, PresentationIdV1(c.program.digest()));
}

#[test]
fn textually_different_equivalent_modules_give_the_identical_program_id() {
    // Whitespace, a comment, and an equivalent integer spelling (`010` vs.
    // `10`) all differ between the two sources; the resulting `ProgramIdV1`
    // must not (ADR-0012 §9 Stage A).
    let a = fixture_plan(FIXTURE_SOURCE);
    let b = fixture_plan(FIXTURE_SOURCE_EQUIVALENT);
    assert_eq!(program_id(&a), program_id(&b));
}
