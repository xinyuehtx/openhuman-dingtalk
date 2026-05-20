//! DWS (DingTalk Workspace CLI) periodic sync scheduler.
//!
//! Runs `dws` commands on a configurable cadence to pull data from selected
//! DingTalk product categories (calendar, todo, contacts, …) and persist
//! per-category timestamps so subsequent runs only fetch what's new.
//!
//! Design notes:
//! - Single live scheduler per process, swap-able via `start_or_restart`.
//! - First run for a category fetches "today" (00:00 local time onward);
//!   subsequent runs use the last successful sync timestamp as the lower
//!   bound and "now" as the upper bound.
//! - Each category builds its own command — the dws CLI is heterogeneous:
//!   some lists need `--start/--end`, attendance needs `--user/--date`,
//!   mail needs `--email/--query`. We probe `dws contact user get-self`
//!   and `dws mail mailbox list` once per sync to fill those in.
//! - The "common `--since` flag with fallback" pattern from the first cut
//!   was bogus — no real dws list command exposes `--since`. Removed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::interval;

use super::extended_path_for_dws;

/// Per-category dws sync command timeout.
const CATEGORY_TIMEOUT_SECS: u64 = 120;

/// Categories of DingTalk data that can be periodically synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DwsSyncCategory {
    Calendar,
    Todo,
    Contact,
    Attendance,
    Approval,
    Report,
    Mail,
    Doc,
    Chat,
}

impl DwsSyncCategory {
    /// Human-readable label for this category.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Calendar => "日历",
            Self::Todo => "待办",
            Self::Contact => "通讯录",
            Self::Attendance => "考勤",
            Self::Approval => "审批",
            Self::Report => "日志",
            Self::Mail => "邮箱",
            Self::Doc => "文档",
            Self::Chat => "群聊",
        }
    }

    /// Stable string id used in the persisted state file.
    pub fn state_key(&self) -> &'static str {
        match self {
            Self::Calendar => "calendar",
            Self::Todo => "todo",
            Self::Contact => "contact",
            Self::Attendance => "attendance",
            Self::Approval => "approval",
            Self::Report => "report",
            Self::Mail => "mail",
            Self::Doc => "doc",
            Self::Chat => "chat",
        }
    }

    /// Whether this category needs the current user's `userId` from
    /// `dws contact user get-self`. Used to decide which probes to run.
    fn needs_user_id(&self) -> bool {
        matches!(self, Self::Attendance)
    }

    /// Whether this category needs the current user's primary mailbox
    /// address from `dws mail mailbox list`.
    fn needs_email(&self) -> bool {
        matches!(self, Self::Mail)
    }

    /// Build the full shell command for one sync attempt.
    ///
    /// `since` is the lower bound of the sync window in unix seconds,
    /// `now` is the upper bound. `ctx` carries probed values (user_id,
    /// email) that some categories need.
    fn build_command(&self, since: u64, now: u64, ctx: &SyncContext) -> Result<String, String> {
        let start = format_iso_local(since);
        let end = format_iso_local(now);
        match self {
            Self::Calendar => Ok(format!(
                "dws calendar event list --start {start} --end {end} --format json"
            )),
            Self::Todo => Ok(
                "dws todo task list --page 1 --size 50 --status false --format json".into(),
            ),
            // No real "list all users" in dws — `get-self` is the only
            // no-arg call. It both validates the connection and gives us
            // the current user's profile to ingest.
            Self::Contact => Ok("dws contact user get-self --format json".into()),
            Self::Attendance => {
                let user = ctx
                    .user_id
                    .as_deref()
                    .ok_or_else(|| "missing user_id (contact get-self probe failed)".to_string())?;
                let date = today_local_date();
                Ok(format!(
                    "dws attendance record get --user {user} --date {date} --format json"
                ))
            }
            Self::Approval => Ok(format!(
                "dws oa approval list-pending --start {start} --end {end} --size 20 --format json"
            )),
            Self::Report => Ok(format!(
                "dws report list --start {start} --end {end} --size 20 --format json"
            )),
            Self::Mail => {
                let email = ctx
                    .email
                    .as_deref()
                    .ok_or_else(|| "missing mailbox email (mail mailbox probe failed)".to_string())?;
                let kql_since = format_kql_utc(since);
                // KQL is single-quoted so the `>=` and ISO timestamp survive `sh -c`.
                Ok(format!(
                    "dws mail message search --email {email} --query 'date>={kql_since}' --size 20 --format json"
                ))
            }
            Self::Doc => Ok("dws doc list --format json".into()),
            Self::Chat => Ok("dws chat list-top-conversations --limit 20 --format json".into()),
        }
    }
}

