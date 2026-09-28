//! Encoding an [`InputSnapshot`] back to a strict `brix.input@2` JSON file.
//!
//! `brix-lower::input` only ever *decodes* `brix.input@1`/`@2` artifacts (they
//! arrive from outside the toolchain, via `--input`); nothing before this
//! crate ever needed to write one back out. A persistent knowledge base does:
//! every revision's exact input snapshot is stored on disk, content-addressed
//! by its `InputSnapshotId`, so a later `brix verify`/`brix kb verify` can
//! recompute that same identity from the stored bytes.
//!
//! The encoder always writes schema `brix.input@2` (never `@1`), because `@2`
//! is a strict superset (it additionally admits `sum`/`record` values) and the
//! existing decoder reads either schema back into the same [`InputValue`]
//! representation, so there is no compatibility reason to prefer `@1` even
//! when every value in a given snapshot happens to be scalar.

use brix_lower::input::{InputSnapshot, InputValue, INPUT_SCHEMA_V2};
use serde_json::{json, Map, Value as Json};

fn encode_value(v: &InputValue) -> Json {
    match v {
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
    }
}

/// Encode `snapshot` as a pretty-printed strict `brix.input@2` JSON document.
pub fn encode_input_snapshot_v2(snapshot: &InputSnapshot) -> String {
    let mut values = Map::new();
    for (name, value) in snapshot.values() {
        values.insert(name.clone(), encode_value(value));
    }
    let doc = json!({
        "schema": INPUT_SCHEMA_V2,
        "values": Json::Object(values),
    });
    serde_json::to_string_pretty(&doc).expect("snapshot JSON encoding cannot fail")
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
}
