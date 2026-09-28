//! The knowledge base manifest (`kb.json`, schema `brix.kb@1`) and `HEAD`
//! pointer (ADR-0041).

use brix_canon::Digest;
use serde_json::json;

use crate::error::KbError;
use crate::revision::hex_to_digest;
use crate::strict_json;

pub const KB_SCHEMA: &str = "brix.kb@1";
pub const HEAD_SCHEMA: &str = "brix.kb.head@1";

/// The static manifest identifying a directory as a `brix-kb` knowledge base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub profile: String,
}

impl Manifest {
    pub fn to_json_string(&self) -> String {
        let doc = json!({"schema": KB_SCHEMA, "profile": self.profile});
        serde_json::to_string_pretty(&doc).expect("manifest JSON encoding cannot fail")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KbError> {
        let v = strict_json::parse_document(bytes)
            .map_err(|e| KbError::unknown("kb-manifest-decode-error", e.to_string()))?;
        v.require_only_keys(&["schema", "profile"])
            .map_err(|e| KbError::unknown("kb-manifest-decode-error", e.to_string()))?;
        let schema = v
            .field_str("schema")
            .map_err(|e| KbError::unknown("kb-manifest-decode-error", e.to_string()))?;
        if schema != KB_SCHEMA {
            return Err(KbError::unknown(
                "kb-manifest-schema-mismatch",
                format!("expected schema '{KB_SCHEMA}', found '{schema}' — this directory is not a brix-kb knowledge base"),
            ));
        }
        let profile = v
            .field_str("profile")
            .map_err(|e| KbError::unknown("kb-manifest-decode-error", e.to_string()))?
            .to_string();
        Ok(Manifest { profile })
    }
}

/// The mutable pointer to the current head revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub seq: u64,
    pub digest: Digest,
}

impl Head {
    pub fn to_json_string(&self) -> String {
        let doc = json!({"schema": HEAD_SCHEMA, "seq": self.seq, "digest": self.digest.to_hex()});
        serde_json::to_string_pretty(&doc).expect("HEAD JSON encoding cannot fail")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KbError> {
        let v = strict_json::parse_document(bytes)
            .map_err(|e| KbError::unknown("kb-head-decode-error", e.to_string()))?;
        v.require_only_keys(&["schema", "seq", "digest"])
            .map_err(|e| KbError::unknown("kb-head-decode-error", e.to_string()))?;
        let schema = v
            .field_str("schema")
            .map_err(|e| KbError::unknown("kb-head-decode-error", e.to_string()))?;
        if schema != HEAD_SCHEMA {
            return Err(KbError::unknown(
                "kb-head-schema-mismatch",
                format!("expected schema '{HEAD_SCHEMA}', found '{schema}'"),
            ));
        }
        let seq = v
            .field_u64("seq")
            .map_err(|e| KbError::unknown("kb-head-decode-error", e.to_string()))?;
        let digest_hex = v
            .field_str("digest")
            .map_err(|e| KbError::unknown("kb-head-decode-error", e.to_string()))?;
        let digest = hex_to_digest(digest_hex, "digest")?;
        Ok(Head { seq, digest })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::Domain;

    #[test]
    fn test_manifest_roundtrip() {
        let m = Manifest {
            profile: "brix.l3.finite-decision@1".to_string(),
        };
        let decoded = Manifest::from_bytes(m.to_json_string().as_bytes()).unwrap();
        assert_eq!(decoded, m);
    }

    #[test]
    fn test_manifest_rejects_wrong_schema() {
        let bad = r#"{"schema": "not-a-kb@1", "profile": "x"}"#;
        let err = Manifest::from_bytes(bad.as_bytes()).unwrap_err();
        assert_eq!(err.code, "kb-manifest-schema-mismatch");
    }

    #[test]
    fn test_head_roundtrip() {
        let h = Head {
            seq: 3,
            digest: Digest::of(Domain::Value, b"x"),
        };
        let decoded = Head::from_bytes(h.to_json_string().as_bytes()).unwrap();
        assert_eq!(decoded, h);
    }
}
