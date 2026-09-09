//! Canonical JSON output format for the Brix Alpha CLI (`brix.cli.result@1`).

use brix_lower::l3_v2::L3ValueV2;
use serde::{Deserialize, Serialize};

/// The canonical top-level CLI JSON result schema.
pub const BRIX_CLI_SCHEMA: &str = "brix.cli.result@1";

/// Top-level result object for all JSON-enabled CLI invocations.
///
/// Contains the exact required fields starting with schema:
/// `schema`, `command`, `ok`, `profile`, `program`, `context`, `status`, `facts`, `candidates`,
/// `decision`, `artifacts`, `diagnostics`.
/// For input-bearing executions/checks, the optional additive fields `input_snapshot` and `inputs`
/// are populated under schema `brix.cli.result@1`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CliResultJson {
    pub schema: String,
    pub command: String,
    pub ok: bool,
    pub profile: Option<String>,
    pub program: Option<String>,
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_snapshot: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<InputJson>>,
    pub facts: Vec<FactJson>,
    pub candidates: Vec<CandidateJson>,
    pub decision: Option<DecisionJson>,
    pub artifacts: Vec<ArtifactJson>,
    pub diagnostics: Vec<String>,
}

impl CliResultJson {
    /// Construct a successful result object with all required fields.
    #[allow(clippy::too_many_arguments)]
    pub fn success(
        command: impl Into<String>,
        profile: Option<String>,
        program: Option<String>,
        context: Option<String>,
        status: impl Into<String>,
        facts: Vec<FactJson>,
        candidates: Vec<CandidateJson>,
        decision: Option<DecisionJson>,
        artifacts: Vec<ArtifactJson>,
        diagnostics: Vec<String>,
    ) -> Self {
        Self {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: command.into(),
            ok: true,
            profile,
            program,
            context,
            input_snapshot: None,
            status: status.into(),
            inputs: None,
            facts,
            candidates,
            decision,
            artifacts,
            diagnostics,
        }
    }

    /// Construct a failure/rejection/unknown result object.
    pub fn failure(
        command: impl Into<String>,
        profile: Option<String>,
        program: Option<String>,
        context: Option<String>,
        status: impl Into<String>,
        diagnostics: Vec<String>,
    ) -> Self {
        Self {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: command.into(),
            ok: false,
            profile,
            program,
            context,
            input_snapshot: None,
            status: status.into(),
            inputs: None,
            facts: Vec::new(),
            candidates: Vec::new(),
            decision: None,
            artifacts: Vec::new(),
            diagnostics,
        }
    }

    /// Explicitly attach input snapshot and inputs collection.
    pub fn with_inputs(
        mut self,
        input_snapshot: Option<String>,
        inputs: Option<Vec<InputJson>>,
    ) -> Self {
        self.input_snapshot = input_snapshot;
        self.inputs = inputs;
        self
    }

    /// Explicitly attach the versioned schema identifier.
    pub fn with_schema(mut self) -> Self {
        self.schema = BRIX_CLI_SCHEMA.to_string();
        self
    }
}

/// An admitted external input record carrying name, tagged value, ordinal string, and grade.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InputJson {
    pub name: String,
    pub value: TaggedValue,
    pub ordinal: String,
    pub grade: String,
}

impl InputJson {
    pub fn new(
        name: impl Into<String>,
        value: TaggedValue,
        ordinal: impl ToString,
        grade: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value,
            ordinal: ordinal.to_string(),
            grade: grade.into(),
        }
    }
}

/// A structured artifact produced or verified by a CLI command.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ArtifactJson {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub bundle_id: String,
    pub final_chain_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ids: Option<Vec<String>>,
    pub count: String,
}

impl ArtifactJson {
    pub fn bundle(
        path: Option<impl Into<String>>,
        bundle_id: impl Into<String>,
        final_chain_digest: impl Into<String>,
        receipt_ids: Option<Vec<String>>,
        count: impl ToString,
    ) -> Self {
        Self {
            kind: "audit-bundle".to_string(),
            path: path.map(Into::into),
            bundle_id: bundle_id.into(),
            final_chain_digest: final_chain_digest.into(),
            receipt_ids,
            count: count.to_string(),
        }
    }
}

/// A derived semantic fact.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FactJson {
    pub name: String,
    pub value: TaggedValue,
    pub ordinal: String,
    pub grade: String,
}

impl FactJson {
    pub fn new(
        name: impl Into<String>,
        value: TaggedValue,
        ordinal: impl ToString,
        grade: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value,
            ordinal: ordinal.to_string(),
            grade: grade.into(),
        }
    }
}

/// A candidate proposal record carrying name, priority string, status, and structured reason.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CandidateJson {
    pub name: String,
    pub priority: String,
    pub status: String,
    pub reason: StructuredReasonJson,
}

