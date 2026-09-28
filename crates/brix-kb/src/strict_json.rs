//! A small, strict, duplicate-key-rejecting JSON reader for `brix-kb`'s own
//! on-disk files (`kb.json`, `HEAD`, `revisions/<seq>.json`).
//!
//! `brix-lower::input` already has a strict duplicate-key-rejecting JSON
//! parser (`StrictJsonParser`), but it is private and shaped exactly for the
//! `brix.input@1`/`@2` tagged-value envelope, not for `brix-kb`'s own record
//! schemas — so this module is a small, independent equivalent, in the same
//! spirit: bounded, no dependence on a generic deserializer's silent
//! last-write-wins duplicate-key behavior (`serde_json`'s default `Value`/
//! struct decoding does not reject duplicate object keys), unknown fields
//! rejected, and no floats (every number in these schemas is a bounded
//! non-negative integer literal).
//!
//! This module only *decodes*. Encoding uses `serde_json` directly (see
//! `revision.rs`/`manifest.rs`) since `brix-kb` writes its own files and there
//! is no adversarial input to guard against on the write path.

use std::fmt;

/// Maximum size of a single `brix-kb` metadata file (manifest, `HEAD`, or one
/// revision record) that will be parsed. Generously larger than any record
/// this schema can produce, while still ruling out unbounded reads of a
/// corrupted or hostile file.
pub const MAX_KB_JSON_BYTES: usize = 1024 * 1024;

/// Maximum nesting depth of objects/arrays.
const MAX_DEPTH: usize = 16;

/// A strictly-decoded JSON value: no floats, duplicate object keys rejected
/// at parse time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Value>),
    /// Insertion-ordered; construction rejects a repeated key.
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Obj(fields) => Some(fields),
            _ => None,
        }
    }

    /// Look up a field of an object by name.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_obj()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// Assert this object has *exactly* the given field names (order-free);
    /// any other field present is an unknown-field rejection.
    pub fn require_only_keys(&self, allowed: &[&str]) -> Result<(), JsonError> {
        let fields = self
            .as_obj()
            .ok_or(JsonError::TypeMismatch { expected: "object" })?;
        for (k, _) in fields {
            if !allowed.contains(&k.as_str()) {
                return Err(JsonError::UnknownField(k.clone()));
            }
        }
        Ok(())
    }

    /// A required string field.
    pub fn field_str(&self, key: &'static str) -> Result<&str, JsonError> {
        self.get(key)
            .ok_or(JsonError::MissingField(key))?
            .as_str()
            .ok_or(JsonError::FieldTypeMismatch {
                field: key,
                expected: "string",
            })
    }

    /// An optional string field (absent or JSON `null` both map to `None`).
    pub fn field_opt_str(&self, key: &'static str) -> Result<Option<&str>, JsonError> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_str().map(Some).ok_or(JsonError::FieldTypeMismatch {
                field: key,
                expected: "string or null",
            }),
        }
    }

    /// A required non-negative integer field.
    pub fn field_u64(&self, key: &'static str) -> Result<u64, JsonError> {
        let n = self
            .get(key)
            .ok_or(JsonError::MissingField(key))?
            .as_i64()
            .ok_or(JsonError::FieldTypeMismatch {
                field: key,
                expected: "non-negative integer",
            })?;
        u64::try_from(n).map_err(|_| JsonError::FieldTypeMismatch {
            field: key,
            expected: "non-negative integer",
        })
    }

    /// A required array-of-string field.
    pub fn field_str_array(&self, key: &'static str) -> Result<Vec<String>, JsonError> {
        let items = self
            .get(key)
            .ok_or(JsonError::MissingField(key))?
            .as_arr()
            .ok_or(JsonError::FieldTypeMismatch {
                field: key,
                expected: "array",
            })?;
        items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or(JsonError::FieldTypeMismatch {
                        field: key,
                        expected: "array of strings",
                    })
            })
            .collect()
    }

    /// A required object field.
    pub fn field_obj(&self, key: &'static str) -> Result<&Value, JsonError> {
        let v = self.get(key).ok_or(JsonError::MissingField(key))?;
        if v.as_obj().is_some() {
            Ok(v)
        } else {
            Err(JsonError::FieldTypeMismatch {
                field: key,
                expected: "object",
            })
        }
    }
}

