//! Chat adapter: `dws chat message list-all` → `ingest_chat`.
//!
//! Pulls every message in the `[since, now]` window via the time-range
//! search, groups them by `openConversationId`, and submits one
//! `ChatBatch` per group to the memory tree. The dws CLI takes plain
//! `yyyy-MM-dd HH:mm:ss` start / end values (no `T`, no offset).
//!
//! Known limitation: `dws chat message list-all` does not guarantee 1:1
//! chat coverage (upstream issue #249). Accepted for v2.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::openhuman::config::Config;
use crate::openhuman::memory::tree::canonicalize::chat::{ChatBatch, ChatMessage};
use crate::openhuman::memory::tree::ingest::ingest_chat;

use super::super::categories::DwsSyncCategory;
use super::super::owner::OwnerIdentity;
use super::super::run::{coerce_timestamp_ms, format_dt_space, run_dws_json};
use super::SyncCategoryResult;

/// Pagination ceiling — protects against API loops and runaway ingestion.
const MAX_PAGES: usize = 5;
const PAGE_LIMIT: u64 = 100;

pub async fn run(
    since: u64,
    now: u64,
    owner: &OwnerIdentity,
    config: &Config,
) -> SyncCategoryResult {
    let start = format_dt_space(since);
    let end = format_dt_space(now);

    let mut cursor = String::from("0");
    let mut raw_messages: Vec<Value> = Vec::new();

    for page in 0..MAX_PAGES {
        let command = format!(
            "dws chat message list-all --start \"{start}\" --end \"{end}\" --limit {PAGE_LIMIT} --cursor \"{cursor}\" --format json"
        );
        let response = match run_dws_json(&command).await {
            Ok(v) => v,
            Err(err) => {
                return SyncCategoryResult::fail(
                    DwsSyncCategory::Chat,
                    format!("page {page} failed: {err}"),
                );
            }
        };

        let (items, next_cursor) = extract_page(&response);
        let item_count = items.len();
        raw_messages.extend(items);

        match next_cursor {
            Some(token) if !token.is_empty() && token != "0" && item_count > 0 => cursor = token,
            _ => break,
        }
    }

    if raw_messages.is_empty() {
        return SyncCategoryResult::ok(DwsSyncCategory::Chat, 0, 0);
    }

    let owner_key = owner.owner_key();
    let groups = group_by_conversation(&raw_messages);
    let group_count = groups.len();

    let mut total_chunks: usize = 0;
    let mut ingest_errors: Vec<String> = Vec::new();

    for (conv_id, conv_msgs) in groups {
        let label = conv_label(&conv_id, &conv_msgs);
        let mut messages: Vec<ChatMessage> = conv_msgs
            .iter()
            .filter_map(|raw| parse_message(raw))
            .collect();
        if messages.is_empty() {
            continue;
        }
        messages.sort_by_key(|m| m.timestamp);

        let batch = ChatBatch {
            platform: "dingtalk".to_string(),
            channel_label: label,
            messages,
        };
        let source_id = format!("dingtalk:chat:{conv_id}");
        match ingest_chat(
            config,
            &source_id,
            &owner_key,
            vec!["dingtalk".to_string(), "chat".to_string()],
            batch,
        )
        .await
        {
            Ok(result) => total_chunks += result.chunks_written,
            Err(err) => ingest_errors.push(format!("{source_id}: {err}")),
        }
    }

    if !ingest_errors.is_empty() && total_chunks == 0 {
        return SyncCategoryResult::fail(
            DwsSyncCategory::Chat,
            format!(
                "all {} conv ingest(s) failed: {}",
                ingest_errors.len(),
                ingest_errors.join("; ")
            ),
        );
    }

    if !ingest_errors.is_empty() {
        tracing::warn!(
            partial_failures = ingest_errors.len(),
            successful_groups = group_count - ingest_errors.len(),
            "[dws:sync] chat: some conversations failed to ingest"
        );
    }

    SyncCategoryResult::ok(DwsSyncCategory::Chat, raw_messages.len(), total_chunks)
}

fn extract_page(v: &Value) -> (Vec<Value>, Option<String>) {
    let items = v
        .get("result")
        .and_then(|r| {
            if let Some(arr) = r.as_array() {
                Some(arr.clone())
            } else {
                r.get("messages")
                    .or_else(|| r.get("items"))
                    .or_else(|| r.get("list"))
                    .and_then(|x| x.as_array())
                    .cloned()
            }
        })
        .unwrap_or_default();

    let next = v
        .get("nextCursor")
        .or_else(|| v.get("next_cursor"))
        .or_else(|| v.get("result").and_then(|r| r.get("nextCursor")))
        .or_else(|| v.get("result").and_then(|r| r.get("cursor")))
        .and_then(|c| c.as_str())
        .map(str::to_string);

    (items, next)
}

