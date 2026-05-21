### dingtalk-at-mention-routing ###
实现 OpenHuman Web UI 中 @钉钉用户 的消息路由机制：当 OpenHuman 用户在聊天中 @钉钉用户时，消息通过钉钉机器人推送到对应用户；普通消息继续和 Agent 对话。


## 背景与目标

在钉钉-OpenHuman 集成中有三个角色：
- **钉钉用户** — 在钉钉中发消息
- **Agent** — AI 助手，自动回复
- **OpenHuman 用户** — 在 Web UI 中操作

当前状态：
- 钉钉用户 → Agent 回复 (已工作)
- OpenHuman 用户 → Agent 回复 (已工作，回复仅在 Web UI)
- OpenHuman 用户 @钉钉用户 → 推送到钉钉 (**需要新增**)

## 实现方案

### 整体思路

在 Rust core 的 `start_chat` / `run_chat_task` 路径中，**拦截消息内容**，解析 `@dingtalk:用户ID` 格式的 mention。如果检测到 mention：
1. 提取目标用户 ID 和消息内容
2. 通过已注册的 `channel.send` native handler 将消息推送到钉钉
3. 同时在 Web UI 中显示发送结果（不走 Agent 对话）

### 任务清单

#### 1. 在 `channels/bus.rs` 中添加 @mention 解析工具函数

**文件**: `src/openhuman/channels/bus.rs`

添加一个解析函数，从消息内容中提取 `@dingtalk:用户ID` 格式的 mention：

```rust
/// Parse @mention directives from a message.
/// Format: `@dingtalk:<userId> <message content>`
/// Returns Some((channel_name, recipient_id, cleaned_message)) if a mention is found.
pub fn parse_channel_mention(message: &str) -> Option<(String, String, String)> {
    // Match pattern like: @dingtalk:userId rest of message
    let re = regex::Regex::new(r"^@(\w+):(\S+)\s+(.+)$").ok()?;
    let caps = re.captures(message.trim())?;
    let channel = caps.get(1)?.as_str().to_string();
    let recipient = caps.get(2)?.as_str().to_string();
    let content = caps.get(3)?.as_str().to_string();
    Some((channel, recipient, content))
}
```

#### 2. 在 `web.rs` 的 `start_chat` 中添加 @mention 拦截逻辑

**文件**: `src/openhuman/channels/providers/web.rs`

在 `start_chat()` 函数中，消息验证之后、启动 `run_chat_task` 之前，添加 @mention 检测：

```rust
// Check for @channel:recipient mention — route to external channel instead of agent
if let Some((channel_name, recipient, content)) = 
    crate::openhuman::channels::bus::parse_channel_mention(&message) 
{
    // Spawn async task to send via channel and emit result as WebChannelEvent
    let client_id_task = client_id.clone();
    let thread_id_task = thread_id.clone();
    let request_id_task = request_id.clone();
    tokio::spawn(async move {
        let result = crate::core::event_bus::request_native_global::<
            ChannelSendRequest, ChannelSendResponse
        >(
            CHANNEL_SEND_METHOD,
            ChannelSendRequest {
                channel_name: channel_name.clone(),
                recipient: recipient.clone(),
                content: content.clone(),
                thread_ts: None,
            },
        ).await;
        
        match result {
            Ok(_) => {
                // Emit success event to Web UI
                publish_web_channel_event(WebChannelEvent {
                    event: "chat_done".to_string(),
                    client_id: client_id_task,
                    thread_id: thread_id_task,
                    request_id: request_id_task,
                    full_response: Some(format!(
                        "Message sent to @{} via {}: {}", 
                        recipient, channel_name, content
                    )),
                    ..Default::default()
                });
            }
            Err(e) => {
                publish_web_channel_event(WebChannelEvent {
                    event: "chat_error".to_string(),
                    client_id: client_id_task,
                    thread_id: thread_id_task,
                    request_id: request_id_task,
                    message: Some(format!(
                        "Failed to send message to @{} via {}: {}", 
                        recipient, channel_name, e
                    )),
                    error_type: Some("inference".to_string()),
                    ..Default::default()
                });
            }
        }
    });
    return Ok(request_id);
}
```

#### 3. 确保 `ChannelSendRequest` 和相关类型从 `bus.rs` 中公开导出

**文件**: `src/openhuman/channels/bus.rs`

当前 `ChannelSendRequest`、`ChannelSendResponse`、`CHANNEL_SEND_METHOD` 已经是 `pub`，但需要确保 `web.rs` 可以正确引用它们。检查 `src/openhuman/channels/mod.rs` 的导出。

#### 4. 编译验证

运行 `cargo check` 确保所有修改编译通过。

#### 5. 日志验证

添加充分的 tracing 日志覆盖 @mention 路径：
- 解析到 @mention 时记录 channel、recipient、content 长度
- 发送成功/失败时记录详细信息

### 消息格式约定

OpenHuman 用户在 Web UI 中发送消息时使用以下格式触发 @mention：

```
@dingtalk:userId 你好，这是一条推送消息
```

其中：
- `dingtalk` — 目标 channel 名称（与 `Channel::name()` 返回值匹配）
- `userId` — 钉钉用户的 senderStaffId（从之前钉钉用户发送的消息中获取）
- 后面的内容为要推送的消息体

### 后续可扩展

- 前端 UI 中添加 @mention 的自动补全（显示已知的钉钉用户列表）
- 支持 `@telegram:chatId`、`@slack:userId` 等其他 channel 的 @mention
- 将钉钉用户的 senderStaffId 和用户名映射关系持久化，方便 UI 展示


updateAtTime: 2026/5/21 14:11:48

planId: a4cb93e9-2786-4ca5-9d19-9c46ffc50f9a