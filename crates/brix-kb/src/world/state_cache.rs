//! The state-cache sidecar `state-cache.json`, schema `brix.world.state-cache@1`.
//!
//! A best-effort pointer to a network-state header for the *current* HEAD revision of a
//! world whose revision record does not name one itself (a legacy `@2` world, or one
//! written under another validity key). The first open that has to replay writes it, so the
//! next open restores instead of replaying again. It lives outside the revision journal: it
//! is never authoritative and never part of any digest, a missing, unparseable or stale file
//! is a cache miss, and every consumer re-checks the header it names exactly as it would a
//! revision's own.

use std::fs;
use std::io::Write;
use std::path::Path;

use brix_canon::Digest;

pub const STATE_CACHE_SCHEMA: &str = "brix.world.state-cache@1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateCache {
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub validity_key_digest: Digest,
    pub network_state_root: Digest,
}

fn parse_hex(s: &str) -> Option<Digest> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut b = [0u8; 32];
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(Digest::from_bytes(b))
}

impl StateCache {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": STATE_CACHE_SCHEMA,
            "revision_seq": self.revision_seq,
            "revision_digest": self.revision_digest.to_hex(),
            "validity_key_digest": self.validity_key_digest.to_hex(),
            "network_state_root": self.network_state_root.to_hex(),
        })
    }

    /// Parse a sidecar; anything but a well-formed `@1` file is `None` (a cache miss).
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        if v.get("schema")?.as_str()? != STATE_CACHE_SCHEMA {
            return None;
        }
        Some(Self {
            revision_seq: v.get("revision_seq")?.as_u64()?,
            revision_digest: parse_hex(v.get("revision_digest")?.as_str()?)?,
            validity_key_digest: parse_hex(v.get("validity_key_digest")?.as_str()?)?,
            network_state_root: parse_hex(v.get("network_state_root")?.as_str()?)?,
        })
    }

    pub fn read(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        Self::from_json(&serde_json::from_slice(&bytes).ok()?)
    }

    /// Write `tmp` durably (create, fsync). The caller renames it into place.
    pub fn write_tmp(&self, tmp: &Path) -> std::io::Result<()> {
        let mut f = fs::File::create(tmp)?;
        f.write_all(&serde_json::to_vec_pretty(&self.to_json())?)?;
        f.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_misses() {
        let d = |n: u8| Digest::from_bytes([n; 32]);
        let c = StateCache {
            revision_seq: 3,
            revision_digest: d(1),
            validity_key_digest: d(2),
            network_state_root: d(3),
        };
        assert_eq!(StateCache::from_json(&c.to_json()), Some(c.clone()));
        let mut bad = c.to_json();
        bad["schema"] = "other".into();
        assert_eq!(StateCache::from_json(&bad), None);
        let mut short = c.to_json();
        short["network_state_root"] = "abcd".into();
        assert_eq!(StateCache::from_json(&short), None);
        assert_eq!(StateCache::from_json(&serde_json::json!({})), None);
    }
}