/// Strict JSON decode errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonError {
    TooLarge {
        limit: usize,
        found: usize,
    },
    Io(String),
    NotUtf8,
    Syntax {
        message: String,
        offset: usize,
    },
    UnexpectedEof,
    TrailingBytes {
        offset: usize,
    },
    DepthExceeded,
    DuplicateKey {
        key: String,
        offset: usize,
    },
    UnknownField(String),
    MissingField(&'static str),
    TypeMismatch {
        expected: &'static str,
    },
    FieldTypeMismatch {
        field: &'static str,
        expected: &'static str,
    },
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonError::TooLarge { limit, found } => {
                write!(f, "file size ({found} bytes) exceeds limit ({limit} bytes)")
            }
            JsonError::Io(msg) => write!(f, "I/O error: {msg}"),
            JsonError::NotUtf8 => write!(f, "file is not valid UTF-8"),
            JsonError::Syntax { message, offset } => {
                write!(f, "JSON syntax error at byte offset {offset}: {message}")
            }
            JsonError::UnexpectedEof => write!(f, "unexpected end of JSON input"),
            JsonError::TrailingBytes { offset } => {
                write!(f, "trailing bytes after JSON value at offset {offset}")
            }
            JsonError::DepthExceeded => write!(f, "JSON nesting depth limit exceeded"),
            JsonError::DuplicateKey { key, offset } => {
                write!(f, "duplicate JSON key '{key}' at byte offset {offset}")
            }
            JsonError::UnknownField(field) => write!(f, "unknown field '{field}'"),
            JsonError::MissingField(field) => write!(f, "missing required field '{field}'"),
            JsonError::TypeMismatch { expected } => write!(f, "expected {expected}"),
            JsonError::FieldTypeMismatch { field, expected } => {
                write!(f, "field '{field}': expected {expected}")
            }
        }
    }
}

impl std::error::Error for JsonError {}

