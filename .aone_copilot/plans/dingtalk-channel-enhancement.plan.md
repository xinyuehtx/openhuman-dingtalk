### dingtalk-channel-enhancement ###
增强现有钉钉 channel 实现：补充前端配置组件与引导说明，增强消息格式兼容性（支持 streaming），并改进连接稳定性。


## 背景

项目中已有完整的钉钉 Stream Mode WebSocket channel 实现（`src/openhuman/channels/providers/dingtalk.rs`），包括连接注册、消息收发、用户权限、心跳等核心逻辑。前端也已注册了 dingtalk channel 的定义和图标。

核心连接方式参考 cc-connect 项目（`https://github.com/chenhg5/cc-connect`）：
- 使用钉钉 Stream Mode（WebSocket 长连接，无需公网 IP）
- 通过 `https://api.dingtalk.com/v1.0/gateway/connections/open` 注册连接获取 WebSocket 端点
- 通过 session webhook 回复消息

**当前缺失项：**
1. 前端没有钉钉专用配置组件（ChannelSetupModal 中 dingtalk case 走 default 分支显示 "config not available"）
2. 缺少接入引导说明（用户如何在钉钉开放平台创建应用、获取凭证）
3. 消息发送仅支持 markdown 格式，不支持 streaming/draft updates
4. 连接断开后缺少内部重连（虽然 supervisor 会重启，但 session webhook 全部丢失）

---

## 实施步骤

### 1. 创建前端 DingTalk 配置组件

**文件**: `app/src/components/channels/DingTalkConfig.tsx`

参考 `TelegramConfig.tsx` 的结构，创建 DingTalk 专用配置组件：
- 渲染 Client ID (AppKey) 和 Client Secret (AppSecret) 输入表单
- 支持 allowed_users 可选配置
- 添加配置引导说明区块（接入步骤）
- 连接/断开按钮

### 2. 在 ChannelSetupModal 中注册 DingTalk 组件

**文件**: `app/src/components/channels/ChannelSetupModal.tsx`

在 `ChannelConfigContent` switch 中添加 `dingtalk` case：
```tsx
case 'dingtalk':
  return <DingTalkConfig definition={definition} />;
```

同时更新 `CHANNEL_ICONS` 映射添加 dingtalk 图标。

### 3. 添加钉钉接入引导文案

在 DingTalkConfig 组件中添加折叠式引导区域，包含：
- 步骤 1：访问钉钉开放平台创建应用
- 步骤 2：获取 AppKey 和 AppSecret
- 步骤 3：启用机器人能力
- 步骤 4：配置事件订阅（选择 Stream 模式）
- 步骤 5：发布应用并添加机器人到聊天

关键链接：
- 钉钉开放平台: `https://open.dingtalk.com/`
- Stream 模式文档: `https://open.dingtalk.com/document/development/introduction-to-stream-mode`

### 4. 增强消息发送 — 支持 streaming/draft updates

**文件**: `src/openhuman/channels/providers/dingtalk.rs`

为 DingTalkChannel 添加 draft/streaming 支持：
- 实现 `supports_draft_updates()` 返回 `true`
- 实现 `send_draft()` — 首次发送消息，返回 message_id（使用 webhook）
- 实现 `update_draft()` — 更新已发送的消息内容（通过钉钉 API 更新卡片消息）
- 实现 `finalize_draft()` — 完成最终消息

注意：钉钉 session webhook 不支持消息更新，需要使用 Interactive Card (互动卡片) 方案：
- 发送时使用 `actionCard` 类型
- 更新时通过 access_token + 卡片 ID 更新内容

如果 Interactive Card 实现复杂度过高，可先保持 markdown 方式，streaming 使用"发送新消息替代更新"的降级方案。

### 5. 增强连接稳定性 — access_token 主动消息

**文件**: `src/openhuman/channels/providers/dingtalk.rs`

session webhook 有 30 分钟有效期限制。增加基于 access_token 的消息发送作为 fallback：
- 添加 `access_token` 缓存字段（含过期时间）
- 当 session webhook 过期或不存在时，通过 `https://oapi.dingtalk.com/gettoken` 获取 access_token
- 使用 `https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend` 发送消息

### 6. 添加 DingTalk 配置的国际化文案

**文件**: 国际化资源文件中添加对应的 key

添加以下文案 key：
- `channels.dingtalk.title`
- `channels.dingtalk.description`
- `channels.dingtalk.setupGuide`
- `channels.dingtalk.step1` ~ `channels.dingtalk.step5`
- `channels.dingtalk.clientIdHelp`
- `channels.dingtalk.clientSecretHelp`

### 7. 单元测试

**Rust 测试**（已有基础测试，需补充）：
- 消息格式解析测试（群聊/私聊路由）
- access_token 获取失败降级逻辑
- webhook 过期场景

**前端测试**：
- DingTalkConfig 组件渲染测试
- 表单验证测试
- 连接/断开流程测试

---

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `app/src/components/channels/DingTalkConfig.tsx` | 新建 | 钉钉配置组件 |
| `app/src/components/channels/ChannelSetupModal.tsx` | 修改 | 注册 DingTalk 组件 |
| `src/openhuman/channels/providers/dingtalk.rs` | 修改 | 增强消息发送、添加 access_token fallback |
| 国际化文件 | 修改 | 添加钉钉相关文案 |
| `app/src/components/channels/DingTalkConfig.test.tsx` | 新建 | 前端测试 |

---

## 优先级

1. **P0** (必须): 步骤 1-3 — 前端配置组件 + 引导说明（让用户能顺利配置）
2. **P1** (重要): 步骤 5 — access_token fallback（解决 webhook 过期问题）
3. **P2** (增强): 步骤 4 — streaming 支持（提升用户体验）
4. **P2** (增强): 步骤 6-7 — 国际化 + 测试


updateAtTime: 2026/5/20 20:04:09

planId: a5041718-fd24-4a42-bcdf-5af8e95b2cd9