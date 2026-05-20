### custom-llm-no-login

修复自定义 LLM 模式下 model 名称未映射到用户配置的真实模型的问题，以及添加未登录状态下的默认身份常量。

## 问题诊断

### 问题 1: LLM 返回空响应 (text_chars=0)

**根因链**：

1. `factory.rs` 的 `make_openhuman_backend` 从 `config.default_model` 取值，但在某些加载路径下 `config.default_model` 为空，fallback 到 `DEFAULT_MODEL = "chat-v1"`
2. `resolve_for_custom_llm("chat-v1", "chat-v1")` 匹配 tier 但 user_default 也是 `"chat-v1"`，所以映射到了自己
3. factory 返回 `model="chat-v1"`，agent builder 存储为 `self.model_name="chat-v1"`
4. agent turn 发送 `model="chat-v1"` 到 idealab → idealab 不认识 → 返回空响应

**日志证据**：

```
14:12:47 [providers][chat-factory] custom LLM mode: model=chat-v1
14:12:47 [agent] iteration 1/10 — sending request to provider model=chat-v1
14:12:47 [stream] custom_openai POST .../chat/completions
14:12:47 [stream] custom_openai aggregated text_chars=0
```

### 问题 2: LLM 配置持久化

**已确认正常**：`~/.openhuman/users/local/config.toml` 中 `default_model = "Qwen3.6-Plus-DogFooding"` 已正确保存。localStorage 和 TOML 双写均工作正常。

### 问题 3: 未登录状态下的身份

系统已有 `PRE_LOGIN_USER_ID = "local"`，但缺少一个可供 agent/socket 使用的 user identity 常量。

---

## 修复方案

### 修复 1: 在 `OpenAiCompatibleProvider` 上添加 `model_override` 字段

**最稳健的方案** — 在 provider 层面做 model 重写，覆盖所有调用路径（agent turn、sub-agent、memory tree、embeddings 等），不依赖上游正确传递 model 名。

#### Step 1.1: 修改 `src/openhuman/inference/provider/compatible.rs`

在 `OpenAiCompatibleProvider` struct 中新增字段：

```rust
/// When set, any model name that is an OpenHuman internal tier alias
/// (e.g. `chat-v1`, `reasoning-v1`, `hint:xxx`) is replaced with this
/// value before the HTTP request is sent. Used exclusively by the
/// custom-LLM factory path so the user's real model name reaches
/// their endpoint regardless of which code path selects the model.
pub(crate) model_override_for_tiers: Option<String>,
```

在 `new_with_options` 中初始化为 `None`。

添加 builder 方法：

```rust
pub fn with_model_override_for_tiers(mut self, model: String) -> Self {
    self.model_override_for_tiers = Some(model);
    self
}
```

添加 model 解析方法（替代 factory 中的 `resolve_for_custom_llm`）：

```rust
fn resolve_model<'a>(&'a self, model: &'a str) -> &'a str {
    if let Some(ref override_model) = self.model_override_for_tiers {
        if model.starts_with("hint:") || matches!(model,
            "reasoning-v1" | "reasoning-quick-v1" | "agentic-v1"
            | "coding-v1" | "chat-v1" | "summarization-v1"
        ) {
            log::debug!(
                "[provider:{}] model override: {} -> {}",
                self.name, model, override_model
            );
            return override_model.as_str();
        }
    }
    model
}
```

在 `chat()` 方法的入口处调用：

```rust
let model = self.resolve_model(model);
```

#### Step 1.2: 修改 `src/openhuman/inference/provider/factory.rs`

在 `make_openhuman_backend` 的 custom LLM 路径中，**删除** `resolve_for_custom_llm` 的调用（不再需要在 factory 层做映射），改为把用户配置的真实 model 通过 `with_model_override_for_tiers` 设置到 provider 上：

