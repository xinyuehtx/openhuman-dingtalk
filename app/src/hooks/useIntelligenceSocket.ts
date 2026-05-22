import { useCallback, useEffect, useRef } from 'react';

import { useCoreState } from '../providers/CoreStateProvider';
import { socketService } from '../services/socketService';
import { useAppSelector } from '../store/hooks';
import { selectSocketStatus } from '../store/socketSelectors';

export const useIntelligenceSocket = () => {
  const socketStatus = useAppSelector(selectSocketStatus);

  return {
    isConnected: socketStatus === 'connected',
    isReady: socketStatus === 'connected',
    sendMessage: async () => {},
    sendChatInit: async () => {},
    sendTyping: () => {},
  };
};

export const useIntelligenceSocketManager = () => {
  const { snapshot } = useCoreState();
  const socketStatus = useAppSelector(selectSocketStatus);
  const isConnected = socketStatus === 'connected';
  const token = snapshot.sessionToken;
  const previousTokenRef = useRef<string | null>(null);

  const connect = useCallback(
    (nextToken?: string | null) => {
      const tokenToUse = nextToken ?? token;
      if (tokenToUse) {
        socketService.connect(tokenToUse);
      }
    },
    [token]
  );

  const disconnect = useCallback(() => {
    socketService.disconnect();
  }, []);

  useEffect(() => {
    const previousToken = previousTokenRef.current;

    // Local-only mode (no cloud session). `SocketProvider` keeps a
    // placeholder-token connection alive so `chatService.chatSend` can route
    // events via `socket.id`. Tearing it down here would silently kill the
    // realtime channel the moment the user opens the Intelligence/Memory
    // page — `connect()` below cannot revive it because its `tokenToUse`
    // guard short-circuits when `token === null`, so the next chatSend
    // throws "Socket not connected — no client ID for event routing". Bail
    // out and let SocketProvider own the lifecycle in this mode.
    if (!token) {
      previousTokenRef.current = null;
      return;
    }

    if (previousToken && previousToken !== token) {
      disconnect();
      previousTokenRef.current = token;
      connect(token);
      return;
    }

    if (!isConnected) {
      previousTokenRef.current = token;
      connect();
    }
  }, [connect, disconnect, isConnected, token]);

  return { connect, disconnect, isConnected, isReady: Boolean(token) && isConnected };
};

export const useIntelligenceEvents = () => ({
  onAgentResponse: () => () => {},
  onExecutionProgress: () => () => {},
  onExecutionComplete: () => () => {},
});
