export type ComposerSendBlockReason =
  | 'empty_input'
  | 'missing_thread'
  | 'composer_blocked'
  | 'usage_limit_reached'
  | 'socket_disconnected';

export type SlashCommandDecision =
  | { kind: 'new_or_clear'; blockedByWelcomeLock: boolean }
  | { kind: 'not_handled' };

export interface ComposerSendDecisionArgs {
  rawText: string;
  selectedThreadId: string | null;
  composerInteractionBlocked: boolean;
  isAtLimit: boolean;
  socketStatus: string;
  /**
   * `true` when the user is running in local-only mode — no cloud session token,
   * or a stored custom-LLM endpoint. In that mode both gates are skipped:
   *   1. cloud usage limit (there is no cloud quota to consult), and
   *   2. socket pre-flight (the socket targets the in-process core, so the
   *      `selectSocketStatus` redux snapshot can race with the boot-time
   *      handshake — `chatService.chatSend` has its own 3s wait that will
   *      catch a genuine outage).
   */
  isLocalOnlyMode?: boolean;
}

export interface ComposerSendDecision {
  shouldSend: boolean;
  trimmedText: string;
  blockReason?: ComposerSendBlockReason;
}

export interface ComposerBlockedSendFeedback {
  error: { code: 'usage_limit_reached' | 'socket_disconnected'; message: string };
}

export interface ComposerKeyDownEventLike {
  key: string;
  shiftKey?: boolean;
  isComposing?: boolean;
  keyCode?: number;
  nativeEvent?: { isComposing?: boolean; keyCode?: number };
}

export const handleComposerSlashCommand = (
  command: string,
  welcomeLocked: boolean
): SlashCommandDecision => {
  const cmd = command.toLowerCase();
  if (cmd === '/new' || cmd === '/clear') {
    return { kind: 'new_or_clear', blockedByWelcomeLock: welcomeLocked };
  }
  return { kind: 'not_handled' };
};

export const evaluateComposerSend = (args: ComposerSendDecisionArgs): ComposerSendDecision => {
  const trimmedText = args.rawText.trim();

  if (!trimmedText) {
    return { shouldSend: false, trimmedText, blockReason: 'empty_input' };
  }

  if (!args.selectedThreadId) {
    return { shouldSend: false, trimmedText, blockReason: 'missing_thread' };
  }

  if (args.composerInteractionBlocked) {
    return { shouldSend: false, trimmedText, blockReason: 'composer_blocked' };
  }

  if (args.isAtLimit && !args.isLocalOnlyMode) {
    return { shouldSend: false, trimmedText, blockReason: 'usage_limit_reached' };
  }

  // Only block on an explicit `'disconnected'`. `'connecting'` is the normal
  // boot-time state (handshake usually completes in a few hundred ms) and
  // `chatService.chatSend` already retries up to 3s for a valid `socket.id`
  // before throwing — pre-flight blocking on `'connecting'` makes the user
  // see a hard error when sending a fraction of a second too early. Genuine
  // outages still fall through to the `chatSend` error path which surfaces
  // the same `socket_disconnected` code.
  if (args.socketStatus === 'disconnected' && !args.isLocalOnlyMode) {
    return { shouldSend: false, trimmedText, blockReason: 'socket_disconnected' };
  }

  return { shouldSend: true, trimmedText };
};

export const isComposerImeComposing = (
  event: ComposerKeyDownEventLike,
  compositionActive = false
): boolean =>
  compositionActive ||
  event.isComposing === true ||
  event.keyCode === 229 ||
  event.nativeEvent?.isComposing === true ||
  event.nativeEvent?.keyCode === 229;

export const shouldSendComposerKeyDown = (
  event: ComposerKeyDownEventLike,
  compositionActive = false
): boolean =>
  event.key === 'Enter' && !event.shiftKey && !isComposerImeComposing(event, compositionActive);

export const getComposerBlockedSendFeedback = (
  blockReason: ComposerSendBlockReason | undefined
): ComposerBlockedSendFeedback | null => {
  if (blockReason === 'usage_limit_reached') {
    return {
      error: {
        code: 'usage_limit_reached',
        message: 'Included budget exhausted. Top up credits or upgrade to continue.',
      },
    };
  }

  if (blockReason === 'socket_disconnected') {
    return {
      error: {
        code: 'socket_disconnected',
        message:
          'Realtime socket is not connected — responses cannot be delivered without a client ID.',
      },
    };
  }

  return null;
};