```rust
if has_custom_inference {
    let url = config.inference_url.as_deref().unwrap();
    let key = config.api_key.as_deref().unwrap();
    log::info!(
        "[providers][chat-factory] custom LLM mode: inference_url={} model={} (api_key bytes={})",
        url, model, key.len()
    );
    let p = Box::new(
        OpenAiCompatibleProvider::new_no_responses_fallback(
            "custom_openai", url, Some(key), CompatAuthStyle::Bearer,
        )
        .with_model_override_for_tiers(model.clone())
    );
    return Ok((p, model));
}
```

这样无论 agent 层传入什么 model name（`chat-v1`、`reasoning-v1`、`hint:agentic` 等），provider 都会在 HTTP 请求前替换为用户的真实 model（`Qwen3.6-Plus-DogFooding`）。

可以保留 `resolve_for_custom_llm` 函数但标记为备用，或者直接删除。

### 修复 2: 添加未登录状态下的默认身份常量

#### Step 2.1: 修改 `src/openhuman/credentials/mod.rs`

添加一个公共常量：

```rust
/// Default user identity for custom-LLM / offline mode when no backend
/// session exists. Used as a stable `user_id` so agent, memory, and
/// socket subsystems have a consistent identity without requiring login.
pub const CUSTOM_LLM_LOCAL_USER_ID: &str = "local-user";
```

#### Step 2.2: 修改 `src/openhuman/credentials/session_support.rs`

在 `build_session_state` 中，当 profile 为 None（未登录）且 custom LLM mode 激活时，返回一个合成的身份：

```rust
pub fn build_session_state(config: &Config) -> Result<AuthStateResponse, String> {
    let profile = load_app_session_profile(config)?;
    let state = session_state_from_profile(profile.as_ref());

    // In custom-LLM mode (inference_url + api_key configured), provide a
    // synthetic local identity so subsystems that key on user_id have a
    // stable value without requiring backend login.
    if !state.is_authenticated && is_custom_llm_mode(config) {
        return Ok(AuthStateResponse {
            is_authenticated: false, // still not authenticated against backend
            user_id: Some(super::CUSTOM_LLM_LOCAL_USER_ID.to_string()),
            user: Some(serde_json::json!({
                "id": super::CUSTOM_LLM_LOCAL_USER_ID,
                "name": "Local User",
            })),
            profile_id: None,
        });
    }

    Ok(state)
}

fn is_custom_llm_mode(config: &Config) -> bool {
    config.inference_url.as_ref().is_some_and(|u| !u.trim().is_empty())
        && config.api_key.as_ref().is_some_and(|k| !k.trim().is_empty())
}
```

### 修复 3: 增强调试日志

#### Step 3.1: 在 `factory.rs` 的 `make_openhuman_backend` 中

将 `resolved model=` 的日志从 `debug` 改为 `info`，使其在默认日志级别可见：

```rust
log::info!(
    "[providers][chat-factory] resolved model={} (config.default_model={:?}, env OPENHUMAN_MODEL={:?})",
    model, config.default_model, std::env::var("OPENHUMAN_MODEL").ok()
);
```

### Step 4: 编译验证

```bash
cargo check --manifest-path Cargo.toml
```

---

## 任务清单

- [ ] 1. 在 `OpenAiCompatibleProvider` 添加 `model_override_for_tiers` 字段和 `resolve_model` 方法
- [ ] 2. 修改 `compatible.rs` 的 `chat()` 方法入口调用 `resolve_model`
- [ ] 3. 修改 `factory.rs` 的 custom LLM 路径使用 `with_model_override_for_tiers`
- [ ] 4. 在 `credentials/mod.rs` 添加 `CUSTOM_LLM_LOCAL_USER_ID` 常量
- [ ] 5. 在 `session_support.rs` 为 custom LLM mode 提供合成身份
- [ ] 6. 将 factory 的 debug 日志改为 info 级别
- [ ] 7. 编译验证 `cargo check`

updateAtTime: 2026/5/20 14:26:52

planId: e9ffba39-c95f-4aa5-ad48-b6d255a8b48d
