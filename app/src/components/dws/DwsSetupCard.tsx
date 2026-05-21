import { useState } from 'react';

import { useDwsStatus } from '../../features/dws/useDwsStatus';
import {
  type DwsSyncCategories,
  type DwsSyncConfig,
  useDwsSyncConfig,
} from '../../features/dws/useDwsSyncConfig';

/** Sync content categories shown in the per-toggle grid. */
const SYNC_CATEGORIES: Array<{
  key: keyof DwsSyncCategories;
  label: string;
  emoji: string;
  hint: string;
}> = [
  { key: 'chat', label: '群聊', emoji: '💬', hint: '近期群消息 → 记忆树' },
  { key: 'doc', label: '文档', emoji: '📄', hint: '我编辑/访问过的文档 → 记忆树' },
  { key: 'calendar', label: '日历', emoji: '📅', hint: '近期与未来 7 天日程 → 记忆树' },
  { key: 'minutes', label: 'AI 听记', emoji: '🎙️', hint: '会议纪要：摘要 + 待办 → 记忆树' },
];

/** Suggested sync intervals (minutes). 5 is the backend-enforced floor. */
const INTERVAL_PRESETS = [5, 15, 30, 60, 120, 360];

export default function DwsSetupCard() {
  const {
    status,
    version,
    statusLabel,
    statusColor,
    refresh,
    install,
    update,
    login,
    logout,
    operating,
  } = useDwsStatus();

  const [opOutput, setOpOutput] = useState<{ kind: 'ok' | 'err'; text: string } | null>(null);

  const isAuthenticated = status === 'authenticated';
  const isNotInstalled = status === 'not_installed';
  const isNotAuth = status === 'not_authenticated';
  const isChecking = status === 'checking';

  const runOp = async (
    op: () => Promise<{ output: string; success: boolean }>,
    okFallback: string,
    errFallback: string
  ) => {
    setOpOutput(null);
    const result = await op();
    setOpOutput({
      kind: result.success ? 'ok' : 'err',
      text: result.output || (result.success ? okFallback : errFallback),
    });
  };

  return (
    <div className="space-y-3">
      {/* ── Header: branding + version + status pill + refresh ───────── */}
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-2xl flex-shrink-0">🔗</span>
          <div className="flex flex-wrap items-baseline gap-1.5 min-w-0">
            <span className="text-sm font-medium text-stone-900 dark:text-neutral-100 truncate">
              DingTalk Workspace CLI
            </span>
            {version && (
              <span className="rounded bg-stone-100 dark:bg-neutral-800 px-1.5 py-0.5 text-[10px] font-mono text-stone-500 dark:text-neutral-400">
                v{version}
              </span>
            )}
          </div>
        </div>
        <div className="flex items-center gap-1.5 flex-shrink-0">
          <span className={`text-xs font-medium ${statusColor}`}>{statusLabel}</span>
          {!isChecking && !operating && (
            <button
              type="button"
              onClick={() => void refresh()}
              className="rounded-lg border border-stone-200 dark:border-neutral-700 bg-white dark:bg-neutral-900 px-2 py-1 text-[10px] font-medium text-stone-600 dark:text-neutral-300 transition-colors hover:bg-stone-50 dark:hover:bg-neutral-800"
              title="重新检测 DWS 状态">
              ↻ 刷新
            </button>
          )}
        </div>
      </div>

      {/* ── State A: not installed ──────────────────────────────────── */}
      {isNotInstalled && !operating && (
        <div className="rounded-xl border border-coral-200 dark:border-coral-500/30 bg-coral-50/50 dark:bg-coral-500/5 p-3 space-y-2">
          <div>
            <p className="text-xs font-semibold text-coral-800 dark:text-coral-200">
              未检测到 DWS CLI
            </p>
            <p className="mt-1 text-[11px] leading-relaxed text-coral-700 dark:text-coral-300">
              DWS 是钉钉官方 CLI，提供 19 个产品、200+ 子命令。点击下方按钮一键安装到本机。
            </p>
          </div>
          <button
            type="button"
            onClick={() => void runOp(install, '安装完成。', '安装失败，请检查网络连接后重试。')}
            className="w-full rounded-lg bg-primary-500 px-3 py-2 text-xs font-semibold text-white shadow-soft transition-colors hover:bg-primary-600 focus:outline-none focus:ring-2 focus:ring-primary-500 focus:ring-offset-1">
            一键安装 DWS CLI
          </button>
          <p className="text-center text-[10px] text-coral-500 dark:text-coral-400">
            源码：
            <a
              href="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli"
              target="_blank"
              rel="noopener noreferrer"
              className="underline hover:text-coral-700 dark:hover:text-coral-200">
              GitHub
            </a>
          </p>
        </div>
      )}

      {/* ── State B: installed but not logged in ─────────────────────── */}
      {isNotAuth && !operating && (
        <div className="rounded-xl border border-amber-200 dark:border-amber-500/30 bg-amber-50/50 dark:bg-amber-500/5 p-3 space-y-2">
          <div>
            <p className="text-xs font-semibold text-amber-800 dark:text-amber-200">
              请登录钉钉账号
            </p>
            <p className="mt-1 text-[11px] leading-relaxed text-amber-700 dark:text-amber-300">
              点击「登录」会弹出新终端窗口运行 <code className="font-mono">dws auth login</code>
              ，请在终端里完成钉钉扫码 / 确认，然后回到这里点「刷新」。
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => void runOp(login, '请在新终端窗口完成登录。', '无法启动终端窗口。')}
              className="rounded-lg bg-primary-500 px-3 py-1.5 text-[11px] font-semibold text-white shadow-soft transition-colors hover:bg-primary-600 focus:outline-none focus:ring-2 focus:ring-primary-500 focus:ring-offset-1">
              📱 登录钉钉
            </button>
            <button
              type="button"
              onClick={() => void runOp(update, '已是最新版本。', '更新失败。')}
              className="rounded-lg border border-stone-200 dark:border-neutral-700 bg-white dark:bg-neutral-900 px-3 py-1.5 text-[11px] font-medium text-stone-700 dark:text-neutral-200 transition-colors hover:bg-stone-50 dark:hover:bg-neutral-800">
              ⬆ 更新 DWS
            </button>
          </div>
        </div>
      )}

      {/* ── State C: authenticated → show maintenance + sync UI ───────── */}
      {isAuthenticated && !operating && (
        <div className="space-y-3">
          {/* Maintenance buttons */}
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => void runOp(update, '已是最新版本。', '更新失败。')}
              className="rounded-lg border border-stone-200 dark:border-neutral-700 bg-white dark:bg-neutral-900 px-3 py-1.5 text-[11px] font-medium text-stone-700 dark:text-neutral-200 transition-colors hover:bg-stone-50 dark:hover:bg-neutral-800">
              ⬆ 更新 DWS
            </button>
            <button
              type="button"
              onClick={() => void runOp(logout, '已退出登录。', '退出登录失败。')}
              className="rounded-lg border border-stone-200 dark:border-neutral-700 bg-white dark:bg-neutral-900 px-3 py-1.5 text-[11px] font-medium text-stone-700 dark:text-neutral-200 transition-colors hover:bg-stone-50 dark:hover:bg-neutral-800">
              🚪 退出登录
            </button>
          </div>

          {/* Sync panel */}
          <DwsSyncPanel />
        </div>
      )}

      {/* ── Spinner ─────────────────────────────────────────────────── */}
      {(isChecking || operating) && (
        <div className="flex items-center justify-center py-4">
          <div className="h-4 w-4 animate-spin rounded-full border-2 border-stone-300 border-t-stone-600 dark:border-neutral-600 dark:border-t-neutral-300" />
          <span className="ml-2 text-xs text-stone-400 dark:text-neutral-500">
            {operating ? '正在执行操作…' : '正在检测 DWS 状态…'}
          </span>
        </div>
      )}

      {/* ── Last operation feedback ─────────────────────────────────── */}
      {opOutput && (
        <OperationLog kind={opOutput.kind} text={opOutput.text} onClose={() => setOpOutput(null)} />
      )}
    </div>
  );
}