/// Parse a complete strict JSON document from `bytes` (already bounded by the
/// caller), rejecting duplicate object keys, unrecognized escapes, and
/// trailing content.
pub fn parse_document(bytes: &[u8]) -> Result<Value, JsonError> {
    if bytes.len() > MAX_KB_JSON_BYTES {
        return Err(JsonError::TooLarge {
            limit: MAX_KB_JSON_BYTES,
            found: bytes.len(),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| JsonError::NotUtf8)?;
    let mut p = Parser {
        bytes: text.as_bytes(),
        text,
        pos: 0,
        depth: 0,
    };
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(JsonError::TrailingBytes { offset: p.pos });
    }
    Ok(value)
}

/// Read and strictly decode a `brix-kb` metadata file from disk.
pub fn read_and_parse(path: &std::path::Path) -> Result<Value, JsonError> {
    let meta = std::fs::metadata(path).map_err(|e| JsonError::Io(e.to_string()))?;
    if meta.len() > MAX_KB_JSON_BYTES as u64 {
        return Err(JsonError::TooLarge {
            limit: MAX_KB_JSON_BYTES,
            found: meta.len() as usize,
        });
    }
    let bytes = std::fs::read(path).map_err(|e| JsonError::Io(e.to_string()))?;
    parse_document(&bytes)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.bytes.get(self.pos) {
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<Value, JsonError> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::Str(self.parse_string()?)),
            Some(b't') => {
                self.expect_lit("true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.expect_lit("false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.expect_lit("null")?;
                Ok(Value::Null)
            }
            Some(b'-') | Some(b'0'..=b'9') => self.parse_int(),
            Some(other) => Err(JsonError::Syntax {
                message: format!("unexpected byte '{}'", other as char),
                offset: self.pos,
            }),
            None => Err(JsonError::UnexpectedEof),
        }
    }

    fn expect_lit(&mut self, lit: &str) -> Result<(), JsonError> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(())
        } else {
            Err(JsonError::Syntax {
                message: format!("expected literal '{lit}'"),
                offset: self.pos,
            })
        }
    }

    fn parse_int(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == digits_start {
            return Err(JsonError::Syntax {
                message: "expected digit".to_string(),
                offset: self.pos,
            });
        }
        // No fractional/exponent parts are admitted in this schema: every
        // number here is a bounded integer, and admitting floats would smuggle
        // non-canonical numeric forms into an identity-bearing file.
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err(JsonError::Syntax {
                message: "floating-point numbers are not admitted in brix-kb metadata".to_string(),
                offset: self.pos,
            });
        }
        let raw = &self.text[start..self.pos];
        raw.parse::<i64>()
            .map(Value::Int)
            .map_err(|_| JsonError::Syntax {
                message: format!("integer literal '{raw}' out of range or malformed"),
                offset: start,
            })
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        let mut s = String::new();
        loop {
            let Some(b) = self.bytes.get(self.pos).copied() else {
                return Err(JsonError::UnexpectedEof);
            };
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(s);
                }
                b'\\' => {
                    self.pos += 1;
                    let Some(esc) = self.bytes.get(self.pos).copied() else {
                        return Err(JsonError::UnexpectedEof);
                    };
                    self.pos += 1;
                    match esc {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'r' => s.push('\r'),
                        b'u' => {
                            if self.pos + 4 > self.bytes.len() {
                                return Err(JsonError::UnexpectedEof);
                            }
                            let hex = &self.text[self.pos..self.pos + 4];
                            let code =
                                u32::from_str_radix(hex, 16).map_err(|_| JsonError::Syntax {
                                    message: "invalid \\u escape".to_string(),
                                    offset: self.pos,
                                })?;
                            self.pos += 4;
                            // Only used for our own ASCII-safe metadata; BMP
                            // scalars (no surrogate pairs) are all this schema
                            // ever needs to round-trip.
                            let ch = char::from_u32(code).ok_or(JsonError::Syntax {
                                message: "invalid unicode scalar in \\u escape".to_string(),
                                offset: self.pos,
                            })?;
                            s.push(ch);
                        }
                        other => {
                            return Err(JsonError::Syntax {
                                message: format!("unrecognized escape '\\{}'", other as char),
                                offset: self.pos - 1,
                            })
                        }
                    }
                }
                b if b < 0x20 => {
                    return Err(JsonError::Syntax {
                        message: "control character in string".to_string(),
                        offset: self.pos,
                    })
                }
                _ => {
                    let ch = self.text[self.pos..].chars().next().unwrap();
                    s.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn enter(&mut self) -> Result<(), JsonError> {
        if self.depth >= MAX_DEPTH {
            return Err(JsonError::DepthExceeded);
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn parse_object(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        self.pos += 1; // consume '{'
        let mut fields: Vec<(String, Value)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave();
            return Ok(Value::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key_offset = self.pos;
            if self.peek() != Some(b'"') {
                return Err(JsonError::Syntax {
                    message: "expected string key".to_string(),
                    offset: self.pos,
                });
            }
            let key = self.parse_string()?;
            if fields.iter().any(|(k, _)| k == &key) {
                return Err(JsonError::DuplicateKey {
                    key,
                    offset: key_offset,
                });
            }
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(JsonError::Syntax {
                    message: "expected ':'".to_string(),
                    offset: self.pos,
                });
            }
            self.pos += 1;
            let value = self.parse_value()?;
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    self.leave();
                    return Ok(Value::Obj(fields));
                }
                Some(_) => {
                    return Err(JsonError::Syntax {
                        message: "expected ',' or '}'".to_string(),
                        offset: self.pos,
                    })
                }
                None => return Err(JsonError::UnexpectedEof),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        self.pos += 1; // consume '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.leave();
            return Ok(Value::Arr(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    self.leave();
                    return Ok(Value::Arr(items));
                }
                Some(_) => {
                    return Err(JsonError::Syntax {
                        message: "expected ',' or ']'".to_string(),
                        offset: self.pos,
                    })
                }
                None => return Err(JsonError::UnexpectedEof),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_flat_object() {
        let v = parse_document(br#"{"a": 1, "b": "x", "c": true, "d": null}"#).unwrap();
        assert_eq!(v.field_u64("a").unwrap(), 1);
        assert_eq!(v.field_str("b").unwrap(), "x");
        assert_eq!(v.get("c").unwrap().as_bool(), Some(true));
        assert_eq!(v.field_opt_str("d").unwrap(), None);
    }

    #[test]
    fn test_rejects_duplicate_keys() {
        let err = parse_document(br#"{"a": 1, "a": 2}"#).unwrap_err();
        assert!(matches!(err, JsonError::DuplicateKey { .. }));
    }

    #[test]
    fn test_rejects_unknown_field() {
        let v = parse_document(br#"{"a": 1}"#).unwrap();
        let err = v.require_only_keys(&["b"]).unwrap_err();
        assert_eq!(err, JsonError::UnknownField("a".to_string()));
    }

    #[test]
    fn test_rejects_trailing_bytes() {
        let err = parse_document(br#"{"a": 1} garbage"#).unwrap_err();
        assert!(matches!(err, JsonError::TrailingBytes { .. }));
    }

    #[test]
    fn test_rejects_float() {
        let err = parse_document(br#"{"a": 1.5}"#).unwrap_err();
        assert!(matches!(err, JsonError::Syntax { .. }));
    }

    #[test]
    fn test_string_array_field() {
        let v = parse_document(br#"{"names": ["x", "y"]}"#).unwrap();
        assert_eq!(
            v.field_str_array("names").unwrap(),
            vec!["x".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn test_nested_object_field() {
        let v = parse_document(br#"{"outer": {"inner": 1}}"#).unwrap();
        let inner = v.field_obj("outer").unwrap();
        assert_eq!(inner.field_u64("inner").unwrap(), 1);
    }
}