impl CandidateJson {
    pub fn new(
        name: impl Into<String>,
        priority: impl ToString,
        status: impl Into<String>,
        reason: StructuredReasonJson,
    ) -> Self {
        Self {
            name: name.into(),
            priority: priority.to_string(),
            status: status.into(),
            reason,
        }
    }
}

/// Structured explanation reason for a candidate proposal.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StructuredReasonJson {
    pub code: String,
    pub detail: String,
}

impl StructuredReasonJson {
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }
}

/// The winning decision selected by deliberation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DecisionJson {
    pub candidate: String,
    pub priority: String,
    pub value: TaggedValue,
    pub grade: String,
}

impl DecisionJson {
    pub fn new(
        candidate: impl Into<String>,
        priority: impl ToString,
        value: TaggedValue,
        grade: impl Into<String>,
    ) -> Self {
        Self {
            candidate: candidate.into(),
            priority: priority.to_string(),
            value,
            grade: grade.into(),
        }
    }
}

/// Tagged object representing an evaluated Brix value.
///
/// Single discriminator named `type`.
/// Supports `int` (with decimal string value), `bool`, `string`, `sum` (with nominal/variant/args),
/// and `record` (with nominal/fields).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaggedValue {
    Int {
        value: String,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
    },
    Sum {
        nominal: String,
        variant: String,
        args: Vec<TaggedValue>,
    },
    Record {
        nominal: String,
        fields: Vec<RecordFieldJson>,
    },
}

impl TaggedValue {
    pub fn int(n: impl ToString) -> Self {
        Self::Int {
            value: n.to_string(),
        }
    }

    pub fn bool(b: bool) -> Self {
        Self::Bool { value: b }
    }

    pub fn string(s: impl Into<String>) -> Self {
        Self::String { value: s.into() }
    }

    pub fn sum(
        nominal: impl Into<String>,
        variant: impl Into<String>,
        args: Vec<TaggedValue>,
    ) -> Self {
        Self::Sum {
            nominal: nominal.into(),
            variant: variant.into(),
            args,
        }
    }

    pub fn record(
        nominal: impl Into<String>,
        fields: Vec<(impl Into<String>, TaggedValue)>,
    ) -> Self {
        Self::Record {
            nominal: nominal.into(),
            fields: fields
                .into_iter()
                .map(|(k, v)| RecordFieldJson {
                    name: k.into(),
                    value: v,
                })
                .collect(),
        }
    }
}

/// A field in a record value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RecordFieldJson {
    pub name: String,
    pub value: TaggedValue,
}

