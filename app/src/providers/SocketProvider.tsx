import { useEffect, useRef } from 'react';

import { useDaemonLifecycle } from '../hooks/useDaemonLifecycle';
import { callCoreRpc } from '../services/coreRpcClient';
import { socketService } from '../services/socketService';
import { setBackend, setCore } from '../store/connectivitySlice';
import { store } from '../store/index';
import { IS_DEV } from '../utils/config';
import { useCoreState } from './CoreStateProvider';

/**
 * Placeholder token used for the core socket connection when there is no
 * cloud session. The core socket.io server does NOT validate the token —
 * it is only used as the `auth` handshake field. Using a constant placeholder
 * lets the socket connect on boot so chat events (chat_done, chat_error,
 * text_delta, etc.) can be routed back to the frontend via the socket's
 * `client_id`. Without this, `chatSend` would throw
 * "Socket not connected — no client ID for event routing" for any user
 * running the DingTalk fork without a backend session.
 */
const LOCAL_PLACEHOLDER_TOKEN = 'openhuman-local';

/**
 * SocketProvider manages the socket connection based on the cloud session
 * token. The frontend TypeScript socket client is the single realtime path
 * for both desktop and web.
 *
 * In local-only mode (no backend session — common in the DingTalk fork)
 * the provider connects with a placeholder token so chat RPC calls can
 * obtain a valid `socket.id` for event routing. The backend session RPC
 * is skipped in that mode since there is nothing to authenticate with.
 */
const SocketProvider = ({ children }: { children: React.ReactNode }) => {
  const { snapshot } = useCoreState();
  const token = snapshot.sessionToken;
  const isLocalOnlyMode = !token;
  const effectiveToken = token ?? LOCAL_PLACEHOLDER_TOKEN;
  const previousTokenRef = useRef<string | null>(null);

  // Keep daemon lifecycle management for desktop health/recovery.
  const daemonLifecycle = useDaemonLifecycle();

  useEffect(() => {
    if (IS_DEV) {
      console.log('[SocketProvider] Daemon lifecycle state:', {
        isAutoStartEnabled: daemonLifecycle.isAutoStartEnabled,
        connectionAttempts: daemonLifecycle.connectionAttempts,
        isRecovering: daemonLifecycle.isRecovering,
        maxAttemptsReached: daemonLifecycle.maxAttemptsReached,
      });
    }
  }, [
    daemonLifecycle.isAutoStartEnabled,
    daemonLifecycle.connectionAttempts,
    daemonLifecycle.isRecovering,
    daemonLifecycle.maxAttemptsReached,
  ]);

  // Handle socket connection based on effective token. `effectiveToken` is
  // always truthy (real session token, or `LOCAL_PLACEHOLDER_TOKEN`) so the
  // socket is always connected — only the backend session RPC is gated on
  // having a real cloud session.
  useEffect(() => {
    const previousToken = previousTokenRef.current;

    if (effectiveToken !== previousToken) {
      previousTokenRef.current = effectiveToken;
      socketService.connect(effectiveToken);

      // In local-only mode we skip the backend socket connection RPC —
      // there is no backend session to authenticate with.
      if (!isLocalOnlyMode) {
        // Also connect the Rust sidecar to backend-alphahuman so inbound
        // Discord/Telegram managed-DM messages reach the agent loop.
        void callCoreRpc({ method: 'openhuman.socket_connect_with_session', params: {} }).catch(
          (err: unknown) => {
            // Non-fatal: sidecar may not be running yet or backend unreachable.
            console.error(
              '[SocketProvider] openhuman.socket_connect_with_session: RPC connection failed (non-fatal) — sidecar may not be running yet or backend unreachable',
              err
            );
            const message = err instanceof Error ? err.message : String(err);
            const isCoreTransportFailure =
              /ECONNREFUSED|ERR_CONNECTION_REFUSED|Failed to fetch|NetworkError/i.test(message);
            if (isCoreTransportFailure) {
              store.dispatch(setCore({ value: 'unreachable', error: message }));
            } else {
              store.dispatch(setBackend({ value: 'disconnected', error: message }));
            }
          }
        );
      } else if (IS_DEV) {
        console.log('[SocketProvider] Local-only mode — skipping backend socket connection');
      }
    }
  }, [effectiveToken, isLocalOnlyMode]);

  // Cleanup on unmount only
  useEffect(() => {
    return () => {
      socketService.disconnect();
    };
  }, []);

  return <>{children}</>;
};

export default SocketProvider;
