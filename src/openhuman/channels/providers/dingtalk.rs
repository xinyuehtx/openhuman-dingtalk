use crate::openhuman::channels::traits::{Channel, ChannelMessage, SendMessage};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const DINGTALK_BOT_CALLBACK_TOPIC: &str = "/v1.0/im/bot/messages/get";

/// How long before expiry we consider the cached access token stale (5-minute buffer).
const ACCESS_TOKEN_REFRESH_BUFFER_SECS: u64 = 300;

/// Cached DingTalk access token with expiry tracking.
struct CachedAccessToken {
    token: String,
    obtained_at: Instant,
    expires_in_secs: u64,
}

impl CachedAccessToken {
    fn is_valid(&self) -> bool {
        let elapsed = self.obtained_at.elapsed().as_secs();
        elapsed + ACCESS_TOKEN_REFRESH_BUFFER_SECS < self.expires_in_secs
    }
}

/// DingTalk channel — connects via Stream Mode WebSocket for real-time messages.
/// Replies are sent through per-message session webhook URLs with access_token fallback.
pub struct DingTalkChannel {
    client_id: String,
    client_secret: String,
    allowed_users: Vec<String>,
    /// Per-chat session webhooks for sending replies (chatID -> webhook URL).
    /// DingTalk provides a unique webhook URL with each incoming message.
    session_webhooks: Arc<RwLock<HashMap<String, String>>>,
    /// Cached access token for fallback message sending when session webhooks expire.
    access_token_cache: Arc<RwLock<Option<CachedAccessToken>>>,
}

/// Response from DingTalk gateway connection registration.
#[derive(serde::Deserialize)]
struct GatewayResponse {
    endpoint: String,
    ticket: String,
}

/// Response from DingTalk access token API.
#[derive(serde::Deserialize)]
struct AccessTokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expireIn")]
    expire_in: u64,
}

impl DingTalkChannel {
    pub fn new(client_id: String, client_secret: String, allowed_users: Vec<String>) -> Self {
        Self {
            client_id,
            client_secret,
            allowed_users,
            session_webhooks: Arc::new(RwLock::new(HashMap::new())),
            access_token_cache: Arc::new(RwLock::new(None)),
        }
    }

    fn http_client(&self) -> reqwest::Client {
        crate::openhuman::config::build_runtime_proxy_client("channel.dingtalk")
    }

    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == "*" || u == user_id)
    }

    fn parse_stream_data(frame: &serde_json::Value) -> Option<serde_json::Value> {
        match frame.get("data") {
            Some(serde_json::Value::String(raw)) => serde_json::from_str(raw).ok(),
            Some(serde_json::Value::Object(_)) => frame.get("data").cloned(),
            _ => None,
        }
    }

    fn resolve_chat_id(data: &serde_json::Value, sender_id: &str) -> String {
        let is_private_chat = data
            .get("conversationType")
            .and_then(|value| {
                value
                    .as_str()
                    .map(|v| v == "1")
                    .or_else(|| value.as_i64().map(|v| v == 1))
            })
            .unwrap_or(true);

        if is_private_chat {
            sender_id.to_string()
        } else {
            data.get("conversationId")
                .and_then(|c| c.as_str())
                .unwrap_or(sender_id)
                .to_string()
        }
    }

    /// Register a connection with DingTalk's gateway to get a WebSocket endpoint.
    async fn register_connection(&self) -> anyhow::Result<GatewayResponse> {
        let body = serde_json::json!({
            "clientId": self.client_id,
            "clientSecret": self.client_secret,
            "subscriptions": [
                {
                    "type": "CALLBACK",
                    "topic": DINGTALK_BOT_CALLBACK_TOPIC,
                }
            ],
        });

        let resp = self
            .http_client()
            .post("https://api.dingtalk.com/v1.0/gateway/connections/open")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("DingTalk gateway registration failed ({status}): {err}");
        }

        let gw: GatewayResponse = resp.json().await?;
        Ok(gw)
    }

    /// Obtain a valid access token, using cache when possible.
    async fn get_access_token(&self) -> anyhow::Result<String> {
        // Check cache first.
        {
            let cache = self.access_token_cache.read().await;
            if let Some(ref cached) = *cache {
                if cached.is_valid() {
                    tracing::debug!("[dingtalk] using cached access_token");
                    return Ok(cached.token.clone());
                }
            }
        }

        // Fetch a new token.
        tracing::debug!("[dingtalk] fetching new access_token from DingTalk API");
        let body = serde_json::json!({
            "appKey": self.client_id,
            "appSecret": self.client_secret,
        });

        let resp = self
            .http_client()
            .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("DingTalk access_token fetch failed ({status}): {err}");
        }

        let token_resp: AccessTokenResponse = resp.json().await?;
        let token = token_resp.access_token.clone();

        // Update cache.
        let mut cache = self.access_token_cache.write().await;
        *cache = Some(CachedAccessToken {
            token: token_resp.access_token,
            obtained_at: Instant::now(),
            expires_in_secs: token_resp.expire_in,
        });

        Ok(token)
    }

    /// Send a message using session webhook (primary path).
    async fn send_via_webhook(&self, webhook_url: &str, message: &SendMessage) -> anyhow::Result<()> {
        let title = message.subject.as_deref().unwrap_or("OpenHuman");
        let body = serde_json::json!({
            "msgtype": "markdown",
            "markdown": {
                "title": title,
                "text": message.content,
            }
        });

        let resp = self
            .http_client()
            .post(webhook_url)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("DingTalk webhook reply failed ({status}): {err}");
        }

        Ok(())
    }

    /// Send a message using access_token-based robot API (fallback when webhook is unavailable).
    async fn send_via_access_token(&self, recipient: &str, message: &SendMessage) -> anyhow::Result<()> {
        let token = self.get_access_token().await?;
        tracing::debug!(
            "[dingtalk] sending message via access_token to recipient={}",
            recipient
        );

        let body = serde_json::json!({
            "robotCode": self.client_id,
            "userIds": [recipient],
            "msgKey": "sampleMarkdown",
            "msgParam": serde_json::json!({
                "title": message.subject.as_deref().unwrap_or("OpenHuman"),
                "text": message.content,
            }).to_string(),
        });

        let resp = self
            .http_client()
            .post("https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend")
            .header("x-acs-dingtalk-access-token", &token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("DingTalk robot batchSend failed ({status}): {err}");
        }

        tracing::debug!("[dingtalk] access_token message sent successfully");
        Ok(())
    }
}

