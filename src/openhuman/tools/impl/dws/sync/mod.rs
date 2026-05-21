//! DWS (DingTalk Workspace CLI) periodic sync scheduler — v2.
//!
//! Pulls content from the dws-authenticated user's DingTalk account on a
//! configurable cadence and feeds every successful pull into the openhuman
//! memory tree via `memory::tree::ingest_chat / ingest_email / ingest_document`.
//! v1 only counted records; v2 actually ingests.
//!
//! Design notes:
//! - Single live scheduler per process, swap-able via `start_or_restart`.
//! - Cold-start window = `now - 1h`; subsequent runs use the last successful
//!   per-category timestamp as the lower bound.
//! - Cursor advances only after the adapter returns Ok, so a failure
//!   transparently retries the same window on the next tick.
//! - Re-ingest overlap is safe: `ingest_chat`/`ingest_email` dedupe at the
//!   chunk level (content-hashed `chunk_id`); `ingest_document` short-circuits
//!   when the `source_id` has already been ingested.
//! - All ingested chunks are tagged with the dws account's owner key
//!   (`dingtalk:<corp_id>:<user_id>`) so memory partitions correctly when
//!   multiple accounts share a workspace.

pub mod adapters;
pub mod categories;
pub mod owner;
pub mod progress;
pub mod run;

pub use categories::DwsSyncCategory;
pub use progress::{snapshot as progress_snapshot, DwsSyncProgress};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::time::interval;

use self::adapters::SyncCategoryResult;
use self::run::now_unix_secs;

// Cold-start windows are per-category — see
// [`DwsSyncCategory::cold_start_seconds`]. Doc / Minutes use 30 days
// because their activity is sparse and a 1-hour first-sync window
// regularly returned zero items; Chat / Calendar stick to 1 hour.

// ── Persisted state ─────────────────────────────────────────────────────────

/// On-disk record of when each category was last successfully synced.
/// Stored at `<workspace>/dws_sync_state.json` so the timestamps survive
/// restarts and the next tick can request only the delta.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DwsSyncState {
    /// Map keyed by [`DwsSyncCategory::state_key`] → unix seconds.
    #[serde(default)]
    pub last_synced_at: HashMap<String, u64>,
}

const STATE_FILE_NAME: &str = "dws_sync_state.json";

fn state_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(STATE_FILE_NAME)
}

