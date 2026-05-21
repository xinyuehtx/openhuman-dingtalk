//! Live progress state for the DWS sync run.
//!
//! `sync_now` used to be a single blocking call that returned only after
//! every adapter (chat/doc/calendar/minutes) finished. For a fresh
//! install with months of dingtalk history the doc + minutes adapters
//! routinely take well past the 30s client-side RPC timeout, and the UI
//! had no way to show "we're 60% in, currently fetching doc 12/30" — the
//! button just said "同步中…" forever.
//!
//! This module hosts a process-wide [`DwsSyncProgress`] snapshot that the
//! adapter loop updates as it walks the categories. A read-only RPC
//! ([`crate::openhuman::config::ops::dws_sync_progress`]) exposes it
//! verbatim so the UI can poll every ~500ms and render per-category state
//! while the background sync task drains.
//!
//! ## Concurrency model
//!
//! - The progress slot is a `Mutex<Option<DwsSyncProgress>>`. Only ever
//!   one entry — overwritten on each new run, never queued.
//! - [`begin_run`] takes the lock, replaces the slot with a fresh
//!   `DwsSyncProgress` keyed by a new `run_id`, and returns that id so
//!   the caller can correlate.
//! - [`update_category`] / [`finish_run`] mutate the existing entry in
//!   place. If a newer run has replaced the slot mid-update the helper
//!   is a no-op (we don't want a stale adapter's finishing write to
//!   clobber the new run's state).
//! - [`snapshot`] clones the current entry so the RPC handler doesn't
//!   hold the lock across an `await`.
//!
//! ## Re-entrancy
//!
//! `sync_now` itself is gated against double-firing via
//! [`is_running_now`] — a second click while a run is in flight returns
//! the existing `run_id` rather than queuing a parallel sync. That keeps
//! `last_synced_at` updates simple and avoids two adapters fighting over
//! the same dws process budget.

use std::sync::Mutex;

use serde::Serialize;

use super::categories::DwsSyncCategory;
use super::run::now_unix_secs;

/// Whole-run progress snapshot. Cloned out of the global slot by
/// [`snapshot`] so the RPC handler returns a stable view.
#[derive(Clone, Debug, Serialize)]
pub struct DwsSyncProgress {
    /// Unique id for this run (16 hex chars). The UI uses this to
    /// detect "the run I asked to poll has been superseded by a newer
    /// one" — when `snapshot().run_id` no longer matches the one
    /// `begin_run` returned, the poll should stop.
    pub run_id: String,
    /// Unix seconds when [`begin_run`] was called.
    pub started_at: u64,
    /// Unix seconds when [`finish_run`] was called. `None` while
    /// any category is still `Pending` or `Running`.
    pub finished_at: Option<u64>,
    /// Per-category state, in the same order the caller passed to
    /// [`begin_run`]. Stays the same shape across the run's lifetime —
    /// only the `state` field of each entry mutates.
    pub categories: Vec<CategoryProgress>,
}

