//! Content digests, computed exclusively through `brix-canon`.
//!
//! Hard rule (OWNER.md / CONTRIBUTING.md): "Serialize semantic data only through
//! `brix-canon`... never a second encoder." Package file trees are not BrixMS
//! graph values, but the rule this module follows is the *spirit* of that one:
//! there is exactly one hashing surface in the whole toolchain
//! (`brix_canon::Digest::of`), so a package digest and a `NodeId` can never
//! collide and a reviewer never has to ask "which hasher produced this hex
//! string?".

use std::collections::BTreeMap;
use std::fmt;

use brix_canon::{CanonError, CanonReader, CanonWriter, Digest, Domain};

pub use camino::Utf8PathBuf;

/// A digest stored on disk (lockfile, registry index) as 64 lowercase hex
/// characters. The *only* way one of these is ever produced is by copying the
/// bytes out of a `brix_canon::Digest` that `Digest::of`/`CanonWriter::digest`
/// actually computed ([`ContentDigest::from_canon`]) — `brix-canon` has no public
/// constructor from raw bytes (by design: a `Digest` you didn't hash yourself
/// shouldn't type-check as one), so this newtype is what lets a digest survive a
/// round trip through TOML text without brixpkg growing a second hasher to
/// "reconstruct" one. [`ContentDigest::from_hex`] only ever produces a value that
/// gets compared against a freshly recomputed [`ContentDigest::from_canon`] —
/// see `Lockfile::parse`'s self-check — never trusted on its own.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    pub fn from_canon(digest: Digest) -> Self {
        ContentDigest(*digest.as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        s
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut bytes = [0u8; 32];
        let chars: Vec<char> = s.chars().collect();
        for (i, pair) in chars.chunks(2).enumerate() {
            let hi = pair[0].to_digit(16)?;
            let lo = pair[1].to_digit(16)?;
            bytes[i] = ((hi << 4) | lo) as u8;
        }
        Some(ContentDigest(bytes))
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentDigest({})", self.to_hex())
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Canonical byte encoding of a file tree: every `(relative path, bytes)` pair,
/// sorted by path (the caller passes a `BTreeMap` so this is enforced at the type
/// level, not by a runtime sort the reviewer has to trust), length-prefixed and
/// concatenated. This is both the registry's on-disk blob format
/// ([`crate::registry`] stores exactly these bytes, addressed by their own
/// digest) and the input to [`tree_digest`] — one encoding serves both jobs, so
/// "unpack the blob" and "recompute its digest" can never disagree.
pub fn pack(files: &BTreeMap<Utf8PathBuf, Vec<u8>>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_uint(files.len() as u64);
    for (path, bytes) in files {
        w.write_bytes(path.as_str().as_bytes());
        w.write_bytes(bytes);
    }
    w.finish()
}

/// Inverse of [`pack`].
pub fn unpack(bytes: &[u8]) -> Result<BTreeMap<Utf8PathBuf, Vec<u8>>, UnpackError> {
    let mut r = CanonReader::new(bytes);
    let count = r.read_uint().map_err(UnpackError::Canon)?;
    let mut files = BTreeMap::new();
    for _ in 0..count {
        let path = r.read_bytes().map_err(UnpackError::Canon)?;
        let content = r.read_bytes().map_err(UnpackError::Canon)?;
        let path = std::str::from_utf8(path).map_err(|_| UnpackError::BadUtf8)?;
        files.insert(Utf8PathBuf::from(path), content.to_vec());
    }
    if !r.is_empty() {
        return Err(UnpackError::Canon(CanonError::BadLength));
    }
    Ok(files)
}

/// Errors unpacking a registry blob back into a file tree.
#[derive(Debug, PartialEq, Eq)]
pub enum UnpackError {
    Canon(CanonError),
    BadUtf8,
}

impl fmt::Display for UnpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnpackError::Canon(e) => write!(f, "malformed package blob: {e:?}"),
            UnpackError::BadUtf8 => write!(f, "malformed package blob: non-UTF-8 path"),
        }
    }
}

impl std::error::Error for UnpackError {}

/// Canonical content digest of a file tree — [`pack`] hashed in [`Domain::Value`].
/// This is the identity used by the content-addressed local registry
/// ([`crate::registry`]) and by lockfile entries ([`crate::lock`]).
pub fn tree_digest(files: &BTreeMap<Utf8PathBuf, Vec<u8>>) -> ContentDigest {
    ContentDigest::from_canon(Digest::of(Domain::Value, &pack(files)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_order_independent_of_insertion() {
        let mut a = BTreeMap::new();
        a.insert(Utf8PathBuf::from("b.brix"), b"two".to_vec());
        a.insert(Utf8PathBuf::from("a.brix"), b"one".to_vec());

        let mut b = BTreeMap::new();
        b.insert(Utf8PathBuf::from("a.brix"), b"one".to_vec());
        b.insert(Utf8PathBuf::from("b.brix"), b"two".to_vec());

        assert_eq!(tree_digest(&a), tree_digest(&b));
    }

    #[test]
    fn digest_changes_with_content() {
        let mut a = BTreeMap::new();
        a.insert(Utf8PathBuf::from("a.brix"), b"one".to_vec());
        let mut b = BTreeMap::new();
        b.insert(Utf8PathBuf::from("a.brix"), b"two".to_vec());
        assert_ne!(tree_digest(&a), tree_digest(&b));
    }

    #[test]
    fn digest_changes_with_path() {
        let mut a = BTreeMap::new();
        a.insert(Utf8PathBuf::from("a.brix"), b"one".to_vec());
        let mut b = BTreeMap::new();
        b.insert(Utf8PathBuf::from("a2.brix"), b"one".to_vec());
        assert_ne!(tree_digest(&a), tree_digest(&b));
    }

    #[test]
    fn hex_roundtrips() {
        let mut a = BTreeMap::new();
        a.insert(Utf8PathBuf::from("a.brix"), b"one".to_vec());
        let d = tree_digest(&a);
        let hex = d.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ContentDigest::from_hex(&hex), Some(d));
    }

    #[test]
    fn hex_rejects_garbage() {
        assert_eq!(ContentDigest::from_hex("too-short"), None);
        assert_eq!(ContentDigest::from_hex(&"zz".repeat(32)), None);
    }

    #[test]
    fn pack_unpack_roundtrips_and_digest_matches() {
        let mut files = BTreeMap::new();
        files.insert(
            Utf8PathBuf::from("world.brix"),
            b"package a @ 1.0.0".to_vec(),
        );
        files.insert(Utf8PathBuf::from("OWNER.md"), b"# Owner".to_vec());
        let blob = pack(&files);
        let unpacked = unpack(&blob).unwrap();
        assert_eq!(files, unpacked);
        assert_eq!(
            tree_digest(&files),
            ContentDigest::from_canon(Digest::of(Domain::Value, &blob))
        );
    }
}