interface OperationLogProps {
  kind: 'ok' | 'err';
  text: string;
  onClose: () => void;
}

function OperationLog({ kind, text, onClose }: OperationLogProps) {
  const tone =
    kind === 'ok'
      ? {
          border: 'border-sage-200 dark:border-sage-500/30',
          bg: 'bg-sage-50/50 dark:bg-sage-500/5',
          title: 'text-sage-800 dark:text-sage-200',
          codeBg: 'bg-sage-100 dark:bg-sage-900/30',
          codeText: 'text-sage-900 dark:text-sage-100',
          closeText:
            'text-sage-500 hover:text-sage-700 dark:text-sage-400 dark:hover:text-sage-200',
          icon: '✓',
          label: '操作完成',
        }
      : {
          border: 'border-coral-200 dark:border-coral-500/30',
          bg: 'bg-coral-50/50 dark:bg-coral-500/5',
          title: 'text-coral-800 dark:text-coral-200',
          codeBg: 'bg-coral-100 dark:bg-coral-900/30',
          codeText: 'text-coral-900 dark:text-coral-100',
          closeText:
            'text-coral-500 hover:text-coral-700 dark:text-coral-400 dark:hover:text-coral-200',
          icon: '✗',
          label: '操作失败',
        };
  return (
    <div className={`rounded-xl border ${tone.border} ${tone.bg} p-3`}>
      <div className="flex items-center justify-between">
        <p className={`text-xs font-semibold ${tone.title}`}>
          {tone.icon} {tone.label}
        </p>
        <button type="button" onClick={onClose} className={`text-[10px] ${tone.closeText}`}>
          关闭
        </button>
      </div>
      <pre
        className={`mt-1.5 max-h-32 overflow-auto rounded-lg ${tone.codeBg} px-2.5 py-1.5 text-[10px] font-mono leading-relaxed ${tone.codeText} whitespace-pre-wrap`}>
        {text}
      </pre>
    </div>
  );
}

