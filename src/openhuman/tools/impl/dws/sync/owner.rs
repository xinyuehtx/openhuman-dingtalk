//! Probes the dws-authenticated user's identity.
//!
//! Memory ingestion needs an `owner` string so chunks from different
//! accounts don't collide. We derive it from `dws contact user get-self`
//! (which returns `corpId` + `userId`) and cache it inside the per-run
//! [`OwnerIdentity`].
//!
//! The probe is best-effort: a failure returns `None` for the field and
//! every adapter falls back to a generic `dingtalk:unknown` owner key
//! rather than wedging the whole sync run.

use super::run::run_dws_json;

/// Per-sync-run probed identity. Populated once at the top of `sync_now`
/// and threaded into every adapter.
#[derive(Debug, Default, Clone)]
pub struct OwnerIdentity {
    /// DingTalk `userId` (numeric string, e.g. `"274264"`).
    pub user_id: Option<String>,
    /// DingTalk `corpId` (opaque string, e.g. `"ding8196cd9a2b2405da..."`).
    pub corp_id: Option<String>,
}

impl OwnerIdentity {
    /// Build the stable owner key passed into every `ingest_*` call so
    /// chunks are partitioned by account. Falls back to a generic
    /// `"dingtalk:unknown"` when both ids are absent — better than
    /// crashing, and the dws auth gate above should prevent it in
    /// practice.
    pub fn owner_key(&self) -> String {
        match (&self.corp_id, &self.user_id) {
            (Some(corp), Some(user)) => format!("dingtalk:{corp}:{user}"),
            (Some(corp), None) => format!("dingtalk:{corp}:unknown"),
            (None, Some(user)) => format!("dingtalk:unknown:{user}"),
            (None, None) => "dingtalk:unknown".to_string(),
        }
    }

    /// Short redacted form for logs — first 6 chars of corp_id + first 4
    /// chars of user_id, so operators can correlate without exposing the
    /// full account identity in plaintext logs.
    pub fn redacted(&self) -> String {
        let corp = self
            .corp_id
            .as_deref()
            .map(|s| s.chars().take(6).collect::<String>())
            .unwrap_or_else(|| "?".into());
        let user = self
            .user_id
            .as_deref()
            .map(|s| s.chars().take(4).collect::<String>())
            .unwrap_or_else(|| "?".into());
        format!("{corp}…/{user}…")
    }
}

/// Probe the user's identity via `dws contact user get-self`.
pub async fn probe() -> OwnerIdentity {
    let (user_id, corp_id) = probe_user_and_corp().await;
    let identity = OwnerIdentity { user_id, corp_id };
    tracing::info!(
        owner = %identity.redacted(),
        "[dws:sync] probed owner identity"
    );
    identity
}

async fn probe_user_and_corp() -> (Option<String>, Option<String>) {
    let v = match run_dws_json("dws contact user get-self --format json").await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "[dws:sync] probe contact get-self failed");
            return (None, None);
        }
    };
    // Shape (verified from a live response):
    //   { "result": [ { "orgEmployeeModel": { "userId": "...", "corpId": "..." } } ],
    //     "success": true }
    let model = v
        .get("result")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("orgEmployeeModel"));
    let user_id = model
        .and_then(|m| m.get("userId"))
        .and_then(|u| u.as_str())
        .map(str::to_string);
    let corp_id = model
        .and_then(|m| m.get("corpId"))
        .and_then(|c| c.as_str())
        .map(str::to_string);
    if user_id.is_none() {
        tracing::warn!("[dws:sync] probe: get-self response missing orgEmployeeModel.userId");
    }
    (user_id, corp_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_key_uses_both_ids_when_present() {
        let id = OwnerIdentity {
            user_id: Some("274264".into()),
            corp_id: Some("dingABC".into()),
        };
        assert_eq!(id.owner_key(), "dingtalk:dingABC:274264");
    }

    #[test]
    fn owner_key_handles_missing_corp() {
        let id = OwnerIdentity {
            user_id: Some("274264".into()),
            corp_id: None,
        };
        assert_eq!(id.owner_key(), "dingtalk:unknown:274264");
    }

    #[test]
    fn owner_key_handles_missing_user() {
        let id = OwnerIdentity {
            user_id: None,
            corp_id: Some("dingABC".into()),
        };
        assert_eq!(id.owner_key(), "dingtalk:dingABC:unknown");
    }

    #[test]
    fn owner_key_falls_back_when_both_missing() {
        let id = OwnerIdentity::default();
        assert_eq!(id.owner_key(), "dingtalk:unknown");
    }

    #[test]
    fn redacted_truncates_both_ids() {
        let id = OwnerIdentity {
            user_id: Some("274264xyz".into()),
            corp_id: Some("dingabcdef9876".into()),
        };
        assert_eq!(id.redacted(), "dingab…/2742…");
    }
}
