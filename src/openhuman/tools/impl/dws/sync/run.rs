//! Shared helpers for the DWS sync adapters: shelling out to the `dws`
//! binary, parsing its JSON envelope, and formatting timestamps in the
//! several flavours different dws subcommands expect.

use std::time::Duration;

use serde_json::Value;

use super::super::extended_path_for_dws;

/// Per-dws-invocation timeout. Generous because some calls (mail message
/// search, minutes get transcription) hit slow backends.
pub const CATEGORY_TIMEOUT_SECS: u64 = 120;

/// Spawn a single dws command. Returns `Ok((stdout, stderr, exit_code))` on
/// completion and a string error on timeout / spawn failure. The PATH is
/// extended with the dws install locations so the binary is findable even
/// when the core process was launched without a user shell profile.
pub async fn run_dws(command: &str) -> Result<(String, String, i32), String> {
    let extended_path = extended_path_for_dws();
    tracing::debug!(command = %command, "[dws:sync] running dws");
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("PATH", &extended_path)
        .output();
    match tokio::time::timeout(Duration::from_secs(CATEGORY_TIMEOUT_SECS), child).await {
        Err(_) => Err(format!("dws command timed out: {command}")),
        Ok(Err(spawn_error)) => Err(format!("failed to spawn dws: {spawn_error}")),
        Ok(Ok(proc_output)) => {
            let stdout = String::from_utf8_lossy(&proc_output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&proc_output.stderr).to_string();
            let code = proc_output.status.code().unwrap_or(-1);
            Ok((stdout, stderr, code))
        }
    }
}

/// Run a dws command that should succeed and return parsed JSON. Returns
/// `Err` on timeout, spawn failure, non-zero exit, or non-JSON stdout.
pub async fn run_dws_json(command: &str) -> Result<Value, String> {
    let (stdout, stderr, code) = run_dws(command).await?;
    if code != 0 {
        return Err(format!(
            "dws exited with code {code}: {}{}",
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!("\n{}", stdout.trim())
            }
        ));
    }
    serde_json::from_str(&stdout).map_err(|err| format!("dws JSON parse failed: {err}"))
}

/// Unwrap dws's standard envelope `{ result: T, success: bool, ... }` into
/// just the `result` payload. When the response isn't wrapped (some
/// subcommands return a bare array), pass it through unchanged.
pub fn unwrap_dws_result(v: Value) -> Value {
    if let Value::Object(map) = &v {
        if let Some(result) = map.get("result") {
            return result.clone();
        }
    }
    v
}

/// Format a unix timestamp as ISO-8601 in **local** time with offset
/// (e.g. `2026-05-20T20:49:33+08:00`). Matches the format dws's `--start`
/// / `--end` flags expect for calendar / report / approval / minutes /
/// doc visited-window subcommands.
pub fn format_iso_local(ts: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Format a unix timestamp as `yyyy-MM-dd HH:mm:ss` in local time, which is
/// the format `dws chat message list-all --start/--end` requires (note the
/// space separator, not the `T` of ISO-8601).
pub fn format_dt_space(ts: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Today's local date as `YYYY-MM-DD`.
pub fn today_local_date() -> String {
    use chrono::Local;
    Local::now().format("%Y-%m-%d").to_string()
}

/// Current unix seconds.
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Best-effort count of items in a dws JSON response. Recognises the dws
/// `{ result: [...], success: true }` envelope plus the usual
/// `data/items/records/list/results` keys.
pub fn count_records(stdout: &str) -> usize {
    serde_json::from_str::<Value>(stdout)
        .ok()
        .and_then(|val| {
            if let Some(arr) = val.as_array() {
                Some(arr.len())
            } else if let Some(obj) = val.as_object() {
                for key in ["result", "data", "items", "records", "list", "results"] {
                    if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                        return Some(arr.len());
                    }
                }
                Some(1)
            } else {
                None
            }
        })
        .unwrap_or(if stdout.trim().is_empty() { 0 } else { 1 })
}

/// Coerce a JSON value into an i64 timestamp. Accepts:
/// - integer milliseconds (`1716240000000`)
/// - integer seconds (`1716240000`)
/// - RFC 3339 / ISO 8601 string (`"2026-05-20T20:49:33+08:00"`).
///
/// Returns `None` for shapes we don't recognise. Heuristic for the int
/// case: anything > 10^12 is treated as milliseconds.
pub fn coerce_timestamp_ms(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => {
            let i = n.as_i64()?;
            Some(if i > 10_000_000_000 { i } else { i * 1000 })
        }
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_records_handles_dws_envelope() {
        let stdout = r#"{"result":[{"a":1},{"a":2},{"a":3}],"success":true}"#;
        assert_eq!(count_records(stdout), 3);
    }

    #[test]
    fn count_records_handles_bare_array() {
        assert_eq!(count_records("[1,2,3,4]"), 4);
    }

    #[test]
    fn count_records_handles_empty_stdout() {
        assert_eq!(count_records(""), 0);
        assert_eq!(count_records("   \n  "), 0);
    }

    #[test]
    fn coerce_timestamp_ms_handles_seconds() {
        assert_eq!(
            coerce_timestamp_ms(&serde_json::json!(1_716_240_000)),
            Some(1_716_240_000_000)
        );
    }

    #[test]
    fn coerce_timestamp_ms_handles_millis() {
        assert_eq!(
            coerce_timestamp_ms(&serde_json::json!(1_716_240_000_000_i64)),
            Some(1_716_240_000_000)
        );
    }

    #[test]
    fn coerce_timestamp_ms_handles_iso_string() {
        let ts = coerce_timestamp_ms(&serde_json::json!("2026-05-20T20:49:33+08:00")).unwrap();
        assert!(ts > 1_700_000_000_000);
    }

    #[test]
    fn format_dt_space_uses_space_separator() {
        let s = format_dt_space(1_716_240_000);
        assert!(s.matches(' ').count() == 1, "got {s}");
        assert!(!s.contains('T'));
    }

    #[test]
    fn unwrap_dws_result_strips_envelope() {
        let v = serde_json::json!({ "result": [1, 2, 3], "success": true });
        assert_eq!(unwrap_dws_result(v), serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn unwrap_dws_result_passes_through_when_no_envelope() {
        let v = serde_json::json!([1, 2, 3]);
        assert_eq!(unwrap_dws_result(v.clone()), v);
    }
}
