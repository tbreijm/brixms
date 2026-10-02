//! Encoding an [`InputSnapshot`] back to a strict `brix.input@2`/`@3`/`@4` JSON file.
//!
//! `brix-lower::input` only ever *decodes* `brix.input@1`/`@2`/`@3` artifacts
//! (they arrive from outside the toolchain, via `--input`); nothing before
//! this crate ever needed to write one back out. A persistent knowledge base
//! does: every revision's exact input snapshot is stored on disk,
//! content-addressed by its `InputSnapshotId`, so a later `brix verify`/
//! `brix kb verify` can recompute that same identity from the stored bytes.
//!
//! The encoder writes schema `brix.input@2` for a snapshot with no list value
//! (never `@1`, since `@2` is a strict superset — it additionally admits
//! `sum`/`record` values, and the existing decoder reads either schema back
//! into the same [`InputValue`] representation, so there is no compatibility
//! reason to prefer `@1` even when every value happens to be scalar), and
//! `brix.input@3` (ADR-0037) the moment any value — including one nested
//! inside a `sum`'s `args` or a `record`'s `fields`, which `@3` still refuses
//! at any depth other than the top level — is a list, since `@1`/`@2` refuse
//! list values outright. Numeric values anywhere in a snapshot require `@4`.

use brix_lower::input::{
    InputSnapshot, InputValue, INPUT_SCHEMA_V2, INPUT_SCHEMA_V3, INPUT_SCHEMA_V4,
};
use serde_json::{json, Map, Value as Json};

fn encode_value(v: &InputValue) -> Json {
    match v {
        InputValue::F64(n) => json!({"type": "f64", "value": n.to_string()}),
        InputValue::Decimal(n) => {
            json!({"type": "decimal", "value": brix_canon::decimal_format(*n)})
        }
        InputValue::Int(n) => json!({"type": "int", "value": n.to_string()}),
        InputValue::Bool(b) => json!({"type": "bool", "value": b}),
        InputValue::Str(s) => json!({"type": "string", "value": s}),
        InputValue::Sum {
            nominal,
            variant,
            args,
        } => json!({
            "type": "sum",
            "nominal": nominal,
            "variant": variant,
            "args": args.iter().map(encode_value).collect::<Vec<_>>(),
        }),
        InputValue::Record { nominal, fields } => json!({
            "type": "record",
            "nominal": nominal,
            "fields": fields
                .iter()
                .map(|(name, val)| json!({"name": name, "value": encode_value(val)}))
                .collect::<Vec<_>>(),
        }),
        InputValue::List(items) => json!({
            "type": "list",
            "items": items.iter().map(encode_value).collect::<Vec<_>>(),
        }),
    }
}

/// Whether `v`, or anything nested within it, is a list (ADR-0037) — the
/// exact same recursive shape `brix-lower::input`'s own decoder-side check
/// uses to decide whether `@3` is required.
fn value_has_list(v: &InputValue) -> bool {
    match v {
        InputValue::List(_) => true,
        InputValue::Sum { args, .. } => args.iter().any(value_has_list),
        InputValue::Record { fields, .. } => fields.values().any(value_has_list),
        InputValue::Int(_)
        | InputValue::Bool(_)
        | InputValue::Str(_)
        | InputValue::F64(_)
        | InputValue::Decimal(_) => false,
    }
}

fn value_has_numeric(v: &InputValue) -> bool {
    match v {
        InputValue::F64(_) | InputValue::Decimal(_) => true,
        InputValue::Sum { args, .. } | InputValue::List(args) => args.iter().any(value_has_numeric),
        InputValue::Record { fields, .. } => fields.values().any(value_has_numeric),
        InputValue::Int(_) | InputValue::Bool(_) | InputValue::Str(_) => false,
    }
}

