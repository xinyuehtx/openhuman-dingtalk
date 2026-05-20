//! DWS (DingTalk Workspace CLI) sync configuration.
//!
//! Controls periodic data synchronization from DingTalk products via the `dws`
//! CLI tool. Each category (calendar, todo, contacts, etc.) can be individually
//! enabled or disabled.

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
/// that the `dws` CLI can pull data from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DwsSyncCategories {
    /// 日历 — calendar events
    #[serde(default = "default_true")]
    pub calendar: bool,
    /// 待办 — todo tasks
    #[serde(default = "default_true")]
    pub todo: bool,
    /// 通讯录 — contacts
    #[serde(default)]
    pub contact: bool,
    /// 考勤 — attendance records
    #[serde(default)]
    pub attendance: bool,
    /// 审批 — OA approval instances
    #[serde(default)]
    pub approval: bool,
    /// 日志 — reports
    #[serde(default)]
    pub report: bool,
    /// 邮箱 — email
    #[serde(default)]
    pub mail: bool,
    /// 文档 — documents
    #[serde(default)]
    pub doc: bool,
    /// 群聊 — group chats
    #[serde(default)]
    pub chat: bool,
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
            calendar: true,
            todo: true,
            contact: false,
            attendance: false,
            approval: false,
            report: false,
            mail: false,
            doc: false,
            chat: false,
        }
    }
}
