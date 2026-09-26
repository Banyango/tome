//! `tome query "<sql>"`: read-only SQL against the run store, executed by the
//! daemon (the only process allowed to open the database).
//!
//! Read-only is enforced three ways:
//! 1. only a single statement starting with a read keyword is accepted;
//! 2. it runs inside a transaction that is always rolled back;
//! 3. the database is opened with external access disabled, so SQL can't
//!    read or write other files.

use crate::output::{CliError, CliResult};
use crate::store::Store;
use chrono::{DateTime, NaiveDate, NaiveTime};
use duckdb::types::Value as Db;
use serde_json::{json, Value};

const READ_KEYWORDS: &[&str] = &["select", "with", "from", "values", "show", "describe", "summarize", "explain", "table"];

#[derive(Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

pub fn run(store: &mut Store, sql: &str) -> CliResult<QueryResult> {
    check_read_only(sql)?;
    let tx = store.transaction().map_err(db_err)?;
    let result = (|| -> duckdb::Result<QueryResult> {
        let mut stmt = tx.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let columns = rows.as_ref().map(|s| s.column_names()).unwrap_or_default();
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut values = Vec::with_capacity(columns.len());
            for i in 0..columns.len() {
                values.push(to_json(row.get::<_, Db>(i)?));
            }
            out.push(values);
        }
        Ok(QueryResult { columns, rows: out })
    })();
    // Always roll back, whatever the statement did.
    tx.rollback().map_err(db_err)?;
    result.map_err(|e| CliError::invalid(format!("query failed: {e}")))
}

fn db_err(e: duckdb::Error) -> CliError {
    CliError::internal(format!("database error: {e}"))
}

pub fn check_read_only(sql: &str) -> CliResult<()> {
    let stripped = strip_comments(sql);
    let trimmed = stripped.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(CliError::invalid("query is empty"));
    }
    if has_statement_separator(trimmed) {
        return Err(CliError::invalid("only a single statement is allowed"));
    }
    let first = trimmed
        .split(|c: char| c.is_whitespace() || c == '(')
        .find(|w| !w.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !READ_KEYWORDS.contains(&first.as_str()) {
        return Err(CliError::invalid(format!(
            "only read-only queries are allowed (statement starts with `{first}`; use one of: {})",
            READ_KEYWORDS.join(", ")
        )));
    }
    Ok(())
}

/// Remove `--` and `/* */` comments outside of string literals.
fn strip_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            out.push(c);
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            out.push(' ');
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn has_statement_separator(sql: &str) -> bool {
    let mut quote: Option<char> = None;
    for c in sql.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == ';' => return true,
            None => {}
        }
    }
    false
}

fn to_json(v: Db) -> Value {
    match v {
        Db::Null => Value::Null,
        Db::Boolean(b) => json!(b),
        Db::TinyInt(n) => json!(n),
        Db::SmallInt(n) => json!(n),
        Db::Int(n) => json!(n),
        Db::BigInt(n) => json!(n),
        Db::UTinyInt(n) => json!(n),
        Db::USmallInt(n) => json!(n),
        Db::UInt(n) => json!(n),
        Db::UBigInt(n) => json!(n),
        // 128-bit integers don't fit JSON numbers reliably.
        Db::HugeInt(n) => i64::try_from(n).map(|n| json!(n)).unwrap_or_else(|_| json!(n.to_string())),
        Db::UHugeInt(n) => u64::try_from(n).map(|n| json!(n)).unwrap_or_else(|_| json!(n.to_string())),
        Db::Float(f) => json!(f),
        Db::Double(f) => json!(f),
        Db::Decimal(d) => json!(d.to_string()),
        Db::Text(s) | Db::Enum(s) => json!(s),
        Db::Blob(b) | Db::Geometry(b) => json!(format!("<{} bytes>", b.len())),
        Db::Timestamp(unit, n) => {
            let micros = unit.to_micros(n);
            DateTime::from_timestamp_micros(micros)
                .map(|t| json!(t.naive_utc().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()))
                .unwrap_or(json!(micros))
        }
        Db::Date32(days) => NaiveDate::from_ymd_opt(1970, 1, 1)
            .and_then(|epoch| epoch.checked_add_signed(chrono::Duration::days(days as i64)))
            .map(|d| json!(d.to_string()))
            .unwrap_or(json!(days)),
        Db::Time64(unit, n) => {
            let micros = unit.to_micros(n);
            NaiveTime::from_num_seconds_from_midnight_opt((micros / 1_000_000) as u32, ((micros % 1_000_000) * 1000) as u32)
                .map(|t| json!(t.to_string()))
                .unwrap_or(json!(micros))
        }
        Db::Interval { months, days, nanos } => json!({ "months": months, "days": days, "nanos": nanos }),
        Db::List(items) | Db::Array(items) => Value::Array(items.into_iter().map(to_json).collect()),
        Db::Struct(fields) => {
            Value::Object(fields.iter().map(|(k, v)| (k.clone(), to_json(v.clone()))).collect())
        }
        Db::Map(entries) => Value::Array(
            entries.iter().map(|(k, v)| json!({ "key": to_json(k.clone()), "value": to_json(v.clone()) })).collect(),
        ),
        Db::Union(inner) => to_json(*inner),
        // `Value` is non-exhaustive; show anything newer as its debug form.
        other => json!(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_guard() {
        for ok in [
            "select 1",
            "  SELECT * FROM runs;",
            "with x as (select 1) select * from x",
            "-- comment\nselect 1",
            "from runs",
            "select ';' as semi",
            "(select 1)",
        ] {
            assert!(check_read_only(ok).is_ok(), "{ok}");
        }
        for bad in [
            "delete from runs",
            "update runs set status='x'",
            "insert into runs values (1)",
            "drop table runs",
            "attach 'x.db'",
            "copy runs to '/tmp/x.csv'",
            "select 1; delete from runs",
            "/* select */ delete from runs",
            "set enable_external_access=true",
            "",
        ] {
            assert!(check_read_only(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn queries_return_typed_values_and_never_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory(dir.path()).unwrap();
        let r = run(
            &mut store,
            "select 1::INTEGER as n, 'a' as s, true as b, TIMESTAMP '2026-01-02 03:04:05' as t, DATE '2026-01-02' as d, null as z",
        )
        .unwrap();
        assert_eq!(r.columns, ["n", "s", "b", "t", "d", "z"]);
        assert_eq!(r.rows[0], vec![json!(1), json!("a"), json!(true), json!("2026-01-02T03:04:05.000Z"), json!("2026-01-02"), Value::Null]);

        let e = run(&mut store, "select * from no_such_table").unwrap_err();
        assert!(e.message.contains("query failed"));
        assert!(run(&mut store, "select * from read_csv('/etc/passwd')").is_err());
        assert_eq!(run(&mut store, "select count(*) from runs").unwrap().rows[0][0], json!(0));
    }
}
