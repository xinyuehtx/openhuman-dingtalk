import { useCallback, useEffect, useState } from 'react';

import { callCoreRpc } from '../../services/coreRpcClient';

export type DwsInstallStatus = 'not_installed' | 'not_authenticated' | 'authenticated' | 'checking';

export interface DwsOperationResult {
  /** Combined stdout/stderr from the underlying shell command. */
  output: string;
  /** True when the shell command completed with exit-code 0. */
  success: boolean;
}

export interface DwsStatusResult {
  status: DwsInstallStatus;
  /** dws CLI version string if installed (e.g. "1.0.15"). */
  version: string | null;
  /** Resolved absolute path to the dws binary on disk (when installed). */
  dwsPath: string | null;
  statusLabel: string;
  /** Tailwind color class for the status text. */
  statusColor: string;
  /** Re-check the status. */
  refresh: () => Promise<void>;
  install: () => Promise<DwsOperationResult>;
  /** Same script as `install` — re-running it upgrades in place. */
  update: () => Promise<DwsOperationResult>;
  /** Open a fresh terminal window with `dws auth login`. */
  login: () => Promise<DwsOperationResult>;
  /** Run `dws auth logout` in the background. */
  logout: () => Promise<DwsOperationResult>;
  /** True while install / update / login / logout is running. */
  operating: boolean;
}

interface DwsRuntimeStatusPayload {
  status?: 'not_installed' | 'not_authenticated' | 'authenticated';
  dws_path?: string | null;
  version?: string | null;
  auth_output?: string | null;
}

interface DwsCommandPayload {
  success?: boolean;
  exit_code?: number;
  output?: string;
}

/**
 * Unwrap a `RpcOutcome<T>` envelope. The Rust core's `into_cli_compatible_json`
 * (see `src/rpc/mod.rs`) returns `{ result: T, logs: string[] }` when the
 * controller emitted any log lines (which our handlers always do), and the
 * raw `T` otherwise. Either shape is accepted here.
 */
function unwrap<T>(value: unknown): T | undefined {
  if (value && typeof value === 'object') {
    const obj = value as Record<string, unknown>;
    if ('result' in obj && 'logs' in obj) {
      return obj.result as T;
    }
  }
  return value as T | undefined;
}

async function fetchRuntimeStatus(): Promise<DwsRuntimeStatusPayload> {
  const raw = await callCoreRpc<unknown>({
    method: 'openhuman.config_dws_runtime_status',
    params: {},
  });
  return unwrap<DwsRuntimeStatusPayload>(raw) ?? {};
}

async function callRuntimeCommand(method: string): Promise<DwsCommandPayload> {
  const raw = await callCoreRpc<unknown>({
    method,
    params: {},
  });
  return unwrap<DwsCommandPayload>(raw) ?? {};
}

function commandToOpResult(payload: DwsCommandPayload, fallback: string): DwsOperationResult {
  return {
    success: payload.success === true,
    output: payload.output?.trim() || fallback,
  };
}

export function useDwsStatus(): DwsStatusResult {
  const [status, setStatus] = useState<DwsInstallStatus>('checking');
  const [version, setVersion] = useState<string | null>(null);
  const [dwsPath, setDwsPath] = useState<string | null>(null);
  const [operating, setOperating] = useState(false);

  const checkStatus = useCallback(async () => {
    setStatus('checking');
    try {
      const payload = await fetchRuntimeStatus();
      const next = payload.status;
      if (next === 'authenticated' || next === 'not_authenticated' || next === 'not_installed') {
        setStatus(next);
      } else {
        setStatus('not_installed');
      }
      setVersion(payload.version ?? null);
      setDwsPath(payload.dws_path ?? null);
    } catch (err) {
      console.warn('[dws][status] runtime status RPC failed', err);
      setStatus('not_installed');
      setVersion(null);
      setDwsPath(null);
    }
  }, []);

  useEffect(() => {
    void checkStatus();
  }, [checkStatus]);

  const install = useCallback(async (): Promise<DwsOperationResult> => {
    setOperating(true);
    try {
      const payload = await callRuntimeCommand('openhuman.config_dws_runtime_install');
      const result = commandToOpResult(payload, payload.success ? '安装完成。' : '安装失败。');
      await checkStatus();
      return result;
    } catch (err) {
      console.warn('[dws][install] RPC failed', err);
      return { success: false, output: err instanceof Error ? err.message : 'install failed' };
    } finally {
      setOperating(false);
    }
  }, [checkStatus]);

  const update = install; // running the install script again upgrades in place

  const login = useCallback(async (): Promise<DwsOperationResult> => {
    setOperating(true);
    try {
      const payload = await callRuntimeCommand('openhuman.config_dws_runtime_open_login');
      return commandToOpResult(
        payload,
        payload.success
          ? '已在新终端窗口中打开 dws auth login，请在终端完成钉钉扫码登录后点击「刷新」。'
          : '无法启动终端窗口。'
      );
    } catch (err) {
      console.warn('[dws][login] RPC failed', err);
      return { success: false, output: err instanceof Error ? err.message : 'login failed' };
    } finally {
      setOperating(false);
    }
  }, []);

  const logout = useCallback(async (): Promise<DwsOperationResult> => {
    setOperating(true);
    try {
      const payload = await callRuntimeCommand('openhuman.config_dws_runtime_logout');
      const result = commandToOpResult(payload, payload.success ? '已退出登录。' : '退出登录失败。');
      await checkStatus();
      return result;
    } catch (err) {
      console.warn('[dws][logout] RPC failed', err);
      return { success: false, output: err instanceof Error ? err.message : 'logout failed' };
    } finally {
      setOperating(false);
    }
  }, [checkStatus]);

  const statusLabel = (() => {
    switch (status) {
      case 'checking':
        return '检测中…';
      case 'not_installed':
        return '未安装';
      case 'not_authenticated':
        return '未登录';
      case 'authenticated':
        return '已连接';
    }
  })();

  const statusColor = (() => {
    switch (status) {
      case 'checking':
        return 'text-stone-400 dark:text-neutral-500';
      case 'not_installed':
        return 'text-coral-600 dark:text-coral-300';
      case 'not_authenticated':
        return 'text-amber-600 dark:text-amber-300';
      case 'authenticated':
        return 'text-sage-600 dark:text-sage-300';
    }
  })();

  return {
    status,
    version,
    dwsPath,
    statusLabel,
    statusColor,
    refresh: checkStatus,
    install,
    update,
    login,
    logout,
    operating,
  };
}
