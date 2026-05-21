//! Calendar adapter: `dws calendar event list` → `ingest_document`.
//!
//! Pulls events in the window `[since, now + 7 days]` (forward-looking so
//! the user's upcoming meetings are in memory before they happen, not
//! only after the fact). Each event becomes its own `DocumentInput` with
//! the title in the `title` field and the structured metadata
//! (time / location / organizer / attendees / agenda) in the body.

use chrono::{Local, TimeZone, Utc};
use serde_json::Value;

use crate::openhuman::config::Config;
use crate::openhuman::memory::tree::canonicalize::document::DocumentInput;
use crate::openhuman::memory::tree::ingest::ingest_document;

use super::super::categories::DwsSyncCategory;
use super::super::owner::OwnerIdentity;
use super::super::run::{coerce_timestamp_ms, format_iso_local, run_dws_json};
use super::SyncCategoryResult;

const FORWARD_WINDOW_SECS: u64 = 7 * 24 * 3_600;

pub async fn run(
    since: u64,
    now: u64,
    owner: &OwnerIdentity,
    config: &Config,
) -> SyncCategoryResult {
    let start = format_iso_local(since);
    let end = format_iso_local(now + FORWARD_WINDOW_SECS);
    let command = format!(
        "dws calendar event list --start {start} --end {end} --format json"
    );

    let response = match run_dws_json(&command).await {
        Ok(v) => v,
        Err(err) => return SyncCategoryResult::fail(DwsSyncCategory::Calendar, err),
    };

    let events = extract_events(&response);
    if events.is_empty() {
        return SyncCategoryResult::ok(DwsSyncCategory::Calendar, 0, 0);
    }

    let owner_key = owner.owner_key();
    let mut total_chunks: usize = 0;
    let mut errors: Vec<String> = Vec::new();

    for event in &events {
        let event_id = match extract_event_id(event) {
            Some(id) => id,
            None => continue,
        };
        let title = extract_summary(event).unwrap_or_else(|| event_id.clone());
        let start_ms = extract_event_start_ms(event);
        let modified_at = start_ms
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now);

        let body = build_event_body(event, start_ms);
        let source_ref = event
            .get("url")
            .or_else(|| event.get("link"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("dingtalk://calendar/{event_id}"));

        let input = DocumentInput {
            provider: "dingtalk_calendar".to_string(),
            title,
            body,
            modified_at,
            source_ref: Some(source_ref),
        };
        let source_id = format!("dingtalk:cal:{event_id}");
        match ingest_document(
            config,
            &source_id,
            &owner_key,
            vec!["dingtalk".to_string(), "calendar".to_string()],
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
            DwsSyncCategory::Calendar,
            format!(
                "all {} event ingest(s) failed: {}",
                errors.len(),
                errors.join("; ")
            ),
        );
    }

    SyncCategoryResult::ok(DwsSyncCategory::Calendar, events.len(), total_chunks)
}

fn extract_events(v: &Value) -> Vec<Value> {
    v.get("result")
        .and_then(|r| {
            if let Some(arr) = r.as_array() {
                Some(arr.clone())
            } else {
                r.get("events")
                    .or_else(|| r.get("items"))
                    .or_else(|| r.get("list"))
                    .and_then(|x| x.as_array())
                    .cloned()
            }
        })
        .unwrap_or_default()
}

fn extract_event_id(event: &Value) -> Option<String> {
    ["id", "eventId", "uid"]
        .iter()
        .find_map(|k| event.get(*k).and_then(|v| v.as_str()).map(str::to_string))
}

fn extract_summary(event: &Value) -> Option<String> {
    ["summary", "subject", "title"]
        .iter()
        .find_map(|k| event.get(*k).and_then(|v| v.as_str()).map(str::to_string))
}

fn extract_event_start_ms(event: &Value) -> Option<i64> {
    extract_dt_ms(event.get("start"))
        .or_else(|| event.get("startTime").and_then(coerce_timestamp_ms))
}

fn extract_event_end_ms(event: &Value) -> Option<i64> {
    extract_dt_ms(event.get("end"))
        .or_else(|| event.get("endTime").and_then(coerce_timestamp_ms))
}

/// dws calendar nests start/end as `{ dateTime: <iso>, timeZone: ... }`
/// or as a bare millis. Handle both.
fn extract_dt_ms(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(ms) = coerce_timestamp_ms(v) {
        return Some(ms);
    }
    v.get("dateTime").and_then(coerce_timestamp_ms)
}

fn format_local_dt(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M (%:z)").to_string())
        .unwrap_or_else(|| ms.to_string())
}