fn group_by_conversation(raw: &[Value]) -> BTreeMap<String, Vec<Value>> {
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for msg in raw {
        let conv_id = msg
            .get("openConversationId")
            .or_else(|| msg.get("conversationId"))
            .or_else(|| msg.get("openConvId"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        groups.entry(conv_id).or_default().push(msg.clone());
    }
    groups
}

fn conv_label(conv_id: &str, msgs: &[Value]) -> String {
    msgs.iter()
        .find_map(|m| {
            m.get("conversationTitle")
                .or_else(|| m.get("groupName"))
                .or_else(|| m.get("conversationName"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| conv_id.to_string())
}

fn parse_message(raw: &Value) -> Option<ChatMessage> {
    let author = raw
        .get("senderNick")
        .or_else(|| raw.get("senderName"))
        .or_else(|| raw.get("sender"))
        .or_else(|| raw.get("senderStaffId"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let ts_ms = ["sendTime", "createAt", "createTime", "msgCreateTime", "ts"]
        .iter()
        .find_map(|k| raw.get(*k).and_then(coerce_timestamp_ms))?;
    let timestamp = Utc.timestamp_millis_opt(ts_ms).single()?;

    let text = extract_text(raw)?;
    if text.trim().is_empty() {
        return None;
    }

    let source_ref = raw
        .get("msgId")
        .or_else(|| raw.get("messageId"))
        .or_else(|| raw.get("openMessageId"))
        .and_then(|v| v.as_str())
        .map(|s| format!("dingtalk://chat/{s}"));

    Some(ChatMessage {
        author,
        timestamp,
        text,
        source_ref,
    })
}

fn extract_text(raw: &Value) -> Option<String> {
    if let Some(s) = raw.get("text").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = raw.get("content").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    // dingtalk wraps text under `msgContent.text.content`.
    raw.get("msgContent")
        .and_then(|m| m.get("text"))
        .and_then(|t| t.get("content"))
        .and_then(|c| c.as_str())
        .map(str::to_string)
}

// `chrono::DateTime` is imported but only used inside parse_message via the
// `Utc.timestamp_millis_opt` call result. Silence unused-import lints when
// the module's tests don't exercise it.
#[allow(dead_code)]
type _ChronoCheck = DateTime<Utc>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_page_handles_envelope_array() {
        let v = serde_json::json!({
            "result": [{ "a": 1 }, { "a": 2 }],
            "nextCursor": "abc",
            "success": true
        });
        let (items, next) = extract_page(&v);
        assert_eq!(items.len(), 2);
        assert_eq!(next.as_deref(), Some("abc"));
    }

    #[test]
    fn extract_page_handles_nested_messages_key() {
        let v = serde_json::json!({
            "result": { "messages": [{ "a": 1 }], "cursor": "xx" }
        });
        let (items, next) = extract_page(&v);
        assert_eq!(items.len(), 1);
        assert_eq!(next.as_deref(), Some("xx"));
    }

    #[test]
    fn group_by_conversation_uses_open_conversation_id() {
        let raw = vec![
            serde_json::json!({ "openConversationId": "A", "text": "hi" }),
            serde_json::json!({ "openConversationId": "B", "text": "yo" }),
            serde_json::json!({ "openConversationId": "A", "text": "bye" }),
        ];
        let groups = group_by_conversation(&raw);
        assert_eq!(groups.get("A").map(|v| v.len()), Some(2));
        assert_eq!(groups.get("B").map(|v| v.len()), Some(1));
    }

    #[test]
    fn parse_message_handles_text_at_top_level() {
        let raw = serde_json::json!({
            "senderNick": "Alice",
            "sendTime": 1_716_240_000_000_i64,
            "text": "hello world",
            "msgId": "m1",
        });
        let m = parse_message(&raw).expect("should parse");
        assert_eq!(m.author, "Alice");
        assert_eq!(m.text, "hello world");
        assert_eq!(m.source_ref.as_deref(), Some("dingtalk://chat/m1"));
    }

    #[test]
    fn parse_message_handles_nested_msg_content() {
        let raw = serde_json::json!({
            "senderName": "Bob",
            "createAt": 1_716_240_000_000_i64,
            "msgContent": { "text": { "content": "nested body" } }
        });
        let m = parse_message(&raw).expect("nested content");
        assert_eq!(m.author, "Bob");
        assert_eq!(m.text, "nested body");
    }

    #[test]
    fn parse_message_skips_empty_text() {
        let raw = serde_json::json!({
            "senderNick": "Alice",
            "sendTime": 1_716_240_000_000_i64,
            "text": "  \n  ",
        });
        assert!(parse_message(&raw).is_none());
    }

    #[test]
    fn parse_message_skips_when_timestamp_missing() {
        let raw = serde_json::json!({
            "senderNick": "Alice",
            "text": "hi",
        });
        assert!(parse_message(&raw).is_none());
    }

    #[test]
    fn conv_label_falls_back_to_id_when_title_absent() {
        let msgs = vec![serde_json::json!({ "text": "hi" })];
        assert_eq!(conv_label("cidXYZ", &msgs), "cidXYZ");
    }

    #[test]
    fn conv_label_uses_conversation_title_when_present() {
        let msgs = vec![serde_json::json!({
            "conversationTitle": "工程师群",
            "text": "hi"
        })];
        assert_eq!(conv_label("cidXYZ", &msgs), "工程师群");
    }
}
