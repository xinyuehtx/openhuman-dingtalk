5.6 ### fix-socket-reconnection ###
修复 useIntelligenceSocketManager 在 local-only 模式下错误断开 SocketProvider 已建立的 socket 连接，导致 IM 对话报 "Socket not connected" 错误。

## 问题根因

`app/src/hooks/useIntelligenceSocket.ts` 中的 `useIntelligenceSocketManager` hook 在 Intelligence/Memory 页面挂载时，如果 `token === null`（local-only 模式），会调用 `disconnect()` 把 `SocketProvider` 用占位符 token 建好的 socket 连接断开。之后因为 `token === null`，`connect()` 的 `if (tokenToUse)` 守卫阻止重连，socket 永久死亡。

复现路径：

1. 应用启动 → SocketProvider 用 'openhuman-local' 连接成功
2. 用户访问 Intelligence/Memory 页面 → useIntelligenceSocketManager 挂载
3. `token = null`, `isConnected = true` → `disconnect()` 被调用
4. socket 永久断开，无法重连
5. 用户切回聊天页 → chatSend → waitForSocketClientId 超时 → 报错

## 修复方案

### 步骤 1：修复 useIntelligenceSocketManager 的 disconnect 逻辑

**文件**: `app/src/hooks/useIntelligenceSocket.ts`（第 43-49 行）

在 `!token` 分支中，只有当 `previousToken` 存在（即从有 token 变成无 token，表示登出场景）时才断开连接。不应该因为 `isConnected === true` 就断开——这是 `SocketProvider` 用占位符 token 维护的合法连接。

```typescript
// 修改前
if (!token) {
  if (previousToken || isConnected) {
    disconnect();
  }
  previousTokenRef.current = null;
  return;
}

// 修改后
if (!token) {
  // Only disconnect if transitioning FROM a real token to no-token (logout).
  // Do NOT disconnect when token was always null — SocketProvider maintains
  // a valid placeholder-token connection for local-only mode.
  if (previousToken) {
    disconnect();
  }
  previousTokenRef.current = null;
  return;
}
```

关键改动：移除 `|| isConnected` 条件。只在 `previousToken` 存在（真正的登出场景）时才 disconnect。

### 步骤 2（防御性）：增加 socketService 重连保护

**文件**: `app/src/services/socketService.ts`（第 188-189 行）

将 `reconnectionAttempts: 5` 改为更大的值，避免因 Core 启动慢导致的永久断连：

```typescript
// 修改前
reconnectionAttempts: 5,

// 修改后
reconnectionAttempts: 30,
reconnectionDelayMax: 5000,
```

### 步骤 3（防御性）：chatSend 中增加重连尝试

**文件**: `app/src/services/chatService.ts`（第 693-703 行）

在 `waitForSocketClientId` 中，如果 socket 完全断开且不活跃，主动触发重连：

```typescript
async function waitForSocketClientId(timeoutMs = 5000): Promise<string | null> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const id = socketService.getSocket()?.id;
    if (id) return id;

    // If socket is fully dead, attempt reconnect with placeholder token.
    const socket = socketService.getSocket();
    if (socket && socket.disconnected && !socket.active) {
      socket.connect();
    }

    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return socketService.getSocket()?.id ?? null;
}
```

## 影响范围

- `app/src/hooks/useIntelligenceSocket.ts` — **核心修复**（1 行条件改动）
- `app/src/services/socketService.ts` — 防御性增强（重连策略）
- `app/src/services/chatService.ts` — 防御性增强（主动重连 + 超时增加）

## 验证方式

1. Local 模式启动 → 访问 Intelligence/Memory 页面 → 切回聊天 → 发消息 → 不再报错
2. Cloud 模式登出 → 确认 disconnect 仍然正常工作
3. 现有单测通过

updateAtTime: 2026/5/22 11:01:36

planId: 388f4e17-5cac-44da-bb99-d0c19af7363f
