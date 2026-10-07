//! Deterministic, versioned tuple codec and field projection contract (ADR-0046 §3.2).

use super::error::WorldError;
use super::manifest::RelationDecl;
use super::types::{WorldKey, WorldTuple};
use brix_canon::{CanonReader, CanonWriter};
use std::collections::BTreeMap;

/// Canonical magic header for version 1 structured tuple records.
pub const TUPLE_MAGIC_V1: &[u8] = b"brix.tuple@1\0";

/// A structured tuple record with named fields supporting deterministic canonical encoding.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TupleRecord {
    pub fields: BTreeMap<String, Vec<u8>>,
}

impl TupleRecord {
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    pub fn set(&mut self, field: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.fields.insert(field.into(), value.into());
    }

    pub fn set_str(&mut self, field: impl Into<String>, value: &str) {
        self.fields.insert(field.into(), value.as_bytes().to_vec());
    }

    pub fn get(&self, field: &str) -> Option<&[u8]> {
        self.fields.get(field).map(|v| v.as_slice())
    }

    pub fn get_str(&self, field: &str) -> Option<&str> {
        self.get(field).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Encode to canonical `brix.tuple@1` format.
    pub fn to_tuple(&self) -> WorldTuple {
        let mut w = CanonWriter::new();
        w.write_bytes(TUPLE_MAGIC_V1);
        w.write_uint(self.fields.len() as u64);
        for (name, val) in &self.fields {
            w.write_str(name);
            w.write_bytes(val);
        }
        WorldTuple::new(w.finish())
    }

    /// Decode from a `WorldTuple` if encoded with `brix.tuple@1`.
    pub fn from_tuple(tuple: &WorldTuple) -> Result<Self, WorldError> {
        let bytes = tuple.as_bytes();
        let mut r = CanonReader::new(bytes);
        let magic = r.read_bytes().map_err(WorldError::Canon)?;
        if magic != TUPLE_MAGIC_V1 {
            let versioned_prefix = b"brix.tuple@";
            if magic.starts_with(versioned_prefix) {
                return Err(WorldError::InvalidSchema(format!(
                    "unsupported tuple record version: {}",
                    String::from_utf8_lossy(magic)
                )));
            }
            return Err(WorldError::InvalidSchema(
                "tuple does not have brix.tuple@1 magic header".to_string(),
            ));
        }
        let count = r.read_uint().map_err(WorldError::Canon)?;
        let mut fields = BTreeMap::new();
        let mut previous_name: Option<String> = None;
        for _ in 0..count {
            let name_bytes = r.read_bytes().map_err(WorldError::Canon)?;
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| WorldError::InvalidSchema("invalid UTF-8 field name".to_string()))?
                .to_string();
            if name.is_empty() {
                return Err(WorldError::InvalidSchema(
                    "tuple field name must not be empty".to_string(),
                ));
            }
            if let Some(previous) = &previous_name {
                match previous.cmp(&name) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => {
                        return Err(WorldError::InvalidSchema(format!(
                            "duplicate tuple field '{name}'"
                        )))
                    }
                    std::cmp::Ordering::Greater => {
                        return Err(WorldError::InvalidSchema(format!(
                            "tuple fields are not in canonical order at '{name}'"
                        )))
                    }
                }
            }
            let val = r.read_bytes().map_err(WorldError::Canon)?.to_vec();
            previous_name = Some(name.clone());
            fields.insert(name, val);
        }
        if !r.is_empty() {
            return Err(WorldError::InvalidSchema(
                "trailing bytes after brix.tuple@1 record".to_string(),
            ));
        }
        Ok(Self { fields })
    }
}

/// Encode an extracted field value into a canonical secondary key with explicit boundary.
pub fn encode_secondary_key(field_bytes: &[u8]) -> WorldKey {
    let mut w = CanonWriter::new();
    w.write_bytes(field_bytes);
    WorldKey::new(w.finish())
}

/// Extract an indexed field value from a `WorldTuple` given a relation declaration.
///
/// Indexed fields are projected only from a strict `brix.tuple@1` record.
/// Legacy text payloads must be converted by an explicit import adapter.
pub fn extract_indexed_field(
    relation: &str,
    decl: &RelationDecl,
    field_name: &str,
    tuple: &WorldTuple,
) -> Result<Vec<u8>, WorldError> {
    let rec = TupleRecord::from_tuple(tuple)?;
    let _ = decl;
    rec.get(field_name)
        .map(ToOwned::to_owned)
        .ok_or_else(|| WorldError::MissingIndexField {
            relation: relation.to_string(),
            field: field_name.to_string(),
            reason: format!("field '{field_name}' not present in structured tuple"),
        })
}
