//! Doc adapter: `dws doc search` + `dws doc read` → `ingest_document`.
//!
//! Search is scoped to docs the dws-authenticated user has edited and
//! visited in `[since, now]`. Each result then gets a full-body read.
//! `source_id` includes the doc's `modified_at_ms` so a revised doc
//! sails past the `ingest_document` source-level dedup gate as a new
//! source — older revisions stay in memory rather than being overwritten.

use serde_json::Value;

use crate::openhuman::config::Config;
use crate::openhuman::memory::tree::canonicalize::document::DocumentInput;
use crate::openhuman::memory::tree::ingest::ingest_document;

use super::super::categories::DwsSyncCategory;
use super::super::owner::OwnerIdentity;
use super::super::run::{coerce_timestamp_ms, run_dws_json};
use super::SyncCategoryResult;

const MAX_PAGES: usize = 3;
/// dws caps `doc search --page-size` at 30 (server replies
/// `pageSize 必须在 1 到 30 之间` for higher values). Hold to the ceiling.
const PAGE_SIZE: u64 = 30;
const MAX_BODY_FETCHES: usize = 30;

pub async fn run(
    since: u64,
    now: u64,
    owner: &OwnerIdentity,
    config: &Config,
) -> SyncCategoryResult {
    let user_id = match owner.user_id.as_deref() {
        Some(u) => u,
        None => {
            return SyncCategoryResult::fail(
                DwsSyncCategory::Doc,
                "missing user_id (contact get-self probe failed)",
            );
        }
    };

    // dws doc search wants millisecond timestamps as strings.
    let visited_from_ms = (since as i64) * 1000;
    let visited_to_ms = (now as i64) * 1000;

    let mut page_token = String::new();
    let mut headers: Vec<Value> = Vec::new();

    for page in 0..MAX_PAGES {
        let token_arg = if page_token.is_empty() {
            String::new()
        } else {
            format!(" --page-token \"{page_token}\"")
        };
        let command = format!(
            "dws doc search --editor-uids {user_id} --visited-from {visited_from_ms} --visited-to {visited_to_ms} --page-size {PAGE_SIZE}{token_arg} --format json"
        );
        let response = match run_dws_json(&command).await {
            Ok(v) => v,
            Err(err) => {
                return SyncCategoryResult::fail(
                    DwsSyncCategory::Doc,
                    format!("search page {page} failed: {err}"),
                );
            }
        };
        let (items, next) = extract_search_page(&response);
        let item_count = items.len();
        headers.extend(items);
        match next {
            Some(t) if !t.is_empty() && item_count > 0 => page_token = t,
            _ => break,
        }
    }

    if headers.is_empty() {
        return SyncCategoryResult::ok(DwsSyncCategory::Doc, 0, 0);
    }

    let owner_key = owner.owner_key();
    let mut total_chunks: usize = 0;
    let mut fetch_errors: Vec<String> = Vec::new();
    let mut fetched = 0;

    for header in &headers {
        if fetched >= MAX_BODY_FETCHES {
            tracing::info!(
                budget = MAX_BODY_FETCHES,
                pending = headers.len() - fetched,
                "[dws:sync] doc: hit per-tick fetch budget, deferring rest"
            );
            break;
        }
        let node_id = match extract_node_id(header) {
            Some(id) => id,
            None => continue,
        };
        let title = extract_title(header).unwrap_or_else(|| node_id.clone());
        let modified_at_ms = extract_modified_at_ms(header).unwrap_or(visited_to_ms);
        let source_ref = extract_source_ref(header, &node_id);

        let read_command = format!("dws doc read --node {node_id} --format json");
        fetched += 1;
        let read_response = match run_dws_json(&read_command).await {
            Ok(v) => v,
            Err(err) => {
                fetch_errors.push(format!("{node_id}: {err}"));
                continue;
            }
        };
        let body = extract_body(&read_response);
        if body.trim().is_empty() {
            // Some doc types (folder, blank) read as empty — skip rather than
            // emit a no-op ingest.
            continue;
        }

        let modified_at = chrono::Utc
            .timestamp_millis_opt(modified_at_ms)
            .single()
            .unwrap_or_else(chrono::Utc::now);

        let input = DocumentInput {
            provider: "dingtalk_doc".to_string(),
            title,
            body,
            modified_at,
            source_ref: Some(source_ref),
        };
        // Including modified_at in the source id means a revised doc creates
        // a new logical source so the version-level dedup gate in
        // ingest_document doesn't swallow updates.
        let source_id = format!("dingtalk:doc:{node_id}:{modified_at_ms}");
        match ingest_document(
            config,
            &source_id,
            &owner_key,
            vec!["dingtalk".to_string(), "doc".to_string()],
            input,
        )
        .await
        {
            Ok(result) => total_chunks += result.chunks_written,
            Err(err) => fetch_errors.push(format!("{source_id}: {err}")),
        }
    }

    if !fetch_errors.is_empty() && total_chunks == 0 {
        return SyncCategoryResult::fail(
            DwsSyncCategory::Doc,
            format!(
                "all {} doc ingest(s) failed: {}",
                fetch_errors.len(),
                fetch_errors.join("; ")
            ),
        );
    }

    SyncCategoryResult::ok(DwsSyncCategory::Doc, headers.len(), total_chunks)
}

