//! Reading fields out of RPC request parameters (`serde_json::Value`).

use crate::ids::RunId;
use crate::output::{CliError, CliResult};
use serde_json::{Map, Value};

/// An optional object field: absent and `null` are `None`.
pub fn opt_object<'a>(p: &'a Value, key: &str) -> CliResult<Option<&'a Map<String, Value>>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(o)) => Ok(Some(o)),
        Some(_) => Err(CliError::invalid(format!("`{key}` must be an object"))),
    }
}

/// An optional boolean field, `default` when absent or `null`.
pub fn opt_flag(p: &Value, key: &str, default: bool) -> CliResult<bool> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(CliError::invalid(format!("`{key}` must be a boolean"))),
    }
}

pub fn internal(e: anyhow::Error) -> CliError {
    CliError::internal(format!("{e:#}"))
}

pub fn req_id(p: &Value) -> CliResult<RunId> {
    req_id_at(p, "id")
}

/// A run id given as a number or a string like `42` / `#42`.
pub fn req_id_at(p: &Value, key: &str) -> CliResult<RunId> {
    req_num_at(p, key).map(RunId::new)
}

/// A numeric id given as a number or a string like `42` / `#42`.
pub fn req_num_at(p: &Value, key: &str) -> CliResult<i64> {
    match p.get(key) {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    }
    .ok_or_else(|| CliError::invalid(format!("missing or invalid run `{key}`")))
}

pub fn req_str<'a>(p: &'a Value, key: &str) -> CliResult<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::invalid(format!("missing string `{key}`")))
}

pub fn opt_str<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}

/// An optional string field: absent and `null` are `None`, anything else
/// that isn't a string is an error.
pub fn opt_string(p: &Value, key: &str) -> CliResult<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(CliError::invalid(format!("`{key}` must be a string"))),
    }
}
