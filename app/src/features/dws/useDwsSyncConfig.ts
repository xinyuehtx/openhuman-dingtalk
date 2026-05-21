import { useCallback, useEffect, useState } from 'react';

import { callCoreRpc } from '../../services/coreRpcClient';

/** Per-category sync toggle state.
 *
 * v2 surface (post-redesign): four content sources that all flow into the
 * memory tree via `ingest_chat` / `ingest_document`. The previous
 * categories (contact / attendance / approval / report / todo) were
 * dropped as low-signal; mail was pulled out because `mail.message:search`
 * requires a separate browser-driven PAT grant and pulling full inbox
 * bodies into local memory raised the privacy bar past what the feature
 * justified.
 */
export interface DwsSyncCategories {
  chat: boolean;
  doc: boolean;
  calendar: boolean;
  minutes: boolean;
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

/** Per-category state during an in-flight sync run. Wire shape mirrors
 *  the Rust `CategoryState` enum's `#[serde(tag = "kind")]` form. */
export type DwsCategoryState =
  | { kind: 'pending' }
  | { kind: 'running'; current: number; total: number | null; label?: string | null }
  | { kind: 'done'; records: number; chunks: number }
  | { kind: 'failed'; error: string };

export interface DwsCategoryProgress {
  category: 'chat' | 'doc' | 'calendar' | 'minutes';
  state: DwsCategoryState;
}

/** Snapshot of an in-flight or just-completed DWS sync run. Polled
 *  every ~500ms from `openhuman.config_dws_sync_progress` while the
 *  sync button is active. */
export interface DwsSyncProgressSnapshot {
  run_id: string;
  started_at: number;
  finished_at: number | null;
  categories: DwsCategoryProgress[];
}

/** Result of kicking off a sync. With the non-blocking RPC redesign
 *  the payload is the initial progress handshake (run_id + optional
 *  seeded progress), not the final per-category counts — those live in
 *  the polled `DwsSyncProgressSnapshot`. */
export interface SyncNowResult {
  synced: boolean;
  message?: string;
  /** Non-blocking marker: true when `config_dws_sync_now` returned
   *  immediately after spawning the background task. Always true on
   *  success since the redesign. */
  async?: boolean;
  /** Stable id for the run, used by progress polling. */
  run_id?: string;
  /** False when another run was already in flight and we're observing
   *  it instead of starting a fresh one. */
  started_fresh?: boolean;
  /** Initial progress snapshot returned alongside the kick-off. The
   *  poll loop overwrites this as new snapshots arrive. */
  progress?: DwsSyncProgressSnapshot | null;
  last_synced_at?: DwsLastSyncedAt;
}

export interface UseDwsSyncConfigResult {
  config: DwsSyncConfig | null;
  loading: boolean;
  syncing: boolean;
  /** Live per-category state during the in-flight run, or the last
   *  completed run's final state. `null` before the first sync. */
  syncProgress: DwsSyncProgressSnapshot | null;
  refreshConfig: () => Promise<void>;
  /** Replace one or more top-level fields. */
  updateConfig: (patch: Partial<Omit<DwsSyncConfig, 'last_synced_at'>>) => Promise<void>;
  /** Toggle a single sync category on/off. */
  toggleCategory: (category: keyof DwsSyncCategories) => Promise<void>;
  /** Trigger an immediate sync for all enabled categories. Returns
   *  after the run *finishes* (i.e. after the poll loop sees
   *  `finished_at`) — `syncing` stays true throughout. */
  syncNow: () => Promise<SyncNowResult | null>;
  /** Drop persisted `last_synced_at` cursors so the next sync uses
   *  each category's full cold-start window (Chat/Calendar=1h,
   *  Doc/Minutes=30d). Pass `null` / omit to clear all categories.
   *  Does NOT trigger a sync — pair with `forceColdStartSync` for
   *  the "强制冷启动拉取" UI flow.
   *  Returns the list of state-keys that were cleared. */
  resetCursors: (
    categories?: Array<keyof DwsSyncCategories> | null
  ) => Promise<string[] | null>;
  /** Convenience: clear cursors AND immediately kick off a sync so
   *  the next run pulls history all the way back to its cold-start
   *  window. Used by the "强制冷启动拉取" button. */
  forceColdStartSync: (
    categories?: Array<keyof DwsSyncCategories> | null
  ) => Promise<SyncNowResult | null>;
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
    chat: false,
    doc: false,
    calendar: false,
    minutes: false,
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
/** Default poll cadence for `dws_sync_progress` while a run is in
 *  flight. Tight enough that per-category transitions feel live;
 *  loose enough that the in-process RPC + lock contention stays
 *  cheap. */
const PROGRESS_POLL_INTERVAL_MS = 500;
/** Hard cap on polling duration so a wedged run can't keep the UI in
 *  "syncing" state forever. 10 minutes covers the worst-case first
 *  full sync (doc + minutes adapters fetching dozens of bodies); after
 *  that we give up and surface an error. */
const PROGRESS_POLL_TIMEOUT_MS = 10 * 60 * 1000;

export function useDwsSyncConfig(): UseDwsSyncConfigResult {
  const [config, setConfig] = useState<DwsSyncConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [syncProgress, setSyncProgress] = useState<DwsSyncProgressSnapshot | null>(null);
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
    setSyncProgress(null);
    try {
      // Kick off the run. With the non-blocking RPC redesign this
      // returns within milliseconds even on a fresh install — the real
      // fetching runs in a background tokio task on the core.
      const raw = await callCoreRpc<unknown>({
        method: 'openhuman.config_dws_sync_now',
        params: {},
      });
      const kickoff = unwrapOutcome<SyncNowResult>(raw) ?? null;
      if (!kickoff?.synced) {
        // "No categories enabled" or similar — surface as-is, nothing
        // to poll.
        return kickoff;
      }
      // Seed the live snapshot from the kick-off payload so the UI
      // immediately shows a "Pending" row per category before the
      // first poll tick lands.
      if (kickoff.progress) {
        setSyncProgress(kickoff.progress);
      }
      // Poll progress until the run reports `finished_at`, or until
      // we hit the hard timeout. The progress RPC is cheap (in-process
      // mutex lookup, no I/O) so the 500ms cadence is comfortable.
      const deadline = Date.now() + PROGRESS_POLL_TIMEOUT_MS;
      let lastSnap: DwsSyncProgressSnapshot | null = kickoff.progress ?? null;
      while (Date.now() < deadline) {
        await sleep(PROGRESS_POLL_INTERVAL_MS);
        const pollRaw = await callCoreRpc<unknown>({
          method: 'openhuman.config_dws_sync_progress',
          params: {},
        });
        const snap = unwrapOutcome<DwsSyncProgressSnapshot | null>(pollRaw) ?? null;
        if (snap) {
          // Drop stale snapshots for prior runs — a user clicking
          // twice in quick succession could otherwise see an older
          // run's "finished" state and stop polling early.
          if (kickoff.run_id && snap.run_id !== kickoff.run_id) {
            continue;
          }
          lastSnap = snap;
          setSyncProgress(snap);
          if (snap.finished_at != null) {
            break;
          }
        }
      }
      if (lastSnap?.finished_at == null) {
        setError(
          `Sync still in flight after ${Math.round(PROGRESS_POLL_TIMEOUT_MS / 1000)}s — ` +
            `check core logs (grep dws:sync:progress) for the stuck adapter`
        );
      }
      // Pull the freshly-recorded last-sync timestamps now that the
      // background task has written them to disk.
      await refreshConfig();
      return { ...kickoff, progress: lastSnap };
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Sync failed';
      setError(message);
      return null;
    } finally {
      setSyncing(false);
    }
  }, [refreshConfig]);

