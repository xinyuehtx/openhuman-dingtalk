### dingtalk-allowed-users-default-wildcard ###
修复钉钉通道 allowed_users 字段的默认值问题：当用户在 UI 中留空该字段时，后端应默认设置为 ["*"]（允许所有用户），同时更新前端 placeholder 提示以匹配实际行为。


## 问题根因

用户通过 UI 连接钉钉通道时如果没有填写 `allowed_users` 字段，后端 `parse_allowed_users` 返回空 `Vec<String>`，而 `DingTalkChannel::is_user_allowed()` 对空列表的 `.any()` 永远返回 `false`，导致所有钉钉消息都被静默丢弃。

## 修改方案

### 1. 后端：`src/openhuman/channels/controllers/ops.rs`（约第 352 行）

在钉钉连接逻辑中，当 `parse_allowed_users` 返回空列表时，默认填入 `["*"]`：

```rust
// 当前代码（第 352-353 行）：
let allowed_users = parse_allowed_users(creds_map.get("allowed_users"));
let allowed_users_count = allowed_users.len();

// 修改为：
let parsed_allowed_users = parse_allowed_users(creds_map.get("allowed_users"));
let allowed_users = if parsed_allowed_users.is_empty() {
    vec!["*".to_string()]
} else {
    parsed_allowed_users
};
let allowed_users_count = allowed_users.len();
```

### 2. 前端：`app/src/lib/channels/definitions.ts`（约第 227 行）

更新 DingTalk `allowed_users` 字段的 `placeholder` 提示文案，明确说明留空即允许所有用户（与修改后的行为一致）：

```typescript
// 当前：
placeholder: 'Comma-separated DingTalk userIds; leave empty to allow any',

// 修改为：
placeholder: '* (default: all users); or comma-separated DingTalk userIds',
```

### 3. 已有配置修复

对于用户当前 `~/.openhuman/users/local/config.toml` 中已经存在的 `allowed_users = []`，用户需要重新通过 UI 连接钉钉通道（会自动使用新的默认值），或手动将配置改为 `allowed_users = ["*"]`。

## 影响范围

- **Rust**: `src/openhuman/channels/controllers/ops.rs` — 仅修改钉钉连接分支中的默认值逻辑
- **TypeScript**: `app/src/lib/channels/definitions.ts` — 仅修改 placeholder 文案
- 不影响已有填写了具体用户 ID 的配置
- 不影响其他通道（Telegram、Discord 等有各自的默认逻辑）


updateAtTime: 2026/5/21 11:33:17

planId: b54183d0-2bf6-44e3-b916-0aa01b29c116