// ── Sync panel ─────────────────────────────────────────────────────────────

function DwsSyncPanel() {
  const { config, loading, syncing, toggleCategory, syncNow, updateConfig, error } =
    useDwsSyncConfig();
  const [syncResult, setSyncResult] = useState<string | null>(null);

  if (!config && loading) {
    return (
      <div className="flex items-center justify-center py-2">
        <div className="h-3 w-3 animate-spin rounded-full border-2 border-stone-300 border-t-stone-600 dark:border-neutral-600 dark:border-t-neutral-300" />
        <span className="ml-1.5 text-[10px] text-stone-400 dark:text-neutral-500">
          加载同步配置…
        </span>
      </div>
    );
  }
  if (!config) return null;

  const handleSyncNow = async () => {
    setSyncResult(null);
    const result = await syncNow();
    if (!result) return;
    if (!result.synced) {
      setSyncResult(result.message ?? '没有启用任何同步类别');
      return;
    }
    if (result.result) {
      const ok = result.result.results.filter(r => r.success).length;
      const total = result.result.results.length;
      const failed = result.result.results
        .filter(r => !r.success)
        .map(r => `${r.category}: ${r.error ?? 'unknown error'}`)
        .join('\n');
      setSyncResult(
        failed
          ? `同步完成：${ok}/${total} 成功\n\n失败明细：\n${failed}`
          : `同步完成：${ok}/${total} 个类别成功`
      );
    }
  };

  return (
    <div className="rounded-xl border border-stone-200 dark:border-neutral-700 bg-stone-50/50 dark:bg-neutral-800/30 p-3 space-y-3">
      {/* Header: enabled toggle + sync-now */}
      <div className="flex items-center justify-between flex-wrap gap-2">
        <div className="flex items-center gap-2">
          <span className="text-sm">🔄</span>
          <span className="text-[11px] font-semibold text-stone-800 dark:text-neutral-100">
            定时同步
          </span>
          <Toggle
            checked={config.enabled}
            disabled={loading}
            onChange={() => void updateConfig({ enabled: !config.enabled })}
            title={config.enabled ? '已启用定时同步' : '已禁用定时同步'}
          />
        </div>
        <button
          type="button"
          onClick={() => void handleSyncNow()}
          disabled={syncing || loading}
          className="rounded-lg bg-primary-500 px-2.5 py-1 text-[10px] font-semibold text-white shadow-soft transition-colors hover:bg-primary-600 disabled:opacity-50 disabled:cursor-not-allowed focus:outline-none focus:ring-2 focus:ring-primary-500 focus:ring-offset-1">
          {syncing ? '同步中…' : '⚡ 立即拉取'}
        </button>
      </div>

      {/* Interval picker */}
      <IntervalPicker
        value={config.interval_minutes}
        disabled={loading || !config.enabled}
        onChange={value => void updateConfig({ interval_minutes: value })}
      />

      {/* Category switchers + last-sync labels */}
      <div className="space-y-1.5">
        <p className="text-[10px] font-medium text-stone-500 dark:text-neutral-400">
          选择要同步的内容（首次拉取今日数据，后续仅拉取增量）
        </p>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-1.5">
          {SYNC_CATEGORIES.map(({ key, label, emoji, hint }) => {
            const enabled = config.categories[key];
            const lastSync = config.last_synced_at[key];
            return (
              <button
                key={key}
                type="button"
                onClick={() => void toggleCategory(key)}
                disabled={loading}
                className={`flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-[11px] text-left transition-colors ${
                  enabled
                    ? 'bg-primary-50 dark:bg-primary-500/10 text-primary-700 dark:text-primary-300 border border-primary-200 dark:border-primary-500/30'
                    : 'bg-stone-100 dark:bg-neutral-800 text-stone-500 dark:text-neutral-400 border border-transparent'
                }`}
                title={hint}>
                <span className="text-base flex-shrink-0">{emoji}</span>
                <span className="flex-1 min-w-0">
                  <span className="block font-medium truncate">{label}</span>
                  <span
                    className={`block text-[9px] truncate ${
                      enabled
                        ? 'text-primary-600/80 dark:text-primary-300/70'
                        : 'text-stone-400 dark:text-neutral-500'
                    }`}>
                    {lastSync ? `上次：${formatRelative(lastSync)}` : '尚未同步'}
                  </span>
                </span>
                <Toggle
                  checked={enabled}
                  disabled={loading}
                  // The whole button toggles; this is purely a visual indicator,
                  // pointer events are pass-through via disabled-on-click.
                  onChange={() => void toggleCategory(key)}
                  small
                  asIndicator
                />
              </button>
            );
          })}
        </div>
      </div>

      {syncResult && (
        <pre className="rounded-lg bg-sage-50 dark:bg-sage-900/20 px-2.5 py-1.5 text-[10px] text-sage-700 dark:text-sage-300 whitespace-pre-wrap">
          {syncResult}
        </pre>
      )}

      {error && (
        <div className="rounded-lg bg-coral-50 dark:bg-coral-900/20 px-2.5 py-1.5 text-[10px] text-coral-700 dark:text-coral-300">
          {error}
        </div>
      )}

      <FooterCaption config={config} />
    </div>
  );
}

