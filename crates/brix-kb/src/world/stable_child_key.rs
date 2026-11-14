//! Versioned typed ordering keys for bounded grouped collection.
//!
//! These keys are built from the declared typed identities carried by a compact
//! derivation. They intentionally do not use derivation digests or encoded key
//! bytes as sort keys.

use std::cmp::Ordering;

use brix_lower::relation_dag::{OperatorId, OperatorNode, RelationDag};

use super::compact_deriv::CompactDeriv;
use super::decision_codec::Value;
use super::network::DerivationId;
use super::state_codec::StateCodec;

/// Maximum static operator or persisted key nesting accepted for typed collect.
pub const MAX_STABLE_IDENTITY_DEPTH: usize = 128;
const MAX_STABLE_KEY_VALIDATION_NODES: usize = brix_lower::input::MAX_COLLECTION_ELEMENTS;

/// Collect ordering semantics selected by the world's recorded execution profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollectOrdering {
    /// Historical alpha.8 ordering: provenance representation followed by digest.
    #[default]
    LegacyV1,
    /// Typed declared-child-key ordering introduced in alpha.9.
    TypedKeyV1,
}

/// A deterministic typed ordering key derived from a child derivation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StableChildKey {
    /// A declared base relation key, qualified by its source relation.
    Source { relation: String, key: Vec<Value> },
    /// A joined child is ordered by its complete ordered pair of source identities.
    Join {
        left: Box<StableChildKey>,
        right: Box<StableChildKey>,
    },
    /// A distinct or grouped child uses its typed output identity and relation name.
    Derived {
        relation: String,
        shape: DerivedKeyShape,
        key: Vec<(String, Value)>,
    },
}

/// Derived identity layout, used to validate canonical component order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DerivedKeyShape {
    Group,
    Distinct,
}

impl StableChildKey {
    /// Build a child key from the derivation tree and declared relation outputs.
    pub fn from_derivation(
        dag: &RelationDag,
        derivation: &CompactDeriv,
        resolve_base_key: &impl Fn(&str, &super::types::WorldKey) -> Result<Vec<Value>, String>,
    ) -> Result<Self, String> {
        let expanded = derivation
            .expand(dag)
            .map_err(|err| format!("invalid collect child derivation: {err}"))?;
        Self::from_expanded(dag, &expanded, resolve_base_key)
    }

    fn from_expanded(
        dag: &RelationDag,
        derivation: &DerivationId,
        resolve_base_key: &impl Fn(&str, &super::types::WorldKey) -> Result<Vec<Value>, String>,
    ) -> Result<Self, String> {
        match derivation {
            DerivationId::Base { relation, key } => Ok(Self::Source {
                relation: relation.clone(),
                key: resolve_base_key(relation, key)?,
            }),
            DerivationId::Unary { parent, .. } => {
                Self::from_expanded(dag, parent, resolve_base_key)
            }
            DerivationId::Join { left, right, .. } => Ok(Self::Join {
                left: Box::new(Self::from_expanded(dag, left, resolve_base_key)?),
                right: Box::new(Self::from_expanded(dag, right, resolve_base_key)?),
            }),
            DerivationId::Group { op, group_key } => Ok(Self::Derived {
                relation: relation_for_operator(dag, *op)?,
                shape: DerivedKeyShape::Group,
                key: group_key
                    .iter()
                    .enumerate()
                    .map(|(index, value)| (format!("group_{index}"), value.clone()))
                    .collect(),
            }),
            DerivationId::Distinct { op, tuple_key } => {
                let tuple = super::network::IntermediateTuple::state_from_bytes(tuple_key)
                    .map_err(|err| format!("cannot decode distinct child key: {err}"))?;
                Ok(Self::Derived {
                    relation: relation_for_operator(dag, *op)?,
                    shape: DerivedKeyShape::Distinct,
                    key: tuple.fields.into_iter().collect(),
                })
            }
        }
    }

