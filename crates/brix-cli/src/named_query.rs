//! Named reads with a host-owned scope supplied separately from query arguments.

use std::collections::BTreeMap;
use std::path::Path;

use brix_kb::world::{
    NamedQueryArgsV1, QueryValueV1, ScopedQueryCursorV1, ScopedQueryPageRequestV1, WorldSession,
};
use serde_json::{json, Value};

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS};
use crate::host_context::{load_host_query_policy, HostQueryPolicy, MAX_HOST_QUERY_CONFIG_BYTES};

const RESULT_SCHEMA: &str = "brix.world.named-query-result@1";

fn arguments(value: &Value) -> Result<NamedQueryArgsV1, String> {
    let object = value.as_object().ok_or("args must be an object")?;
    let mut result = BTreeMap::new();
    for (name, value) in object {
        let value = match value {
            Value::String(s) => QueryValueV1::Str(s.clone()),
            Value::Bool(b) => QueryValueV1::Bool(*b),
            Value::Number(n) => QueryValueV1::Int(n.as_i64().ok_or_else(|| {
                format!("argument {name} must be an Int within signed 64-bit range")
            })?),
            _ => return Err(format!("argument {name} must be Str, Int or Bool")),
        };
        result.insert(name.clone(), value);
    }
    Ok(NamedQueryArgsV1(result))
}

fn execute(
    dir: &Path,
    name: &str,
    args: &Value,
    policy: &HostQueryPolicy,
    rev: Option<u64>,
    limit: Option<usize>,
    cursor: Option<&str>,
) -> Result<Value, String> {
    let args = arguments(args)?;
    let session = WorldSession::open_read_only(dir).map_err(|e| e.to_string())?;
    let cursor_revision = cursor
        .map(|token| ScopedQueryCursorV1::decode_token(token).map(|c| c.revision_seq()))
        .transpose()
        .map_err(|e| format!("invalid scoped query cursor: {e:?}"))?;
    let revision = rev
        .or(cursor_revision)
        .unwrap_or(session.current_revision());
    let scope = policy.authorize(&session, revision)?;
    let caps = scope.query_limits();
    let request = ScopedQueryPageRequestV1 {
        query_name: name.to_owned(),
        arguments: args,
        revision: Some(revision),
        limit: limit.unwrap_or(caps.page_rows),
        cursor: cursor.map(str::to_owned),
        max_input_rows: caps.max_input_rows,
        max_input_bytes: caps.max_input_bytes,
        max_eval_work: caps.max_eval_work,
        max_eval_matches: caps.max_eval_matches,
        max_output_rows: caps.max_output_rows,
    };
    let page = session
        .query_named_scoped_page(&request, &scope)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "schema": RESULT_SCHEMA, "command": "world.query-named", "ok": true,
        "query": name, "query_identity": page.query_identity.to_hex(),
        "context_identity": scope.context_id().digest().to_hex(), "scope_identity": scope.identity().to_hex(),
        "revision": page.revision_seq, "revision_digest": page.revision_digest.to_hex(),
        "count": page.typed_rows.len(), "rows": page.typed_rows,
        "has_more": page.has_more, "next_cursor": page.next_cursor,
    }))
}

fn rejected(message: String) -> (u8, Value) {
    (
        EXIT_REJECTED_OR_UNKNOWN,
        json!({
            "schema": RESULT_SCHEMA, "command": "world.query-named", "ok": false,
            "status": "Rejected", "error": message,
        }),
    )
}

/// Call only after establishing that the stdio peer is the trusted host.
/// The policy is deliberately a separate envelope field from untrusted args.
pub fn dispatch_host_json(params: &Value) -> Result<(u8, Value), String> {
    let p = params.as_object().ok_or("params must be an object")?;
    const KEYS: &[&str] = &[
        "dir",
        "name",
        "args",
        "host_policy",
        "rev",
        "limit",
        "cursor",
    ];
    if let Some(key) = p.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(format!("unknown named query parameter: {key}"));
    }
    let required_str = |key: &str| {
        p.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{key} must be a string"))
    };
    let dir = Path::new(required_str("dir")?);
    let name = required_str("name")?;
    let policy =
        HostQueryPolicy::from_json(p.get("host_policy").ok_or("host_policy is required")?)?;
    let optional_uint = |key: &str| -> Result<Option<u64>, String> {
        p.get(key)
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| format!("{key} must be a nonnegative integer"))
            })
            .transpose()
    };
    let rev = optional_uint("rev")?;
    let limit = optional_uint("limit")?
        .map(usize::try_from)
        .transpose()
        .map_err(|_| "limit out of range")?;
    let cursor = p
        .get("cursor")
        .map(|v| v.as_str().ok_or("cursor must be a string"))
        .transpose()?;
    let empty = json!({});
    Ok(
        match execute(
            dir,
            name,
            p.get("args").unwrap_or(&empty),
            &policy,
            rev,
            limit,
            cursor,
        ) {
            Ok(value) => (EXIT_SUCCESS, value),
            Err(message) => rejected(message),
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_cli(
    dir: &Path,
    name: &str,
    args: &str,
    host_scope: &Path,
    rev: Option<u64>,
    limit: Option<usize>,
    cursor: Option<&str>,
    json_out: bool,
) -> u8 {
    let result = (|| {
        if args.len() > MAX_HOST_QUERY_CONFIG_BYTES {
            return Err("query arguments exceed byte limit".to_string());
        }
        let args = crate::host_context::parse_unique_json(args)?;
        let policy = load_host_query_policy(host_scope)?;
        execute(dir, name, &args, &policy, rev, limit, cursor)
    })();
    let (code, value) = match result {
        Ok(value) => (EXIT_SUCCESS, value),
        Err(message) => rejected(message),
    };
    if json_out {
        crate::json::emit_result_json(&value);
    } else if code == EXIT_SUCCESS {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("JSON value")
        );
    } else {
        eprintln!(
            "{}",
            value["error"].as_str().unwrap_or("named query rejected")
        );
    }
    code
}
