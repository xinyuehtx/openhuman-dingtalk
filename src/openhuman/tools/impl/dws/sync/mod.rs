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
pub mod run;

pub use categories::DwsSyncCategory;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::time::interval;

use self::adapters::SyncCategoryResult;
use self::run::now_unix_secs;

/// Cold-start window for any category that has never synced before, in
/// seconds. v2 default: last hour.
const COLD_START_SECONDS: u64 = 3_600;

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
        .unwrap_or_else(|| now.saturating_sub(COLD_START_SECONDS))
}

// ── sync_now entry point ────────────────────────────────────────────────────

/// Immediately sync the specified categories. Reads & updates the persisted
/// state for incremental pulls, returns one result per category.
pub async fn sync_now(categories: &[DwsSyncCategory]) -> DwsSyncResult {
    let started_at = now_unix_secs();
    let config = match crate::openhuman::config::load_config_with_timeout().await {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "[dws:sync] could not load config; aborting run");
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
        categories = ?categories,
        "[dws:sync] starting immediate sync"
    );

    let identity = owner::probe().await;

    let mut results = Vec::with_capacity(categories.len());
    for &category in categories {
        let since = resolve_since(&state, category, started_at);
        let result = adapters::dispatch(category, since, started_at, &identity, &config).await;
        if result.success {
            if let Some(ts) = result.last_synced_at {
                state
                    .last_synced_at
                    .insert(category.state_key().to_string(), ts);
            }
        }
        results.push(result);
    }

    save_state(&workspace, &state).await;

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
    fn cold_start_window_is_one_hour_when_state_empty() {
        let state = DwsSyncState::default();
        let now = 1_716_240_000_u64;
        assert_eq!(
            resolve_since(&state, DwsSyncCategory::Chat, now),
            now - COLD_START_SECONDS
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

