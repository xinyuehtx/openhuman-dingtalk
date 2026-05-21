//! Per-category sync adapters.
//!
//! Each adapter implements one DingTalk content source: it builds the
//! relevant `dws` commands, parses the JSON, canonicalises records into
//! `memory::tree::canonicalize::*` payloads, and forwards them to the
//! matching `memory::tree::ingest_*` function. The dispatcher in this
//! module just routes to the right adapter and uniformly logs success /
//! failure.

pub mod calendar;
pub mod chat;
pub mod doc;
pub mod minutes;

use serde::Serialize;

use crate::openhuman::config::Config;

use super::categories::DwsSyncCategory;
use super::owner::OwnerIdentity;
use super::run::now_unix_secs;

/// Outcome of one category's sync attempt. `records_count` is the number
/// of raw records pulled from dws; `chunks_written` is the number of
/// memory chunks produced (sum across all `ingest_*` calls). Some chunks
/// may have been dropped by the fast-score path, hence the two numbers
/// can differ.
#[derive(Debug, Clone, Serialize)]
pub struct SyncCategoryResult {
    pub category: DwsSyncCategory,
    pub success: bool,
    pub records_count: usize,
    #[serde(default)]
    pub chunks_written: usize,
    /// Unix seconds when the sync finished (only set on success). Caller
    /// writes this into `DwsSyncState.last_synced_at` so the next tick can
    /// resume from here.
    pub last_synced_at: Option<u64>,
    pub error: Option<String>,
}

impl SyncCategoryResult {
    pub fn ok(category: DwsSyncCategory, records: usize, chunks: usize) -> Self {
        Self {
            category,
            success: true,
            records_count: records,
            chunks_written: chunks,
            last_synced_at: Some(now_unix_secs()),
            error: None,
        }
    }

    pub fn fail(category: DwsSyncCategory, err: impl Into<String>) -> Self {
        Self {
            category,
            success: false,
            records_count: 0,
            chunks_written: 0,
            last_synced_at: None,
            error: Some(err.into()),
        }
    }
}

/// Route a single category through the right adapter.
///
/// `since` is the lower bound of the sync window (unix seconds); `now` is
/// the upper bound (the sync run's start time). The adapter is allowed to
/// stretch `now` for forward-looking sources (calendar pulls events up to
/// `now + 7d`), but the cursor advance stored by the caller is always
/// `now_unix_secs()` at adapter-return time.
pub async fn dispatch(
    category: DwsSyncCategory,
    since: u64,
    now: u64,
    owner: &OwnerIdentity,
    config: &Config,
) -> SyncCategoryResult {
    let result = match category {
        DwsSyncCategory::Chat => chat::run(since, now, owner, config).await,
        DwsSyncCategory::Doc => doc::run(since, now, owner, config).await,
        DwsSyncCategory::Calendar => calendar::run(since, now, owner, config).await,
        DwsSyncCategory::Minutes => minutes::run(since, now, owner, config).await,
    };

    if result.success {
        tracing::info!(
            category = ?category,
            records_count = result.records_count,
            chunks_written = result.chunks_written,
            "[dws:sync] category ok"
        );
    } else {
        tracing::warn!(
            category = ?category,
            error = %result.error.as_deref().unwrap_or("(unknown)"),
            "[dws:sync] category failed"
        );
    }

    result
}