/// Encode `snapshot` as a compact strict `brix.input@2` (or `@3`, the moment
/// any value is a list — ADR-0037) JSON document. Compact, because a stored
/// snapshot must fit the knowledge base's byte limit and indentation would
/// spend it on whitespace.
pub fn encode_input_snapshot_v2(snapshot: &InputSnapshot) -> String {
    let mut values = Map::new();
    let mut needs_v3 = false;
    let mut needs_v4 = false;
    for (name, value) in snapshot.values() {
        needs_v3 |= value_has_list(value);
        needs_v4 |= value_has_numeric(value);
        values.insert(name.clone(), encode_value(value));
    }
    let schema = if needs_v4 {
        INPUT_SCHEMA_V4
    } else if needs_v3 {
        INPUT_SCHEMA_V3
    } else {
        INPUT_SCHEMA_V2
    };
    let doc = json!({
        "schema": schema,
        "values": Json::Object(values),
    });
    serde_json::to_string(&doc).expect("snapshot JSON encoding cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_lower::input::{canonicalize_input_shards, decode_input_shard, InputLimits};

    fn roundtrip(snapshot: &InputSnapshot) {
        let encoded = encode_input_snapshot_v2(snapshot);
        let limits = InputLimits::default();
        let shard = decode_input_shard(encoded.as_bytes(), &limits).expect("decode roundtrip");
        let decoded = canonicalize_input_shards(vec![shard], &limits).expect("canonicalize");
        assert_eq!(decoded.id(), snapshot.id(), "snapshot id must round-trip");
        assert_eq!(decoded.values(), snapshot.values());
    }

    #[test]
    fn numeric_snapshots_write_v4_and_preserve_canonical_identity() {
        let limits = InputLimits::default();
        for value in [
            json!({"type":"f64", "value":"-0.0"}),
            json!({"type":"decimal", "value":"9007199254740993.123456789012345678"}),
            json!({"type":"list", "items":[{"type":"decimal", "value":"0.10"}]}),
            json!({"type":"record", "nominal":"Price", "fields":[{"name":"amount", "value":{"type":"decimal", "value":"12.50"}}]}),
            json!({"type":"sum", "nominal":"Measure", "variant":"Reading", "args":[{"type":"f64", "value":"1.5"}]}),
        ] {
            let bytes = serde_json::to_vec(&json!({"schema":"brix.input@4", "values":{"x":value}}))
                .unwrap();
            let shard = decode_input_shard(&bytes, &limits).unwrap();
            let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();
            let encoded = encode_input_snapshot_v2(&snapshot);
            assert!(encoded.contains("brix.input@4"));
            roundtrip(&snapshot);
        }
    }

    #[test]
    fn test_roundtrip_scalars() {
        let shard_bytes = {
            let mut values_obj = Map::new();
            values_obj.insert("stock".into(), json!({"type":"int","value":"12"}));
            values_obj.insert("eligible".into(), json!({"type":"bool","value":true}));
            values_obj.insert("region".into(), json!({"type":"string","value":"EU-NORTH"}));
            serde_json::to_vec(&json!({"schema":"brix.input@1","values":values_obj})).unwrap()
        };
        let limits = InputLimits::default();
        let shard = decode_input_shard(&shard_bytes, &limits).unwrap();
        let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();
        roundtrip(&snapshot);
    }

    #[test]
    fn test_roundtrip_negative_int() {
        let shard_bytes = serde_json::to_vec(
            &json!({"schema":"brix.input@1","values":{"x":{"type":"int","value":"-42"}}}),
        )
        .unwrap();
        let limits = InputLimits::default();
        let shard = decode_input_shard(&shard_bytes, &limits).unwrap();
        let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();
        roundtrip(&snapshot);
    }

    #[test]
    fn test_roundtrip_sum_and_record() {
        let shard_bytes = serde_json::to_vec(&json!({
            "schema": "brix.input@2",
            "values": {
                "dest": {
                    "type": "sum",
                    "nominal": "Destination",
                    "variant": "Export",
                    "args": [{"type": "string", "value": "DE"}]
                },
                "order": {
                    "type": "record",
                    "nominal": "Order",
                    "fields": [
                        {"name": "units", "value": {"type": "int", "value": "10"}},
                        {"name": "destination", "value": {"type": "sum", "nominal": "Destination", "variant": "Domestic", "args": []}}
                    ]
                }
            }
        }))
        .unwrap();
        let limits = InputLimits::default();
        let shard = decode_input_shard(&shard_bytes, &limits).unwrap();
        let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();
        roundtrip(&snapshot);
    }

    #[test]
    fn test_roundtrip_empty_snapshot() {
        roundtrip(&InputSnapshot::empty());
    }

    #[test]
    fn test_roundtrip_list_writes_schema_v3() {
        let shard_bytes = serde_json::to_vec(&json!({
            "schema": "brix.input@3",
            "values": {
                "orders": {
                    "type": "list",
                    "items": [
                        {"type": "int", "value": "1"},
                        {"type": "int", "value": "2"},
                        {"type": "int", "value": "3"}
                    ]
                }
            }
        }))
        .unwrap();
        let limits = InputLimits::default();
        let shard = decode_input_shard(&shard_bytes, &limits).unwrap();
        let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();

        let encoded = encode_input_snapshot_v2(&snapshot);
        assert!(
            encoded.contains("brix.input@3"),
            "a snapshot holding a list must be written back as @3: {encoded}"
        );
        roundtrip(&snapshot);
    }

    #[test]
    fn test_scalar_only_snapshot_still_writes_schema_v2() {
        let shard_bytes = serde_json::to_vec(
            &json!({"schema":"brix.input@1","values":{"x":{"type":"int","value":"1"}}}),
        )
        .unwrap();
        let limits = InputLimits::default();
        let shard = decode_input_shard(&shard_bytes, &limits).unwrap();
        let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();

        let encoded = encode_input_snapshot_v2(&snapshot);
        assert!(
            encoded.contains("brix.input@2") && !encoded.contains("brix.input@3"),
            "a snapshot with no list value should keep writing @2: {encoded}"
        );
    }
}