  const resetCursors = useCallback(
    async (
      categories?: Array<keyof DwsSyncCategories> | null
    ): Promise<string[] | null> => {
      setError(null);
      try {
        const raw = await callCoreRpc<unknown>({
          method: 'openhuman.config_dws_sync_reset_cursors',
          params:
            categories && categories.length > 0
              ? { categories }
              : {},
        });
        const out = unwrapOutcome<{ cleared?: string[]; count?: number }>(raw) ?? {};
        // Refresh so the UI's last_synced_at labels disappear for cleared cats.
        await refreshConfig();
        return out.cleared ?? [];
      } catch (err) {
        const message =
          err instanceof Error ? err.message : 'Failed to reset DWS sync cursors';
        setError(message);
        return null;
      }
    },
    [refreshConfig]
  );

  const forceColdStartSync = useCallback(
    async (
      categories?: Array<keyof DwsSyncCategories> | null
    ): Promise<SyncNowResult | null> => {
      // Clear first so the subsequent sync_now sees no cursor and falls
      // back to the per-category cold-start window. A reset failure
      // surfaces the error and aborts before the sync — better than
      // running an incremental sync the user thought was a full pull.
      const cleared = await resetCursors(categories);
      if (cleared === null) return null;
      return syncNow();
    },
    [resetCursors, syncNow]
  );

  useEffect(() => {
    void refreshConfig();
  }, [refreshConfig]);

  return {
    config,
    loading,
    syncing,
    syncProgress,
    refreshConfig,
    updateConfig,
    toggleCategory,
    syncNow,
    resetCursors,
    forceColdStartSync,
    error,
  };
}

/** `setTimeout` promisified — used by the progress poll loop. Pulled
 *  inline so we don't take a new dependency just for this. */
function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}