#[async_trait]
impl Channel for DingTalkChannel {
    fn name(&self) -> &str {
        "dingtalk"
    }

    fn supports_draft_updates(&self) -> bool {
        true
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        // Try session webhook first (primary path).
        let webhook_url = {
            let webhooks = self.session_webhooks.read().await;
            webhooks.get(&message.recipient).cloned()
        };

        if let Some(url) = webhook_url {
            tracing::debug!(
                "[dingtalk] sending via session webhook to recipient={}",
                message.recipient
            );
            match self.send_via_webhook(&url, message).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!(
                        "[dingtalk] webhook send failed (possibly expired): {e}; \
                         falling back to access_token API"
                    );
                }
            }
        } else {
            tracing::debug!(
                "[dingtalk] no session webhook for recipient={}; using access_token API",
                message.recipient
            );
        }

        // Fallback: send via access_token-based robot API.
        self.send_via_access_token(&message.recipient, message).await
    }

    async fn send_draft(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
        // DingTalk does not support message editing via session webhooks.
        // We send an initial "thinking" message and return a synthetic draft ID.
        // Subsequent updates are sent as new messages (append strategy).
        let draft_id = Uuid::new_v4().to_string();
        tracing::debug!(
            "[dingtalk] send_draft: draft_id={}, recipient={}",
            draft_id,
            message.recipient
        );

        // Send the initial content (or a placeholder).
        self.send(message).await?;
        Ok(Some(draft_id))
    }

    async fn update_draft(
        &self,
        _recipient: &str,
        _message_id: &str,
        _text: &str,
    ) -> anyhow::Result<()> {
        // DingTalk session webhooks do not support message editing.
        // Silently skip intermediate updates to avoid spamming the chat.
        // The finalize_draft call will send the complete response.
        tracing::trace!("[dingtalk] update_draft: skipping intermediate update (not supported)");
        Ok(())
    }

    async fn finalize_draft(
        &self,
        recipient: &str,
        _message_id: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        // Send the final complete message.
        tracing::debug!(
            "[dingtalk] finalize_draft: sending final message to recipient={}",
            recipient
        );
        let final_message = SendMessage::new(text, recipient).in_thread(thread_ts.map(String::from));
        self.send(&final_message).await
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        tracing::info!("DingTalk: registering gateway connection...");

        let gw = self.register_connection().await?;
        let ws_url = format!("{}?ticket={}", gw.endpoint, gw.ticket);

        tracing::info!("DingTalk: connecting to stream WebSocket...");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        tracing::info!("DingTalk: connected and listening for messages...");

        while let Some(msg) = read.next().await {
            let msg = match msg {
                Ok(Message::Text(t)) => t,
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    tracing::warn!("DingTalk WebSocket error: {e}");
                    break;
                }
                _ => continue,
            };

            let frame: serde_json::Value = match serde_json::from_str(msg.as_ref()) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let frame_type = frame.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match frame_type {
                "SYSTEM" => {
                    // Respond to system pings to keep the connection alive
                    let message_id = frame
                        .get("headers")
                        .and_then(|h| h.get("messageId"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("");

                    let pong = serde_json::json!({
                        "code": 200,
                        "headers": {
                            "contentType": "application/json",
                            "messageId": message_id,
                        },
                        "message": "OK",
                        "data": "",
                    });

                    if let Err(e) = write.send(Message::Text(pong.to_string())).await {
                        tracing::warn!("DingTalk: failed to send pong: {e}");
                        break;
                    }
                }
                "EVENT" | "CALLBACK" => {
                    // Parse the chatbot callback data from the frame.
                    let data = match Self::parse_stream_data(&frame) {
                        Some(v) => v,
                        None => {
                            tracing::debug!("DingTalk: frame has no parseable data payload");
                            continue;
                        }
                    };

                    // Extract message content
                    let content = data
                        .get("text")
                        .and_then(|t| t.get("content"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .trim();

                    if content.is_empty() {
                        continue;
                    }

                    let sender_id = data
                        .get("senderStaffId")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");

                    if !self.is_user_allowed(sender_id) {
                        tracing::warn!(
                            "DingTalk: ignoring message from unauthorized user: {sender_id}"
                        );
                        continue;
                    }

                    // Private chat uses sender ID, group chat uses conversation ID.
                    let chat_id = Self::resolve_chat_id(&data, sender_id);

                    // Store session webhook for later replies
                    if let Some(webhook) = data.get("sessionWebhook").and_then(|w| w.as_str()) {
                        let webhook = webhook.to_string();
                        let mut webhooks = self.session_webhooks.write().await;
                        // Use both keys so reply routing works for both group and private flows.
                        webhooks.insert(chat_id.clone(), webhook.clone());
                        webhooks.insert(sender_id.to_string(), webhook);
                    }

                    // Acknowledge the event
                    let message_id = frame
                        .get("headers")
                        .and_then(|h| h.get("messageId"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("");

                    let ack = serde_json::json!({
                        "code": 200,
                        "headers": {
                            "contentType": "application/json",
                            "messageId": message_id,
                        },
                        "message": "OK",
                        "data": "",
                    });
                    let _ = write.send(Message::Text(ack.to_string())).await;

                    let channel_msg = ChannelMessage {
                        id: Uuid::new_v4().to_string(),
                        sender: sender_id.to_string(),
                        reply_target: chat_id,
                        content: content.to_string(),
                        channel: "dingtalk".to_string(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        thread_ts: None,
                    };

                    if tx.send(channel_msg).await.is_err() {
                        tracing::warn!("DingTalk: message channel closed");
                        break;
                    }
                }
                _ => {}
            }
        }

        anyhow::bail!("DingTalk WebSocket stream ended")
    }

    async fn health_check(&self) -> bool {
        self.register_connection().await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let ch = DingTalkChannel::new("id".into(), "secret".into(), vec![]);
        assert_eq!(ch.name(), "dingtalk");
    }

    #[test]
    fn test_user_allowed_wildcard() {
        let ch = DingTalkChannel::new("id".into(), "secret".into(), vec!["*".into()]);
        assert!(ch.is_user_allowed("anyone"));
    }

    #[test]
    fn test_user_allowed_specific() {
        let ch = DingTalkChannel::new("id".into(), "secret".into(), vec!["user123".into()]);
        assert!(ch.is_user_allowed("user123"));
        assert!(!ch.is_user_allowed("other"));
    }

    #[test]
    fn test_user_denied_empty() {
        let ch = DingTalkChannel::new("id".into(), "secret".into(), vec![]);
        assert!(!ch.is_user_allowed("anyone"));
    }

    #[test]
    fn test_config_serde() {
        let toml_str = r#"
client_id = "app_id_123"
client_secret = "secret_456"
allowed_users = ["user1", "*"]
"#;
        let config: crate::openhuman::config::schema::DingTalkConfig =
            toml::from_str(toml_str).unwrap();
        assert_eq!(config.client_id, "app_id_123");
        assert_eq!(config.client_secret, "secret_456");
        assert_eq!(config.allowed_users, vec!["user1", "*"]);
    }

    #[test]
    fn test_config_serde_defaults() {
        let toml_str = r#"
client_id = "id"
client_secret = "secret"
"#;
        let config: crate::openhuman::config::schema::DingTalkConfig =
            toml::from_str(toml_str).unwrap();
        assert!(config.allowed_users.is_empty());
    }

    #[test]
    fn parse_stream_data_supports_string_payload() {
        let frame = serde_json::json!({
            "data": "{\"text\":{\"content\":\"hello\"}}"
        });
        let parsed = DingTalkChannel::parse_stream_data(&frame).unwrap();
        assert_eq!(
            parsed.get("text").and_then(|v| v.get("content")),
            Some(&serde_json::json!("hello"))
        );
    }

    #[test]
    fn parse_stream_data_supports_object_payload() {
        let frame = serde_json::json!({
            "data": {"text": {"content": "hello"}}
        });
        let parsed = DingTalkChannel::parse_stream_data(&frame).unwrap();
        assert_eq!(
            parsed.get("text").and_then(|v| v.get("content")),
            Some(&serde_json::json!("hello"))
        );
    }

    #[test]
    fn resolve_chat_id_handles_numeric_group_conversation_type() {
        let data = serde_json::json!({
            "conversationType": 2,
            "conversationId": "cid-group",
        });
        let chat_id = DingTalkChannel::resolve_chat_id(&data, "staff-1");
        assert_eq!(chat_id, "cid-group");
    }
}
