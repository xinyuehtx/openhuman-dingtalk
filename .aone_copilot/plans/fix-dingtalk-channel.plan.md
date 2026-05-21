### fix-dingtalk-channel ###
修复钉钉 channel 的消息收发问题：补充群聊回复 API 路径、修复 sessionWebhook 过期后的 fallback 逻辑，并在 UI 连接引导中添加权限配置步骤。


## 问题分析

### 1. 权限配置缺失（导致消息发送失败）

钉钉 API 调用需要在开放平台**手动申请权限**：

- **`/v1.0/robot/oToMessages/batchSend`**（单聊发消息）需要权限：`企业内机器人发送消息权限`
- **`/v1.0/robot/groupMessages/send`**（群聊发消息）需要权限：`企业内机器人发送消息权限`
- Stream 模式接收消息本身不需要额外权限，但**发送回复**需要上述权限

当前 `SetupGuide` 组件中缺少权限配置步骤。

### 2. 群聊回复 API 路径错误

当前代码的消息回复策略：
1. 优先使用 `sessionWebhook`（每条消息附带的临时 URL）
2. fallback 到 `send_via_access_token` → 调用 `/v1.0/robot/oToMessages/batchSend`

**问题**：`oToMessages/batchSend` 只支持**人与机器人单聊**场景，参数是 `userIds`。群聊场景需要使用 `/v1.0/robot/groupMessages/send`，参数是 `openConversationId`。

当 `sessionWebhook` 过期后（有 `sessionWebhookExpiredTime` 字段标识过期时间），群聊消息的 fallback 路径会调用单聊 API，导致失败。

### 3. sessionWebhook 过期时间未跟踪

当前代码只存储了 webhook URL，没有存储过期时间（`sessionWebhookExpiredTime`），无法判断 webhook 是否已过期，只能在请求失败后才 fallback。

---

## 实施计划

### Task 1: 修复 Rust 端钉钉 channel 群聊回复逻辑

**文件**: `src/openhuman/channels/providers/dingtalk.rs`

#### 1.1 存储 sessionWebhook 过期时间

当前 `session_webhooks` 只存 `HashMap<String, String>`（chatID -> webhook URL），需要改为存储过期时间：

```rust
struct SessionWebhookEntry {
    url: String,
    expires_at: u64, // 毫秒时间戳
}
// session_webhooks: Arc<RwLock<HashMap<String, SessionWebhookEntry>>>
```

在 `listen()` 的消息解析中，同时存储 `sessionWebhookExpiredTime`。

#### 1.2 存储 conversationId 用于群聊 fallback

在 `listen()` 中，对于群聊消息（`conversationType == "2"`），额外存储 `conversationId`（即 `openConversationId`）：

```rust
// group_conversations: Arc<RwLock<HashMap<String, String>>>  // chatID -> conversationId
```

#### 1.3 添加群聊发送 API

新增 `send_via_group_api()` 方法，调用 `/v1.0/robot/groupMessages/send`：

```rust
async fn send_via_group_api(
    &self,
    open_conversation_id: &str,
    message: &SendMessage,
) -> anyhow::Result<()> {
    let token = self.get_access_token().await?;
    let body = serde_json::json!({
        "robotCode": self.client_id,
        "openConversationId": open_conversation_id,
        "msgKey": "sampleMarkdown",
        "msgParam": serde_json::json!({
            "title": message.subject.as_deref().unwrap_or("OpenHuman"),
            "text": message.content,
        }).to_string(),
    });
    // POST https://api.dingtalk.com/v1.0/robot/groupMessages/send
    // Header: x-acs-dingtalk-access-token
}
```

#### 1.4 修改 send() 的 fallback 逻辑

```
send() 策略:
1. 检查 sessionWebhook 是否存在且未过期 → send_via_webhook()
2. 如果 webhook 过期或不存在:
   a. 检查 recipient 是否有对应的 group conversationId → send_via_group_api()
   b. 否则 → send_via_access_token()（单聊）
3. 如果 webhook 发送失败（可能过期了）:
   a. 清理过期的 webhook
   b. 按 2 的逻辑 fallback
```

### Task 2: 更新前端连接引导，添加权限配置步骤

**文件**: `app/src/components/channels/DingTalkConfig.tsx`

在 `SetupGuide` 组件中，在步骤 3（启用机器人能力）和步骤 4（事件订阅）之间，插入权限配置步骤：

```
新步骤：在「权限管理」中申请以下权限：
- 企业内机器人发送消息权限（必需，用于机器人回复消息）

如果需要获取用户信息（可选）：
- 成员信息读权限
- 通讯录个人信息读权限
```

同时在 `SetupGuide` 的提示框中强调：
- 权限申请后需要等待审批（企业内部应用通常自动通过）
- 没有此权限，机器人可以接收消息但**无法回复**

### Task 3: 添加 i18n 翻译

**文件**: 相关 i18n locale 文件

为新增的权限配置引导文案添加中英文翻译。

### Task 4: 增强诊断日志

**文件**: `src/openhuman/channels/providers/dingtalk.rs`

- 在 `listen()` 中记录收到的 `conversationType`、`sessionWebhookExpiredTime`
- 在 `send()` 中记录使用的发送路径（webhook / group API / single chat API）
- 在 webhook 过期时记录警告日志
- 在权限不足导致发送失败时，输出明确的错误提示（如 "DingTalk: 发送失败，请检查是否已在开放平台申请「企业内机器人发送消息权限」"）

### Task 5: 单元测试

**文件**: `src/openhuman/channels/providers/dingtalk.rs` (mod tests)

- 测试 `resolve_chat_id` 正确区分单聊/群聊
- 测试 sessionWebhook 过期判断逻辑
- 测试 fallback 路径选择逻辑（群聊 vs 单聊）
- 测试 `is_user_allowed` 通配符和特定用户

---

## 涉及的钉钉 API 清单

| API | 用途 | 权限 |
|-----|------|------|
| `POST /v1.0/gateway/connections/open` | 注册 Stream 连接 | 无需额外权限 |
| `POST /v1.0/oauth2/accessToken` | 获取访问令牌 | 无需额外权限 |
| `POST /v1.0/robot/oToMessages/batchSend` | 单聊发消息 | 企业内机器人发送消息权限 |
| `POST /v1.0/robot/groupMessages/send` | 群聊发消息 | 企业内机器人发送消息权限 |
| `sessionWebhook` URL | 回复当前消息（临时） | 无需额外权限 |

## 风险与注意事项

- `sessionWebhook` 是最优先的回复路径，因为它不需要额外权限且延迟最低
- 群聊发送 API 不支持 @ 功能（钉钉文档明确说明）
- `oToMessages/batchSend` 的调用量限制：标准版 1 万次/月
- 需要确认 `openConversationId` 和当前代码中的 `conversationId` 是否一致（从文档消息体看是同一个字段）


updateAtTime: 2026/5/20 21:44:50

planId: 7d99d1aa-112c-4835-af26-7659e051ead1