//! Doc adapter: `dws doc search` + `dws doc read` → `ingest_document`.
//!
//! Pipeline (v2 — after the "doc list 拉取空 + 没有 markdown 内容" fix):
//!
//! 1. **List**: `dws doc search --extensions adoc --visited-from ... --visited-to ...`.
//!    No `--editor-uids` filter — we ingest every doc the user has
//!    visited in the window so reading-only context (other people's
//!    docs the user reads) lands in memory too. `--extensions adoc`
//!    restricts to markdown-doc nodes so we don't try to render
//!    spreadsheets (`axls`) or slides (`apt`) as markdown.
//! 2. **Read body**: for each header, call `dws doc read --node <id>`.
//!    The response has `markdown` + `title` + `docUrl` at the **top
//!    level** (no `result` wrapper — different from `chat` /
//!    `minutes`). Prefer the read response's `title` over the search
//!    header's `name`.
//! 3. **Permission gate**: some docs require a per-doc PAT grant
//!    (medium-risk auth). `dws doc read` returns
//!    `{success: false, code: "PAT_MEDIUM_RISK_NO_PERMISSION", uri: "..."}`.
//!    Log the grant URL and skip that doc — do NOT fail the whole
//!    adapter run, the other docs are still ingestable.
//! 4. **Ingest**: `source_id` includes the doc's `modified_at_ms` so a
//!    revised doc sails past the `ingest_document` source-level dedup
//!    gate as a new source — older revisions stay in memory rather
//!    than being overwritten.

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
/// Restrict `dws doc search` to text-doc extensions. `adoc` is dingtalk
/// markdown docs (`alidocs.dingtalk.com/i/p/...`); other extensions
/// (`axls` spreadsheet, `apt` slides) have body shapes that don't map
/// cleanly onto the memory-tree markdown contract — including them
/// would land binary/structured content as opaque text under
/// `dingtalk:doc:*`. Sync targets the user-facing "钉钉文档" doc type
/// only; spreadsheets / slides need their own adapters before they're
/// safe to enable.
const DOC_EXTENSIONS: &str = "adoc";
/// PAT (Privileged Access Token) error code dws returns when a doc
/// requires the user to grant a one-shot read permission via the
/// returned URI. Surfaced verbatim so the log message helps the user
/// click through; the doc is skipped (not a fatal sync failure).
const PAT_PERMISSION_ERROR_CODE: &str = "PAT_MEDIUM_RISK_NO_PERMISSION";

pub async fn run(
    since: u64,
    now: u64,
    owner: &OwnerIdentity,
    config: &Config,
) -> SyncCategoryResult {
    // `--editor-uids` is no longer required — `dws doc search` returns
    // visited docs scoped to the dws session even without it, and a
    // user-id filter was excluding every doc the user only reads (a
    // common pattern in collaborative orgs). Keep the probed identity
    // around purely for the `owner` tag on each ingested source so
    // multi-account workspaces still partition correctly.
    let _ = owner.user_id.as_deref();

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
            "dws doc search --extensions {DOC_EXTENSIONS} --visited-from {visited_from_ms} --visited-to {visited_to_ms} --page-size {PAGE_SIZE}{token_arg} --format json"
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
        // Defensive client-side filter: even with `--extensions adoc`
        // the dws response can occasionally include non-adoc nodes
        // (the server treats it as a hint, not a strict filter).
        // Drop anything whose `extension` field is set and not adoc
        // so we never feed a non-markdown body into `extract_body`.
        let filtered: Vec<Value> = items
            .into_iter()
            .filter(|h| matches_text_doc_extension(h))
            .collect();
        let kept_count = filtered.len();
        if kept_count != item_count {
            tracing::debug!(
                page,
                dropped = item_count - kept_count,
                "[dws:sync][doc] filtered non-adoc nodes out of search page"
            );
        }
        headers.extend(filtered);
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

    let mut permission_skipped: usize = 0;
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
        let header_title = extract_title(header);
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
        // PAT gating: dws returns the same JSON envelope for permission
        // errors as for successes (HTTP 200 + `success: false`), so the
        // run_dws_json call above succeeds and we need to look at the
        // shape. Skip the doc with a clear log line and keep going.
        if let Some(grant_uri) = extract_pat_grant_uri(&read_response) {
            permission_skipped += 1;
            tracing::warn!(
                node_id = %node_id,
                grant_uri = %grant_uri,
                "[dws:sync] doc: skipping (needs PAT permission grant — \
                 open the URI in a browser to authorise, then sync again)"
            );
            continue;
        }
        let body = extract_body(&read_response);
        if body.trim().is_empty() {
            // Some doc types (folder, blank) read as empty — skip rather than
            // emit a no-op ingest.
            continue;
        }
        // Prefer the title from the read response (set by dws based on
        // the live doc), falling back to the search-header `name`, and
        // finally to the node_id as a last resort. Picks up renames that
        // haven't propagated through the search index yet.
        let title = extract_read_title(&read_response)
            .or(header_title)
            .unwrap_or_else(|| node_id.clone());

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

    if permission_skipped > 0 {
        tracing::info!(
            permission_skipped,
            headers = headers.len(),
            chunks = total_chunks,
            "[dws:sync] doc: completed with PAT-gated docs skipped"
        );
    }

    SyncCategoryResult::ok(DwsSyncCategory::Doc, headers.len(), total_chunks)
}

