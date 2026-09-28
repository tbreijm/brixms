//! Property-based robustness ("fuzz") tests for the external input decoder
//! (ADR-0031, `crates/brix-lower/src/input.rs`): the strict, bounded
//! `brix.input@1`/`@2`/`@3` shard decoder and canonical snapshot builder.
//! `@3` additionally admits a top-level list value (ADR-0037).
//!
//! `crates/brix-lower/tests/external_input.rs` already pins the decoder's
//! contract by hand (duplicate-key rejection, strict schema/type checking,
//! bounded reads, TOCTOU hardening, UTF-8/surrogate correctness, shard-order
//! identity...). This file explores the same "attacker-controlled bytes in,
//! typed value or typed error out, never a panic" surface by generation.
//!
//! Case counts are proptest's own default (256) unless overridden via
//! `PROPTEST_CASES`. See `docs/performance.md` for measured wall-clock cost.

use proptest::prelude::*;

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, validate_against_declarations,
    validate_completeness, InputLimits,
};
use brix_syntax::parse;

// ---------------------------------------------------------------------------
// Arbitrary bytes.
// ---------------------------------------------------------------------------

proptest! {
    /// Arbitrary bytes — not necessarily UTF-8, not necessarily JSON at all —
    /// must decode to `Ok` or a typed `Err`, never panic.
    #[test]
    fn decode_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let limits = InputLimits::default();
        let _ = decode_input_shard(&bytes, &limits);
    }
}

// ---------------------------------------------------------------------------
// Arbitrary JSON values.
// ---------------------------------------------------------------------------

/// A restricted charset (`[a-zA-Z0-9 ]`) for generated object keys and string
/// leaves: it makes every generated value already-valid JSON text without
/// needing a JSON string escaper in the test harness itself (exotic string
/// content — control characters, surrogate pairs, raw non-ASCII UTF-8— is
/// already covered byte-for-byte by `external_input.rs`'s
/// `test_json_unicode_escapes_and_utf8_correctness` and by the
/// arbitrary-bytes/mutation properties in this file). What is being fuzzed
/// here is JSON *shape* — nesting, object/array mixing, duplicate keys,
/// wrong top-level type — not string escaping.
fn json_key_or_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ]{0,20}"
}

#[derive(Clone, Debug)]
enum JsonVal {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<JsonVal>),
    /// Deliberately a `Vec`, not a map: JSON object keys need not be unique
    /// at the syntax level, and generating occasional duplicate keys is
    /// exactly what exercises the decoder's duplicate-key rejection path.
    Obj(Vec<(String, JsonVal)>),
}

fn json_strategy() -> impl Strategy<Value = JsonVal> {
    let leaf = prop_oneof![
        Just(JsonVal::Null),
        any::<bool>().prop_map(JsonVal::Bool),
        (-1.0e6f64..1.0e6f64).prop_map(JsonVal::Num),
        json_key_or_string().prop_map(JsonVal::Str),
    ];
    leaf.prop_recursive(4, 64, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(JsonVal::Arr),
            proptest::collection::vec((json_key_or_string(), inner), 0..4).prop_map(JsonVal::Obj),
        ]
    })
}

