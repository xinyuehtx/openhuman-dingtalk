//! Minutes adapter: `dws minutes list mine` + `get info` + `get summary` +
//! `get todos` → `ingest_document`.
//!
//! User-confirmed depth: ingest the AI-generated summary and the extracted
//! action items, NOT the full transcript (transcripts tend to be long and
//! noisy — keep them in the source system).

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::openhuman::config::Config;
use crate::openhuman::memory::tree::canonicalize::document::DocumentInput;
use crate::openhuman::memory::tree::ingest::ingest_document;

use super::super::categories::DwsSyncCategory;
use super::super::owner::OwnerIdentity;
use super::super::run::{coerce_timestamp_ms, format_iso_local, run_dws_json};
use super::SyncCategoryResult;

const MAX_PAGES: usize = 3;
const PAGE_SIZE: u64 = 20;
const MAX_DETAIL_FETCHES: usize = 20;

pub async fn run(
    since: u64,
    now: u64,
    owner: &OwnerIdentity,
    config: &Config,
) -> SyncCategoryResult {
    let start = format_iso_local(since);
    let end = format_iso_local(now);

    let mut next_token = String::new();
    let mut headers: Vec<Value> = Vec::new();

    for page in 0..MAX_PAGES {
        let token_arg = if next_token.is_empty() {
            String::new()
        } else {
            format!(" --next-token \"{next_token}\"")
        };
        let command = format!(
            "dws minutes list mine --start {start} --end {end} --max {PAGE_SIZE}{token_arg} --format json"
        );
        let response = match run_dws_json(&command).await {
            Ok(v) => v,
            Err(err) => {
                return SyncCategoryResult::fail(
                    DwsSyncCategory::Minutes,
                    format!("list page {page} failed: {err}"),
                );
            }
        };
        let (items, next) = extract_list_page(&response);
        let item_count = items.len();
        headers.extend(items);
        match next {
            Some(t) if !t.is_empty() && item_count > 0 => next_token = t,
            _ => break,
        }
    }

    if headers.is_empty() {
        return SyncCategoryResult::ok(DwsSyncCategory::Minutes, 0, 0);
    }

    let owner_key = owner.owner_key();
    let mut total_chunks: usize = 0;
    let mut errors: Vec<String> = Vec::new();
    let mut fetched = 0;

    for header in &headers {
        if fetched >= MAX_DETAIL_FETCHES {
            tracing::info!(
                budget = MAX_DETAIL_FETCHES,
                pending = headers.len() - fetched,
                "[dws:sync] minutes: hit per-tick fetch budget, deferring rest"
            );
            break;
        }
        let task_uuid = match extract_task_uuid(header) {
            Some(id) => id,
            None => continue,
        };
        fetched += 1;

        // Three round trips per meeting; fail soft and continue on any of them.
        let info = match run_dws_json(&format!(
            "dws minutes get info --id {task_uuid} --format json"
        ))
        .await
        {
            Ok(v) => unwrap_result(v),
            Err(err) => {
                errors.push(format!("{task_uuid} info: {err}"));
                continue;
            }
        };
        let summary = run_dws_json(&format!(
            "dws minutes get summary --id {task_uuid} --format json"
        ))
        .await
        .map(unwrap_result)
        .unwrap_or(Value::Null);
        let todos = run_dws_json(&format!(
            "dws minutes get todos --id {task_uuid} --format json"
        ))
        .await
        .map(unwrap_result)
        .unwrap_or(Value::Null);

        let title = extract_title(&info, header).unwrap_or_else(|| task_uuid.clone());
        let meeting_time_ms = extract_meeting_time_ms(&info, header).unwrap_or(now as i64 * 1000);
        let modified_at = Utc
            .timestamp_millis_opt(meeting_time_ms)
            .single()
            .unwrap_or_else(Utc::now);

        let body = build_body(&info, &summary, &todos, meeting_time_ms);
        if body.trim().is_empty() {
            continue;
        }

        let source_ref = extract_source_ref(&info, &task_uuid);
        let input = DocumentInput {
            provider: "dingtalk_minutes".to_string(),
            title,
            body,
            modified_at,
            source_ref: Some(source_ref),
        };
        let source_id = format!("dingtalk:minutes:{task_uuid}");
        match ingest_document(
            config,
            &source_id,
            &owner_key,
            vec!["dingtalk".to_string(), "minutes".to_string()],
            input,
        )
        .await
        {
            Ok(r) => total_chunks += r.chunks_written,
            Err(err) => errors.push(format!("{source_id}: {err}")),
        }
    }

    if !errors.is_empty() && total_chunks == 0 {
        return SyncCategoryResult::fail(
            DwsSyncCategory::Minutes,
            format!(
                "all {} minute ingest(s) failed: {}",
                errors.len(),
                errors.join("; ")
            ),
        );
    }

    SyncCategoryResult::ok(DwsSyncCategory::Minutes, headers.len(), total_chunks)
}