// ── Persisted state ─────────────────────────────────────────────────────────

/// On-disk record of when each category was last successfully synced.
///
/// Stored at `<workspace>/dws_sync_state.json` so the timestamps survive
/// restarts and the next sync can request only the delta.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DwsSyncState {
    /// Map keyed by [`DwsSyncCategory::state_key`] → unix seconds.
    #[serde(default)]
    pub last_synced_at: HashMap<String, u64>,
}

const STATE_FILE_NAME: &str = "dws_sync_state.json";

fn state_path(workspace_dir: &std::path::Path) -> PathBuf {
    workspace_dir.join(STATE_FILE_NAME)
}

/// Workspace dir resolved at runtime via the loaded config. Loaders fall
/// back to `None` when the config can't be read so callers stay best-effort.
async fn current_workspace_dir() -> Option<PathBuf> {
    crate::openhuman::config::load_config_with_timeout()
        .await
        .ok()
        .map(|cfg| cfg.workspace_dir)
}

/// Read the persisted sync state. Returns an empty struct on first launch
/// or any I/O / parse error so a corrupt file doesn't wedge sync.
pub async fn load_state(workspace_dir: &std::path::Path) -> DwsSyncState {
    let path = state_path(workspace_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "[dws:sync] failed to parse state file, resetting to empty"
            );
            DwsSyncState::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => DwsSyncState::default(),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "[dws:sync] failed to read state file, treating as empty"
            );
            DwsSyncState::default()
        }
    }
}

/// Write the persisted sync state. Errors are logged but never propagated —
/// a failed write should not turn a successful pull into a user-visible error.
pub async fn save_state(workspace_dir: &std::path::Path, state: &DwsSyncState) {
    let path = state_path(workspace_dir);
    if let Some(parent) = path.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "[dws:sync] failed to mkdir for state file"
            );
            return;
        }
    }
    match serde_json::to_vec_pretty(state) {
        Ok(bytes) => {
            if let Err(err) = tokio::fs::write(&path, bytes).await {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "[dws:sync] failed to write state file"
                );
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "[dws:sync] state serialization error");
        }
    }
}

// ── Single sync run ─────────────────────────────────────────────────────────

/// Result of a single category sync operation.
#[derive(Debug, Clone, Serialize)]
pub struct SyncCategoryResult {
    pub category: DwsSyncCategory,
    pub success: bool,
    pub records_count: usize,
    /// Unix seconds when the sync finished (only set on success).
    pub last_synced_at: Option<u64>,
    pub error: Option<String>,
}

