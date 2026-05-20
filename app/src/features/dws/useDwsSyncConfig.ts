import { useCallback, useEffect, useState } from 'react';

import { callCoreRpc } from '../../services/coreRpcClient';

/** Per-category sync toggle state. */
export interface DwsSyncCategories {
  calendar: boolean;
  todo: boolean;
  contact: boolean;
  attendance: boolean;
  approval: boolean;
  report: boolean;
  mail: boolean;
  doc: boolean;
  chat: boolean;
}

/** Last-sync unix timestamps keyed by category id (snake_case). */
export type DwsLastSyncedAt = Partial<Record<keyof DwsSyncCategories, number>>;

/** Full DWS sync configuration as returned by the core RPC. */
export interface DwsSyncConfig {
  enabled: boolean;
  interval_minutes: number;
  categories: DwsSyncCategories;
  last_synced_at: DwsLastSyncedAt;
}

/** Result of a sync-now operation. */
export interface SyncNowResult {
  synced: boolean;
  message?: string;
  result?: {
    results: Array<{
      category: string;
      success: boolean;
      records_count: number;
      last_synced_at: number | null;
      error?: string;
    }>;
    started_at: number;
    finished_at: number;
  };
  last_synced_at?: DwsLastSyncedAt;
}

export interface UseDwsSyncConfigResult {
  config: DwsSyncConfig | null;
  loading: boolean;
  syncing: boolean;
  refreshConfig: () => Promise<void>;
  /** Replace one or more top-level fields. */
  updateConfig: (patch: Partial<Omit<DwsSyncConfig, 'last_synced_at'>>) => Promise<void>;
  /** Toggle a single sync category on/off. */
  toggleCategory: (category: keyof DwsSyncCategories) => Promise<void>;
  /** Trigger an immediate sync for all enabled categories. */
  syncNow: () => Promise<SyncNowResult | null>;
  error: string | null;
}

interface RawConfigResponse {
  enabled?: boolean;
  interval_minutes?: number;
  categories?: Partial<DwsSyncCategories>;
  last_synced_at?: DwsLastSyncedAt;
}

/**
 * Unwrap a `RpcOutcome<T>` envelope. The Rust core wraps a controller's
 * payload in `{ result: T, logs: string[] }` when any log lines were emitted
 * (which all our handlers do), and returns the raw payload otherwise. We
 * accept either shape here so the hook is resilient.
 */
function unwrapOutcome<T>(value: unknown): T | undefined {
  if (value && typeof value === 'object') {
    const obj = value as Record<string, unknown>;
    if ('result' in obj && 'logs' in obj) {
      return obj.result as T;
    }
  }
  return value as T | undefined;
}

function normalizeConfig(raw: RawConfigResponse): DwsSyncConfig {
  const categories: DwsSyncCategories = {
    calendar: false,
    todo: false,
    contact: false,
    attendance: false,
    approval: false,
    report: false,
    mail: false,
    doc: false,
    chat: false,
    ...(raw.categories ?? {}),
  };
  return {
    enabled: raw.enabled ?? false,
    interval_minutes: raw.interval_minutes ?? 30,
    categories,
    last_synced_at: raw.last_synced_at ?? {},
  };
}

/**
 * Hook to manage DWS (DingTalk) periodic sync configuration. Reads/writes via
 * `openhuman.config_get_dws_sync_settings` /
 * `openhuman.config_update_dws_sync_settings` /
 * `openhuman.config_dws_sync_now`.
 */
export function useDwsSyncConfig(): UseDwsSyncConfigResult {
  const [config, setConfig] = useState<DwsSyncConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshConfig = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const raw = await callCoreRpc<unknown>({
        method: 'openhuman.config_get_dws_sync_settings',
        params: {},
      });
      const payload = unwrapOutcome<RawConfigResponse>(raw) ?? {};
      setConfig(normalizeConfig(payload));
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to load DWS sync config';
      setError(message);
    } finally {
      setLoading(false);
    }
  }, []);

  const updateConfig = useCallback(
    async (patch: Partial<Omit<DwsSyncConfig, 'last_synced_at'>>) => {
      setLoading(true);
      setError(null);
      try {
        await callCoreRpc({ method: 'openhuman.config_update_dws_sync_settings', params: patch });
        await refreshConfig();
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to update DWS sync config';
        setError(message);
      } finally {
        setLoading(false);
      }
    },
    [refreshConfig]
  );

  const toggleCategory = useCallback(
    async (category: keyof DwsSyncCategories) => {
      if (!config) return;
      const newValue = !config.categories[category];
      await updateConfig({ categories: { ...config.categories, [category]: newValue } });
    },
    [config, updateConfig]
  );

  const syncNow = useCallback(async (): Promise<SyncNowResult | null> => {
    setSyncing(true);
    setError(null);
    try {
      const raw = await callCoreRpc<unknown>({
        method: 'openhuman.config_dws_sync_now',
        params: {},
      });
      // The sync writes new last-synced timestamps; refresh so the UI shows them.
      await refreshConfig();
      return unwrapOutcome<SyncNowResult>(raw) ?? null;
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Sync failed';
      setError(message);
      return null;
    } finally {
      setSyncing(false);
    }
  }, [refreshConfig]);

  useEffect(() => {
    void refreshConfig();
  }, [refreshConfig]);

  return { config, loading, syncing, refreshConfig, updateConfig, toggleCategory, syncNow, error };
}