use chrono::TimeZone;

fn extract_search_page(v: &Value) -> (Vec<Value>, Option<String>) {
    let items = v
        .get("result")
        .and_then(|r| {
            if let Some(arr) = r.as_array() {
                Some(arr.clone())
            } else {
                r.get("documents")
                    .or_else(|| r.get("items"))
                    .or_else(|| r.get("nodes"))
                    .or_else(|| r.get("list"))
                    .and_then(|x| x.as_array())
                    .cloned()
            }
        })
        .unwrap_or_default();
    let next = v
        .get("nextPageToken")
        .or_else(|| v.get("pageToken"))
        .or_else(|| v.get("result").and_then(|r| r.get("nextPageToken")))
        .or_else(|| v.get("result").and_then(|r| r.get("pageToken")))
        .and_then(|x| x.as_str())
        .map(str::to_string);
    (items, next)
}

fn extract_node_id(header: &Value) -> Option<String> {
    ["nodeId", "node_id", "id", "documentId", "docId"]
        .iter()
        .find_map(|k| header.get(*k).and_then(|v| v.as_str()).map(str::to_string))
}

fn extract_title(header: &Value) -> Option<String> {
    ["name", "title", "displayName"]
        .iter()
        .find_map(|k| header.get(*k).and_then(|v| v.as_str()).map(str::to_string))
}

fn extract_modified_at_ms(header: &Value) -> Option<i64> {
    [
        "modifiedAt",
        "modifyTime",
        "updateTime",
        "lastModifiedTime",
        "visitedAt",
        "visitedTime",
    ]
    .iter()
    .find_map(|k| header.get(*k).and_then(coerce_timestamp_ms))
}

fn extract_source_ref(header: &Value, node_id: &str) -> String {
    header
        .get("url")
        .or_else(|| header.get("link"))
        .or_else(|| header.get("webUrl"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("dingtalk://doc/{node_id}"))
}

fn extract_body(read_response: &Value) -> String {
    // dws envelope: { result: <markdown or { content: "..." }> }
    let payload = read_response
        .get("result")
        .cloned()
        .unwrap_or_else(|| read_response.clone());
    if let Value::String(s) = &payload {
        return s.clone();
    }
    if let Some(s) = payload
        .get("markdown")
        .or_else(|| payload.get("content"))
        .or_else(|| payload.get("body"))
        .or_else(|| payload.get("text"))
        .and_then(|v| v.as_str())
    {
        return s.to_string();
    }
    // Last resort: serialise the payload so something gets ingested.
    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_search_page_handles_envelope_array() {
        let v = serde_json::json!({
            "result": [{"id": "n1"}],
            "nextPageToken": "tok"
        });
        let (items, next) = extract_search_page(&v);
        assert_eq!(items.len(), 1);
        assert_eq!(next.as_deref(), Some("tok"));
    }

    #[test]
    fn extract_search_page_handles_nested_documents() {
        let v = serde_json::json!({
            "result": { "documents": [{"id": "n1"}, {"id": "n2"}], "pageToken": "p2" }
        });
        let (items, next) = extract_search_page(&v);
        assert_eq!(items.len(), 2);
        assert_eq!(next.as_deref(), Some("p2"));
    }

    #[test]
    fn extract_node_id_tries_multiple_keys() {
        assert_eq!(
            extract_node_id(&serde_json::json!({"nodeId": "n1"})).as_deref(),
            Some("n1")
        );
        assert_eq!(
            extract_node_id(&serde_json::json!({"documentId": "d1"})).as_deref(),
            Some("d1")
        );
        assert_eq!(extract_node_id(&serde_json::json!({})), None);
    }

    #[test]
    fn extract_body_handles_string_payload() {
        let v = serde_json::json!({ "result": "# title\nbody" });
        assert_eq!(extract_body(&v), "# title\nbody");
    }

    #[test]
    fn extract_body_handles_markdown_field() {
        let v = serde_json::json!({ "result": { "markdown": "# md" } });
        assert_eq!(extract_body(&v), "# md");
    }

    #[test]
    fn extract_body_handles_content_field() {
        let v = serde_json::json!({ "result": { "content": "raw text" } });
        assert_eq!(extract_body(&v), "raw text");
    }

    #[test]
    fn extract_source_ref_prefers_url_when_present() {
        let h = serde_json::json!({ "url": "https://alidocs.dingtalk.com/i/p/abc" });
        assert_eq!(
            extract_source_ref(&h, "abc"),
            "https://alidocs.dingtalk.com/i/p/abc"
        );
    }

    #[test]
    fn extract_source_ref_falls_back_to_synthetic_uri() {
        let h = serde_json::json!({});
        assert_eq!(extract_source_ref(&h, "abc"), "dingtalk://doc/abc");
    }
}
