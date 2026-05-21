/**
 * @-mention picker helpers for the chat composer.
 *
 * The composer supports a `@` trigger that opens a popover listing
 * targets the user can route to:
 *
 *   - The agent (default — no prefix inserted).
 *   - Each known DingTalk recipient (or other registered channel
 *     recipient), parsed from existing workspace conversation threads.
 *
 * Selecting a channel recipient inserts `@<channel>:<recipientId> `
 * into the input. The Rust core's `parse_channel_mention` then routes
 * the message to that external channel instead of running the agent
 * loop.
 *
 * These helpers are pure so they can be unit-tested without React.
 */
import type { Thread } from '../../types/thread';

/** A potential @-mention target. */
export type MentionTarget =
  | { kind: 'agent'; label: string }
  | {
      kind: 'channel';
      channel: string;
      recipientId: string;
      label: string;
      threadId: string;
      lastMessageAt: string;
    };

/** Mention-detection state, derived from the current input value + caret. */
export interface MentionDetection {
  /** `true` when the caret sits inside an active `@…` token. */
  active: boolean;
  /** Index in the input string of the leading `@`. -1 when inactive. */
  queryStart: number;
  /** Substring after `@` and before the caret (case-preserved). */
  query: string;
}

const CHANNEL_THREAD_PREFIX = 'channel:';

/**
 * Parse a conversation thread id of the form
 * `channel:<channel>_<sender>_<replyTarget>` (optionally suffixed with
 * `_thread:<ts>`) into its components.
 *
 * Returns `null` for thread ids that don't look like channel threads or
 * that don't have all three required segments — those are workspace
 * threads owned by the desktop UI, not external-channel mirrors.
 */
export function parseChannelThreadId(
  threadId: string
): { channel: string; sender: string; replyTarget: string } | null {
  if (typeof threadId !== 'string' || !threadId.startsWith(CHANNEL_THREAD_PREFIX)) {
    return null;
  }
  let body = threadId.slice(CHANNEL_THREAD_PREFIX.length);
  // Strip the optional `_thread:<ts>` suffix used by Slack/Discord — it
  // is never part of the recipient triple.
  const threadSuffixIdx = body.indexOf('_thread:');
  if (threadSuffixIdx >= 0) body = body.slice(0, threadSuffixIdx);
  // Body is `<channel>_<sender>_<replyTarget>`. Sender and replyTarget
  // can themselves contain underscores in some providers (Telegram chat
  // ids in particular), so we split off the channel from the head and
  // the reply target from the tail, leaving sender as the middle slice.
  const firstUnderscore = body.indexOf('_');
  if (firstUnderscore < 0) return null;
  const channel = body.slice(0, firstUnderscore);
  const lastUnderscore = body.lastIndexOf('_');
  if (lastUnderscore <= firstUnderscore) return null;
  const sender = body.slice(firstUnderscore + 1, lastUnderscore);
  const replyTarget = body.slice(lastUnderscore + 1);
  if (!channel || !sender || !replyTarget) return null;
  return { channel, sender, replyTarget };
}

/**
 * Derive the unique list of mention targets from the current Redux
 * thread list, filtered to channel mirror threads. Recipient identity
 * is keyed on `(channel, sender)` — group chats appear as a single row
 * per sender (using their staff id) so the OpenHuman user is always
 * picking *who* to message, not *which conversation thread*.
 *
 * Targets are sorted by most-recent activity so the people the user
 * has heard from lately surface to the top of the picker.
 *
 * When `channelFilter` is provided, only recipients on that channel are
 * returned. Otherwise every channel surface is included.
 */
