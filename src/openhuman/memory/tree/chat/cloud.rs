//! Cloud chat provider — routes the memory-tree's `memory` workload through
//! the unified inference factory so the same code path covers both the
//! OpenHuman backend (session-JWT auth, `summarization-v1`) and any
//! user-configured OpenAI-compatible endpoint (`inference_url + api_key` or a
//! `<slug>:<model>` cloud_providers entry).
//!
//! Used whenever the workload isn't routed to local Ollama (i.e.
//! `Config::workload_uses_local("memory") == false`).
//!
//! Internally it wraps a `Box<dyn Provider>` produced by
//! [`create_chat_provider`]. That factory already knows how to:
//!
//! - shortcut to a custom OpenAI-compatible endpoint when
//!   `inference_url + api_key` are set (so users running entirely on their
//!   own LLM, with no OpenHuman backend session, still get extract/summarise),
//! - rewrite OpenHuman tier names (`summarization-v1`, `hint:<tier>`) to the
//!   user's `default_model` inside the custom-LLM provider,
//! - resolve `<slug>:<model>` against `cloud_providers`,
//! - and fall back to the OpenHuman backend with session-JWT for plain
//!   `"openhuman"` strings.
//!
//! When the resolved inner provider is the OpenHuman backend and the
//! configured model is unprovisioned for the user's org, this wrapper still
//! walks the historical fallback list (`summarization-v1`,
//! DeepSeek variants) so existing behaviour is preserved.

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::openhuman::config::Config;
use crate::openhuman::inference::provider::create_chat_provider;
use crate::openhuman::inference::provider::traits::{ChatMessage, Provider};

use super::{ChatPrompt, ChatProvider};

/// Fallback models tried in order when the configured model is unavailable
/// on the OpenHuman backend. For custom OpenAI-compatible providers the
/// upstream `model_override_for_tiers` mapping already collapses every
/// OpenHuman tier name onto the user's `default_model`, so this list is a
/// no-op there — kept as a safety net for the backend path.
const FALLBACK_MODELS: &[&str] = &[
    "summarization-v1",
    "deepseek-ai/DeepSeek-V3-0324",
    "deepseek-ai/DeepSeek-V3",
];

/// Returns true if the error indicates the model is not provisioned for the org.
/// Only matches the explicit "not available for your organization" phrase from
/// the GMI API — generic 404s are NOT treated as model-unavailable to avoid
/// masking unrelated backend failures.
fn is_model_unavailable_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:?}");
    msg.contains("not available for your organization")
}

/// Cloud-routed chat provider. Holds a factory-built [`Provider`] and forwards
/// each [`ChatProvider::chat_for_json`] call through its `chat_with_history`
/// method.
pub struct CloudChatProvider {
    inner: Box<dyn Provider>,
    model: String,
    /// Cached display name `"cloud:<model>"` for logs.
    display: String,
}

impl CloudChatProvider {
    /// Build a cloud provider via the unified inference factory using the
    /// `"memory"` workload role. This honours `memory_provider` plus the
    /// `inference_url + api_key` shortcut so users running on a custom
    /// OpenAI-compatible endpoint (and not signed in to the OpenHuman
    /// backend) still get a working chat surface for extract/summarise.
    pub fn from_factory(config: &Config) -> Result<Self> {
        let (inner, model) = create_chat_provider("memory", config)
            .context("memory_tree::chat::cloud build_chat_provider(role=memory)")?;
        let display = format!("cloud:{model}");
        log::debug!(
            "[memory_tree::chat::cloud] from_factory resolved_model={}",
            model
        );
        Ok(Self {
            inner,
            model,
            display,
        })
    }

    /// Construct directly from a pre-built provider — used by tests so the
    /// soft-fallback / display-name behaviour can be exercised without going
    /// through the factory.
    #[cfg(test)]
    pub(crate) fn from_provider(inner: Box<dyn Provider>, model: impl Into<String>) -> Self {
        let model = model.into();
        let display = format!("cloud:{model}");
        Self {
            inner,
            model,
            display,
        }
    }

    /// Try a single model, returning Ok(text) or the error.
    async fn try_model(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> Result<String> {
        self.inner
            .chat_with_history(messages, model, temperature)
            .await
    }
}

#[async_trait]
impl ChatProvider for CloudChatProvider {
    fn name(&self) -> &str {
        &self.display
    }

