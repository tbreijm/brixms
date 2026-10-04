//! Core typed wrappers for persistent world records and pagination cursors (ADR-0046, P3).

use brix_canon::{CanonDecode, CanonError, CanonReader, CanonWriter, Canonical, Digest};
use std::fmt;

use super::error::WorldError;

/// An opaque, canonical primary or secondary key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorldKey(pub Vec<u8>);

impl WorldKey {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn from_str(s: &str) -> Self {
        let mut w = CanonWriter::new();
        w.write_str(s);
        Self(w.finish())
    }

    pub fn from_u64(n: u64) -> Self {
        let mut w = CanonWriter::new();
        w.write_uint(n);
        Self(w.finish())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(self.0.len() * 2);
        for b in &self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        s
    }

    pub fn from_hex(s: &str) -> Result<Self, WorldError> {
        if s.len() % 2 != 0 {
            return Err(WorldError::InvalidCursor("odd hex length".to_string()));
        }
        let mut bytes = Vec::with_capacity(s.len() / 2);
        let chars: Vec<char> = s.chars().collect();
        for i in (0..chars.len()).step_by(2) {
            let high = chars[i]
                .to_digit(16)
                .ok_or_else(|| WorldError::InvalidCursor("invalid hex character".to_string()))?;
            let low = chars[i + 1]
                .to_digit(16)
                .ok_or_else(|| WorldError::InvalidCursor("invalid hex character".to_string()))?;
            bytes.push(((high << 4) | low) as u8);
        }
        Ok(Self(bytes))
    }
}

impl Canonical for WorldKey {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(&self.0);
    }
}

impl CanonDecode for WorldKey {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        let b = r.read_bytes()?;
        Ok(Self(b.to_vec()))
    }
}

impl fmt::Debug for WorldKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorldKey({})", self.to_hex())
    }
}

/// An opaque, canonical record tuple payload.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorldTuple(pub Vec<u8>);

impl WorldTuple {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn from_str(s: &str) -> Self {
        let mut w = CanonWriter::new();
        w.write_str(s);
        Self(w.finish())
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(b);
        Self(w.finish())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(self.0.len() * 2);
        for b in &self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        s
    }
}

impl Canonical for WorldTuple {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(&self.0);
    }
}

impl CanonDecode for WorldTuple {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        let b = r.read_bytes()?;
        Ok(Self(b.to_vec()))
    }
}

impl fmt::Debug for WorldTuple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorldTuple(len={})", self.0.len())
    }
}

/// An opaque pagination cursor encoding `(hash, key)` for logarithmic HAMT pagination.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldCursor {
    pub hash: [u8; 32],
    pub key: WorldKey,
}

impl WorldCursor {
    pub fn new(hash: [u8; 32], key: WorldKey) -> Self {
        Self { hash, key }
    }

    pub fn to_token(&self) -> String {
        let digest = Digest::from_bytes(self.hash);
        format!("{}:{}", digest.to_hex(), self.key.to_hex())
    }

    pub fn from_token(token: &str) -> Result<Self, WorldError> {
        let parts: Vec<&str> = token.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(WorldError::InvalidCursor(
                "cursor must be of format '<hash_hex>:<key_hex>'".to_string(),
            ));
        }
        if parts[0].len() != 64 {
            return Err(WorldError::InvalidCursor(
                "hash hex must be exactly 64 characters".to_string(),
            ));
        }
        let mut hash = [0u8; 32];
        let chars: Vec<char> = parts[0].chars().collect();
        for i in (0..64).step_by(2) {
            let high = chars[i].to_digit(16).ok_or_else(|| {
                WorldError::InvalidCursor("invalid hex in cursor hash".to_string())
            })?;
            let low = chars[i + 1].to_digit(16).ok_or_else(|| {
                WorldError::InvalidCursor("invalid hex in cursor hash".to_string())
            })?;
            hash[i / 2] = ((high << 4) | low) as u8;
        }
        let key = WorldKey::from_hex(parts[1])?;
        Ok(Self { hash, key })
    }
}