    /// Encode the versioned typed key as a persistable Value.
    pub fn to_value(&self) -> Value {
        match self {
            Self::Source { relation, key } => Value::Record {
                nominal: "world::CollectChildKeyV1".into(),
                fields: std::collections::BTreeMap::from([
                    ("kind".into(), Value::Str("source".into())),
                    ("relation".into(), Value::Str(relation.clone())),
                    ("key".into(), Value::List(key.clone())),
                ]),
            },
            Self::Join { left, right } => Value::Record {
                nominal: "world::CollectChildKeyV1".into(),
                fields: std::collections::BTreeMap::from([
                    ("kind".into(), Value::Str("join".into())),
                    ("left".into(), left.to_value()),
                    ("right".into(), right.to_value()),
                ]),
            },
            Self::Derived {
                relation,
                shape,
                key,
            } => Value::Record {
                nominal: "world::CollectChildKeyV1".into(),
                fields: std::collections::BTreeMap::from([
                    ("kind".into(), Value::Str("derived".into())),
                    ("relation".into(), Value::Str(relation.clone())),
                    (
                        "shape".into(),
                        Value::Str(
                            match shape {
                                DerivedKeyShape::Group => "group",
                                DerivedKeyShape::Distinct => "distinct",
                            }
                            .into(),
                        ),
                    ),
                    (
                        "key".into(),
                        Value::List(
                            key.iter()
                                .map(|(name, value)| Value::Record {
                                    nominal: "world::CollectChildKeyFieldV1".into(),
                                    fields: std::collections::BTreeMap::from([
                                        ("name".into(), Value::Str(name.clone())),
                                        ("value".into(), value.clone()),
                                    ]),
                                })
                                .collect(),
                        ),
                    ),
                ]),
            },
        }
    }
}

fn relation_for_operator(dag: &RelationDag, op: OperatorId) -> Result<String, String> {
    if let Some((name, _)) = dag
        .relation_outputs
        .iter()
        .find(|(_, candidate)| **candidate == op)
    {
        return Ok(name.clone());
    }
    if dag.relation_outputs.len() > dag.nodes.len() {
        return Err("collect relation resolution exceeded operator node budget".into());
    }
    fn unary_ancestor(
        dag: &RelationDag,
        current: OperatorId,
        target: OperatorId,
        depth: usize,
        memo: &mut [Option<(bool, usize)>],
        visiting: &mut [bool],
        work: &mut usize,
    ) -> Result<(bool, usize), String> {
        if depth > MAX_STABLE_IDENTITY_DEPTH {
            return Err(format!(
                "collect relation path exceeds maximum depth {MAX_STABLE_IDENTITY_DEPTH}"
            ));
        }
        if current == target {
            return Ok((true, 1));
        }
        let node = dag
            .nodes
            .get(current.0)
            .ok_or_else(|| format!("unknown relation output operator {}", current.0))?;
        if let Some((found, height)) = memo[current.0] {
            if depth.saturating_add(height.saturating_sub(1)) > MAX_STABLE_IDENTITY_DEPTH {
                return Err(format!(
                    "collect relation path exceeds maximum depth {MAX_STABLE_IDENTITY_DEPTH}"
                ));
            }
            return Ok((found, height));
        }
        if visiting[current.0] {
            return Err("cycle in collect relation output path".into());
        }
        *work = work
            .checked_add(1)
            .ok_or_else(|| "collect relation resolution work overflow".to_string())?;
        if *work > dag.nodes.len() {
            return Err("collect relation resolution exceeded operator node budget".into());
        }
        visiting[current.0] = true;
        let result = match node {
            OperatorNode::Bind { input, .. }
            | OperatorNode::Filter { input, .. }
            | OperatorNode::Project { input, .. }
            | OperatorNode::Distinct { input } => {
                let (found, height) =
                    unary_ancestor(dag, *input, target, depth + 1, memo, visiting, work)?;
                (found, height + 1)
            }
            _ => (false, 1),
        };
        visiting[current.0] = false;
        if depth.saturating_add(result.1.saturating_sub(1)) > MAX_STABLE_IDENTITY_DEPTH {
            return Err(format!(
                "collect relation path exceeds maximum depth {MAX_STABLE_IDENTITY_DEPTH}"
            ));
        }
        memo[current.0] = Some(result);
        Ok(result)
    }

    let mut memo = vec![None; dag.nodes.len()];
    let mut visiting = vec![false; dag.nodes.len()];
    let mut work = 0;
    let mut relation = None;
    for (name, output) in &dag.relation_outputs {
        let (found, _) = unary_ancestor(dag, *output, op, 1, &mut memo, &mut visiting, &mut work)?;
        if found {
            if relation.is_some() {
                return Err(format!(
                    "operator {} has ambiguous qualified output relations",
                    op.0
                ));
            }
            relation = Some(name.clone());
        }
    }
    relation.ok_or_else(|| format!("operator {} has no qualified output relation", op.0))
}