/// Read the persisted sync state. Returns an empty struct on first launch
/// or any I/O / parse error so a corrupt file doesn't wedge sync.
pub async fn load_state(workspace_dir: &Path) -> DwsSyncState {
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

/// Drop the `last_synced_at` cursors for the named categories so the
/// next `sync_now` falls through to the category's [`DwsSyncCategory::cold_start_seconds`]
/// lookback window. Pass an empty slice to clear ALL cursors.
///
/// Used by the "强制冷启动拉取" UI button to recover from a stuck
/// cursor — e.g. an earlier adapter bug landed cursor=now even
/// though zero records were ingested, so every subsequent tick had
/// an empty window and the user saw `records_count=0` forever.
///
/// Returns the list of category state-keys that were actually
/// cleared (so the UI can report "cleared X cursor(s)"). Best-effort:
/// I/O errors are logged and a write failure leaves the on-disk state
/// untouched — the in-memory cursor reset doesn't survive a restart
/// in that case, but a subsequent successful sync writes a fresh
/// state file.
pub async fn reset_cursors(workspace_dir: &Path, categories: &[DwsSyncCategory]) -> Vec<String> {
    let mut state = load_state(workspace_dir).await;
    let cleared: Vec<String> = if categories.is_empty() {
        let all: Vec<String> = state.last_synced_at.keys().cloned().collect();
        state.last_synced_at.clear();
        all
    } else {
        let keys: Vec<&str> = categories.iter().map(|c| c.state_key()).collect();
        let mut cleared = Vec::new();
        for key in &keys {
            if state.last_synced_at.remove(*key).is_some() {
                cleared.push((*key).to_string());
            }
        }
        cleared
    };
    tracing::info!(
        cleared = ?cleared,
        "[dws:sync] reset_cursors: dropped per-category last_synced_at"
    );
    save_state(workspace_dir, &state).await;
    cleared
}

/// Write the persisted sync state. Errors are logged but never propagated —
/// a failed write should not turn a successful pull into a user-visible error.
pub async fn save_state(workspace_dir: &Path, state: &DwsSyncState) {
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

// ── Top-level result types ──────────────────────────────────────────────────

/// Result of a full sync run (one or more categories).
#[derive(Debug, Clone, Serialize)]
pub struct DwsSyncResult {
    pub results: Vec<SyncCategoryResult>,
    pub started_at: u64,
    pub finished_at: u64,
}

// ── Cursor resolution ───────────────────────────────────────────────────────

fn resolve_since(state: &DwsSyncState, category: DwsSyncCategory, now: u64) -> u64 {
    state
        .last_synced_at
        .get(category.state_key())
        .copied()
        .unwrap_or_else(|| now.saturating_sub(category.cold_start_seconds()))
}

// ── sync_now entry point ────────────────────────────────────────────────────

/// Immediately sync the specified categories. Reads & updates the persisted
/// state for incremental pulls, returns one result per category. Drives the
/// global [`progress`] slot at each lifecycle transition so the
/// `config_dws_sync_progress` RPC can show per-category state to the UI.
pub async fn sync_now(categories: &[DwsSyncCategory]) -> DwsSyncResult {
    let run_id = progress::begin_run(categories);
    let result = sync_now_inner(categories, &run_id).await;
    progress::finish_run(&run_id);
    result
}

async fn sync_now_inner(categories: &[DwsSyncCategory], run_id: &str) -> DwsSyncResult {
    let started_at = now_unix_secs();
    let config = match crate::openhuman::config::load_config_with_timeout().await {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "[dws:sync] could not load config; aborting run");
            // Mark every category as Failed so the UI doesn't spin on
            // a Pending slot the worker will never visit.
            for &category in categories {
                progress::update_category(
                    run_id,
                    category,
                    progress::CategoryState::Failed {
                        error: format!("config load failed: {err}"),
                    },
                );
            }
            return DwsSyncResult {
                results: Vec::new(),
                started_at,
                finished_at: started_at,
            };
        }
    };
    let workspace = config.workspace_dir.clone();
    let mut state = load_state(&workspace).await;

    tracing::info!(
        run_id = %run_id,
        categories = ?categories,
        "[dws:sync] starting immediate sync"
    );

    let identity = owner::probe().await;

    let mut results = Vec::with_capacity(categories.len());
    for &category in categories {
        progress::update_category(
            run_id,
            category,
            progress::CategoryState::Running {
                current: 0,
                total: None,
                label: None,
            },
        );
        let since = resolve_since(&state, category, started_at);
        let result = adapters::dispatch(category, since, started_at, &identity, &config).await;
        if result.success {
            if let Some(ts) = result.last_synced_at {
                state
                    .last_synced_at
                    .insert(category.state_key().to_string(), ts);
            }
            progress::update_category(
                run_id,
                category,
                progress::CategoryState::Done {
                    records: result.records_count as u64,
                    chunks: result.chunks_written as u64,
                },
            );
        } else {
            progress::update_category(
                run_id,
                category,
                progress::CategoryState::Failed {
                    error: result
                        .error
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                },
            );
        }
        results.push(result);
    }

    save_state(&workspace, &state).await;

    let finished_at = now_unix_secs();
    tracing::info!(
        run_id = %run_id,
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

/// Kick off `sync_now` in a detached background task and return the
/// freshly-minted `run_id` immediately. The caller (RPC handler) hands
/// the id straight back to the UI, which then polls
/// `dws_sync_progress` until `finished_at` is set.
///
/// If a run is already in flight the new request is dropped and the
/// existing run's id is returned — duplicate clicks shouldn't queue a
/// second parallel sync (it'd fight over the dws process budget and
/// confuse the state-file cursor).
///
/// Returns `(run_id, started_fresh)`. `started_fresh=false` means the
/// caller is observing an in-flight run that someone else started.
pub fn spawn_sync_now(categories: Vec<DwsSyncCategory>) -> (String, bool) {
    if let Some(existing) = progress::is_running_now() {
        tracing::info!(
            run_id = %existing,
            "[dws:sync] spawn_sync_now: existing run in flight, returning its id"
        );
        return (existing, false);
    }
    // Pre-allocate the run id by calling begin_run synchronously — that
    // way the RPC response can include it even though the actual sync
    // work happens in the spawned task. `sync_now` would otherwise
    // mint a brand-new id (and overwrite the slot we just seeded), so
    // we drive the inner function directly inside the task.
    let run_id = progress::begin_run(&categories);
    let task_run_id = run_id.clone();
    tokio::spawn(async move {
        sync_now_inner(&categories, &task_run_id).await;
        progress::finish_run(&task_run_id);
    });
    (run_id, true)
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
/// scheduler / sync_now. Order matters — see [`DwsSyncCategory`] doc.
pub fn enabled_categories(
    cats: &crate::openhuman::config::DwsSyncCategories,
) -> Vec<DwsSyncCategory> {
    let mut out = Vec::new();
    if cats.chat {
        out.push(DwsSyncCategory::Chat);
    }
    if cats.doc {
        out.push(DwsSyncCategory::Doc);
    }
    if cats.calendar {
        out.push(DwsSyncCategory::Calendar);
    }
    if cats.minutes {
        out.push(DwsSyncCategory::Minutes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_window_is_one_hour_for_chat_and_calendar() {
        let state = DwsSyncState::default();
        let now = 1_716_240_000_u64;
        assert_eq!(
            resolve_since(&state, DwsSyncCategory::Chat, now),
            now - 3_600
        );
        assert_eq!(
            resolve_since(&state, DwsSyncCategory::Calendar, now),
            now - 3_600
        );
    }

    #[tokio::test]
    async fn reset_cursors_with_empty_list_clears_all() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        // Seed a state file with three cursors.
        let mut state = DwsSyncState::default();
        state.last_synced_at.insert("doc".into(), 100);
        state.last_synced_at.insert("chat".into(), 200);
        state.last_synced_at.insert("calendar".into(), 300);
        save_state(workspace, &state).await;
        let cleared = reset_cursors(workspace, &[]).await;
        assert_eq!(cleared.len(), 3);
        let after = load_state(workspace).await;
        assert!(after.last_synced_at.is_empty());
    }

    #[tokio::test]
    async fn reset_cursors_with_subset_drops_only_those_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let mut state = DwsSyncState::default();
        state.last_synced_at.insert("doc".into(), 100);
        state.last_synced_at.insert("chat".into(), 200);
        save_state(workspace, &state).await;
        let cleared = reset_cursors(workspace, &[DwsSyncCategory::Doc]).await;
        assert_eq!(cleared, vec!["doc".to_string()]);
        let after = load_state(workspace).await;
        // chat survived, doc dropped.
        assert!(after.last_synced_at.get("doc").is_none());
        assert_eq!(after.last_synced_at.get("chat"), Some(&200));
    }

    #[tokio::test]
    async fn reset_cursors_on_missing_key_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let mut state = DwsSyncState::default();
        state.last_synced_at.insert("chat".into(), 200);
        save_state(workspace, &state).await;
        // doc isn't present — should not appear in cleared list.
        let cleared = reset_cursors(workspace, &[DwsSyncCategory::Doc]).await;
        assert!(cleared.is_empty());
        let after = load_state(workspace).await;
        assert_eq!(after.last_synced_at.get("chat"), Some(&200));
    }

    #[test]
    fn cold_start_window_is_30_days_for_doc_and_minutes() {
        // Regression for "拉取文档列表为空" — a 1-hour cold start
        // almost never finds doc/minutes activity (users rarely edit
        // docs hourly). 30 days lets the first sync surface real
        // history; the persisted cursor keeps incremental ticks
        // narrow afterwards.
        let state = DwsSyncState::default();
        let now = 1_716_240_000_u64;
        let thirty_days = 30 * 24 * 3_600;
        assert_eq!(
            resolve_since(&state, DwsSyncCategory::Doc, now),
            now - thirty_days
        );
        assert_eq!(
            resolve_since(&state, DwsSyncCategory::Minutes, now),
            now - thirty_days
        );
    }

    #[test]
    fn cursor_uses_recorded_timestamp_when_present() {
        let mut state = DwsSyncState::default();
        state
            .last_synced_at
            .insert("chat".to_string(), 1_716_239_000);
        assert_eq!(
            resolve_since(&state, DwsSyncCategory::Chat, 1_716_240_000),
            1_716_239_000
        );
    }

    #[test]
    fn cold_start_does_not_underflow_on_tiny_now() {
        let state = DwsSyncState::default();
        let now = 100_u64;
        assert_eq!(resolve_since(&state, DwsSyncCategory::Chat, now), 0);
    }

    #[test]
    fn enabled_categories_preserves_processing_order() {
        let cats = crate::openhuman::config::DwsSyncCategories {
            chat: true,
            doc: true,
            calendar: true,
            minutes: true,
        };
        assert_eq!(
            enabled_categories(&cats),
            vec![
                DwsSyncCategory::Chat,
                DwsSyncCategory::Doc,
                DwsSyncCategory::Calendar,
                DwsSyncCategory::Minutes,
            ]
        );
    }

    #[test]
    fn enabled_categories_drops_disabled_entries() {
        let cats = crate::openhuman::config::DwsSyncCategories {
            chat: true,
            doc: true,
            calendar: false,
            minutes: false,
        };
        assert_eq!(
            enabled_categories(&cats),
            vec![DwsSyncCategory::Chat, DwsSyncCategory::Doc]
        );
    }
}