use chrono::TimeZone;

/// Parse a doc-search page.
///
/// Real-world dws shape (verified against a live response):
/// ```json
/// {
///   "documents": [...],
///   "hasMore": true,
///   "nextPageToken": "<opaque>",
///   "success": true
/// }
/// ```
///
/// Notably the response is **NOT** wrapped in a `result` envelope — unlike
/// `chat message list-all` (`result.conversationMessagesList`) or
/// `minutes list mine` (`result.minutesDetails`). The historical
/// `result.documents | items | nodes | list` checks are retained as
/// fallbacks for forward compatibility, but the live shape lives at the
/// top level so we look there first.
fn extract_search_page(v: &Value) -> (Vec<Value>, Option<String>) {
    let items = v
        .get("documents")
        .or_else(|| v.get("items"))
        .or_else(|| v.get("nodes"))
        .or_else(|| v.get("list"))
        .and_then(|x| x.as_array())
        .cloned()
        .or_else(|| {
            v.get("result").and_then(|r| {
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

/// Extract the doc body from a `dws doc read` response.
///
/// Real-world envelope (verified against the live backend):
/// ```json
/// {
///   "docUrl": "https://alidocs.dingtalk.com/i/nodes/...",
///   "logId": "...",
///   "markdown": "# 年度工作概述\n\n...",
///   "nodeId": "...",
///   "title": "通用个人年终总结",
///   "success": true
/// }
/// ```
///
/// Fields are at the **top level** — there is NO `result` wrapper for
/// `doc read` (unlike `chat message list-all` and `minutes list mine`).
/// We check the top-level keys first and only fall through to a
/// `result.*` lookup as a last resort for forward compatibility.
fn extract_body(read_response: &Value) -> String {
    // Top-level (production shape).
    if let Some(s) = read_response
        .get("markdown")
        .or_else(|| read_response.get("content"))
        .or_else(|| read_response.get("body"))
        .or_else(|| read_response.get("text"))
        .and_then(|v| v.as_str())
    {
        return s.to_string();
    }
    // Forward-compat fallback: `result` may be either a bare string or
    // a nested object with `markdown` / `content` / etc.
    if let Some(payload) = read_response.get("result") {
        if let Value::String(s) = payload {
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
    }
    String::new()
}

/// True when a search-header node looks like a text-doc (`adoc` —
/// 钉钉文档 markdown). The server accepts `--extensions adoc` as a
/// hint but occasionally returns other extensions anyway; this client
/// filter keeps spreadsheets / slides / files from ever reaching
/// `dws doc read`.
fn matches_text_doc_extension(header: &Value) -> bool {
    match header.get("extension").and_then(|v| v.as_str()) {
        // Field present → strict whitelist.
        Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "adoc"),
        // Field absent → admit; some legacy nodes don't tag the
        // extension and we'd rather try to read them than silently
        // exclude the whole class.
        None => true,
    }
}

/// Pull a doc title from a `dws doc read` response. Prefer the
/// top-level `title` field (live shape) over `name` / `displayName`
/// fallbacks, but skip a key whose value is empty / whitespace-only so
/// a blank top-level title doesn't shadow a populated fallback.
fn extract_read_title(read_response: &Value) -> Option<String> {
    for k in ["title", "name", "displayName"] {
        if let Some(s) = read_response.get(k).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Detect the per-doc PAT permission error envelope dws returns when
/// the user needs to grant a one-shot read scope. Returns the grant
/// URI the user can visit to authorise; `None` for successful reads
/// or any other error shape (those flow through normal error
/// handling).
///
/// Real envelope:
/// ```json
/// {
///   "code": "PAT_MEDIUM_RISK_NO_PERMISSION",
///   "data": {
///     "uri": "https://open-dev.dingtalk.com/fe/...?flowId=...&userCode=...",
///     "requiredScopes": [{"scope": "doc:read", ...}],
///     ...
///   },
///   "success": false
/// }
/// ```
fn extract_pat_grant_uri(read_response: &Value) -> Option<String> {
    if read_response.get("success").and_then(|v| v.as_bool()) != Some(false) {
        return None;
    }
    let code = read_response.get("code").and_then(|v| v.as_str())?;
    if code != PAT_PERMISSION_ERROR_CODE {
        return None;
    }
    read_response
        .get("data")
        .and_then(|d| d.get("uri"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
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
    fn extract_search_page_handles_top_level_documents() {
        // Regression for the production envelope discovered against the
        // live dws backend (`doc search`). The response has NO `result`
        // wrapper — `documents`, `hasMore`, `nextPageToken` sit at the
        // top level. Earlier versions only checked under `result.*` and
        // returned 0 records for every sync.
        let v = serde_json::json!({
            "documents": [
                {"nodeId": "nA", "name": "doc A"},
                {"nodeId": "nB", "name": "doc B"}
            ],
            "hasMore": true,
            "nextPageToken": "TOK",
            "success": true
        });
        let (items, next) = extract_search_page(&v);
        assert_eq!(items.len(), 2);
        assert_eq!(next.as_deref(), Some("TOK"));
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
    fn extract_body_handles_top_level_markdown_envelope() {
        // Regression for the production envelope discovered against
        // the live dws backend (`doc read`): markdown / title / docUrl
        // sit at the **top level**, NOT under `result`. Earlier
        // versions only looked under `result.markdown` and returned an
        // empty body for every successful read (the chunk landed in
        // `mem_tree_chunks` with no content).
        let v = serde_json::json!({
            "docUrl": "https://alidocs.dingtalk.com/i/nodes/abc",
            "markdown": "# 年度工作概述\n\n💯 完成 95%",
            "nodeId": "abc",
            "title": "通用个人年终总结",
            "success": true
        });
        assert!(extract_body(&v).starts_with("# 年度工作概述"));
    }

    #[test]
    fn extract_body_handles_string_payload() {
        // Forward-compat fallback: `result` as a bare markdown string.
        let v = serde_json::json!({ "result": "# title\nbody" });
        assert_eq!(extract_body(&v), "# title\nbody");
    }

    #[test]
    fn extract_body_handles_markdown_field() {
        // Forward-compat fallback: `result.markdown`.
        let v = serde_json::json!({ "result": { "markdown": "# md" } });
        assert_eq!(extract_body(&v), "# md");
    }

    #[test]
    fn extract_body_handles_content_field() {
        let v = serde_json::json!({ "result": { "content": "raw text" } });
        assert_eq!(extract_body(&v), "raw text");
    }

    #[test]
    fn extract_body_returns_empty_when_no_known_field_present() {
        // No `markdown` / `content` / `body` / `text` anywhere — adapter
        // should return an empty string so the outer loop's
        // `body.trim().is_empty()` check skips the doc cleanly. Earlier
        // versions fell back to serialising the whole JSON payload,
        // which landed a JSON dump as the chunk content.
        let v = serde_json::json!({ "docUrl": "https://...", "nodeId": "n" });
        assert!(extract_body(&v).is_empty());
    }

    #[test]
    fn matches_text_doc_extension_accepts_adoc() {
        assert!(matches_text_doc_extension(&serde_json::json!({
            "extension": "adoc"
        })));
        assert!(matches_text_doc_extension(&serde_json::json!({
            "extension": "ADOC"
        })));
    }

    #[test]
    fn matches_text_doc_extension_rejects_spreadsheet_and_slides() {
        // Regression: `axls` (spreadsheet) and `apt` (slides) reach
        // `doc read` even with `--extensions adoc` because the dws
        // server treats the flag as a hint. The client-side filter
        // keeps them out so we never feed binary/structured content
        // through the markdown ingest path.
        assert!(!matches_text_doc_extension(&serde_json::json!({
            "extension": "axls"
        })));
        assert!(!matches_text_doc_extension(&serde_json::json!({
            "extension": "apt"
        })));
    }

    #[test]
    fn matches_text_doc_extension_admits_when_absent() {
        // Some legacy nodes don't tag the extension field at all.
        // Better to attempt the read and surface a meaningful error
        // than silently exclude.
        assert!(matches_text_doc_extension(&serde_json::json!({})));
    }

    #[test]
    fn extract_read_title_prefers_top_level_title() {
        let v = serde_json::json!({
            "title": "通用个人年终总结",
            "name": "ignored",
        });
        assert_eq!(
            extract_read_title(&v).as_deref(),
            Some("通用个人年终总结")
        );
    }

    #[test]
    fn extract_read_title_skips_whitespace_only_title() {
        let v = serde_json::json!({ "title": "  ", "name": "fallback" });
        assert_eq!(extract_read_title(&v).as_deref(), Some("fallback"));
    }

    #[test]
    fn extract_pat_grant_uri_recognises_permission_envelope() {
        // Regression: dws returns the permission-error envelope as a
        // successful (HTTP 200) JSON response with `success: false` +
        // `code: PAT_MEDIUM_RISK_NO_PERMISSION` + `data.uri`. The
        // adapter must detect this shape and skip the doc with a
        // helpful log line rather than ingesting the JSON dump as
        // markdown.
        let v = serde_json::json!({
            "code": "PAT_MEDIUM_RISK_NO_PERMISSION",
            "data": {
                "uri": "https://open-dev.dingtalk.com/fe/old?hash=%23%2FpersonalAuthorization%3FflowId%3Dabc",
                "requiredScopes": [{ "scope": "doc:read" }]
            },
            "success": false
        });
        assert_eq!(
            extract_pat_grant_uri(&v).as_deref(),
            Some("https://open-dev.dingtalk.com/fe/old?hash=%23%2FpersonalAuthorization%3FflowId%3Dabc")
        );
    }

    #[test]
    fn extract_pat_grant_uri_returns_none_on_success() {
        let v = serde_json::json!({
            "success": true,
            "markdown": "# body",
        });
        assert!(extract_pat_grant_uri(&v).is_none());
    }

    #[test]
    fn extract_pat_grant_uri_returns_none_for_other_error_codes() {
        // A different error code should NOT be treated as a PAT grant
        // requirement — the caller pushes it onto `fetch_errors`
        // instead so the user sees the real failure mode.
        let v = serde_json::json!({
            "success": false,
            "code": "SOME_OTHER_ERROR",
            "data": { "uri": "https://..." }
        });
        assert!(extract_pat_grant_uri(&v).is_none());
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