/// Compare values by explicit type tags and natural typed values. Strings use
/// UTF-8 byte order, records use nominal/name order, and lists compare components
/// lexicographically. This avoids making derived enum declaration order a contract.
pub fn compare_typed_values(left: &Value, right: &Value) -> Ordering {
    let left_rank = value_rank(left);
    let right_rank = value_rank(right);
    match left_rank.cmp(&right_rank) {
        Ordering::Equal => match (left, right) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Str(a), Value::Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Value::F64(a), Value::F64(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
            (
                Value::Record {
                    nominal: an,
                    fields: af,
                },
                Value::Record {
                    nominal: bn,
                    fields: bf,
                },
            ) => an.cmp(bn).then_with(|| compare_record_fields(af, bf)),
            (
                Value::Variant {
                    nominal: an,
                    variant: av,
                    args: aa,
                },
                Value::Variant {
                    nominal: bn,
                    variant: bv,
                    args: ba,
                },
            ) => an
                .cmp(bn)
                .then_with(|| av.cmp(bv))
                .then_with(|| compare_value_slices(aa, ba)),
            (Value::List(a), Value::List(b)) => compare_value_slices(a, b),
            _ => Ordering::Equal,
        },
        other => other,
    }
}

/// Compare already validated versioned child keys. Callers must validate keys
/// at persistence/input boundaries before sorting; malformed values compare equal
/// rather than falling back to generic record or hash order.
pub fn compare_stable_keys(left: &Value, right: &Value) -> Ordering {
    fn tagged(value: &Value) -> Option<(&str, &std::collections::BTreeMap<String, Value>)> {
        match value {
            Value::Record { nominal, fields } if nominal == "world::CollectChildKeyV1" => {
                match fields.get("kind") {
                    Some(Value::Str(kind)) => Some((kind, fields)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    if let (Some((left_kind, left_fields)), Some((right_kind, right_fields))) =
        (tagged(left), tagged(right))
    {
        let kind_rank = |kind: &str| match kind {
            "source" => 0,
            "join" => 1,
            "derived" => 2,
            _ => 3,
        };
        let order = kind_rank(left_kind).cmp(&kind_rank(right_kind));
        if order != Ordering::Equal {
            return order;
        }
        return match (left_kind, right_kind) {
            ("source", "source") => compare_named_field(left_fields, right_fields, "key")
                .then_with(|| compare_named_field(left_fields, right_fields, "relation")),
            ("derived", "derived") => compare_named_field(left_fields, right_fields, "shape")
                .then_with(|| compare_named_field(left_fields, right_fields, "key"))
                .then_with(|| compare_named_field(left_fields, right_fields, "relation")),
            ("join", "join") => match (left_fields.get("left"), right_fields.get("left")) {
                (Some(a), Some(b)) => compare_stable_keys(a, b),
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
            }
            .then_with(|| {
                match (left_fields.get("right"), right_fields.get("right")) {
                    (Some(a), Some(b)) => compare_stable_keys(a, b),
                    (None, None) => Ordering::Equal,
                    (None, Some(_)) => Ordering::Less,
                    (Some(_), None) => Ordering::Greater,
                }
            }),
            _ => Ordering::Equal,
        };
    }
    Ordering::Equal
}

fn compare_named_field(
    left: &std::collections::BTreeMap<String, Value>,
    right: &std::collections::BTreeMap<String, Value>,
    name: &str,
) -> Ordering {
    match (left.get(name), right.get(name)) {
        (Some(a), Some(b)) => compare_typed_values(a, b),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
    }
}

fn compare_value_slices(left: &[Value], right: &[Value]) -> Ordering {
    for (a, b) in left.iter().zip(right) {
        let order = compare_typed_values(a, b);
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

fn compare_record_fields(
    left: &std::collections::BTreeMap<String, Value>,
    right: &std::collections::BTreeMap<String, Value>,
) -> Ordering {
    for ((left_name, left_value), (right_name, right_value)) in left.iter().zip(right) {
        let order = left_name
            .as_bytes()
            .cmp(right_name.as_bytes())
            .then_with(|| compare_typed_values(left_value, right_value));
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

fn value_rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::Decimal(_) => 3,
        Value::F64(_) => 4,
        Value::Str(_) => 5,
        Value::Record { .. } => 6,
        Value::Variant { .. } => 7,
        Value::List(_) => 8,
    }
}

/// Return the stable identity for one collect child, failing rather than using
/// a digest when a source identity cannot be decoded.
pub fn stable_child_key_value(
    dag: &RelationDag,
    derivation: &CompactDeriv,
    resolve_base_key: &impl Fn(&str, &super::types::WorldKey) -> Result<Vec<Value>, String>,
) -> Result<Value, String> {
    Ok(StableChildKey::from_derivation(dag, derivation, resolve_base_key)?.to_value())
}

/// Validate the exact versioned stable-key shape before it can enter a comparator.
pub fn validate_stable_child_key(value: &Value) -> Result<(), String> {
    fn exact_fields(fields: &std::collections::BTreeMap<String, Value>, names: &[&str]) -> bool {
        fields.len() == names.len() && names.iter().all(|name| fields.contains_key(*name))
    }

    fn consume_node(depth: usize, work: &mut usize) -> Result<(), String> {
        if depth > MAX_STABLE_IDENTITY_DEPTH {
            return Err(format!(
                "typed collect child key exceeds maximum nesting depth {MAX_STABLE_IDENTITY_DEPTH}"
            ));
        }
        *work = work
            .checked_add(1)
            .ok_or_else(|| "typed collect child key validation work overflow".to_string())?;
        if *work > MAX_STABLE_KEY_VALIDATION_NODES {
            return Err(format!(
                "typed collect child key exceeds validation node budget {MAX_STABLE_KEY_VALIDATION_NODES}"
            ));
        }
        Ok(())
    }

    fn typed_component(value: &Value, depth: usize, work: &mut usize) -> Result<(), String> {
        consume_node(depth, work)?;
        match value {
            Value::Null => Err("null is not a typed collect key component".into()),
            Value::Bool(_) | Value::Int(_) | Value::Str(_) | Value::F64(_) | Value::Decimal(_) => {
                Ok(())
            }
            Value::Record { fields, .. } => {
                for value in fields.values() {
                    typed_component(value, depth + 1, work)?;
                }
                Ok(())
            }
            Value::Variant { args, .. } | Value::List(args) => {
                for value in args {
                    typed_component(value, depth + 1, work)?;
                }
                Ok(())
            }
        }
    }

    fn validate(value: &Value, depth: usize, work: &mut usize) -> Result<(), String> {
        consume_node(depth, work)?;
        let Value::Record { nominal, fields } = value else {
            return Err("typed collect child key must be a record".into());
        };
        if nominal != "world::CollectChildKeyV1" {
            return Err("unsupported typed collect child key version".into());
        }
        let Some(Value::Str(kind)) = fields.get("kind") else {
            return Err("typed collect child key has no kind tag".into());
        };
        match kind.as_str() {
            "source" => {
                if !exact_fields(fields, &["kind", "relation", "key"]) {
                    return Err("malformed source collect child key fields".into());
                }
                let Some(Value::Str(relation)) = fields.get("relation") else {
                    return Err("source collect child key relation must be a string".into());
                };
                if relation.is_empty() {
                    return Err("source collect child key relation must not be empty".into());
                }
                let Some(Value::List(parts)) = fields.get("key") else {
                    return Err("source collect child key components must be a list".into());
                };
                if parts.is_empty() {
                    return Err("source collect child key must have components".into());
                }
                for part in parts {
                    typed_component(part, depth + 1, work)?;
                }
                Ok(())
            }
            "join" => {
                if !exact_fields(fields, &["kind", "left", "right"]) {
                    return Err("malformed join collect child key fields".into());
                }
                validate(
                    fields.get("left").ok_or_else(|| {
                        "join collect child key has no left component".to_string()
                    })?,
                    depth + 1,
                    work,
                )?;
                validate(
                    fields.get("right").ok_or_else(|| {
                        "join collect child key has no right component".to_string()
                    })?,
                    depth + 1,
                    work,
                )
            }
            "derived" => {
                if !exact_fields(fields, &["kind", "relation", "shape", "key"]) {
                    return Err("malformed derived collect child key fields".into());
                }
                let Some(Value::Str(relation)) = fields.get("relation") else {
                    return Err("derived collect child key relation must be a string".into());
                };
                if relation.is_empty() {
                    return Err("derived collect child key relation must not be empty".into());
                }
                let Some(Value::Str(shape)) = fields.get("shape") else {
                    return Err("derived collect child key shape must be a string".into());
                };
                let Some(Value::List(parts)) = fields.get("key") else {
                    return Err("derived collect child key components must be a list".into());
                };
                let mut names = std::collections::BTreeSet::new();
                let mut ordered_names = Vec::with_capacity(parts.len());
                for (index, part) in parts.iter().enumerate() {
                    let Value::Record {
                        nominal: field_nominal,
                        fields: field,
                    } = part
                    else {
                        return Err("derived collect key component must be a record".into());
                    };
                    if field_nominal != "world::CollectChildKeyFieldV1"
                        || !exact_fields(field, &["name", "value"])
                    {
                        return Err("malformed derived collect key component".into());
                    }
                    let Some(Value::Str(name)) = field.get("name") else {
                        return Err("derived collect key field name must be a string".into());
                    };
                    if name.is_empty() || !names.insert(name) {
                        return Err(
                            "derived collect key field names must be unique and nonempty".into(),
                        );
                    }
                    match shape.as_str() {
                        "group" if name != &format!("group_{index}") => {
                            return Err("group collect key fields are not in declared order".into());
                        }
                        "group" => {}
                        "distinct" => ordered_names.push(name.as_str()),
                        _ => return Err("unknown derived collect child key shape".into()),
                    }
                    typed_component(
                        field
                            .get("value")
                            .ok_or_else(|| "derived collect key field has no value".to_string())?,
                        depth + 1,
                        work,
                    )?;
                }
                if shape == "distinct" && ordered_names.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err("distinct collect key fields are not in canonical order".into());
                }
                Ok(())
            }
            _ => Err("unknown typed collect child key kind".into()),
        }
    }

    let mut work = 0;
    validate(value, 1, &mut work)
}

/// Whether an operator is a supported input to child identity derivation.
pub fn validate_stable_identity_path(dag: &RelationDag, op: OperatorId) -> Result<(), String> {
    fn visit(
        dag: &RelationDag,
        op: OperatorId,
        depth: usize,
        memo: &mut [Option<usize>],
        visiting: &mut [bool],
        work: &mut usize,
    ) -> Result<usize, String> {
        if depth > MAX_STABLE_IDENTITY_DEPTH {
            return Err(format!(
                "collect child identity path exceeds maximum depth {MAX_STABLE_IDENTITY_DEPTH}"
            ));
        }
        let node = dag
            .nodes
            .get(op.0)
            .ok_or_else(|| format!("unknown child identity operator {}", op.0))?;
        if let Some(height) = memo[op.0] {
            if depth.saturating_add(height.saturating_sub(1)) > MAX_STABLE_IDENTITY_DEPTH {
                return Err(format!(
                    "collect child identity path exceeds maximum depth {MAX_STABLE_IDENTITY_DEPTH}"
                ));
            }
            return Ok(height);
        }
        if visiting[op.0] {
            return Err("cycle in collect child identity operator DAG".into());
        }
        *work = work
            .checked_add(1)
            .ok_or_else(|| "collect identity validation work overflow".to_string())?;
        if *work > dag.nodes.len() {
            return Err("collect identity validation exceeded operator node budget".into());
        }
        visiting[op.0] = true;
        let child_height = match node {
            OperatorNode::Scan {
                relation,
                key_fields,
                schema,
            } => {
                if key_fields.is_empty() {
                    return Err(format!(
                        "collect source relation '{relation}' has no declared child key"
                    ));
                }
                let fields = match schema {
                    brix_syntax::ast::Ty::Record(fields) => fields,
                    _ => {
                        return Err(format!(
                            "collect source relation '{relation}' has no record schema"
                        ));
                    }
                };
                for key in key_fields {
                    if !fields.iter().any(|field| field.name == *key) {
                        return Err(format!(
                            "collect source relation '{relation}' key field '{key}' is absent from its row schema"
                        ));
                    }
                }
                0
            }
            OperatorNode::DecisionRows { decide, schema } => match schema {
                brix_syntax::ast::Ty::Record(fields) if !fields.is_empty() => 0,
                _ => {
                    return Err(format!(
                        "collect source decision '{decide}' has no typed child key"
                    ));
                }
            },
            OperatorNode::Bind { input, .. }
            | OperatorNode::Filter { input, .. }
            | OperatorNode::Project { input, .. }
            | OperatorNode::AntiJoin { left: input, .. }
            | OperatorNode::ParentCount { left: input, .. } => {
                visit(dag, *input, depth + 1, memo, visiting, work)?
            }
            OperatorNode::Distinct { .. } => {
                relation_for_operator(dag, op)?;
                0
            }
            OperatorNode::EquiJoin { left, right, .. } => visit(
                dag,
                *left,
                depth + 1,
                memo,
                visiting,
                work,
            )?
            .max(visit(dag, *right, depth + 1, memo, visiting, work)?),
            OperatorNode::DisjointUnion { .. }
            | OperatorNode::GroupedCount { .. }
            | OperatorNode::UnionDistinct { .. } => {
                relation_for_operator(dag, op)?;
                0
            }
        };
        visiting[op.0] = false;
        let height = child_height + 1;
        if depth.saturating_add(height.saturating_sub(1)) > MAX_STABLE_IDENTITY_DEPTH {
            return Err(format!(
                "collect child identity path exceeds maximum depth {MAX_STABLE_IDENTITY_DEPTH}"
            ));
        }
        memo[op.0] = Some(height);
        Ok(height)
    }

    let mut memo = vec![None; dag.nodes.len()];
    let mut visiting = vec![false; dag.nodes.len()];
    let mut work = 0;
    visit(dag, op, 1, &mut memo, &mut visiting, &mut work).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{
        compare_stable_keys, validate_stable_child_key, validate_stable_identity_path,
        DerivedKeyShape, StableChildKey, MAX_STABLE_IDENTITY_DEPTH,
    };
    use crate::world::Value;
    use brix_lower::relation_dag::{OperatorNode, RelationDag};
    use std::cmp::Ordering;

    fn source_scan(dag: &mut RelationDag) -> brix_lower::relation_dag::OperatorId {
        dag.add_node(OperatorNode::Scan {
            relation: "root::rows".into(),
            schema: brix_syntax::ast::Ty::Record(vec![brix_syntax::ast::FieldDecl {
                name: "id".into(),
                ty: brix_syntax::ast::Ty::Named("Str".into()),
            }]),
            key_fields: vec!["id".into()],
        })
    }

    fn shared_join(
        dag: &mut RelationDag,
        input: brix_lower::relation_dag::OperatorId,
    ) -> brix_lower::relation_dag::OperatorId {
        dag.add_node(OperatorNode::EquiJoin {
            left: input,
            right: input,
            left_keys: vec![],
            right_keys: vec![],
        })
    }

    #[test]
    fn declared_key_precedes_qualified_source_tiebreak() {
        let key_z = StableChildKey::Source {
            relation: "app::a_source".into(),
            key: vec![Value::Str("z".into())],
        }
        .to_value();
        let key_a = StableChildKey::Source {
            relation: "app::z_source".into(),
            key: vec![Value::Str("a".into())],
        }
        .to_value();
        assert_eq!(compare_stable_keys(&key_a, &key_z), Ordering::Less);

        let same_key_a = StableChildKey::Source {
            relation: "app::a_source".into(),
            key: vec![Value::Str("same".into())],
        }
        .to_value();
        let same_key_z = StableChildKey::Source {
            relation: "app::z_source".into(),
            key: vec![Value::Str("same".into())],
        }
        .to_value();
        assert_eq!(
            compare_stable_keys(&same_key_a, &same_key_z),
            Ordering::Less
        );
    }

    #[test]
    fn child_key_strings_use_utf8_byte_order_and_join_keys_recurse() {
        let a = StableChildKey::Source {
            relation: "app::rows".into(),
            key: vec![Value::Str("é".into())],
        }
        .to_value();
        let z = StableChildKey::Source {
            relation: "app::rows".into(),
            key: vec![Value::Str("z".into())],
        }
        .to_value();
        assert_eq!(compare_stable_keys(&z, &a), Ordering::Less);

        let join_a = StableChildKey::Join {
            left: Box::new(StableChildKey::Source {
                relation: "app::left".into(),
                key: vec![Value::Int(2)],
            }),
            right: Box::new(StableChildKey::Source {
                relation: "app::right".into(),
                key: vec![Value::Int(1)],
            }),
        }
        .to_value();
        let join_b = StableChildKey::Join {
            left: Box::new(StableChildKey::Source {
                relation: "app::left".into(),
                key: vec![Value::Int(3)],
            }),
            right: Box::new(StableChildKey::Source {
                relation: "app::right".into(),
                key: vec![Value::Int(0)],
            }),
        }
        .to_value();
        assert_eq!(compare_stable_keys(&join_a, &join_b), Ordering::Less);
    }

    #[test]
    fn rejects_wrong_version_fields_kind_and_empty_source_components() {
        let valid = StableChildKey::Source {
            relation: "app::rows".into(),
            key: vec![Value::Str("r1".into())],
        }
        .to_value();
        assert!(validate_stable_child_key(&valid).is_ok());

        let mut wrong_version = valid.clone();
        if let Value::Record { nominal, .. } = &mut wrong_version {
            *nominal = "world::CollectChildKeyV0".into();
        }
        assert!(validate_stable_child_key(&wrong_version).is_err());

        let mut extra_field = valid.clone();
        if let Value::Record { fields, .. } = &mut extra_field {
            fields.insert("hash".into(), Value::Str("not a tie breaker".into()));
        }
        assert!(validate_stable_child_key(&extra_field).is_err());

        let empty_source = StableChildKey::Source {
            relation: "app::rows".into(),
            key: vec![],
        }
        .to_value();
        assert!(validate_stable_child_key(&empty_source).is_err());

        let mut unknown_kind = valid;
        if let Value::Record { fields, .. } = &mut unknown_kind {
            fields.insert("kind".into(), Value::Str("digest".into()));
        }
        assert!(validate_stable_child_key(&unknown_kind).is_err());
    }

    #[test]
    fn validates_recursive_join_and_derived_component_shapes() {
        let source = StableChildKey::Source {
            relation: "app::rows".into(),
            key: vec![Value::Int(1)],
        };
        let valid_join = StableChildKey::Join {
            left: Box::new(source.clone()),
            right: Box::new(source.clone()),
        }
        .to_value();
        assert!(validate_stable_child_key(&valid_join).is_ok());
        let malformed_join = Value::Record {
            nominal: "world::CollectChildKeyV1".into(),
            fields: std::collections::BTreeMap::from([
                ("kind".into(), Value::Str("join".into())),
                ("left".into(), Value::Str("not a child key".into())),
                ("right".into(), source.to_value()),
            ]),
        };
        assert!(validate_stable_child_key(&malformed_join).is_err());

        let group = StableChildKey::Derived {
            relation: "app::groups".into(),
            shape: DerivedKeyShape::Group,
            key: vec![
                ("group_0".into(), Value::Str("tenant".into())),
                ("group_1".into(), Value::Str("parent".into())),
            ],
        }
        .to_value();
        assert!(validate_stable_child_key(&group).is_ok());

        let reordered_group = StableChildKey::Derived {
            relation: "app::groups".into(),
            shape: DerivedKeyShape::Group,
            key: vec![
                ("group_1".into(), Value::Str("parent".into())),
                ("group_0".into(), Value::Str("tenant".into())),
            ],
        }
        .to_value();
        assert!(validate_stable_child_key(&reordered_group).is_err());

        let mut reordered_distinct = StableChildKey::Derived {
            relation: "app::distinct".into(),
            shape: DerivedKeyShape::Distinct,
            key: vec![("a".into(), Value::Int(1)), ("z".into(), Value::Int(2))],
        }
        .to_value();
        if let Value::Record { fields, .. } = &mut reordered_distinct {
            if let Some(Value::List(parts)) = fields.get_mut("key") {
                parts.swap(0, 1);
            }
        }
        assert!(validate_stable_child_key(&reordered_distinct).is_err());
    }

    #[test]
    fn memoized_static_walk_handles_shared_diamonds_and_rejects_excess_depth() {
        let mut diamond = RelationDag::new();
        let mut input = source_scan(&mut diamond);
        for _ in 0..30 {
            input = shared_join(&mut diamond, input);
        }
        assert!(validate_stable_identity_path(&diamond, input).is_ok());

        let mut boundary = RelationDag::new();
        let mut input = source_scan(&mut boundary);
        for _ in 0..MAX_STABLE_IDENTITY_DEPTH - 1 {
            input = shared_join(&mut boundary, input);
        }
        assert!(validate_stable_identity_path(&boundary, input).is_ok());

        let mut too_deep = RelationDag::new();
        let mut input = source_scan(&mut too_deep);
        for _ in 0..MAX_STABLE_IDENTITY_DEPTH {
            input = shared_join(&mut too_deep, input);
        }
        assert!(validate_stable_identity_path(&too_deep, input).is_err());
    }

    #[test]
    fn persisted_join_key_depth_is_bounded() {
        fn join(left: Value, right: Value) -> Value {
            Value::Record {
                nominal: "world::CollectChildKeyV1".into(),
                fields: std::collections::BTreeMap::from([
                    ("kind".into(), Value::Str("join".into())),
                    ("left".into(), left),
                    ("right".into(), right),
                ]),
            }
        }
        let leaf = StableChildKey::Source {
            relation: "root::rows".into(),
            key: vec![Value::Str("r1".into())],
        }
        .to_value();
        let mut within_limit = leaf.clone();
        for _ in 0..MAX_STABLE_IDENTITY_DEPTH - 2 {
            within_limit = join(within_limit, leaf.clone());
        }
        assert!(validate_stable_child_key(&within_limit).is_ok());

        let too_deep = join(within_limit, leaf);
        assert!(validate_stable_child_key(&too_deep).is_err());
    }
}