fn to_json(v: &JsonVal) -> String {
    match v {
        JsonVal::Null => "null".to_string(),
        JsonVal::Bool(b) => b.to_string(),
        JsonVal::Num(f) => {
            if *f == f.trunc() && f.abs() < 1.0e15 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        JsonVal::Str(s) => format!("\"{s}\""),
        JsonVal::Arr(items) => format!(
            "[{}]",
            items.iter().map(to_json).collect::<Vec<_>>().join(",")
        ),
        JsonVal::Obj(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(k, v)| format!("\"{k}\":{}", to_json(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

proptest! {
    /// Arbitrary well-formed JSON — any shape, any top-level type, possibly
    /// duplicate object keys — almost never matches the `brix.input@1`/`@2`
    /// envelope schema, but the decoder must still only ever return
    /// `Ok`/`Err`.
    #[test]
    fn decode_arbitrary_json_never_panics(v in json_strategy()) {
        let text = to_json(&v);
        let limits = InputLimits::default();
        let _ = decode_input_shard(text.as_bytes(), &limits);
    }
}

// ---------------------------------------------------------------------------
// Random mutations of the shipped example input JSON.
// ---------------------------------------------------------------------------

const EXAMPLE_JSONS: &[&str] = &[
    include_str!("../../../examples/allocation.json"),
    include_str!("../../../examples/order-policy.json"),
    include_str!("../../../examples/shipping-functions.json"),
    include_str!("../../../examples/shipping-input.json"),
];

#[derive(Clone, Debug)]
enum ByteMutation {
    DeleteByte(f64),
    InsertByte(f64, u8),
    DuplicateByte(f64),
    FlipBit(f64, u8),
}

fn byte_mutation_strategy() -> impl Strategy<Value = ByteMutation> {
    let unit = 0.0f64..1.0f64;
    prop_oneof![
        unit.clone().prop_map(ByteMutation::DeleteByte),
        (unit.clone(), any::<u8>()).prop_map(|(f, b)| ByteMutation::InsertByte(f, b)),
        unit.clone().prop_map(ByteMutation::DuplicateByte),
        (unit, 0u8..8).prop_map(|(f, bit)| ByteMutation::FlipBit(f, bit)),
    ]
}

fn frac_index(f: f64, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        ((f.clamp(0.0, 1.0) * len as f64) as usize).min(len - 1)
    }
}

fn apply_byte_mutation(bytes: &mut Vec<u8>, m: &ByteMutation) {
    if bytes.is_empty() {
        bytes.push(b' ');
    }
    match m {
        ByteMutation::DeleteByte(f) => {
            let i = frac_index(*f, bytes.len());
            bytes.remove(i);
        }
        ByteMutation::InsertByte(f, b) => {
            let i = frac_index(*f, bytes.len() + 1).min(bytes.len());
            bytes.insert(i, *b);
        }
        ByteMutation::DuplicateByte(f) => {
            let i = frac_index(*f, bytes.len());
            let b = bytes[i];
            bytes.insert(i, b);
        }
        ByteMutation::FlipBit(f, bit) => {
            let i = frac_index(*f, bytes.len());
            bytes[i] ^= 1u8 << bit;
        }
    }
}

proptest! {
    /// 0..16 random byte-level edits to one of the shipped example
    /// `examples/*.json` input artifacts. Almost always no longer valid
    /// UTF-8, valid JSON, or a valid envelope, but the decoder must still
    /// only ever return `Ok`/`Err`.
    #[test]
    fn mutated_example_json_never_panics(
        fixture_idx in 0..EXAMPLE_JSONS.len(),
        muts in proptest::collection::vec(byte_mutation_strategy(), 0..16),
    ) {
        let mut bytes = EXAMPLE_JSONS[fixture_idx].as_bytes().to_vec();
        for m in &muts {
            apply_byte_mutation(&mut bytes, m);
        }
        let limits = InputLimits::default();
        let _ = decode_input_shard(&bytes, &limits);

        // A mutated shard that DOES decode must still canonicalize (or fail
        // typed) rather than panic — exercises the aggregation path too.
        if let Ok(shard) = decode_input_shard(&bytes, &limits) {
            let _ = canonicalize_input_shards(vec![shard], &limits);
        }
    }
}

// ---------------------------------------------------------------------------
// Generated valid inputs for a declared schema decode successfully.
// ---------------------------------------------------------------------------

const DECLARED_INPUTS_SOURCE: &str = r#"
input a: Int
input b: Bool
input c: Str
config D = Yes
propose p() priority 1 when true = Yes
commit outcome from (p)
"#;

fn json_escape_ascii_only(s: &str) -> String {
    // `c`'s generator below is restricted to printable ASCII minus '"'/'\\',
    // so this is a defensive no-op in practice; kept so a future widening of
    // the generator fails loudly (mismatched output) rather than producing
    // invalid JSON silently.
    s.chars()
        .map(|ch| match ch {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            c if (c as u32) < 0x20 => format!("\\u{:04x}", c as u32),
            c => c.to_string(),
        })
        .collect()
}

proptest! {
    /// Any `(Int, Bool, Str)` triple, encoded as a strict `brix.input@1`
    /// envelope matching `DECLARED_INPUTS_SOURCE`'s declarations, must decode,
    /// canonicalize, and validate successfully — and building/running the
    /// runtime over it must not panic (the program is a single
    /// always-`true`-guarded proposal, so it always selects `Yes`).
    #[test]
    fn valid_generated_input_for_declared_schema_decodes(
        a in any::<i64>(),
        b in any::<bool>(),
        c in "[ -~]{0,64}", // printable ASCII, excludes '"' only via replace below
    ) {
        let c = c.replace('"', "'"); // keep the fixture trivially escaping-free
        let json = format!(
            r#"{{"schema":"brix.input@1","values":{{"a":{{"type":"int","value":"{a}"}},"b":{{"type":"bool","value":{b}}},"c":{{"type":"string","value":"{}"}}}}}}"#,
            json_escape_ascii_only(&c)
        );

        let limits = InputLimits::default();
        let shard = decode_input_shard(json.as_bytes(), &limits)
            .unwrap_or_else(|e| panic!("generated valid shard must decode: {e} (json={json})"));
        let snapshot = canonicalize_input_shards(vec![shard], &limits)
            .expect("generated valid shard must canonicalize");

        let module = parse(DECLARED_INPUTS_SOURCE).expect("fixture parses");
        let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
            .expect("fixture lowers");

        prop_assert!(validate_against_declarations(&snapshot, &plan).is_ok());
        prop_assert!(validate_completeness(&snapshot, &plan).is_ok());

        let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
            .expect("runtime builds over a complete, declaration-matching snapshot");
        let run = runtime.run();
        prop_assert!(run.is_selected(), "the fixture program always selects 'Yes'");
    }
}

// ---------------------------------------------------------------------------
// `brix.input@3` list values (ADR-0037): valid generated lists decode.
// ---------------------------------------------------------------------------

const LIST_DECLARED_INPUTS_SOURCE: &str = r#"
input xs: List<Int> max 16
config D = Yes
propose p() priority 1 when true = Yes
commit outcome from (p)
"#;

proptest! {
    /// Any `Vec<i64>` of length 0..=16, encoded as a strict `brix.input@3`
    /// list value matching `LIST_DECLARED_INPUTS_SOURCE`'s declared
    /// `List<Int> max 16`, must decode, canonicalize, and validate
    /// successfully — and building/running the runtime over it must not
    /// panic (the program is a single always-`true`-guarded proposal, so it
    /// always selects `Yes`, regardless of `xs`'s length or contents).
    #[test]
    fn valid_generated_list_input_for_declared_schema_decodes(
        values in proptest::collection::vec(any::<i64>(), 0..=16),
    ) {
        let items = values
            .iter()
            .map(|n| format!(r#"{{"type":"int","value":"{n}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"schema":"brix.input@3","values":{{"xs":{{"type":"list","items":[{items}]}}}}}}"#
        );

        let limits = InputLimits::default();
        let shard = decode_input_shard(json.as_bytes(), &limits)
            .unwrap_or_else(|e| panic!("generated valid @3 list shard must decode: {e} (json={json})"));
        let snapshot = canonicalize_input_shards(vec![shard], &limits)
            .expect("generated valid @3 list shard must canonicalize");

        let module = parse(LIST_DECLARED_INPUTS_SOURCE).expect("fixture parses");
        let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
            .expect("fixture lowers");

        prop_assert!(validate_against_declarations(&snapshot, &plan).is_ok());
        prop_assert!(validate_completeness(&snapshot, &plan).is_ok());

        let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
            .expect("runtime builds over a complete, declaration-matching snapshot");
        let run = runtime.run();
        prop_assert!(run.is_selected(), "the fixture program always selects 'Yes'");
    }

    /// A list value nested inside a `sum`'s argument is refused on `@3` at
    /// any depth other than the top level (ADR-0037): decoding must return a
    /// typed error, never panic, and never silently accept it.
    #[test]
    fn nested_list_inside_sum_is_refused_on_v3_never_panics(
        values in proptest::collection::vec(any::<i64>(), 0..=4),
    ) {
        let items = values
            .iter()
            .map(|n| format!(r#"{{"type":"int","value":"{n}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"schema":"brix.input@3","values":{{"x":{{"type":"sum","nominal":"Wrap","variant":"Some","args":[{{"type":"list","items":[{items}]}}]}}}}}}"#
        );
        let limits = InputLimits::default();
        let result = decode_input_shard(json.as_bytes(), &limits);
        prop_assert!(result.is_err(), "a list nested inside a sum's args must be refused on @3");
    }
}

// ---------------------------------------------------------------------------
// Snapshot id is independent of shard order.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum SmallValue {
    Int(i64),
    Bool(bool),
    Str(String),
}

fn small_value_strategy() -> impl Strategy<Value = SmallValue> {
    prop_oneof![
        any::<i64>().prop_map(SmallValue::Int),
        any::<bool>().prop_map(SmallValue::Bool),
        "[a-zA-Z0-9 ]{0,16}".prop_map(SmallValue::Str),
    ]
}

fn value_json(v: &SmallValue) -> String {
    match v {
        SmallValue::Int(n) => format!(r#"{{"type":"int","value":"{n}"}}"#),
        SmallValue::Bool(b) => format!(r#"{{"type":"bool","value":{b}}}"#),
        SmallValue::Str(s) => format!(r#"{{"type":"string","value":"{s}"}}"#),
    }
}

/// Build a single-shard JSON envelope for `entries` (already-unique names).
fn shard_json(entries: &[(String, SmallValue)]) -> String {
    let body = entries
        .iter()
        .map(|(name, v)| format!("\"{name}\":{}", value_json(v)))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"schema":"brix.input@1","values":{{{body}}}}}"#)
}

/// A tiny deterministic LCG step, used only to assign N items to K buckets in
/// two independent, reproducible ways — proptest's own generation already
/// covers the interesting distributions; this just needs *some* cheap,
/// panic-free way to split a list into shards.
fn lcg_next(state: &mut u64, modulus: usize) -> usize {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    ((*state >> 33) as usize) % modulus.max(1)
}

fn assign_shards(
    entries: &[(String, SmallValue)],
    k: usize,
    state: &mut u64,
) -> Vec<Vec<(String, SmallValue)>> {
    let mut shards: Vec<Vec<(String, SmallValue)>> = vec![Vec::new(); k];
    for e in entries {
        let idx = lcg_next(state, k);
        shards[idx].push(e.clone());
    }
    shards
}

proptest! {
    /// A random set of distinct scalar inputs, split into a random number of
    /// disjoint shards and canonicalized in two different orders (and with
    /// entries assigned to different shards on each side), must produce the
    /// same snapshot id and the same preimage either way (ADR-0031
    /// ⟨D-IDENTITY⟩): shard order — and which disjoint shard a given input
    /// happened to arrive in — must never be observable in the result.
    #[test]
    fn snapshot_id_is_independent_of_shard_order_and_split(
        raw_names in proptest::collection::vec("[a-z][a-z0-9_]{0,12}", 1..20),
        values in proptest::collection::vec(small_value_strategy(), 1..20),
        n_shards_a in 1usize..6,
        n_shards_b in 1usize..6,
        seed in any::<u64>(),
    ) {
        // De-duplicate names with a `BTreeSet` (never `HashSet` — Ring0 §0 /
        // clippy `disallowed_types`), preserving first-seen order.
        let mut seen = std::collections::BTreeSet::new();
        let mut names: Vec<String> = Vec::new();
        for n in raw_names {
            if seen.insert(n.clone()) {
                names.push(n);
            }
        }
        let n = names.len();
        prop_assume!(n >= 1);
        let entries: Vec<(String, SmallValue)> = names
            .into_iter()
            .zip(values.into_iter().cycle())
            .take(n)
            .collect();

        let mut state = seed | 1;
        let shards_a = assign_shards(&entries, n_shards_a, &mut state);
        let shards_b = assign_shards(&entries, n_shards_b, &mut state);

        let limits = InputLimits::default();
        let decode_all = |shards: &[Vec<(String, SmallValue)>]| {
            shards
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let json = shard_json(s);
                    decode_input_shard(json.as_bytes(), &limits)
                        .unwrap_or_else(|e| panic!("generated shard must decode: {e}"))
                })
                .collect::<Vec<_>>()
        };

        let decoded_a = decode_all(&shards_a);
        let decoded_b = decode_all(&shards_b);

        let snap_a = canonicalize_input_shards(decoded_a, &limits)
            .expect("disjoint-by-construction shards must canonicalize");
        let snap_b = canonicalize_input_shards(decoded_b, &limits)
            .expect("disjoint-by-construction shards must canonicalize");

        prop_assert_eq!(snap_a.id(), snap_b.id(), "snapshot id must not depend on shard split/order");
        prop_assert_eq!(snap_a.preimage(), snap_b.preimage(), "preimage must not depend on shard split/order");
        prop_assert_eq!(snap_a.len(), n);
    }
}