fn build_event_body(event: &Value, start_ms: Option<i64>) -> String {
    let mut body = String::new();

    let end_ms = extract_event_end_ms(event);
    if let Some(s) = start_ms {
        body.push_str("**时间**: ");
        body.push_str(&format_local_dt(s));
        if let Some(e) = end_ms {
            body.push_str(" — ");
            body.push_str(&format_local_dt(e));
        }
        body.push('\n');
    }

    if let Some(loc) = event
        .get("location")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        body.push_str("**地点**: ");
        body.push_str(loc);
        body.push('\n');
    } else if let Some(loc) = event
        .get("location")
        .and_then(|v| v.get("displayName"))
        .and_then(|v| v.as_str())
    {
        body.push_str("**地点**: ");
        body.push_str(loc);
        body.push('\n');
    }

    if let Some(meeting_url) = event
        .get("onlineMeetingUrl")
        .or_else(|| event.get("meetingUrl"))
        .and_then(|v| v.as_str())
    {
        body.push_str("**会议链接**: ");
        body.push_str(meeting_url);
        body.push('\n');
    }

    if let Some(organizer) = extract_person_name(event.get("organizer")) {
        body.push_str("**组织者**: ");
        body.push_str(&organizer);
        body.push('\n');
    }

    let attendees = event
        .get("attendees")
        .or_else(|| event.get("participants"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| extract_person_name(Some(p)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !attendees.is_empty() {
        body.push_str("**参会人**: ");
        body.push_str(&attendees.join(", "));
        body.push('\n');
    }

    // Agenda: try the `agenda` field, else the first 6 lines of `description`.
    let agenda_lines: Vec<String> = if let Some(agenda) = event
        .get("agenda")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        agenda
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        event
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| {
                s.lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .take(6)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    if !agenda_lines.is_empty() {
        body.push_str("\n**议程**:\n");
        for line in agenda_lines {
            body.push_str("- ");
            body.push_str(&line);
            body.push('\n');
        }
    }

    body
}

fn extract_person_name(v: Option<&Value>) -> Option<String> {
    let v = v?;
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => map
            .get("displayName")
            .or_else(|| map.get("name"))
            .or_else(|| map.get("nick"))
            .or_else(|| map.get("userId"))
            .or_else(|| map.get("id"))
            .and_then(|x| x.as_str())
            .map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_events_handles_envelope_array() {
        let v = serde_json::json!({ "result": [{"id": "e1"}, {"id": "e2"}] });
        assert_eq!(extract_events(&v).len(), 2);
    }

    #[test]
    fn extract_events_handles_nested_events_key() {
        let v = serde_json::json!({ "result": { "events": [{"id": "e1"}] } });
        assert_eq!(extract_events(&v).len(), 1);
    }

    #[test]
    fn extract_event_start_handles_nested_datetime_object() {
        let v = serde_json::json!({ "start": { "dateTime": "2026-05-20T10:00:00+08:00" } });
        assert!(extract_event_start_ms(&v).unwrap() > 1_000_000_000_000);
    }

    #[test]
    fn extract_event_start_handles_top_level_ms() {
        let v = serde_json::json!({ "startTime": 1_716_240_000_000_i64 });
        assert_eq!(extract_event_start_ms(&v), Some(1_716_240_000_000));
    }

    #[test]
    fn build_event_body_includes_time_attendees_agenda() {
        let event = serde_json::json!({
            "summary": "Q3 Plan",
            "start": { "dateTime": "2026-05-20T10:00:00+08:00" },
            "end": { "dateTime": "2026-05-20T11:00:00+08:00" },
            "location": "会议室 A",
            "organizer": { "displayName": "张三" },
            "attendees": [
                { "displayName": "李四" },
                { "name": "王五" }
            ],
            "agenda": "项目状态同步\nQ3 OKR review"
        });
        let body = build_event_body(&event, extract_event_start_ms(&event));
        assert!(body.contains("**时间**"));
        assert!(body.contains("会议室 A"));
        assert!(body.contains("**组织者**: 张三"));
        assert!(body.contains("**参会人**: 李四, 王五"));
        assert!(body.contains("**议程**"));
        assert!(body.contains("- 项目状态同步"));
        assert!(body.contains("- Q3 OKR review"));
    }

    #[test]
    fn build_event_body_falls_back_to_description_when_no_agenda() {
        let event = serde_json::json!({
            "summary": "Sync",
            "description": "line1\nline2\nline3\n\nline4"
        });
        let body = build_event_body(&event, None);
        assert!(body.contains("- line1"));
        assert!(body.contains("- line4"));
    }

    #[test]
    fn extract_person_name_handles_string_and_object() {
        assert_eq!(
            extract_person_name(Some(&serde_json::json!("张三"))).as_deref(),
            Some("张三")
        );
        assert_eq!(
            extract_person_name(Some(&serde_json::json!({"displayName": "李四"}))).as_deref(),
            Some("李四")
        );
    }
}
