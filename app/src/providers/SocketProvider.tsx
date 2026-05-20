import { useEffect, useRef } from 'react';

import { useDaemonLifecycle } from '../hooks/useDaemonLifecycle';
import { callCoreRpc } from '../services/coreRpcClient';
import { socketService } from '../services/socketService';
import { setBackend, setCore } from '../store/connectivitySlice';
import { store } from '../store/index';
import { IS_DEV } from '../utils/config';
import { hasStoredLlmSettings } from '../utils/configPersistence';
import { useCoreState } from './CoreStateProvider';

/**
 * Placeholder token used for the core socket connection in custom-LLM mode.
 * The core socket.io server does not validate the token — it is only used as
 * the `auth` handshake field. Using a constant placeholder lets the socket
 * connect normally so chat events (chat_done, chat_error, text_delta, etc.)
 * can be routed back to the frontend via the socket's `client_id`.
 */
const CUSTOM_LLM_PLACEHOLDER_TOKEN = 'custom-llm-local';

/**
 * SocketProvider manages the socket connection based on JWT token.
 * The frontend TypeScript socket client is the single realtime path
 * for both desktop and web.
 *
 * In custom-LLM mode (user has configured inference_url + api_key but has
 * no backend session), the provider connects with a placeholder token so
 * chat RPC calls can obtain a valid `socket.id` for event routing.
 */
const SocketProvider = ({ children }: { children: React.ReactNode }) => {
  const { snapshot } = useCoreState();
  const token = snapshot.sessionToken;
  const isCustomLlmMode = !token && hasStoredLlmSettings();
  const effectiveToken = token ?? (isCustomLlmMode ? CUSTOM_LLM_PLACEHOLDER_TOKEN : null);
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

  // Handle socket connection based on effective token
  useEffect(() => {
    const previousToken = previousTokenRef.current;

    // Token was set - connect
    if (effectiveToken && effectiveToken !== previousToken) {
      previousTokenRef.current = effectiveToken;
      socketService.connect(effectiveToken);

      // In custom-LLM mode we skip the backend socket connection RPC —
      // there is no backend session to authenticate with.
      if (!isCustomLlmMode) {
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
        console.log('[SocketProvider] Custom LLM mode — skipping backend socket connection');
      }
    }

    // Token was unset - disconnect
    if (!effectiveToken && previousToken) {
      previousTokenRef.current = null;
      socketService.disconnect();
    }
  }, [effectiveToken, isCustomLlmMode]);

  // Cleanup on unmount only
  useEffect(() => {
    return () => {
      if (!effectiveToken) {
        socketService.disconnect();
      }
    };
  }, [effectiveToken]);

  return <>{children}</>;
};

export default SocketProvider;
