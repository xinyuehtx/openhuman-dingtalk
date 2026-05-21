//! DWS (DingTalk Workspace CLI) sync configuration.
//!
//! Controls periodic data synchronization from DingTalk products via the `dws`
//! CLI tool. Each category (chat, mail, doc, calendar, minutes) can be
//! individually enabled or disabled.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for periodic DWS data synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DwsSyncConfig {
    /// Master switch for periodic sync. When `false`, the scheduler is not
    /// started and no automatic pulls occur.
    #[serde(default)]
    pub enabled: bool,

    /// Interval in minutes between periodic sync runs. Minimum enforced at
    /// runtime is 5 minutes. Defaults to 30.
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u32,

    /// Per-category toggles controlling which DingTalk products are synced.
    #[serde(default)]
    pub categories: DwsSyncCategories,
}

/// Per-category sync toggles. Each field corresponds to a DingTalk product
/// that the `dws` CLI can pull data from and the openhuman memory tree can
/// ingest. Unknown fields from older config files (`contact`, `attendance`,
/// `report`, `todo`, `approval`, `mail`) are silently dropped by serde —
/// those categories were retired. Mail was pulled out specifically because
/// the dws mail-search scope requires a separate browser-driven PAT grant
/// and the privacy surface (full inbox bodies in local memory) didn't
/// justify the friction.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DwsSyncCategories {
    /// 群聊 — group chat messages → `ingest_chat`.
    #[serde(default = "default_true")]
    pub chat: bool,
    /// 文档 — DingTalk docs → `ingest_document`.
    #[serde(default = "default_true")]
    pub doc: bool,
    /// 日历 — calendar events → `ingest_document`.
    #[serde(default = "default_true")]
    pub calendar: bool,
    /// AI 听记 — meeting minutes (summary + todos) → `ingest_document`.
    #[serde(default = "default_true")]
    pub minutes: bool,
}

fn default_interval_minutes() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

impl Default for DwsSyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: default_interval_minutes(),
            categories: DwsSyncCategories::default(),
        }
    }
}

impl Default for DwsSyncCategories {
    fn default() -> Self {
        Self {
            chat: true,
            doc: true,
            calendar: true,
            minutes: true,
        }
    }
}
