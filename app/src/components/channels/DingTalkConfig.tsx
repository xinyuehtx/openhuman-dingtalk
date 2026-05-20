import debug from 'debug';
import { useCallback, useState } from 'react';

import { AUTH_MODE_LABELS } from '../../lib/channels/definitions';
import { useT } from '../../lib/i18n/I18nContext';
import { channelConnectionsApi } from '../../services/api/channelConnectionsApi';
import {
  disconnectChannelConnection,
  setChannelConnectionStatus,
  upsertChannelConnection,
} from '../../store/channelConnectionsSlice';
import { useAppDispatch, useAppSelector } from '../../store/hooks';
import type {
  AuthModeSpec,
  ChannelAuthMode,
  ChannelConnectionStatus,
  ChannelDefinition,
} from '../../types/channels';
import { restartCoreProcess } from '../../utils/tauriCommands/core';
import ChannelFieldInput from './ChannelFieldInput';
import ChannelStatusBadge from './ChannelStatusBadge';

const log = debug('channels:dingtalk');

interface DingTalkConfigProps {
  definition: ChannelDefinition;
}

/** Collapsible setup guide for DingTalk Stream Mode integration. */
function SetupGuide() {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="rounded-lg border border-stone-200 dark:border-neutral-800 bg-stone-50 dark:bg-neutral-800/60 p-3">
      <button
        type="button"
        onClick={() => setExpanded(prev => !prev)}
        className="w-full flex items-center justify-between text-left">
        <span className="text-sm font-medium text-stone-900 dark:text-neutral-100">
          📖 接入引导 / Setup Guide
        </span>
        <svg
          className={`w-4 h-4 text-stone-400 transition-transform ${expanded ? 'rotate-180' : ''}`}
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {expanded && (
        <div className="mt-3 space-y-3 text-xs text-stone-600 dark:text-neutral-300">
          <p className="font-medium text-stone-700 dark:text-neutral-200">
            使用钉钉 Stream 模式连接（无需公网 IP）：
          </p>

          <ol className="list-decimal list-inside space-y-2 pl-1">
            <li>
              访问{' '}
              <a
                href="https://open.dingtalk.com/"
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary-600 dark:text-primary-400 underline">
                钉钉开放平台
              </a>{' '}
              → 控制台 → 应用开发 → 创建应用
            </li>
            <li>
              在应用「基础信息」页面获取{' '}
              <span className="font-mono bg-stone-200 dark:bg-neutral-700 px-1 rounded">
                AppKey
              </span>{' '}
              和{' '}
              <span className="font-mono bg-stone-200 dark:bg-neutral-700 px-1 rounded">
                AppSecret
              </span>
            </li>
            <li>在「机器人配置」中启用机器人能力</li>
            <li>
              在「事件订阅」中选择{' '}
              <span className="font-semibold">Stream 模式</span>（无需回调地址）
            </li>
            <li>发布应用 → 在钉钉中添加机器人到群聊或私聊</li>
          </ol>

          <div className="mt-2 p-2 rounded bg-amber-50 dark:bg-amber-500/10 border border-amber-200 dark:border-amber-500/20">
            <p className="text-amber-700 dark:text-amber-300">
              💡 <span className="font-medium">提示：</span>
              Stream 模式使用 WebSocket 长连接，无需公网 IP、域名或反向代理。
            </p>
          </div>

          <div className="mt-1">
            <a
              href="https://open.dingtalk.com/document/orgapp/overview-of-development-process"
              target="_blank"
              rel="noopener noreferrer"
              className="text-primary-600 dark:text-primary-400 underline text-xs">
              查看钉钉开放平台完整文档 →
            </a>
          </div>
        </div>
      )}
    </div>
  );
}

