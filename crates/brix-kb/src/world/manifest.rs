//! World manifest and catalog schema `brix.world@1` (ADR-0046 §3.2, P3).

use brix_canon::{CanonWriter, Digest, Domain};
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;

use super::error::WorldError;

pub const WORLD_SCHEMA: &str = "brix.world@1";
pub const WORLD_PROFILE: &str = "brix.world@1";
const MANIFEST_DOMAIN_TAG: &str = "brix.world@1";

/// Declaration of a single keyed relation in the world catalog.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RelationDecl {
    pub name: String,
    pub key_fields: Vec<String>,
    pub value_fields: Vec<String>,
    pub secondary_indexes: Vec<String>,
}

impl RelationDecl {
    pub fn new(
        name: impl Into<String>,
        key_fields: Vec<String>,
        value_fields: Vec<String>,
        secondary_indexes: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            key_fields,
            value_fields,
            secondary_indexes,
        }
    }

    pub fn canon_write(&self, w: &mut CanonWriter) {
        w.write_ident(&self.name);
        w.write_uint(self.key_fields.len() as u64);
        for f in &self.key_fields {
            w.write_ident(f);
        }
        w.write_uint(self.value_fields.len() as u64);
        for f in &self.value_fields {
            w.write_ident(f);
        }
        w.write_uint(self.secondary_indexes.len() as u64);
        for f in &self.secondary_indexes {
            w.write_ident(f);
        }
    }

    pub fn to_json(&self) -> JsonValue {
        json!({
            "name": self.name,
            "key_fields": self.key_fields,
            "value_fields": self.value_fields,
            "secondary_indexes": self.secondary_indexes,
        })
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("relation missing 'name'".to_string()))?
            .to_string();

        let key_fields = v
            .get("key_fields")
            .and_then(|x| x.as_array())
            .ok_or_else(|| WorldError::Json("relation missing 'key_fields'".to_string()))?
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();

        let value_fields = v
            .get("value_fields")
            .and_then(|x| x.as_array())
            .ok_or_else(|| WorldError::Json("relation missing 'value_fields'".to_string()))?
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();

        let secondary_indexes = v
            .get("secondary_indexes")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            name,
            key_fields,
            value_fields,
            secondary_indexes,
        })
    }
}

/// The root world manifest (`world.json`), schema `brix.world@1`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldManifest {
    pub schema: String,
    pub profile: String,
    pub world_id: String,
    pub created_at: String,
    pub program_digest: Digest,
    /// Whether this world is executable and therefore requires a pinned source closure.
    /// Missing values in older manifests decode as `false` (storage-only worlds).
    pub program_required: bool,
    pub relations: BTreeMap<String, RelationDecl>,
}

impl WorldManifest {
    pub fn new(
        world_id: impl Into<String>,
        created_at: impl Into<String>,
        program_digest: Digest,
        relations: Vec<RelationDecl>,
    ) -> Self {
        let mut rel_map = BTreeMap::new();
        for r in relations {
            rel_map.insert(r.name.clone(), r);
        }
        Self {
            schema: WORLD_SCHEMA.to_string(),
            profile: WORLD_PROFILE.to_string(),
            world_id: world_id.into(),
            created_at: created_at.into(),
            program_digest,
            program_required: false,
            relations: rel_map,
        }
    }

    /// Validate manifest invariants: secondary indexes must be declared in key or value fields.
    pub fn validate(&self) -> Result<(), WorldError> {
        for (rel_name, decl) in &self.relations {
            for sec in &decl.secondary_indexes {
                if !decl.value_fields.contains(sec) && !decl.key_fields.contains(sec) {
                    return Err(WorldError::InvalidIndexDeclaration(format!(
                        "relation '{rel_name}' declares secondary index on '{sec}', but '{sec}' is not in value_fields or key_fields"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(MANIFEST_DOMAIN_TAG);
        w.write_ident(&self.schema);
        w.write_ident(&self.profile);
        w.write_str(&self.world_id);
        w.write_str(&self.created_at);
        w.write_bytes(self.program_digest.as_bytes());
        w.write_uint(self.relations.len() as u64);
        for (name, decl) in &self.relations {
            w.write_ident(name);
            decl.canon_write(&mut w);
        }
        w.digest(Domain::Value)
    }

    pub fn to_json(&self) -> JsonValue {
        let rel_json: Vec<JsonValue> = self.relations.values().map(|r| r.to_json()).collect();
        json!({
            "schema": self.schema,
            "profile": self.profile,
            "world_id": self.world_id,
            "created_at": self.created_at,
            "program_digest": self.program_digest.to_hex(),
            "program_required": self.program_required,
            "relations": rel_json,
        })
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let schema = v
            .get("schema")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("manifest missing 'schema'".to_string()))?;
        if schema != WORLD_SCHEMA {
            return Err(WorldError::InvalidSchema(format!(
                "expected schema {WORLD_SCHEMA}, got {schema}"
            )));
        }

        let profile = v
            .get("profile")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("manifest missing 'profile'".to_string()))?
            .to_string();

        let world_id = v
            .get("world_id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("manifest missing 'world_id'".to_string()))?
            .to_string();

        let created_at = v
            .get("created_at")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("manifest missing 'created_at'".to_string()))?
            .to_string();

        let prog_hex = v
            .get("program_digest")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("manifest missing 'program_digest'".to_string()))?;

        if prog_hex.len() != 64 {
            return Err(WorldError::Json(
                "program_digest must be 64-char hex".to_string(),
            ));
        }
        let mut p_bytes = [0u8; 32];
        let chars: Vec<char> = prog_hex.chars().collect();
        for i in (0..64).step_by(2) {
            let high = chars[i]
                .to_digit(16)
                .ok_or_else(|| WorldError::Json("invalid hex in program_digest".to_string()))?;
            let low = chars[i + 1]
                .to_digit(16)
                .ok_or_else(|| WorldError::Json("invalid hex in program_digest".to_string()))?;
            p_bytes[i / 2] = ((high << 4) | low) as u8;
        }
        let program_digest = Digest::from_bytes(p_bytes);

        let relations_arr = v
            .get("relations")
            .and_then(|x| x.as_array())
            .ok_or_else(|| WorldError::Json("manifest missing 'relations'".to_string()))?;

        let mut relations = BTreeMap::new();
        for r_val in relations_arr {
            let decl = RelationDecl::from_json(r_val)?;
            relations.insert(decl.name.clone(), decl);
        }

        let manifest = Self {
            schema: schema.to_string(),
            profile,
            world_id,
            created_at,
            program_digest,
            program_required: v
                .get("program_required")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            relations,
        };
        manifest.validate()?;
        Ok(manifest)
    }
}