impl DwsSyncProgress {
    /// Convenience: number of categories that have settled (Done or
    /// Failed). Used by the UI for the headline "x / N" counter.
    pub fn completed_count(&self) -> usize {
        self.categories
            .iter()
            .filter(|c| matches!(c.state, CategoryState::Done { .. } | CategoryState::Failed { .. }))
            .count()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CategoryProgress {
    pub category: DwsSyncCategory,
    pub state: CategoryState,
}

/// Per-category lifecycle. Order matches the actual transitions the
/// adapter loop drives: `Pending → Running → Done | Failed`.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CategoryState {
    /// Not started yet. Default after [`begin_run`].
    Pending,
    /// Adapter is currently fetching. `current` / `total` describe
    /// sub-progress when the adapter knows the page count up front
    /// (e.g. doc adapter knows it'll fetch up to N bodies). Adapters
    /// with no sub-progress hint set `total=None` and the UI shows a
    /// spinner without a fraction.
    Running {
        current: u64,
        total: Option<u64>,
        /// Optional human-readable label, e.g. "fetching doc bodies".
        /// Cap length to keep wire payloads bounded; the UI may
        /// truncate further.
        label: Option<String>,
    },
    /// Adapter finished successfully. `records` is the raw item count
    /// pulled from dws; `chunks` is what landed in the memory tree
    /// after admission. They differ when the fast-score path drops
    /// some chunks, or when the source-id dedup gate suppresses
    /// already-known sources.
    Done {
        records: u64,
        chunks: u64,
    },
    /// Adapter failed end-to-end (or partial failure where 0 chunks
    /// were written and at least one item errored).
    Failed {
        error: String,
    },
}

static PROGRESS: Mutex<Option<DwsSyncProgress>> = Mutex::new(None);

/// Generate a stable 16-hex-char run id. Pattern mirrors the
/// obsidian-register vault id format so log-grep idioms stay
/// consistent across the codebase.
fn new_run_id() -> String {
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    // Mix in the thread id so simultaneous calls in a multi-threaded
    // runtime produce different ids even when the wall clock reads the
    // same nanosecond.
    std::thread::current().id().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Replace the progress slot with a fresh run keyed by the returned
/// `run_id`. Caller passes the category list in the order the adapter
/// loop will visit them.
pub fn begin_run(categories: &[DwsSyncCategory]) -> String {
    let run_id = new_run_id();
    let progress = DwsSyncProgress {
        run_id: run_id.clone(),
        started_at: now_unix_secs(),
        finished_at: None,
        categories: categories
            .iter()
            .map(|&category| CategoryProgress {
                category,
                state: CategoryState::Pending,
            })
            .collect(),
    };
    if let Ok(mut guard) = PROGRESS.lock() {
        *guard = Some(progress);
    }
    log::info!(
        "[dws:sync:progress] run started id={} categories={:?}",
        run_id,
        categories
    );
    run_id
}

/// Update a single category's state for the active run identified by
/// `run_id`. A mismatch (e.g. a newer run replaced the slot) is a
/// silent no-op so a stale adapter can't trample fresh state.
pub fn update_category(run_id: &str, category: DwsSyncCategory, state: CategoryState) {
    let Ok(mut guard) = PROGRESS.lock() else {
        return;
    };
    let Some(progress) = guard.as_mut() else {
        return;
    };
    if progress.run_id != run_id {
        return;
    }
    if let Some(entry) = progress
        .categories
        .iter_mut()
        .find(|c| c.category == category)
    {
        log::debug!(
            "[dws:sync:progress] run={} category={:?} state={:?}",
            run_id,
            category,
            state
        );
        entry.state = state;
    }
}

/// Mark the current run finished. Only applies when `run_id` matches —
/// otherwise the slot already belongs to a newer run we don't want to
/// retroactively complete.
pub fn finish_run(run_id: &str) {
    let Ok(mut guard) = PROGRESS.lock() else {
        return;
    };
    let Some(progress) = guard.as_mut() else {
        return;
    };
    if progress.run_id != run_id {
        return;
    }
    progress.finished_at = Some(now_unix_secs());
    log::info!(
        "[dws:sync:progress] run finished id={} completed={}/{}",
        run_id,
        progress.completed_count(),
        progress.categories.len()
    );
}

/// Clone the current progress snapshot, if any. Used by the read RPC.
pub fn snapshot() -> Option<DwsSyncProgress> {
    PROGRESS.lock().ok().and_then(|g| g.clone())
}

/// True when a run is in flight (started but not yet finished). Used
/// by `sync_now` to short-circuit duplicate kicks.
pub fn is_running_now() -> Option<String> {
    let guard = PROGRESS.lock().ok()?;
    let progress = guard.as_ref()?;
    if progress.finished_at.is_none() {
        Some(progress.run_id.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share the global mutex; serialize via a guard so they
    /// don't trample each other's progress slot.
    fn lock_for_test() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::OnceLock;
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn reset() {
        if let Ok(mut g) = PROGRESS.lock() {
            *g = None;
        }
    }

    #[test]
    fn begin_run_seeds_pending_categories_in_order() {
        let _g = lock_for_test();
        reset();
        let run_id = begin_run(&[DwsSyncCategory::Chat, DwsSyncCategory::Doc]);
        assert_eq!(run_id.len(), 16);
        let snap = snapshot().expect("just started");
        assert_eq!(snap.run_id, run_id);
        assert_eq!(snap.categories.len(), 2);
        assert_eq!(snap.categories[0].category, DwsSyncCategory::Chat);
        assert!(matches!(snap.categories[0].state, CategoryState::Pending));
        assert!(snap.finished_at.is_none());
    }

    #[test]
    fn update_category_changes_state_for_matching_run() {
        let _g = lock_for_test();
        reset();
        let id = begin_run(&[DwsSyncCategory::Chat]);
        update_category(
            &id,
            DwsSyncCategory::Chat,
            CategoryState::Running {
                current: 1,
                total: Some(3),
                label: None,
            },
        );
        let snap = snapshot().unwrap();
        match &snap.categories[0].state {
            CategoryState::Running { current, total, .. } => {
                assert_eq!(*current, 1);
                assert_eq!(*total, Some(3));
            }
            other => panic!("expected Running, got {other:?}"),
        }
    }

    #[test]
    fn update_category_is_noop_on_run_id_mismatch() {
        let _g = lock_for_test();
        reset();
        let id1 = begin_run(&[DwsSyncCategory::Chat]);
        let id2 = begin_run(&[DwsSyncCategory::Chat]);
        assert_ne!(id1, id2);
        // The first run's id no longer owns the slot — stale update
        // must not touch it.
        update_category(
            &id1,
            DwsSyncCategory::Chat,
            CategoryState::Done {
                records: 99,
                chunks: 99,
            },
        );
        let snap = snapshot().unwrap();
        assert_eq!(snap.run_id, id2);
        assert!(matches!(snap.categories[0].state, CategoryState::Pending));
    }

    #[test]
    fn finish_run_sets_timestamp_and_completed_count() {
        let _g = lock_for_test();
        reset();
        let id = begin_run(&[DwsSyncCategory::Chat, DwsSyncCategory::Doc]);
        update_category(
            &id,
            DwsSyncCategory::Chat,
            CategoryState::Done {
                records: 5,
                chunks: 4,
            },
        );
        update_category(
            &id,
            DwsSyncCategory::Doc,
            CategoryState::Failed {
                error: "boom".into(),
            },
        );
        finish_run(&id);
        let snap = snapshot().unwrap();
        assert!(snap.finished_at.is_some());
        assert_eq!(snap.completed_count(), 2);
    }

    #[test]
    fn is_running_now_reflects_lifecycle() {
        let _g = lock_for_test();
        reset();
        assert!(is_running_now().is_none());
        let id = begin_run(&[DwsSyncCategory::Chat]);
        assert_eq!(is_running_now().as_deref(), Some(id.as_str()));
        finish_run(&id);
        assert!(is_running_now().is_none());
    }
}
