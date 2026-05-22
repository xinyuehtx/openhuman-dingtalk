### fix-socket-not-connected-error ###
修复 chatSend 调用时 Socket 未连接导致的 "Socket not connected — no client ID for event routing" 错误，通过添加等待 Socket 连接的重试机制和改善错误处理来解决时序竞争问题。


## 问题诊断

错误来源：`app/src/services/chatService.ts:698`

```typescript
export async function chatSend(params: ChatSendParams): Promise<void> {
  const socket = socketService.getSocket();
  const clientId = socket?.id;
  if (!clientId) {
    throw new Error('Socket not connected — no client ID for event routing');
  }
  // ...
}
```

**根因**：存在时序竞争——`evaluateComposerSend` 使用 Redux store 的 `socketStatus` 做发送前检查，但 `chatSend` 直接访问 `socketService.getSocket()?.id`。两者之间存在不一致窗口：
1. Redux 状态 `connected` 但 socket.id 尚未赋值（connect 事件回调执行前后的微小窗口）
2. Custom LLM 模式下 `evaluateComposerSend` 跳过 socket 检查但 socket 实际未连上
3. 网络闪断导致 socket 断开但 Redux 状态尚未同步更新

## 修复方案

### 步骤 1：在 `chatSend` 中添加带超时的 Socket 连接等待

修改 `app/src/services/chatService.ts`，在 `chatSend` 函数中添加一个短暂的等待机制，给 Socket 一个连接窗口，而不是立即抛错。

```typescript
/**
 * Wait for the socket to become connected and have an ID, with a timeout.
 * Returns the client ID if successful, null if timed out.
 */
function waitForSocketId(timeoutMs = 3000): Promise<string | null> {
  const socket = socketService.getSocket();
  if (socket?.id) return Promise.resolve(socket.id);

  return new Promise((resolve) => {
    const timeout = setTimeout(() => {
      resolve(null);
    }, timeoutMs);

    // Check periodically if socket connects
    const interval = setInterval(() => {
      const s = socketService.getSocket();
      if (s?.id) {
        clearTimeout(timeout);
        clearInterval(interval);
        resolve(s.id);
      }
    }, 100);

    // Also cleanup interval on timeout
    setTimeout(() => clearInterval(interval), timeoutMs);
  });
}

export async function chatSend(params: ChatSendParams): Promise<void> {
  let clientId = socketService.getSocket()?.id;

  // If socket is not immediately available, wait briefly for connection
  if (!clientId) {
    clientId = await waitForSocketId(3000);
  }

  if (!clientId) {
    throw new Error('Socket not connected — no client ID for event routing');
  }

  await callCoreRpc({
    method: 'openhuman.channel_web_chat',
    params: {
      client_id: clientId,
      thread_id: params.threadId,
      message: params.message,
      model_override: params.model ?? undefined,
      profile_id: params.profileId ?? undefined,
      locale: params.locale ?? undefined,
    },
  });
}
```

### 步骤 2：改善 `Conversations.tsx` 中的错误处理

修改 `app/src/pages/Conversations.tsx`，为 socket 断连错误添加更友好的用户提示，使用已有的 `socket_disconnected` 错误码：

```typescript
// 在 chatSend 的 catch 块中（约第 860-870 行）
} catch (err) {
  // ...existing logic...
  const msg = err instanceof Error ? err.message : String(err);
  if (msg.includes('no client ID for event routing') || msg.includes('Socket not connected')) {
    setSendError(chatSendError('socket_disconnected', 
      'Realtime socket is not connected — responses cannot be delivered without a client ID.'));
  } else if (/* existing conditions */) {
    // ...existing error handling...
  }
}
```

### 步骤 3：修复 Custom LLM 模式下的竞态

在 `SocketProvider.tsx` 中，`socketService.connect(effectiveToken)` 是异步的（内部调用 `connectAsync`），但没有 await。Socket.IO 连接需要时间才能建立。在 Custom LLM 模式下，如果用户在 Socket 尚未连接完成时就发送消息，就会触发此错误。

步骤 1 的等待机制已经覆盖了这个场景。

### 步骤 4：添加 debug 日志

在 `chatService.ts` 的 `chatSend` 函数中添加 debug 日志，帮助后续诊断：

```typescript
import debug from 'debug';
const chatLog = debug('chat:send');

export async function chatSend(params: ChatSendParams): Promise<void> {
  let clientId = socketService.getSocket()?.id;
  chatLog('chatSend called', { 
    hasSocket: !!socketService.getSocket(), 
    hasId: !!clientId, 
    threadId: params.threadId 
  });

  if (!clientId) {
    chatLog('Socket ID not immediately available, waiting...');
    clientId = await waitForSocketId(3000);
    chatLog('waitForSocketId result', { clientId: clientId ?? 'timeout' });
  }
  // ...
}
```

## 涉及文件

- `app/src/services/chatService.ts` — 主要修改，添加等待机制和日志
- `app/src/pages/Conversations.tsx` — 改善 catch 块的 socket 断连错误识别

## 验证

1. `pnpm typecheck` — 类型检查通过
2. `pnpm lint` — lint 通过
3. `pnpm test` — 已有的 chatService 相关测试通过
4. 手动验证：启动 `pnpm dev` + 核心服务，确认聊天发送正常工作


updateAtTime: 2026/5/22 09:29:52

planId: e15ad845-b217-4be5-9796-dadccd8f95fc