/// Parse a minutes list page.
///
/// Real-world dws shape (verified against a live `minutes list mine`
/// response):
/// ```json
/// {
///   "dingOpenErrcode": 0,
///   "errorMsg": "ok",
///   "result": {
///     "hasNext": true,
///     "minutesDetails": [...],
///     "nextToken": "<opaque>"
///   }
/// }
/// ```
///
/// `result.minutesDetails` is the production key — earlier versions only
/// checked `items` / `minutes` / `list` and silently returned 0 records.
/// The historical fallbacks are retained for forward compatibility.
fn extract_list_page(v: &Value) -> (Vec<Value>, Option<String>) {
    let items = v
        .get("result")
        .and_then(|r| {
            if let Some(arr) = r.as_array() {
                Some(arr.clone())
            } else {
                r.get("minutesDetails")
                    .or_else(|| r.get("items"))
                    .or_else(|| r.get("minutes"))
                    .or_else(|| r.get("list"))
                    .and_then(|x| x.as_array())
                    .cloned()
            }
        })
        .unwrap_or_default();
    let next = v
        .get("nextToken")
        .or_else(|| v.get("next_token"))
        .or_else(|| v.get("result").and_then(|r| r.get("nextToken")))
        .and_then(|x| x.as_str())
        .map(str::to_string);
    (items, next)
}

fn unwrap_result(v: Value) -> Value {
    if let Value::Object(map) = &v {
        if let Some(r) = map.get("result") {
            return r.clone();
        }
    }
    v
}

fn extract_task_uuid(header: &Value) -> Option<String> {
    ["taskUuid", "task_uuid", "id", "uuid"]
        .iter()
        .find_map(|k| header.get(*k).and_then(|v| v.as_str()).map(str::to_string))
}

