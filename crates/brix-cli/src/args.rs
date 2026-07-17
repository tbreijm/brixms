//! A tiny hand-rolled argument parser.
//!
//! Deliberately not a dependency: the CLI's surface is a fixed, small set of
//! verbs (Part XIII §2's `brix` contract), and the Ring 0 whitelist bar is high
//! (DEPS.md) — a `clap` would be more code to audit than the parser it replaces.
//! This handles exactly what the verbs need: a verb, positional operands, and
//! `--flag` / `--key value` / `--key=value` options. Unknown options are an error
//! (fail closed), not silently ignored.

use std::collections::BTreeMap;

/// A parsed command line: the verb plus its operands and options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedArgs {
    pub verb: String,
    pub positionals: Vec<String>,
    /// `--key value`, `--key=value`, or bare `--flag` (stored with value `""`).
    pub options: BTreeMap<String, String>,
}

/// The top-level parse result: either a verb invocation, or one of the two
/// zero-verb intents (`--help` / `--version`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invocation {
    Help,
    Version,
    Verb(ParsedArgs),
}

/// An argument-parsing error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgError {
    /// No verb given (and no `--help`/`--version`).
    NoVerb,
    /// An option expecting a value was the last token.
    MissingValue { option: String },
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgError::NoVerb => write!(f, "no command given (try `brix --help`)"),
            ArgError::MissingValue { option } => {
                write!(f, "option `--{option}` expects a value")
            }
        }
    }
}

impl std::error::Error for ArgError {}

/// Options that take a following value (`--key value`) rather than being bare
/// boolean flags. Kept explicit so `brix why --rust` (a flag) and `brix explain
/// --rule Foo` (a value option) parse unambiguously without a schema per verb.
fn is_value_option(key: &str) -> bool {
    matches!(
        key,
        "rule"
            | "manifest"
            | "name"
            | "at"
            | "seed"
            | "profile"
            | "target"
            | "registry"
            | "diagnostic-format"
    )
}

/// Parse `args` (excluding argv[0]).
pub fn parse(args: &[String]) -> Result<Invocation, ArgError> {
    let mut iter = args.iter().peekable();

    // Leading global flags with no verb.
    match iter.peek().map(|s| s.as_str()) {
        None => return Err(ArgError::NoVerb),
        Some("--help" | "-h" | "help") => return Ok(Invocation::Help),
        Some("--version" | "-V") => return Ok(Invocation::Version),
        _ => {}
    }

    let verb = iter.next().expect("peeked Some above").clone();
    let mut positionals = Vec::new();
    let mut options = BTreeMap::new();

    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((key, value)) = rest.split_once('=') {
                options.insert(key.to_string(), value.to_string());
            } else if is_value_option(rest) {
                let value = iter
                    .next()
                    .ok_or_else(|| ArgError::MissingValue {
                        option: rest.to_string(),
                    })?
                    .clone();
                options.insert(rest.to_string(), value);
            } else {
                // Bare flag.
                options.insert(rest.to_string(), String::new());
            }
        } else {
            positionals.push(arg.clone());
        }
    }

    Ok(Invocation::Verb(ParsedArgs {
        verb,
        positionals,
        options,
    }))
}

impl ParsedArgs {
    /// First positional operand, if any (usually a path or a package name).
    pub fn operand(&self) -> Option<&str> {
        self.positionals.first().map(|s| s.as_str())
    }

    /// Whether a bare flag was given.
    pub fn flag(&self, key: &str) -> bool {
        self.options.contains_key(key)
    }

    /// A value option's value, if present.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.options.get(key).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_verb_and_positional() {
        let inv = parse(&args(&["build", "world.brix"])).unwrap();
        let Invocation::Verb(p) = inv else {
            panic!("expected verb")
        };
        assert_eq!(p.verb, "build");
        assert_eq!(p.operand(), Some("world.brix"));
    }

    #[test]
    fn parses_bare_flag_and_value_option() {
        let inv = parse(&args(&["explain", "--rust", "--rule", "Waiting"])).unwrap();
        let Invocation::Verb(p) = inv else {
            panic!("expected verb")
        };
        assert!(p.flag("rust"));
        assert_eq!(p.value("rule"), Some("Waiting"));
    }

    #[test]
    fn parses_key_equals_value() {
        let inv = parse(&args(&["run", "--profile=serve"])).unwrap();
        let Invocation::Verb(p) = inv else {
            panic!("expected verb")
        };
        assert_eq!(p.value("profile"), Some("serve"));
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse(&args(&["--help"])).unwrap(), Invocation::Help);
        assert_eq!(parse(&args(&["help"])).unwrap(), Invocation::Help);
        assert_eq!(parse(&args(&["--version"])).unwrap(), Invocation::Version);
    }

    #[test]
    fn empty_is_no_verb() {
        assert_eq!(parse(&[]), Err(ArgError::NoVerb));
    }

    #[test]
    fn value_option_at_end_errors() {
        assert_eq!(
            parse(&args(&["explain", "--rule"])),
            Err(ArgError::MissingValue {
                option: "rule".into()
            })
        );
    }

    #[test]
    fn parses_diagnostic_format() {
        let inv = parse(&args(&[
            "build",
            "world.brix",
            "--diagnostic-format",
            "sarif",
        ]))
        .unwrap();
        let Invocation::Verb(p) = inv else {
            panic!("expected verb")
        };
        assert_eq!(p.value("diagnostic-format"), Some("sarif"));
    }
}