/// Convert an internal [`L3ValueV2`] into a [`TaggedValue`].
///
/// Integers are formatted as decimal strings.
pub fn to_tagged_value(v: &L3ValueV2) -> TaggedValue {
    match v {
        L3ValueV2::Int(n) => TaggedValue::Int {
            value: n.to_string(),
        },
        L3ValueV2::Bool(b) => TaggedValue::Bool { value: *b },
        L3ValueV2::Str(s) => TaggedValue::String { value: s.clone() },
        L3ValueV2::Ctor {
            nominal_sum,
            variant,
            args,
        } => TaggedValue::Sum {
            nominal: nominal_sum.clone(),
            variant: variant.clone(),
            args: args.iter().map(to_tagged_value).collect(),
        },
        L3ValueV2::Record {
            nominal_config,
            fields,
        } => TaggedValue::Record {
            nominal: nominal_config.clone(),
            fields: fields
                .iter()
                .map(|(k, val)| RecordFieldJson {
                    name: k.clone(),
                    value: to_tagged_value(val),
                })
                .collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_json_fields_serialization() {
        let res = CliResultJson::success(
            "check",
            Some("brix.l3.finite-decision@1".to_string()),
            Some("1234abcd".to_string()),
            Some("5678ef01".to_string()),
            "accepted",
            vec![FactJson::new("rule_1", TaggedValue::int(42), 0, "Derived")],
            vec![CandidateJson::new(
                "c1",
                0,
                "selected",
                StructuredReasonJson::new("selected", "minimal key"),
            )],
            Some(DecisionJson::new("c1", 0, TaggedValue::int(42), "Derived")),
            vec![ArtifactJson::bundle(
                Some("bundle.bin"),
                "bundle_id_123",
                "final_chain_456",
                Some(vec!["rec_1".to_string()]),
                1,
            )],
            vec!["diagnostic_info".to_string()],
        );

        let json_str = serde_json::to_string(&res).expect("serialization failed");
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("json parse failed");
        let obj = val.as_object().expect("expected json object");

        // Exactly the 12 top-level fields required:
        let expected_fields = [
            "schema",
            "command",
            "ok",
            "profile",
            "program",
            "context",
            "status",
            "facts",
            "candidates",
            "decision",
            "artifacts",
            "diagnostics",
        ];

        assert_eq!(
            obj.len(),
            12,
            "expected exactly 12 top-level fields, found: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        for f in &expected_fields {
            assert!(
                obj.contains_key(*f),
                "missing expected top-level field: {f}"
            );
        }

        // Verify values
        assert_eq!(obj["schema"], "brix.cli.result@1");
        assert_eq!(obj["command"], "check");
        assert_eq!(obj["ok"], true);
        assert_eq!(obj["profile"], "brix.l3.finite-decision@1");
        assert_eq!(obj["program"], "1234abcd");
        assert_eq!(obj["context"], "5678ef01");
        assert_eq!(obj["status"], "accepted");
        assert_eq!(obj["facts"].as_array().unwrap().len(), 1);
        assert_eq!(obj["candidates"].as_array().unwrap().len(), 1);
        assert!(obj["decision"].is_object());
        assert_eq!(obj["artifacts"].as_array().unwrap().len(), 1);
        let art = &obj["artifacts"].as_array().unwrap()[0];
        assert_eq!(art["kind"], "audit-bundle");
        assert_eq!(art["path"], "bundle.bin");
        assert_eq!(art["bundle_id"], "bundle_id_123");
        assert_eq!(art["final_chain_digest"], "final_chain_456");
        assert_eq!(art["count"], "1");
        assert_eq!(obj["diagnostics"].as_array().unwrap()[0], "diagnostic_info");

        // Deserialization roundtrip
        let roundtrip: CliResultJson =
            serde_json::from_str(&json_str).expect("deserialization failed");
        assert_eq!(roundtrip.schema, "brix.cli.result@1");
        assert_eq!(roundtrip.command, "check");
        assert!(roundtrip.ok);
        assert_eq!(roundtrip.status, "accepted");
        assert_eq!(roundtrip.facts.len(), 1);
        assert_eq!(roundtrip.candidates.len(), 1);
        assert_eq!(roundtrip.artifacts.len(), 1);
        assert_eq!(roundtrip.diagnostics.len(), 1);
    }

    #[test]
    fn test_failure_result_exact_twelve_fields() {
        let res = CliResultJson::failure(
            "run",
            Some("brix.l3.finite-decision@1".to_string()),
            None,
            None,
            "rejected",
            vec!["lowering failed".to_string()],
        );

        let json_str = serde_json::to_string(&res).expect("serialization failed");
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("json parse failed");
        let obj = val.as_object().expect("expected json object");

        assert_eq!(obj.len(), 12);
        assert_eq!(obj["schema"], "brix.cli.result@1");
        assert_eq!(obj["command"], "run");
        assert_eq!(obj["ok"], false);
        assert_eq!(obj["status"], "rejected");
        assert_eq!(obj["facts"].as_array().unwrap().len(), 0);
        assert_eq!(obj["candidates"].as_array().unwrap().len(), 0);
        assert!(obj["decision"].is_null());
        assert_eq!(obj["artifacts"].as_array().unwrap().len(), 0);
        assert_eq!(obj["diagnostics"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_tagged_value_int_decimal_string() {
        let positive = TaggedValue::int(9223372036854775807i64);
        let negative = TaggedValue::int(-100);
        let zero = TaggedValue::int(0);

        let json_pos = serde_json::to_string(&positive).unwrap();
        assert!(
            json_pos.contains(r#""value":"9223372036854775807""#),
            "expected decimal string in json, got {json_pos}"
        );
        let json_neg = serde_json::to_string(&negative).unwrap();
        assert!(
            json_neg.contains(r#""value":"-100""#),
            "expected negative decimal string in json, got {json_neg}"
        );
        let json_zero = serde_json::to_string(&zero).unwrap();
        assert!(
            json_zero.contains(r#""value":"0""#),
            "expected zero decimal string in json, got {json_zero}"
        );

        // Verify roundtrip deserialization
        let de_pos: TaggedValue = serde_json::from_str(&json_pos).unwrap();
        assert_eq!(de_pos, positive);
    }

    #[test]
    fn test_semantic_integers_in_candidate_fact_and_decision() {
        let fact = FactJson::new("rule_fact", TaggedValue::int(123), 5, "Derived");
        assert_eq!(fact.ordinal, "5");
        let fact_json = serde_json::to_string(&fact).unwrap();
        assert!(fact_json.contains(r#""ordinal":"5""#));

        let candidate = CandidateJson::new(
            "cand_a",
            10,
            "admitted-not-selected",
            StructuredReasonJson::new("overshadowed", "overshadowed by winner"),
        );
        assert_eq!(candidate.priority, "10");
        let cand_json = serde_json::to_string(&candidate).unwrap();
        assert!(cand_json.contains(r#""priority":"10""#));

        let decision = DecisionJson::new("cand_a", 0, TaggedValue::bool(true), "Derived");
        assert_eq!(decision.priority, "0");
        let dec_json = serde_json::to_string(&decision).unwrap();
        assert!(dec_json.contains(r#""priority":"0""#));
    }

    #[test]
    fn test_tagged_values_all_variants() {
        let b = TaggedValue::bool(true);
        let s = TaggedValue::string("hello world");
        let sum_val = TaggedValue::sum("Outcome", "Audited", vec![]);
        let rec_val = TaggedValue::record(
            "Config",
            vec![
                ("count", TaggedValue::int(42)),
                ("active", TaggedValue::bool(false)),
            ],
        );

        for val in [&b, &s, &sum_val, &rec_val] {
            let json = serde_json::to_string(val).unwrap();
            assert!(!json.contains(r#""tag":"#));
            assert!(!json.contains(r#""type_name":"#));
            assert!(!json.contains(r#""nominal_sum":"#));
            assert!(!json.contains(r#""nominal_config":"#));
            let roundtrip: TaggedValue = serde_json::from_str(&json).unwrap();
            assert_eq!(&roundtrip, val);
        }
    }

    #[test]
    fn test_to_tagged_value_conversion() {
        let v_int = L3ValueV2::Int(42);
        let tv_int = to_tagged_value(&v_int);
        assert_eq!(tv_int, TaggedValue::int("42"));

        let v_bool = L3ValueV2::Bool(false);
        let tv_bool = to_tagged_value(&v_bool);
        assert_eq!(tv_bool, TaggedValue::bool(false));

        let v_str = L3ValueV2::Str("abc".to_string());
        let tv_str = to_tagged_value(&v_str);
        assert_eq!(tv_str, TaggedValue::string("abc"));
    }

    #[test]
    fn test_schema_identifier_constant() {
        assert_eq!(BRIX_CLI_SCHEMA, "brix.cli.result@1");

        // If a json string includes "schema": "brix.cli.result@1", it deserializes cleanly
        let raw = r#"{
            "schema": "brix.cli.result@1",
            "command": "check",
            "ok": true,
            "profile": null,
            "program": null,
            "context": null,
            "status": "accepted",
            "facts": [],
            "candidates": [],
            "decision": null,
            "artifacts": [],
            "diagnostics": []
        }"#;
        let res: CliResultJson = serde_json::from_str(raw).unwrap();
        assert_eq!(res.schema, "brix.cli.result@1");
        assert_eq!(res.command, "check");
    }

    #[test]
    fn test_input_json_serialization_and_additive_fields() {
        let input_entry = InputJson::new("limit", TaggedValue::int(100), 0, "Derived");
        assert_eq!(input_entry.name, "limit");
        assert_eq!(input_entry.ordinal, "0");
        assert_eq!(input_entry.grade, "Derived");

        let res_with_inputs = CliResultJson::success(
            "run",
            Some("brix.l3.finite-decision@1".to_string()),
            Some("1234abcd".to_string()),
            Some("5678ef01".to_string()),
            "selected",
            vec![],
            vec![],
            None,
            vec![],
            vec![],
        )
        .with_inputs(
            Some("snap_id_999".to_string()),
            Some(vec![input_entry.clone()]),
        );

        let json_str = serde_json::to_string(&res_with_inputs).expect("serialization failed");
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("json parse failed");
        let obj = val.as_object().expect("expected json object");

        // Exactly 14 fields when input_snapshot and inputs are present
        assert_eq!(
            obj.len(),
            14,
            "expected exactly 14 top-level fields with inputs, found: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert_eq!(obj["input_snapshot"], "snap_id_999");
        let inputs_arr = obj["inputs"].as_array().expect("inputs is array");
        assert_eq!(inputs_arr.len(), 1);
        assert_eq!(inputs_arr[0]["name"], "limit");
        assert_eq!(inputs_arr[0]["ordinal"], "0");
        assert_eq!(inputs_arr[0]["grade"], "Derived");
        assert_eq!(inputs_arr[0]["value"]["type"], "int");
        assert_eq!(inputs_arr[0]["value"]["value"], "100");

        // Roundtrip deserialization
        let roundtrip: CliResultJson = serde_json::from_str(&json_str).expect("roundtrip");
        assert_eq!(roundtrip.input_snapshot, Some("snap_id_999".to_string()));
        assert_eq!(roundtrip.inputs, Some(vec![input_entry]));
    }
}