export function deriveMentionTargets(
  threads: Thread[],
  channelFilter?: string
): MentionTarget[] {
  const byKey = new Map<string, Extract<MentionTarget, { kind: 'channel' }>>();
  if (!Array.isArray(threads)) {
    return [{ kind: 'agent', label: 'Agent' }];
  }
  for (const thread of threads) {
    if (!thread || typeof thread !== 'object') continue;
    const parsed = parseChannelThreadId(thread.id);
    if (!parsed) continue;
    if (channelFilter && parsed.channel !== channelFilter) continue;
    const key = `${parsed.channel}:${parsed.sender}`;
    const existing = byKey.get(key);
    const lastMessageAt =
      typeof thread.lastMessageAt === 'string' ? thread.lastMessageAt : '';
    const candidate: Extract<MentionTarget, { kind: 'channel' }> = {
      kind: 'channel',
      channel: parsed.channel,
      recipientId: parsed.sender,
      label: parsed.sender,
      threadId: thread.id,
      lastMessageAt,
    };
    if (!existing || existing.lastMessageAt < lastMessageAt) {
      byKey.set(key, candidate);
    }
  }
  const channelTargets = Array.from(byKey.values()).sort((a, b) =>
    b.lastMessageAt.localeCompare(a.lastMessageAt)
  );
  return [{ kind: 'agent', label: 'Agent' }, ...channelTargets];
}

/**
 * Inspect the input value + caret position and decide whether the
 * picker should be open. Mention tokens are:
 *   - opened by an `@` that is at the start of the input or preceded
 *     by whitespace; and
 *   - kept open while the caret is on a contiguous run of non-whitespace
 *     characters following that `@`.
 *
 * Returns `active: false` when the rules above aren't satisfied so the
 * caller can close the popover.
 */
export function detectActiveMention(value: string, caret: number): MentionDetection {
  if (caret < 0 || caret > value.length) {
    return { active: false, queryStart: -1, query: '' };
  }
  // Walk backwards from the caret to find an `@` with no intervening
  // whitespace. Any whitespace before the `@` is fine — but whitespace
  // *between* the `@` and the caret terminates the mention.
  let i = caret - 1;
  while (i >= 0) {
    const ch = value[i];
    if (ch === '@') {
      // The `@` must be at the start of input or preceded by whitespace.
      const before = i > 0 ? value[i - 1] : '';
      if (i === 0 || /\s/.test(before)) {
        return {
          active: true,
          queryStart: i,
          query: value.slice(i + 1, caret),
        };
      }
      return { active: false, queryStart: -1, query: '' };
    }
    if (/\s/.test(ch)) {
      return { active: false, queryStart: -1, query: '' };
    }
    i -= 1;
  }
  return { active: false, queryStart: -1, query: '' };
}

/**
 * Filter the candidate target list against the current query, comparing
 * lowercase substrings against both `label` and (for channel targets)
 * the channel name. Returns the matching subset preserving the original
 * order from {@link deriveMentionTargets}.
 */
export function filterMentionTargets(
  targets: MentionTarget[],
  query: string
): MentionTarget[] {
  const q = query.trim().toLowerCase();
  if (!q) return targets;
  return targets.filter(target => {
    if (target.kind === 'agent') return 'agent'.includes(q);
    return (
      target.label.toLowerCase().includes(q) ||
      target.channel.toLowerCase().includes(q) ||
      target.recipientId.toLowerCase().includes(q)
    );
  });
}

/**
 * Produce the text that should replace the active mention token in the
 * input box when the user picks `target`.
 *
 * - For the agent target we strip the `@…` token entirely; the user is
 *   confirming they want to talk to the local agent so no prefix is
 *   needed.
 * - For a channel target we emit `@<channel>:<recipientId> ` so the
 *   Rust core's mention parser sees the canonical form.
 */
export function applyMentionInsertion(
  value: string,
  detection: MentionDetection,
  target: MentionTarget,
  caret: number
): { value: string; caret: number } {
  if (!detection.active || detection.queryStart < 0) {
    return { value, caret };
  }
  const before = value.slice(0, detection.queryStart);
  const after = value.slice(caret);
  if (target.kind === 'agent') {
    const next = `${before}${after}`;
    return { value: next, caret: before.length };
  }
  const prefix = `@${target.channel}:${target.recipientId} `;
  const next = `${before}${prefix}${after}`;
  return { value: next, caret: before.length + prefix.length };
}

/** Convenience: human label for a channel id. */
export function channelDisplayName(channel: string): string {
  if (channel === 'dingtalk') return '钉钉';
  return channel.charAt(0).toUpperCase() + channel.slice(1);
}