fn extract_title(info: &Value, header: &Value) -> Option<String> {
    for k in ["title", "subject", "name", "meetingTitle"] {
        if let Some(s) = info.get(k).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
        if let Some(s) = header.get(k).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

fn extract_meeting_time_ms(info: &Value, header: &Value) -> Option<i64> {
    for k in [
        "startTime",
        "createTime",
        "meetingTime",
        "createdAt",
        "start",
    ] {
        if let Some(ms) = info.get(k).and_then(coerce_timestamp_ms) {
            return Some(ms);
        }
        if let Some(ms) = header.get(k).and_then(coerce_timestamp_ms) {
            return Some(ms);
        }
    }
    None
}

fn extract_source_ref(info: &Value, task_uuid: &str) -> String {
    info.get("url")
        .or_else(|| info.get("link"))
        .or_else(|| info.get("webUrl"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("dingtalk://minutes/{task_uuid}"))
}

fn build_body(info: &Value, summary: &Value, todos: &Value, meeting_time_ms: i64) -> String {
    let mut body = String::new();

    // Meeting header — time + participants when available.
    body.push_str("**会议时间**: ");
    body.push_str(&format_meeting_time(meeting_time_ms));
    body.push('\n');

    let participants = info
        .get("participants")
        .or_else(|| info.get("speakers"))
        .or_else(|| info.get("members"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| match p {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(m) => m
                        .get("name")
                        .or_else(|| m.get("displayName"))
                        .or_else(|| m.get("nick"))
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !participants.is_empty() {
        body.push_str("**参会人**: ");
        body.push_str(&participants.join(", "));
        body.push('\n');
    }

    // Summary section.
    let summary_text = extract_summary_text(summary);
    if !summary_text.trim().is_empty() {
        body.push_str("\n## 摘要\n");
        body.push_str(summary_text.trim());
        body.push('\n');
    }

    // Todos section.
    let todo_lines = extract_todo_lines(todos);
    if !todo_lines.is_empty() {
        body.push_str("\n## 待办\n");
        for line in todo_lines {
            body.push_str("- [ ] ");
            body.push_str(&line);
            body.push('\n');
        }
    }

    body
}

fn format_meeting_time(ms: i64) -> String {
    use chrono::Local;
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M (%:z)").to_string())
        .unwrap_or_else(|| ms.to_string())
}

fn extract_summary_text(summary: &Value) -> String {
    match summary {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            // The live dws `minutes get summary` envelope returns the
            // AI-generated meeting summary under `fullSummary` (markdown
            // with `> **主题** ...`, sections, AI insights, etc.). The
            // historical keys (`summary`, `content`, `text`, `body`,
            // `abstract`) are retained as fallbacks in case a future
            // dws version or a sibling endpoint reverts to a flatter
            // shape.
            for k in [
                "fullSummary",
                "summary",
                "content",
                "text",
                "body",
                "abstract",
            ] {
                if let Some(s) = map.get(k).and_then(|v| v.as_str()) {
                    return s.to_string();
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn extract_todo_lines(todos: &Value) -> Vec<String> {
    // The live dws `minutes get todos` envelope returns two parallel
    // arrays:
    // - `actions` — opaque strings (sometimes a JSON-encoded blob with
    //   `{mark, value}`), one per AI-derived action item;
    // - `dingtalkTodoList` — structured todos (`title`, `executorList`,
    //   `createdTime`, ...) when the meeting host promoted any of them
    //   into the dingtalk Todo app.
    //
    // Prefer the structured list when present (richer + lets
    // `arr_to_lines` pick up owner / due fields); fall back to the
    // unstructured `actions` array so we don't lose AI suggestions
    // even when the user hasn't promoted any. Historical keys kept as
    // additional fallbacks.
    let arr = match todos {
        Value::Array(a) => a.clone(),
        Value::Object(map) => {
            for k in [
                "dingtalkTodoList",
                "actions",
                "todos",
                "items",
                "tasks",
                "list",
            ] {
                if let Some(a) = map.get(k).and_then(|v| v.as_array()) {
                    let lines = arr_to_lines(a);
                    if !lines.is_empty() {
                        return lines;
                    }
                }
            }
            return Vec::new();
        }
        _ => return Vec::new(),
    };
    arr_to_lines(&arr)
}

fn arr_to_lines(arr: &[Value]) -> Vec<String> {
    arr.iter()
        .filter_map(|t| match t {
            Value::String(s) => {
                // `minutes get todos`'s `actions` array hands us each
                // entry as a JSON-encoded string like
                // `{"mark":[],"value":"黑键在群内总结..."}`. Try to
                // parse it: if the JSON shape matches, surface the
                // `value` field as the todo text. On any parse
                // failure / shape mismatch we fall back to the raw
                // string — that way unstructured action entries still
                // land in memory verbatim.
                if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                    if let Some(value) = parsed.get("value").and_then(|v| v.as_str()) {
                        if !value.trim().is_empty() {
                            return Some(value.to_string());
                        }
                    }
                }
                Some(s.clone())
            }
            Value::Object(map) => {
                // Structured todo objects (e.g. `dingtalkTodoList`
                // entries) use `title` / `executorList` / etc. The
                // historical `content` / `text` keys are kept as
                // fallbacks for adapter sources that pre-date the
                // dingtalk Todo integration.
                let text = map
                    .get("title")
                    .or_else(|| map.get("content"))
                    .or_else(|| map.get("text"))
                    .and_then(|v| v.as_str())?;
                let owner = map
                    .get("owner")
                    .or_else(|| map.get("assignee"))
                    // `executorList` from dingtalkTodoList is an array
                    // of opaque user-id strings or objects with
                    // `displayName`. Render the first one if present.
                    .or_else(|| {
                        map.get("executorList")
                            .and_then(|v| v.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|first| match first {
                                Value::String(s) => Some(first).filter(|_| !s.is_empty()),
                                Value::Object(o) => o
                                    .get("displayName")
                                    .or_else(|| o.get("name"))
                                    .or_else(|| o.get("nick"))
                                    .map(|_| first),
                                _ => None,
                            })
                    })
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.as_str()),
                        Value::Object(o) => o
                            .get("displayName")
                            .or_else(|| o.get("name"))
                            .or_else(|| o.get("nick"))
                            .and_then(|x| x.as_str()),
                        _ => None,
                    });
                let due = map
                    .get("due")
                    .or_else(|| map.get("deadline"))
                    .and_then(|v| v.as_str());
                let mut s = text.to_string();
                if let Some(o) = owner {
                    s.push_str(" — ");
                    s.push_str(o);
                }
                if let Some(d) = due {
                    s.push_str(" — ");
                    s.push_str(d);
                }
                Some(s)
            }
            _ => None,
        })
        .filter(|s| !s.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_list_page_handles_envelope_array() {
        let v = serde_json::json!({
            "result": [{"taskUuid": "m1"}, {"taskUuid": "m2"}],
            "nextToken": "tok"
        });
        let (items, next) = extract_list_page(&v);
        assert_eq!(items.len(), 2);
        assert_eq!(next.as_deref(), Some("tok"));
    }

    #[test]
    fn extract_list_page_handles_minutes_details_envelope() {
        // Regression for the production envelope discovered against the
        // live dws backend (`minutes list mine`). Adapter must accept
        // `result.minutesDetails` — earlier versions only knew about
        // `items` / `minutes` / `list` and silently returned 0 records.
        let v = serde_json::json!({
            "dingOpenErrcode": 0,
            "errorMsg": "ok",
            "result": {
                "hasNext": true,
                "minutesDetails": [
                    {"taskUuid": "u1", "title": "M1"},
                    {"taskUuid": "u2", "title": "M2"}
                ],
                "nextToken": "TOK"
            }
        });
        let (items, next) = extract_list_page(&v);
        assert_eq!(items.len(), 2);
        assert_eq!(next.as_deref(), Some("TOK"));
    }

    #[test]
    fn extract_task_uuid_tries_multiple_keys() {
        assert_eq!(
            extract_task_uuid(&serde_json::json!({"taskUuid": "t1"})).as_deref(),
            Some("t1")
        );
        assert_eq!(
            extract_task_uuid(&serde_json::json!({"id": "t2"})).as_deref(),
            Some("t2")
        );
    }

    #[test]
    fn extract_summary_text_handles_string_or_object() {
        assert_eq!(
            extract_summary_text(&serde_json::json!("bare summary")),
            "bare summary"
        );
        assert_eq!(
            extract_summary_text(&serde_json::json!({"summary": "from key"})),
            "from key"
        );
    }

    #[test]
    fn extract_summary_text_handles_full_summary_envelope() {
        // Regression for the production envelope from the live dws
        // backend (`minutes get summary`): the AI-generated meeting
        // summary lives under `fullSummary`, not `summary`. Earlier
        // versions returned an empty string for every meeting and the
        // saved md only contained the meeting time.
        let v = serde_json::json!({
            "fullSummary": "> **主题**: X\n\n## 任务目标\nfoo bar"
        });
        let out = extract_summary_text(&v);
        assert!(out.contains("**主题**"));
        assert!(out.contains("任务目标"));
    }

    #[test]
    fn extract_todo_lines_handles_array_of_strings() {
        let v = serde_json::json!(["finish doc", "send email"]);
        assert_eq!(extract_todo_lines(&v), vec!["finish doc", "send email"]);
    }

    #[test]
    fn extract_todo_lines_handles_object_with_owner_and_due() {
        let v = serde_json::json!({
            "todos": [
                { "content": "finish doc", "owner": "Alice", "due": "2026-05-25" },
                { "text": "review PR" }
            ]
        });
        let lines = extract_todo_lines(&v);
        assert_eq!(lines[0], "finish doc — Alice — 2026-05-25");
        assert_eq!(lines[1], "review PR");
    }

    #[test]
    fn extract_todo_lines_handles_dingtalk_todo_list_envelope() {
        // Regression for the production `dingtalkTodoList` shape:
        // structured todos use `title` + `executorList[].displayName`
        // rather than `content` + `owner`. Earlier versions returned
        // an empty list for every meeting.
        let v = serde_json::json!({
            "dingtalkTodoList": [
                {
                    "title": "黑键在群内总结专属域名Cookie未种问题",
                    "executorList": [{ "displayName": "黑键" }]
                },
                {
                    "title": "参省团队拉通相关方"
                }
            ]
        });
        let lines = extract_todo_lines(&v);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("黑键在群内总结"));
        assert!(lines[0].contains("— 黑键"));
        assert_eq!(lines[1], "参省团队拉通相关方");
    }

    #[test]
    fn extract_todo_lines_handles_actions_envelope_with_json_strings() {
        // The `actions` array in `minutes get todos` hands us each
        // entry as a JSON-encoded string. Adapter must unwrap the
        // `value` field rather than dumping the raw `{"mark":[],"value":..}`.
        let v = serde_json::json!({
            "actions": [
                r#"{"mark":[],"value":"黑键在群内总结专属域名Cookie未种问题及解决路径"}"#,
                "plain string fallback"
            ]
        });
        let lines = extract_todo_lines(&v);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "黑键在群内总结专属域名Cookie未种问题及解决路径");
        assert_eq!(lines[1], "plain string fallback");
    }

    #[test]
    fn extract_todo_lines_prefers_structured_list_over_actions() {
        // When both arrays are present, the structured
        // `dingtalkTodoList` is the user-curated promotion and should
        // win — `actions` is the raw AI suggestion list.
        let v = serde_json::json!({
            "actions": [r#"{"mark":[],"value":"raw action"}"#],
            "dingtalkTodoList": [{ "title": "promoted todo" }]
        });
        let lines = extract_todo_lines(&v);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "promoted todo");
    }

    #[test]
    fn build_body_combines_summary_todos_and_participants() {
        let info = serde_json::json!({
            "title": "Plan",
            "participants": ["Alice", { "displayName": "Bob" }]
        });
        let summary = serde_json::json!({"summary": "we agreed on X"});
        let todos = serde_json::json!({"todos": [{"content": "do Y", "owner": "Alice"}]});
        let body = build_body(&info, &summary, &todos, 1_716_240_000_000);
        assert!(body.contains("**参会人**: Alice, Bob"));
        assert!(body.contains("## 摘要"));
        assert!(body.contains("we agreed on X"));
        assert!(body.contains("## 待办"));
        assert!(body.contains("- [ ] do Y — Alice"));
    }
}
