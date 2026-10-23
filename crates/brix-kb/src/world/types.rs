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

    // Not `std::str::FromStr`: this is an infallible canonical-encoding
    // constructor (parallel to `from_u64` below), not string parsing.
    #[allow(clippy::should_implement_trait)]
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
        if !s.len().is_multiple_of(2) {
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

impl fmt::Display for WorldKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut r = CanonReader::new(&self.0);
        let s = r
            .read_bytes()
            .ok()
            .filter(|_| r.is_empty())
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.to_hex());
        write!(f, "{s}")
    }
}

/// An opaque, canonical record tuple payload.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorldTuple(pub Vec<u8>);

impl WorldTuple {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    // Not `std::str::FromStr`: this is an infallible canonical-encoding
    // constructor (parallel to `from_bytes` below), not string parsing.
    #[allow(clippy::should_implement_trait)]
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

/// Position within a relation for logarithmic pagination.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum QueryCursorPosition {
    Hamt { hash: [u8; 32], key: WorldKey },
    Offset(usize),
}

/// A scope-bound pagination cursor ensuring a continuation is resumed
/// only against the exact world, revision, relation, and ordering it was issued for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QueryCursor {
    pub world_id: String,
    pub revision: Digest,
    pub relation: String,
    pub ordering: String,
    pub position: QueryCursorPosition,
}

impl QueryCursor {
    pub fn new_hamt(
        world_id: impl Into<String>,
        revision: Digest,
        relation: impl Into<String>,
        hash: [u8; 32],
        key: WorldKey,
    ) -> Self {
        Self {
            world_id: world_id.into(),
            revision,
            relation: relation.into(),
            ordering: "canonical".to_string(),
            position: QueryCursorPosition::Hamt { hash, key },
        }
    }

    pub fn new_offset(
        world_id: impl Into<String>,
        revision: Digest,
        relation: impl Into<String>,
        offset: usize,
    ) -> Self {
        Self {
            world_id: world_id.into(),
            revision,
            relation: relation.into(),
            ordering: "offset".to_string(),
            position: QueryCursorPosition::Offset(offset),
        }
    }

    pub fn to_token(&self) -> String {
        let mut writer = CanonWriter::new();
        writer.write_str("brix.query-cursor@1");
        writer.write_str(&self.world_id);
        writer.write_bytes(self.revision.as_bytes());
        writer.write_str(&self.relation);
        writer.write_str(&self.ordering);
        match &self.position {
            QueryCursorPosition::Hamt { hash, key } => {
                writer.write_uint(0);
                writer.write_bytes(hash);
                writer.write_bytes(key.as_bytes());
            }
            QueryCursorPosition::Offset(offset) => {
                writer.write_uint(1);
                writer.write_uint(*offset as u64);
            }
        }
        WorldKey::new(writer.finish()).to_hex()
    }

    pub fn from_token(token: &str) -> Result<Self, WorldError> {
        let bytes = WorldKey::from_hex(token).map_err(|_| {
            WorldError::InvalidCursor("invalid hex in query cursor".into())
        })?;
        let mut reader = CanonReader::new(bytes.as_bytes());
        let invalid = || WorldError::InvalidCursor("malformed query cursor".into());
        let magic = match reader.read_bytes() {
            Ok(m) => m,
            Err(_) => {
                if let Ok(legacy) = WorldCursor::from_token(token) {
                    return Ok(Self {
                        world_id: String::new(),
                        revision: Digest::from_bytes([0u8; 32]),
                        relation: String::new(),
                        ordering: "canonical".to_string(),
                        position: QueryCursorPosition::Hamt {
                            hash: legacy.hash,
                            key: legacy.key,
                        },
                    });
                }
                if let Ok(offset) = token.parse::<usize>() {
                    return Ok(Self {
                        world_id: String::new(),
                        revision: Digest::from_bytes([0u8; 32]),
                        relation: String::new(),
                        ordering: "offset".to_string(),
                        position: QueryCursorPosition::Offset(offset),
                    });
                }
                return Err(invalid());
            }
        };
        if magic != b"brix.query-cursor@1" {
            if let Ok(legacy) = WorldCursor::from_token(token) {
                return Ok(Self {
                    world_id: String::new(),
                    revision: Digest::from_bytes([0u8; 32]),
                    relation: String::new(),
                    ordering: "canonical".to_string(),
                    position: QueryCursorPosition::Hamt {
                        hash: legacy.hash,
                        key: legacy.key,
                    },
                });
            }
            if let Ok(offset) = token.parse::<usize>() {
                return Ok(Self {
                    world_id: String::new(),
                    revision: Digest::from_bytes([0u8; 32]),
                    relation: String::new(),
                    ordering: "offset".to_string(),
                    position: QueryCursorPosition::Offset(offset),
                });
            }
            return Err(invalid());
        }
        let world_id = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_string();
        let revision_bytes: [u8; 32] = reader
            .read_bytes()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let relation = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_string();
        let ordering = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_string();
        let kind = reader.read_uint().map_err(|_| invalid())?;
        let position = match kind {
            0 => {
                let hash: [u8; 32] = reader
                    .read_bytes()
                    .map_err(|_| invalid())?
                    .try_into()
                    .map_err(|_| invalid())?;
                let key_bytes = reader.read_bytes().map_err(|_| invalid())?.to_vec();
                QueryCursorPosition::Hamt {
                    hash,
                    key: WorldKey::new(key_bytes),
                }
            }
            1 => {
                let offset = reader.read_uint().map_err(|_| invalid())? as usize;
                QueryCursorPosition::Offset(offset)
            }
            _ => return Err(invalid()),
        };
        if !reader.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            world_id,
            revision: Digest::from_bytes(revision_bytes),
            relation,
            ordering,
            position,
        })
    }
}