    async fn chat_for_json(&self, prompt: &ChatPrompt) -> Result<String> {
        log::debug!(
            "[memory_tree::chat::cloud] kind={} model={} sys_chars={} user_chars={}",
            prompt.kind,
            self.model,
            prompt.system.len(),
            prompt.user.len()
        );

        let messages = vec![
            ChatMessage::system(prompt.system.clone()),
            ChatMessage::user(prompt.user.clone()),
        ];

        // Try the configured model first.
        match self
            .try_model(&messages, &self.model, prompt.temperature)
            .await
        {
            Ok(text) => {
                log::debug!(
                    "[memory_tree::chat::cloud] response chars={} kind={}",
                    text.len(),
                    prompt.kind
                );
                return Ok(text);
            }
            Err(e) if is_model_unavailable_error(&e) => {
                log::warn!(
                    "[memory_tree::chat::cloud] model={} unavailable, trying fallbacks",
                    self.model
                );
            }
            Err(e) => {
                log::warn!(
                    "[memory_tree::chat::cloud] model={} failed kind={} err={:#}",
                    self.model,
                    prompt.kind,
                    e
                );
                return Err(e).with_context(|| {
                    format!(
                        "cloud chat request kind={} model={} failed",
                        prompt.kind, self.model
                    )
                });
            }
        }

        // Fallback chain — skip the configured model if it appears in the list.
        for &fallback in FALLBACK_MODELS {
            if fallback == self.model {
                continue;
            }
            log::debug!(
                "[memory_tree::chat::cloud] trying fallback model={}",
                fallback
            );
            match self
                .try_model(&messages, fallback, prompt.temperature)
                .await
            {
                Ok(text) => {
                    log::info!(
                        "[memory_tree::chat::cloud] fallback model={} succeeded kind={}",
                        fallback,
                        prompt.kind
                    );
                    return Ok(text);
                }
                Err(e) if is_model_unavailable_error(&e) => {
                    log::debug!(
                        "[memory_tree::chat::cloud] fallback model={} also unavailable",
                        fallback
                    );
                    continue;
                }
                Err(e) => {
                    log::warn!(
                        "[memory_tree::chat::cloud] fallback model={} failed kind={} err={:#}",
                        fallback,
                        prompt.kind,
                        e
                    );
                    return Err(e).with_context(|| {
                        format!(
                            "cloud chat request kind={} fallback model={} failed",
                            prompt.kind, fallback
                        )
                    });
                }
            }
        }

        log::warn!(
            "[memory_tree::chat::cloud] configured model={} and all fallbacks unavailable kind={}",
            self.model,
            prompt.kind
        );
        anyhow::bail!(
            "cloud chat kind={}: configured model '{}' and all fallback models are unavailable \
             for this organization",
            prompt.kind,
            self.model
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal stub Provider for unit tests — records the model arg of each
    /// `chat_with_history` call and returns either a canned response or an
    /// error.
    struct StubProvider {
        response: anyhow::Result<String>,
        last_model: std::sync::Mutex<Option<String>>,
        calls: AtomicUsize,
    }

    impl StubProvider {
        fn ok(text: impl Into<String>) -> Self {
            Self {
                response: Ok(text.into()),
                last_model: std::sync::Mutex::new(None),
                calls: AtomicUsize::new(0),
            }
        }

        fn err(msg: impl Into<String>) -> Self {
            Self {
                response: Err(anyhow::anyhow!(msg.into())),
                last_model: std::sync::Mutex::new(None),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Provider for StubProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_model.lock().unwrap() = Some(model.to_string());
            self.response
                .as_ref()
                .map(|s| s.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    }

    fn sample_prompt() -> ChatPrompt {
        ChatPrompt {
            system: "sys".into(),
            user: "u".into(),
            temperature: 0.0,
            kind: "test",
        }
    }

    #[test]
    fn name_includes_model() {
        let p = CloudChatProvider::from_provider(
            Box::new(StubProvider::ok("noop")),
            "summarization-v1",
        );
        assert_eq!(p.name(), "cloud:summarization-v1");
    }

    #[test]
    fn name_changes_with_model() {
        let p = CloudChatProvider::from_provider(
            Box::new(StubProvider::ok("noop")),
            "claude-haiku-4.5",
        );
        assert!(p.name().contains("claude-haiku-4.5"));
    }

    #[test]
    fn detects_model_unavailable_error() {
        let err = anyhow::anyhow!(
            "OpenHuman API error (404 Not Found): {{\"success\":false,\"error\":\"GMI model \
             'deepseek-ai/DeepSeek-V4-Flash' is not available for your organization.\"}}"
        );
        assert!(is_model_unavailable_error(&err));
    }

    #[test]
    fn non_model_error_not_detected_as_unavailable() {
        let err = anyhow::anyhow!("connection timeout after 30s");
        assert!(!is_model_unavailable_error(&err));
    }

    #[test]
    fn generic_404_with_model_not_treated_as_unavailable() {
        let err =
            anyhow::anyhow!("OpenHuman API error (404 Not Found): model endpoint returned 404");
        assert!(!is_model_unavailable_error(&err));
    }

    #[test]
    fn fallback_list_contains_summarization_v1() {
        assert!(FALLBACK_MODELS.contains(&"summarization-v1"));
    }

    #[test]
    fn fallback_list_not_empty() {
        assert!(!FALLBACK_MODELS.is_empty());
    }

    #[tokio::test]
    async fn forwards_call_with_configured_model() {
        let stub = StubProvider::ok("hi");
        let p = CloudChatProvider::from_provider(Box::new(stub), "Qwen3.6-Plus-DogFooding");
        let out = p.chat_for_json(&sample_prompt()).await.unwrap();
        assert_eq!(out, "hi");
    }
}
