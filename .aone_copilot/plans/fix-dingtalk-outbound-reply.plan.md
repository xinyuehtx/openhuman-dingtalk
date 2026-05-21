### fix-dingtalk-outbound-reply ###
修复从 OpenHuman Web UI 发消息到钉钉失败的问题。核心原因是 agent 回复只在 web channel 中展示，缺少将回复转发到本地 DingTalkChannel 实例的机制。


## 问题诊断

### 根因分析

系统中存在两条独立的消息处理路径：

1. **路径A（正常工作）**：钉钉消息通过 DingTalk Stream WebSocket 接收 → `process_channel_message()` → agent → `DingTalkChannel::send()` 直接回复。这条路径完全在本地 channel runtime 中完成。

2. **路径B（失败）**：Web UI 发消息时，走 `web channel provider` 的 `start_chat()` → agent 完成后，回复通过 Socket.IO WebChannelEvent 发送到前端。**没有任何机制**将该回复转发到对应的外部 channel（DingTalk）。

`ChannelInboundSubscriber`（`bus.rs`）虽然存在，但它依赖远程后端 REST API + JWT 发送消息，在自定义 LLM / 无后端登录场景下不可用。而且它监听的是来自后端 Socket.IO 的 `ChannelInboundMessage` 事件（从远端推送的），不是本地 web UI 的消息。

### 日志证据

```
13:35:31 [web-channel] routing chat turn to 'orchestrator' via profile 'default'
         thread=channel:dingtalk_2218280140125194_2218280140125194
13:35:54 [agent_loop] turn complete: tokens_out=199
         (之后没有任何 send/reply 相关日志)
```

---

## 修复方案

### 核心思路

在 `ChannelInboundSubscriber`（`src/openhuman/channels/bus.rs`）的消息处理完成后，添加一条**本地通道转发路径**：当 agent 回复生成后，检查 thread_id 是否以 `channel:` 前缀开头并对应一个已注册的外部通道，如果是，则通过 native event bus 请求该通道的本地实例发送消息，而非依赖远程 REST API。

### 具体步骤

#### 步骤 1: 在 event bus 中注册 channel 发送的 native request handler

**文件**: `src/openhuman/channels/bus.rs`

定义 native request 类型和注册 handler：

```rust
pub struct ChannelSendRequest {
    pub channel_name: String,
    pub recipient: String,
    pub content: String,
    pub thread_ts: Option<String>,
}

pub struct ChannelSendResponse {
    pub success: bool,
}
```

在 channel runtime 启动时（`startup.rs`），将所有已注册的 channel 实例注册为 `"channel.send"` 的 native handler。这样任何模块都可以通过 `request_native_global("channel.send", ChannelSendRequest { ... })` 来向外部通道发送消息。

#### 步骤 2: 修改 ChannelInboundSubscriber 的回复路径

**文件**: `src/openhuman/channels/bus.rs`

在 `finalize_channel_reply()` 函数中，添加**本地通道发送作为首选路径**：

1. 首先尝试通过 `request_native_global("channel.send", ...)` 发送到本地通道实例
2. 如果 native handler 未注册（没有外部 channel 运行），则回退到现有的 backend REST API 路径

#### 步骤 3: 在 channel runtime startup 中注册 native handler

**文件**: `src/openhuman/channels/runtime/startup.rs`

在 `start_channels()` 中，遍历所有已启动的 channel 实例，注册 `"channel.send"` native handler，使其持有 `Arc<HashMap<String, Arc<dyn Channel>>>` 引用。

#### 步骤 4: 确保 Web UI 的 thread_id 能正确映射到 channel 名称

**文件**: `src/openhuman/channels/bus.rs`

当前代码中 `thread_id = format!("channel:{}", channel)` 已经包含了 channel 名称。需要确保在 `ChannelInboundSubscriber::handle()` 中从 `channel` 字段正确提取出外部通道名称。

从日志看，thread_id 格式为 `channel:dingtalk_2218280140125194_2218280140125194`，而 `channel` 字段直接就是 `"dingtalk"`，所以这一步已经满足。

#### 步骤 5: 添加调试日志

在所有修改点添加详细的 `tracing::debug!` / `tracing::info!` 日志，以便追踪消息转发的完整链路。

---

## 影响范围

- **只修改 Rust 代码**，不涉及前端
- **向后兼容**：现有的 backend REST API 路径作为 fallback 保留
- **适用于所有外部通道**（DingTalk、Telegram、Slack 等），不仅限于 DingTalk
- 遵循现有的 native event bus 模式（与 `agent.run_turn` 相同的架构风格）

## 涉及文件

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src/openhuman/channels/bus.rs` | 修改 | 添加 ChannelSendRequest/Response 类型，修改 finalize_channel_reply 添加本地发送路径 |
| `src/openhuman/channels/runtime/startup.rs` | 修改 | 注册 channel.send native handler |
| `src/openhuman/channels/providers/dingtalk.rs` | 无需修改 | DingTalkChannel 已实现 Channel trait 的 send() 方法 |


updateAtTime: 2026/5/21 13:46:19

planId: 9557d0cc-1ba2-46e3-b5b0-84e1034013bf