const DingTalkConfig = ({ definition }: DingTalkConfigProps) => {
  const { t } = useT();
  const dispatch = useAppDispatch();
  const channelConnections = useAppSelector(state => state.channelConnections);

  const [busyKeys, setBusyKeys] = useState<Record<string, boolean>>({});
  const [fieldValues, setFieldValues] = useState<Record<string, Record<string, string>>>({});
  const [error, setError] = useState<string | null>(null);

  const runBusy = useCallback(async (key: string, task: () => Promise<void>) => {
    setBusyKeys(prev => ({ ...prev, [key]: true }));
    setError(null);
    try {
      await task();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
    } finally {
      setBusyKeys(prev => ({ ...prev, [key]: false }));
    }
  }, []);

  const updateField = useCallback((compositeKey: string, fieldKey: string, value: string) => {
    setFieldValues(prev => ({
      ...prev,
      [compositeKey]: { ...(prev[compositeKey] ?? {}), [fieldKey]: value },
    }));
  }, []);

  const handleConnect = useCallback(
    (spec: AuthModeSpec) => {
      const key = `dingtalk:${spec.mode}`;
      void runBusy(key, async () => {
        dispatch(
          setChannelConnectionStatus({
            channel: 'dingtalk',
            authMode: spec.mode,
            status: 'connecting',
          })
        );
        log('connecting dingtalk via %s', spec.mode);

        // Build credentials from field values.
        const credentials: Record<string, string> = {};
        for (const field of spec.fields) {
          const val = fieldValues[key]?.[field.key]?.trim() ?? '';
          if (field.required && !val) {
            dispatch(
              setChannelConnectionStatus({
                channel: 'dingtalk',
                authMode: spec.mode,
                status: 'error',
                lastError: `${field.label} is required`,
              })
            );
            return;
          }
          if (val) credentials[field.key] = val;
        }

        const result = await channelConnectionsApi.connectChannel('dingtalk', {
          authMode: spec.mode,
          credentials: Object.keys(credentials).length > 0 ? credentials : undefined,
        });
        log('connect result: %o', result);

        // Credential-based connection succeeded.
        if (result.restart_required) {
          log('restart required after connect — restarting core process');
          try {
            await restartCoreProcess();
            log('core process restarted successfully');
            dispatch(
              upsertChannelConnection({
                channel: 'dingtalk',
                authMode: spec.mode,
                patch: {
                  status: 'connected',
                  lastError: undefined,
                  capabilities: ['read', 'write'],
                },
              })
            );
          } catch (restartErr) {
            const msg = restartErr instanceof Error ? restartErr.message : String(restartErr);
            log('core restart failed: %s', msg);
            setError(t('channels.dingtalk.savedRestartRequired'));
          }
        } else {
          dispatch(
            upsertChannelConnection({
              channel: 'dingtalk',
              authMode: spec.mode,
              patch: { status: 'connected', lastError: undefined, capabilities: ['read', 'write'] },
            })
          );
        }
      });
    },
    [dispatch, fieldValues, runBusy, t]
  );

  const handleDisconnect = useCallback(
    (authMode: ChannelAuthMode) => {
      const key = `dingtalk:${authMode}`;
      void runBusy(key, async () => {
        log('disconnecting dingtalk via %s', authMode);
        await channelConnectionsApi.disconnectChannel('dingtalk', authMode);
        dispatch(disconnectChannelConnection({ channel: 'dingtalk', authMode }));
      });
    },
    [dispatch, runBusy]
  );

  return (
    <div className="space-y-3">
      {/* Setup guide */}
      <SetupGuide />

      {/* Error banner */}
      {error && (
        <div className="rounded-lg border border-coral-200 dark:border-coral-500/30 bg-coral-50 dark:bg-coral-500/10 px-4 py-3 text-sm text-coral-700 dark:text-coral-300">
          {error}
        </div>
      )}

      {/* Auth mode sections */}
      {definition.auth_modes.map(spec => {
        const compositeKey = `dingtalk:${spec.mode}`;
        const connection = channelConnections.connections.dingtalk?.[spec.mode];
        const status: ChannelConnectionStatus = connection?.status ?? 'disconnected';

        return (
          <div
            key={spec.mode}
            className="rounded-lg border border-stone-200 dark:border-neutral-800 bg-stone-50 dark:bg-neutral-800/60 p-3">
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="text-sm font-medium text-stone-900 dark:text-neutral-100">
                  {AUTH_MODE_LABELS[spec.mode] ?? spec.mode}
                </p>
                <p className="text-xs text-stone-500 dark:text-neutral-400 mt-1">
                  {spec.description}
                </p>
                {connection?.lastError && (
                  <p className="text-xs text-coral-600 mt-1">{connection.lastError}</p>
                )}
              </div>
              <ChannelStatusBadge status={status} />
            </div>

            {spec.fields.length > 0 && (
              <div className="mt-3 space-y-2">
                {spec.fields.map(field => (
                  <ChannelFieldInput
                    key={field.key}
                    field={field}
                    value={fieldValues[compositeKey]?.[field.key] ?? ''}
                    onChange={val => updateField(compositeKey, field.key, val)}
                    disabled={busyKeys[compositeKey]}
                  />
                ))}
              </div>
            )}

            <div className="mt-3 flex gap-2">
              <button
                type="button"
                disabled={busyKeys[compositeKey]}
                onClick={() => handleConnect(spec)}
                className="rounded-lg bg-primary-500 px-3 py-1.5 text-xs font-medium text-white hover:bg-primary-600 disabled:opacity-50">
                {status === 'connected'
                  ? t('channels.dingtalk.reconnect')
                  : t('channels.dingtalk.connect')}
              </button>
              <button
                type="button"
                disabled={busyKeys[compositeKey] || status === 'disconnected'}
                onClick={() => handleDisconnect(spec.mode)}
                className="rounded-lg border border-stone-200 dark:border-neutral-800 px-3 py-1.5 text-xs font-medium text-stone-600 dark:text-neutral-300 hover:border-stone-300 dark:hover:border-neutral-700 disabled:opacity-50">
                {t('accounts.disconnect')}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
};

export default DingTalkConfig;