interface IntervalPickerProps {
  value: number;
  disabled?: boolean;
  onChange: (next: number) => void;
}

function IntervalPicker({ value, disabled, onChange }: IntervalPickerProps) {
  return (
    <div className="flex items-center gap-2 flex-wrap">
      <span className="text-[10px] font-medium text-stone-500 dark:text-neutral-400">同步间隔</span>
      <div className="flex items-center gap-1 flex-wrap">
        {INTERVAL_PRESETS.map(min => {
          const active = value === min;
          return (
            <button
              key={min}
              type="button"
              disabled={disabled}
              onClick={() => onChange(min)}
              className={`rounded-md px-2 py-0.5 text-[10px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${
                active
                  ? 'bg-primary-500 text-white'
                  : 'bg-white dark:bg-neutral-900 border border-stone-200 dark:border-neutral-700 text-stone-600 dark:text-neutral-300 hover:bg-stone-50 dark:hover:bg-neutral-800'
              }`}>
              {labelForMinutes(min)}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function FooterCaption({ config }: { config: DwsSyncConfig }) {
  if (!config.enabled) {
    return (
      <p className="text-[10px] text-stone-400 dark:text-neutral-500">
        定时同步已关闭。开启后每隔 {labelForMinutes(config.interval_minutes)}{' '}
        自动拉取一次启用的内容。
      </p>
    );
  }
  const enabledCount = Object.values(config.categories).filter(Boolean).length;
  if (enabledCount === 0) {
    return (
      <p className="text-[10px] text-amber-600 dark:text-amber-400">
        ⚠ 已开启定时同步，但未选择任何内容类别。
      </p>
    );
  }
  return (
    <p className="text-[10px] text-stone-400 dark:text-neutral-500">
      每隔 {labelForMinutes(config.interval_minutes)} 拉取一次，共 {enabledCount} 个内容类别。
    </p>
  );
}

interface ToggleProps {
  checked: boolean;
  disabled?: boolean;
  onChange: () => void;
  title?: string;
  small?: boolean;
  /** Render as a non-interactive visual cue — useful when an outer button
   * already owns the click handler. */
  asIndicator?: boolean;
}

function Toggle({ checked, disabled, onChange, title, small, asIndicator }: ToggleProps) {
  const sizeOuter = small ? 'h-3.5 w-6' : 'h-4 w-7';
  const sizeInner = small ? 'h-2.5 w-2.5' : 'h-3 w-3';
  const translate = checked ? (small ? 'translate-x-2.5' : 'translate-x-3.5') : 'translate-x-0.5';
  const baseClass = `relative inline-flex ${sizeOuter} shrink-0 items-center rounded-full transition-colors ${
    asIndicator ? '' : 'cursor-pointer focus:outline-none focus:ring-2 focus:ring-primary-500/40'
  } ${checked ? 'bg-primary-500' : 'bg-stone-300 dark:bg-neutral-600'}`;
  const knob = (
    <span
      className={`inline-block ${sizeInner} transform rounded-full bg-white shadow transition-transform ${translate}`}
    />
  );
  if (asIndicator) {
    return (
      <span title={title} className={baseClass}>
        {knob}
      </span>
    );
  }
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={e => {
        e.stopPropagation();
        onChange();
      }}
      className={baseClass}>
      {knob}
    </button>
  );
}

function labelForMinutes(min: number): string {
  if (min < 60) return `${min} 分钟`;
  if (min % 60 === 0) return `${min / 60} 小时`;
  return `${(min / 60).toFixed(1)} 小时`;
}

function formatRelative(unixSeconds: number): string {
  const now = Date.now() / 1000;
  const diff = Math.max(0, now - unixSeconds);
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 7) return `${Math.floor(diff / 86400)} 天前`;
  const date = new Date(unixSeconds * 1000);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}
