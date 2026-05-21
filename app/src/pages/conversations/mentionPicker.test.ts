import { describe, expect, it } from 'vitest';

import type { Thread } from '../../types/thread';
import {
  applyMentionInsertion,
  channelDisplayName,
  deriveMentionTargets,
  detectActiveMention,
  filterMentionTargets,
  type MentionTarget,
  parseChannelThreadId,
} from './mentionPicker';

function thread(id: string, lastMessageAt: string): Thread {
  return {
    id,
    title: 'fixture',
    chatId: null,
    isActive: true,
    messageCount: 1,
    lastMessageAt,
    createdAt: lastMessageAt,
    labels: [],
  };
}

describe('parseChannelThreadId', () => {
  it('parses dingtalk private thread ids', () => {
    expect(parseChannelThreadId('channel:dingtalk_staff42_staff42')).toEqual({
      channel: 'dingtalk',
      sender: 'staff42',
      replyTarget: 'staff42',
    });
  });

  it('strips _thread:<ts> suffix when present', () => {
    expect(parseChannelThreadId('channel:slack_alice_general_thread:T123')).toEqual({
      channel: 'slack',
      sender: 'alice',
      replyTarget: 'general',
    });
  });

  it('returns null for non-channel ids', () => {
    expect(parseChannelThreadId('proactive:morning')).toBeNull();
    expect(parseChannelThreadId('any-other-id')).toBeNull();
  });

  it('returns null when shape is incomplete', () => {
    expect(parseChannelThreadId('channel:dingtalk')).toBeNull();
    expect(parseChannelThreadId('channel:dingtalk_only')).toBeNull();
  });
});

describe('deriveMentionTargets', () => {
  it('always includes the agent target first', () => {
    const result = deriveMentionTargets([]);
    expect(result[0]).toEqual({ kind: 'agent', label: 'Agent' });
  });

  it('dedupes recipients across multiple threads with the same sender', () => {
    const threads = [
      thread('channel:dingtalk_staff42_staff42', '2026-05-20T10:00:00Z'),
      thread('channel:dingtalk_staff42_groupABC', '2026-05-21T11:00:00Z'),
      thread('channel:dingtalk_staffOther_staffOther', '2026-05-19T09:00:00Z'),
      thread('regular-thread-id', '2026-05-22T12:00:00Z'),
    ];
    const result = deriveMentionTargets(threads);
    expect(result).toHaveLength(3); // agent + 2 unique recipients
    expect(result[1]).toMatchObject({
      kind: 'channel',
      channel: 'dingtalk',
      recipientId: 'staff42',
    });
    expect(result[2]).toMatchObject({
      kind: 'channel',
      channel: 'dingtalk',
      recipientId: 'staffOther',
    });
  });

  it('respects the channelFilter param', () => {
    const threads = [
      thread('channel:dingtalk_a_a', '2026-05-21T10:00:00Z'),
      thread('channel:slack_b_general', '2026-05-21T11:00:00Z'),
    ];
    const result = deriveMentionTargets(threads, 'dingtalk');
    expect(result).toHaveLength(2);
    expect(result[1]).toMatchObject({ channel: 'dingtalk', recipientId: 'a' });
  });
});

describe('detectActiveMention', () => {
  it('detects an active mention at the start of input', () => {
    expect(detectActiveMention('@dingt', 6)).toEqual({
      active: true,
      queryStart: 0,
      query: 'dingt',
    });
  });

  it('detects after whitespace', () => {
    expect(detectActiveMention('hi @sta', 7)).toEqual({
      active: true,
      queryStart: 3,
      query: 'sta',
    });
  });

  it('returns inactive when @ is in the middle of a word', () => {
    expect(detectActiveMention('foo@bar', 7)).toEqual({ active: false, queryStart: -1, query: '' });
  });

  it('returns inactive when whitespace separates @ and caret', () => {
    expect(detectActiveMention('@user other', 11)).toEqual({
      active: false,
      queryStart: -1,
      query: '',
    });
  });

  it('handles empty query', () => {
    expect(detectActiveMention('@', 1)).toEqual({ active: true, queryStart: 0, query: '' });
  });
});

describe('filterMentionTargets', () => {
  const targets: MentionTarget[] = [
    { kind: 'agent', label: 'Agent' },
    {
      kind: 'channel',
      channel: 'dingtalk',
      recipientId: 'staff42',
      label: 'staff42',
      threadId: 'channel:dingtalk_staff42_staff42',
      lastMessageAt: '2026-05-21T10:00:00Z',
    },
    {
      kind: 'channel',
      channel: 'slack',
      recipientId: 'alice',
      label: 'alice',
      threadId: 'channel:slack_alice_general',
      lastMessageAt: '2026-05-20T10:00:00Z',
    },
  ];

  it('returns everything when the query is empty', () => {
    expect(filterMentionTargets(targets, '')).toEqual(targets);
  });

  it('matches by recipient id', () => {
    expect(filterMentionTargets(targets, 'sta')).toHaveLength(1);
  });

  it('matches by channel name', () => {
    expect(filterMentionTargets(targets, 'slack')).toHaveLength(1);
    expect(filterMentionTargets(targets, 'dingtalk')).toHaveLength(1);
  });

  it('matches the agent target case-insensitively', () => {
    expect(filterMentionTargets(targets, 'AGE')).toContainEqual({ kind: 'agent', label: 'Agent' });
  });
});

describe('applyMentionInsertion', () => {
  const detection = { active: true, queryStart: 0, query: 'sta' };

  it('strips the @-token when picking the agent target', () => {
    const result = applyMentionInsertion('@sta', detection, { kind: 'agent', label: 'Agent' }, 4);
    expect(result).toEqual({ value: '', caret: 0 });
  });

  it('inserts a channel prefix with trailing space', () => {
    const target: MentionTarget = {
      kind: 'channel',
      channel: 'dingtalk',
      recipientId: 'staff42',
      label: 'staff42',
      threadId: 'channel:dingtalk_staff42_staff42',
      lastMessageAt: '2026-05-21T10:00:00Z',
    };
    const result = applyMentionInsertion('@sta', detection, target, 4);
    expect(result.value).toBe('@dingtalk:staff42 ');
    expect(result.caret).toBe(result.value.length);
  });

  it('preserves text on either side of the @-token', () => {
    const target: MentionTarget = {
      kind: 'channel',
      channel: 'dingtalk',
      recipientId: 'staff42',
      label: 'staff42',
      threadId: 'channel:dingtalk_staff42_staff42',
      lastMessageAt: '2026-05-21T10:00:00Z',
    };
    const before = 'hi ';
    const after = ' bye';
    const value = `${before}@sta${after}`;
    const det = { active: true, queryStart: before.length, query: 'sta' };
    const caret = before.length + 4;
    const result = applyMentionInsertion(value, det, target, caret);
    expect(result.value).toBe('hi @dingtalk:staff42  bye');
    expect(result.caret).toBe('hi @dingtalk:staff42 '.length);
  });

  it('is a no-op when detection is inactive', () => {
    const result = applyMentionInsertion(
      'hi',
      { active: false, queryStart: -1, query: '' },
      { kind: 'agent', label: 'Agent' },
      2
    );
    expect(result).toEqual({ value: 'hi', caret: 2 });
  });
});

describe('channelDisplayName', () => {
  it('translates dingtalk', () => {
    expect(channelDisplayName('dingtalk')).toBe('钉钉');
  });

  it('capitalises everything else', () => {
    expect(channelDisplayName('slack')).toBe('Slack');
    expect(channelDisplayName('telegram')).toBe('Telegram');
  });
});
