import { useEffect, useRef } from 'react';

import {
  channelDisplayName,
  type MentionTarget,
} from '../mentionPicker';

interface MentionPickerProps {
  targets: MentionTarget[];
  activeIndex: number;
  onHoverIndex: (index: number) => void;
  onSelect: (target: MentionTarget) => void;
  emptyHint?: string;
}

/**
 * Floating panel rendered above the chat composer when the user is
 * mid-`@` token. Lists the agent first and then every known channel
 * recipient (DingTalk staff ids etc.). Keyboard navigation lives in
 * the composer's `onKeyDown`; this component only renders + emits
 * `onSelect` on click.
 */
export function MentionPicker({
  targets,
  activeIndex,
  onHoverIndex,
  onSelect,
  emptyHint,
}: MentionPickerProps) {
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the highlighted row scrolled into view as the user types or
  // arrows through the list.
  useEffect(() => {
    const root = listRef.current;
    if (!root) return;
    const row = root.querySelector<HTMLButtonElement>(
      `[data-mention-row="${activeIndex}"]`
    );
    if (row) {
      row.scrollIntoView({ block: 'nearest' });
    }
  }, [activeIndex]);

  return (
    <div
      ref={listRef}
      role="listbox"
      aria-label="@-mention targets"
      data-testid="mention-picker"
      className="absolute bottom-full left-0 mb-2 z-30 max-h-60 w-72 overflow-y-auto rounded-xl border border-stone-200 bg-white shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
      {targets.length === 0 ? (
        <p
          className="px-3 py-2 text-xs text-stone-500 dark:text-neutral-400"
          data-testid="mention-picker-empty">
          {emptyHint ?? 'No matching targets.'}
        </p>
      ) : (
        targets.map((target, index) => {
          const active = index === activeIndex;
          if (target.kind === 'agent') {
            return (
              <button
                key="mention-agent"
                type="button"
                role="option"
                aria-selected={active}
                data-mention-row={index}
                data-testid="mention-row-agent"
                onMouseDown={event => {
                  event.preventDefault();
                  onSelect(target);
                }}
                onMouseEnter={() => onHoverIndex(index)}
                className={`flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors ${
                  active
                    ? 'bg-primary-50 text-primary-700 dark:bg-primary-900/30 dark:text-primary-200'
                    : 'text-stone-700 hover:bg-stone-50 dark:text-neutral-200 dark:hover:bg-neutral-800/60'
                }`}>
                <span className="flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-full bg-primary-500 text-[10px] font-semibold uppercase text-white">
                  AI
                </span>
                <span className="flex flex-col">
                  <span className="font-medium">{target.label}</span>
                  <span className="text-[11px] text-stone-500 dark:text-neutral-400">
                    Default — runs the local agent loop.
                  </span>
                </span>
              </button>
            );
          }
          const channelLabel = channelDisplayName(target.channel);
          return (
            <button
              key={`${target.channel}:${target.recipientId}`}
              type="button"
              role="option"
              aria-selected={active}
              data-mention-row={index}
              data-testid={`mention-row-${target.channel}-${target.recipientId}`}
              onMouseDown={event => {
                event.preventDefault();
                onSelect(target);
              }}
              onMouseEnter={() => onHoverIndex(index)}
              className={`flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors ${
                active
                  ? 'bg-primary-50 text-primary-700 dark:bg-primary-900/30 dark:text-primary-200'
                  : 'text-stone-700 hover:bg-stone-50 dark:text-neutral-200 dark:hover:bg-neutral-800/60'
              }`}>
              <span className="flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-full bg-amber-500/90 text-[10px] font-semibold uppercase text-white">
                {channelLabel.slice(0, 2)}
              </span>
              <span className="flex min-w-0 flex-col">
                <span className="truncate font-medium">{target.label}</span>
                <span className="truncate text-[11px] text-stone-500 dark:text-neutral-400">
                  {channelLabel} · @{target.channel}:{target.recipientId}
                </span>
              </span>
            </button>
          );
        })
      )}
    </div>
  );
}

export default MentionPicker;
