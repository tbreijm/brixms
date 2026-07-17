//! A deliberately small, deterministic transaction-stream boundary for
//! generated binaries.
//!
//! Each non-empty line is `+ <relation> <hex-row>` (assert) or
//! `- <relation> <hex-row>` (retract). Blank lines terminate a transaction;
//! comments begin with `#`. The format is line-oriented so a generated
//! command can consume stdin without a second semantic serializer: row bytes
//! are already canonical payloads and output is the runtime's canonical dump.

use std::fmt;

use brix_canon::Digest;

use crate::delta::CanonRow;
use crate::ids::DataRevision;
use crate::scheduler::{dump_bytes, dump_digest, Scheduler, Transaction};

/// One published dump from a transaction stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionDump {
    pub revision: DataRevision,
    pub digest: Digest,
    pub bytes: Vec<u8>,
}

/// Parse and execute a complete input stream, publishing one settled result
/// per blank-line-delimited transaction.
pub fn run_text(scheduler: &mut Scheduler, input: &str) -> Result<Vec<RevisionDump>, StreamError> {
    let mut output = Vec::new();
    let mut transaction = Transaction::new(stream_intent(0));
    let mut ordinal = 0usize;
    let mut has_operations = false;

    for (line_number, raw_line) in input.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            if has_operations {
                output.push(commit(scheduler, transaction));
                ordinal += 1;
                transaction = Transaction::new(stream_intent(ordinal));
                has_operations = false;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_ascii_whitespace();
        let operation = fields.next().ok_or(StreamError::InvalidLine {
            line: line_number + 1,
            message: "missing operation",
        })?;
        let relation = fields.next().ok_or(StreamError::InvalidLine {
            line: line_number + 1,
            message: "missing relation",
        })?;
        let payload = fields.next().ok_or(StreamError::InvalidLine {
            line: line_number + 1,
            message: "missing hexadecimal canon row",
        })?;
        if fields.next().is_some() {
            return Err(StreamError::InvalidLine {
                line: line_number + 1,
                message: "expected exactly an operation, relation, and row",
            });
        }
        let row = CanonRow(
            decode_hex(payload).map_err(|message| StreamError::InvalidLine {
                line: line_number + 1,
                message,
            })?,
        );
        transaction = match operation {
            "+" => transaction.assert(relation, row),
            "-" => transaction.retract(relation, row),
            _ => {
                return Err(StreamError::InvalidLine {
                    line: line_number + 1,
                    message: "operation must be `+` or `-`",
                });
            }
        };
        has_operations = true;
    }
    if has_operations {
        output.push(commit(scheduler, transaction));
    }
    Ok(output)
}

/// Render output as `revision digest hex-dump`, one line per published
/// revision. The dump field is exact canonical bytes encoded as lowercase hex.
pub fn render_dumps(dumps: &[RevisionDump]) -> String {
    let mut output = String::new();
    for dump in dumps {
        output.push_str(&format!(
            "{} {} {}\n",
            dump.revision.0,
            dump.digest.to_hex(),
            encode_hex(&dump.bytes)
        ));
    }
    output
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamError {
    InvalidLine { line: usize, message: &'static str },
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLine { line, message } => write!(f, "stream line {line}: {message}"),
        }
    }
}

impl std::error::Error for StreamError {}

fn commit(scheduler: &mut Scheduler, transaction: Transaction) -> RevisionDump {
    let settled = scheduler.commit(transaction);
    RevisionDump {
        revision: settled.revision,
        digest: dump_digest(settled),
        bytes: dump_bytes(settled),
    }
}

fn stream_intent(ordinal: usize) -> Vec<u8> {
    format!("brix-stdin-{ordinal}").into_bytes()
}

fn decode_hex(input: &str) -> Result<Vec<u8>, &'static str> {
    if !input.len().is_multiple_of(2) {
        return Err("hex row must have an even number of characters");
    }
    let mut bytes = Vec::with_capacity(input.len() / 2);
    let mut pairs = input.as_bytes().chunks_exact(2);
    for pair in &mut pairs {
        let high = hex_nibble(pair[0]).ok_or("hex row contains a non-hex character")?;
        let low = hex_nibble(pair[1]).ok_or("hex row contains a non-hex character")?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_commits_blank_line_delimited_transactions() {
        let mut scheduler = Scheduler::new();
        let dumps = run_text(&mut scheduler, "+ Input 6162\n\n+ Input 63\n").unwrap();
        assert_eq!(dumps.len(), 2);
        assert_eq!(dumps[0].revision, DataRevision(1));
        assert_eq!(dumps[1].revision, DataRevision(2));
        assert!(render_dumps(&dumps).contains(&dumps[0].digest.to_hex()));
    }

    #[test]
    fn stream_rejects_malformed_rows() {
        let error = run_text(&mut Scheduler::new(), "+ Input z").unwrap_err();
        assert_eq!(
            error.to_string(),
            "stream line 1: hex row must have an even number of characters"
        );
    }
}
