//! The five DingTalk content categories that DWS sync v2 pulls into memory.
//!
//! v1 had nine categories (calendar, todo, contact, attendance, approval,
//! report, mail, doc, chat) but only counted records — nothing reached the
//! memory tree. v2 narrows the surface to the highest-signal sources and
//! routes every successful pull through `memory::tree::ingest_*`.

use serde::{Deserialize, Serialize};

/// Categories of DingTalk data that DWS sync v2 pulls into the memory tree.
///
/// Order here is the order categories are processed in a single `sync_now`
/// run; cheaper / more important categories go first so a long-running tail
/// (minutes detail fetches) can't starve them.
///
/// Mail was deliberately excluded — `mail.message:search` requires a
/// separate browser-driven PAT grant and the privacy surface of pulling
/// full inbox bodies into local memory didn't justify the friction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DwsSyncCategory {
    /// Group chat messages — `dws chat message list-all` → `ingest_chat`.
    Chat,
    /// DingTalk docs — `dws doc search` + `read` → `ingest_document`.
    Doc,
    /// Calendar events — `dws calendar event list` → `ingest_document`.
    Calendar,
    /// AI 听记 meeting minutes (summary + todos) — `dws minutes list mine` +
    /// `get summary` + `get todos` → `ingest_document`.
    Minutes,
}

impl DwsSyncCategory {
    /// Human-readable label used in user-facing surfaces.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Chat => "群聊",
            Self::Doc => "文档",
            Self::Calendar => "日历",
            Self::Minutes => "AI 听记",
        }
    }

    /// Stable string id used as the key in the persisted sync-state file
    /// (`<workspace>/dws_sync_state.json`). Must NOT change between releases
    /// — renaming a key would silently reset that category's cursor and
    /// trigger a one-hour cold-start pull.
    pub fn state_key(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Doc => "doc",
            Self::Calendar => "calendar",
            Self::Minutes => "minutes",
        }
    }

    /// Whether this category needs the current user's `userId` (`dws contact
    /// user get-self`). Doc adapter uses it as `--editor-uids` so the search
    /// only returns docs the dws-authenticated user has touched.
    pub fn needs_user_id(&self) -> bool {
        matches!(self, Self::Doc)
    }

    /// Cold-start lookback window (seconds) when no `last_synced_at` cursor
    /// exists for this category yet.
    ///
    /// Chat / Calendar activity is dense (multiple events per hour) so a
    /// one-hour cold start surfaces useful context immediately without
    /// dragging months of dingtalk history into memory. Doc / Minutes are
    /// the opposite: typical users edit a handful of docs per week and
    /// record meetings even less often, so the 1-hour window almost always
    /// returned **zero** items on the first sync (the bug behind "拉取
    /// 文档列表为空"). Widen those two to 30 days so the first sync has
    /// something to feed the memory tree; incremental ticks afterwards
    /// stay narrow thanks to the persisted cursor.
    pub fn cold_start_seconds(&self) -> u64 {
        const HOUR: u64 = 3_600;
        const DAY: u64 = 24 * HOUR;
        match self {
            Self::Chat | Self::Calendar => HOUR,
            Self::Doc | Self::Minutes => 30 * DAY,
        }
    }
}