/// Result of a full sync run (one or more categories).
#[derive(Debug, Clone, Serialize)]
pub struct DwsSyncResult {
    pub results: Vec<SyncCategoryResult>,
    pub started_at: u64,
    pub finished_at: u64,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Returns the unix seconds for today's local 00:00.
fn today_start_unix() -> u64 {
    use chrono::{Local, TimeZone};
    let now = Local::now().date_naive();
    Local
        .from_local_datetime(&now.and_hms_opt(0, 0, 0).unwrap_or_default())
        .single()
        .map(|dt| dt.timestamp().max(0) as u64)
        .unwrap_or_else(now_unix_secs)
}

/// Today's local date as `YYYY-MM-DD`, used by `dws attendance record get`.
fn today_local_date() -> String {
    use chrono::Local;
    Local::now().format("%Y-%m-%d").to_string()
}

/// Format a unix timestamp as ISO-8601 in **local** time with offset
/// (e.g. `2026-05-20T20:49:33+08:00`). Matches the format dws's `--start`
/// / `--end` flags expect (see `dws report list --help`, etc.).
fn format_iso_local(ts: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Format a unix timestamp as KQL-friendly UTC ISO-8601 (`YYYY-MM-DDTHH:MM:SSZ`).
/// Used in the `--query 'date>=...'` predicate of `dws mail message search`.
fn format_kql_utc(ts: u64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Spawn a single dws command. Returns `Ok((stdout, stderr, exit_code))` on
/// completion and a string error on timeout / spawn failure.
async fn run_dws(command: &str) -> Result<(String, String, i32), String> {
    let extended_path = extended_path_for_dws();
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

fn count_records(stdout: &str) -> usize {
    serde_json::from_str::<Value>(stdout)
        .ok()
        .and_then(|val| {
            if let Some(arr) = val.as_array() {
                Some(arr.len())
            } else if let Some(obj) = val.as_object() {
                // Many CLIs wrap the array under `data` / `items` / `records` /
                // `result`. dws's own envelope is `{ result: [...], success: true }`.
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

// ── Probes for category-specific required args ──────────────────────────────

/// Per-sync-run probed context. Populated once at the top of `sync_now`
/// for the categories that actually need it, then threaded into each
/// `build_command` call.
#[derive(Debug, Default, Clone)]
struct SyncContext {
    /// Current user's dingtalk `userId` — required by attendance.
    user_id: Option<String>,
    /// Current user's primary mailbox address — required by mail.
    email: Option<String>,
}

/// Best-effort: ask `dws contact user get-self` for the current user's
/// `userId`. Returns `None` if the command fails or the response shape
/// is unrecognised so the dependent categories can fail cleanly with a
/// "missing user_id" error rather than crashing the whole run.
async fn probe_self_user_id() -> Option<String> {
    let (stdout, _stderr, code) = run_dws("dws contact user get-self --format json")
        .await
        .ok()?;
    if code != 0 {
        tracing::warn!(
            exit_code = code,
            "[dws:sync] probe: contact user get-self exited non-zero"
        );
        return None;
    }
    let v: Value = serde_json::from_str(&stdout).ok()?;
    let id = v
        .get("result")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("orgEmployeeModel"))
        .and_then(|m| m.get("userId"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());
    if id.is_none() {
        tracing::warn!("[dws:sync] probe: get-self response missing orgEmployeeModel.userId");
    }
    id
}

/// Best-effort: ask `dws mail mailbox list` for the primary mailbox
/// address. We scan the response for the first email-looking string
/// since the field name varies across dws versions.
async fn probe_primary_email() -> Option<String> {
    let (stdout, _stderr, code) = run_dws("dws mail mailbox list --format json").await.ok()?;
    if code != 0 {
        tracing::warn!(
            exit_code = code,
            "[dws:sync] probe: mail mailbox list exited non-zero"
        );
        return None;
    }
    let v: Value = serde_json::from_str(&stdout).ok()?;
    let email = find_email(&v);
    if email.is_none() {
        tracing::warn!("[dws:sync] probe: mail mailbox response had no email-looking field");
    }
    email
}

fn find_email(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if s.contains('@') && !s.contains(char::is_whitespace) => Some(s.clone()),
        Value::Array(arr) => arr.iter().find_map(find_email),
        Value::Object(map) => {
            for key in ["email", "mailbox", "address", "mailAddress"] {
                if let Some(s) = map.get(key).and_then(|x| x.as_str()) {
                    if s.contains('@') {
                        return Some(s.to_string());
                    }
                }
            }
            map.values().find_map(find_email)
        }
        _ => None,
    }
}

async fn build_sync_context(categories: &[DwsSyncCategory]) -> SyncContext {
    let needs_user_id = categories.iter().any(DwsSyncCategory::needs_user_id);
    let needs_email = categories.iter().any(DwsSyncCategory::needs_email);

    let user_id = if needs_user_id {
        probe_self_user_id().await
    } else {
        None
    };
    let email = if needs_email {
        probe_primary_email().await
    } else {
        None
    };

    tracing::debug!(
        needs_user_id,
        needs_email,
        user_id_resolved = user_id.is_some(),
        email_resolved = email.is_some(),
        "[dws:sync] probed context"
    );

    SyncContext { user_id, email }
}

async fn sync_one(
    category: DwsSyncCategory,
    since: u64,
    now: u64,
    ctx: &SyncContext,
) -> SyncCategoryResult {
    let command = match category.build_command(since, now, ctx) {
        Ok(cmd) => cmd,
        Err(err) => {
            tracing::warn!(
                category = ?category,
                error = %err,
                "[dws:sync] cannot build command, missing required context"
            );
            return SyncCategoryResult {
                category,
                success: false,
                records_count: 0,
                last_synced_at: None,
                error: Some(err),
            };
        }
    };

    tracing::debug!(
        category = ?category,
        command = %command,
        "[dws:sync] running"
    );

    match run_dws(&command).await {
        Err(err) => {
            tracing::warn!(
                category = ?category,
                error = %err,
                "[dws:sync] category failed"
            );
            SyncCategoryResult {
                category,
                success: false,
                records_count: 0,
                last_synced_at: None,
                error: Some(err),
            }
        }
        Ok((stdout, stderr, code)) => {
            if code == 0 {
                let records_count = count_records(&stdout);
                tracing::info!(
                    category = ?category,
                    records_count = records_count,
                    "[dws:sync] category ok"
                );
                SyncCategoryResult {
                    category,
                    success: true,
                    records_count,
                    last_synced_at: Some(now_unix_secs()),
                    error: None,
                }
            } else {
                let error = format!(
                    "dws exited with code {code}: {}{}",
                    stderr.trim(),
                    if stdout.trim().is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", stdout.trim())
                    }
                );
                tracing::warn!(
                    category = ?category,
                    error = %error,
                    "[dws:sync] category failed"
                );
                SyncCategoryResult {
                    category,
                    success: false,
                    records_count: 0,
                    last_synced_at: None,
                    error: Some(error),
                }
            }
        }
    }
}

/// Resolve the timestamp to use as the lower bound for this category's
/// pull. Falls back to today 00:00 when the category has never been synced
/// before — the spec is "first pull today, subsequent pulls incremental".
fn resolve_since(state: &DwsSyncState, category: DwsSyncCategory) -> u64 {
    state
        .last_synced_at
        .get(category.state_key())
        .copied()
        .unwrap_or_else(today_start_unix)
}

/// Immediately sync the specified categories. Reads & updates the persisted
/// state for incremental pulls, returns one result per category.
pub async fn sync_now(categories: &[DwsSyncCategory]) -> DwsSyncResult {
    let started_at = now_unix_secs();
    let workspace = current_workspace_dir().await;
    let mut state = match workspace.as_deref() {
        Some(dir) => load_state(dir).await,
        None => DwsSyncState::default(),
    };

    tracing::info!(
        categories = ?categories,
        "[dws:sync] starting immediate sync"
    );

    let ctx = build_sync_context(categories).await;

    let mut results = Vec::with_capacity(categories.len());
    for &category in categories {
        let since = resolve_since(&state, category);
        let result = sync_one(category, since, started_at, &ctx).await;
        if result.success {
            if let Some(ts) = result.last_synced_at {
                state
                    .last_synced_at
                    .insert(category.state_key().to_string(), ts);
            }
        }
        results.push(result);
    }

    if let Some(dir) = workspace.as_deref() {
        save_state(dir, &state).await;
    }

    let finished_at = now_unix_secs();

    tracing::info!(
        duration_secs = finished_at.saturating_sub(started_at),
        success_count = results.iter().filter(|r| r.success).count(),
        fail_count = results.iter().filter(|r| !r.success).count(),
        "[dws:sync] immediate sync completed"
    );

    DwsSyncResult {
        results,
        started_at,
        finished_at,
    }
}

// ── Periodic scheduler ──────────────────────────────────────────────────────

struct SchedulerHandle {
    interval_minutes: u32,
    categories: Vec<DwsSyncCategory>,
    join: JoinHandle<()>,
}

static SCHEDULER: Mutex<Option<SchedulerHandle>> = Mutex::new(None);

/// Stop the periodic scheduler if one is running.
pub fn stop_periodic_sync() {
    if let Some(prev) = SCHEDULER.lock().ok().and_then(|mut guard| guard.take()) {
        prev.join.abort();
        tracing::info!("[dws:sync] periodic scheduler stopped");
    }
}

/// Spawn (or replace) the periodic DWS sync background task.
///
/// Re-entrant: when called with the same `(interval_minutes, categories)`
/// while a scheduler is already running, this is a no-op. Called with a new
/// configuration, it cancels the previous task and starts a fresh one — this
/// lets the UI flip the switch and have the change take effect immediately
/// without restarting the core.
///
/// `categories.is_empty()` is treated as "nothing to do" and stops any
/// running scheduler.
pub fn start_or_restart(interval_minutes: u32, categories: Vec<DwsSyncCategory>) {
    let mut guard = match SCHEDULER.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    if categories.is_empty() {
        if let Some(prev) = guard.take() {
            prev.join.abort();
            tracing::info!("[dws:sync] no enabled categories, scheduler stopped");
        }
        return;
    }

    let interval_mins = interval_minutes.max(5);

    if let Some(existing) = guard.as_ref() {
        if existing.interval_minutes == interval_mins && existing.categories == categories {
            tracing::debug!("[dws:sync] scheduler already running with same config, skipping");
            return;
        }
        existing.join.abort();
    }

    let cats_for_task = categories.clone();
    let join = tokio::spawn(async move {
        tracing::info!(
            interval_minutes = interval_mins,
            categories = ?cats_for_task,
            "[dws:sync] periodic scheduler starting"
        );
        let mut timer = interval(Duration::from_secs(u64::from(interval_mins) * 60));
        // First tick fires immediately; skip it so we don't blast off the
        // moment a user toggles the switch — they may still be configuring.
        timer.tick().await;
        loop {
            timer.tick().await;
            tracing::debug!("[dws:sync] periodic tick — running sync");
            let _ = sync_now(&cats_for_task).await;
        }
    });

    *guard = Some(SchedulerHandle {
        interval_minutes: interval_mins,
        categories,
        join,
    });
}

/// Apply a fresh `DwsSyncConfig` to the running scheduler. Convenience wrapper
/// that translates the per-category booleans into the categories vector and
/// stops the scheduler when the master switch is off.
pub fn apply_config(config: &crate::openhuman::config::DwsSyncConfig) {
    if !config.enabled {
        stop_periodic_sync();
        return;
    }
    let cats = enabled_categories(&config.categories);
    start_or_restart(config.interval_minutes, cats);
}

/// Translate the per-category booleans into the enum list used by the
/// scheduler / sync_now.
pub fn enabled_categories(
    cats: &crate::openhuman::config::DwsSyncCategories,
) -> Vec<DwsSyncCategory> {
    let mut out = Vec::new();
    if cats.calendar {
        out.push(DwsSyncCategory::Calendar);
    }
    if cats.todo {
        out.push(DwsSyncCategory::Todo);
    }
    if cats.contact {
        out.push(DwsSyncCategory::Contact);
    }
    if cats.attendance {
        out.push(DwsSyncCategory::Attendance);
    }
    if cats.approval {
        out.push(DwsSyncCategory::Approval);
    }
    if cats.report {
        out.push(DwsSyncCategory::Report);
    }
    if cats.mail {
        out.push(DwsSyncCategory::Mail);
    }
    if cats.doc {
        out.push(DwsSyncCategory::Doc);
    }
    if cats.chat {
        out.push(DwsSyncCategory::Chat);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_empty() -> SyncContext {
        SyncContext::default()
    }

    fn ctx_full() -> SyncContext {
        SyncContext {
            user_id: Some("274264".into()),
            email: Some("alice@example.com".into()),
        }
    }

    #[test]
    fn calendar_command_uses_local_iso_window() {
        let cmd = DwsSyncCategory::Calendar
            .build_command(1_716_153_600, 1_716_240_000, &ctx_empty())
            .expect("calendar build");
        assert!(cmd.starts_with("dws calendar event list "));
        assert!(cmd.contains("--start "));
        assert!(cmd.contains("--end "));
        assert!(cmd.contains("--format json"));
        // Local ISO has a trailing offset like `+08:00` (or `-07:00`).
        assert!(cmd.matches('T').count() >= 2);
    }

    #[test]
    fn todo_command_paginates_open_tasks() {
        let cmd = DwsSyncCategory::Todo
            .build_command(0, 0, &ctx_empty())
            .unwrap();
        assert_eq!(
            cmd,
            "dws todo task list --page 1 --size 50 --status false --format json"
        );
    }

    #[test]
    fn contact_command_is_get_self() {
        let cmd = DwsSyncCategory::Contact
            .build_command(0, 0, &ctx_empty())
            .unwrap();
        assert_eq!(cmd, "dws contact user get-self --format json");
    }

    #[test]
    fn attendance_requires_user_id() {
        let err = DwsSyncCategory::Attendance
            .build_command(0, 0, &ctx_empty())
            .expect_err("must fail without user_id");
        assert!(err.contains("user_id"), "error mentions user_id: {err}");

        let cmd = DwsSyncCategory::Attendance
            .build_command(0, 0, &ctx_full())
            .expect("attendance build with user_id");
        assert!(cmd.starts_with("dws attendance record get "));
        assert!(cmd.contains("--user 274264"));
        assert!(cmd.contains("--date "));
    }

    #[test]
    fn approval_uses_list_pending_with_window() {
        let cmd = DwsSyncCategory::Approval
            .build_command(1_716_153_600, 1_716_240_000, &ctx_empty())
            .unwrap();
        assert!(cmd.starts_with("dws oa approval list-pending "));
        assert!(cmd.contains("--start "));
        assert!(cmd.contains("--end "));
        assert!(cmd.contains("--size 20"));
    }

    #[test]
    fn report_uses_iso_window() {
        let cmd = DwsSyncCategory::Report
            .build_command(1_716_153_600, 1_716_240_000, &ctx_empty())
            .unwrap();
        assert!(cmd.starts_with("dws report list "));
        assert!(cmd.contains("--start "));
        assert!(cmd.contains("--end "));
        assert!(cmd.contains("--size 20"));
    }

    #[test]
    fn mail_requires_email_and_quotes_kql() {
        let err = DwsSyncCategory::Mail
            .build_command(0, 0, &ctx_empty())
            .expect_err("must fail without email");
        assert!(err.contains("email"), "error mentions email: {err}");

        let cmd = DwsSyncCategory::Mail
            .build_command(1_716_153_600, 1_716_240_000, &ctx_full())
            .expect("mail build with email");
        assert!(cmd.contains("--email alice@example.com"));
        assert!(cmd.contains("--query 'date>="));
        assert!(cmd.contains("Z'"), "KQL date should end with Z");
    }

    #[test]
    fn doc_command_has_no_required_args() {
        let cmd = DwsSyncCategory::Doc
            .build_command(0, 0, &ctx_empty())
            .unwrap();
        assert_eq!(cmd, "dws doc list --format json");
    }

    #[test]
    fn chat_uses_top_conversations_with_limit() {
        let cmd = DwsSyncCategory::Chat
            .build_command(0, 0, &ctx_empty())
            .unwrap();
        assert_eq!(
            cmd,
            "dws chat list-top-conversations --limit 20 --format json"
        );
    }

    #[test]
    fn count_records_handles_dws_envelope() {
        // dws's own response shape: { result: [...], success: true }
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
    fn find_email_prefers_named_keys() {
        let v: Value = serde_json::json!({
            "result": [
                { "displayName": "Alice", "email": "alice@example.com" }
            ]
        });
        assert_eq!(find_email(&v).as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn find_email_falls_back_to_any_email_string() {
        let v: Value = serde_json::json!({
            "result": [{ "weird_field": "bob@example.com" }]
        });
        assert_eq!(find_email(&v).as_deref(), Some("bob@example.com"));
    }
}
