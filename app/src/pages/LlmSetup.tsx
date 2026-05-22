import createDebug from 'debug';
import { useCallback, useEffect, useState } from 'react';

import RotatingTetrahedronCanvas from '../components/RotatingTetrahedronCanvas';
import Button from '../components/ui/Button';
import {
  getStoredLlmSettings,
  type LlmSettings,
  storeLlmSettings,
} from '../utils/configPersistence';

const log = createDebug('app:llm-setup');

/** Default LLM settings seeded from env vars at build time. */
const ENV_DEFAULTS: LlmSettings = {
  inferenceUrl: (import.meta.env.VITE_LLM_INFERENCE_URL as string | undefined)?.trim() ?? '',
  apiKey: (import.meta.env.VITE_LLM_API_KEY as string | undefined)?.trim() ?? '',
  model: (import.meta.env.VITE_LLM_MODEL as string | undefined)?.trim() ?? '',
};

interface LlmSetupProps {
  /** Called after settings are saved successfully. */
  onComplete: (settings: LlmSettings) => void;
  /** Display mode: 'setup' for first-time wizard, 'edit' for inline editing. */
  mode?: 'setup' | 'edit';
  /** When true, renders without the full-page container/logo chrome. */
  compact?: boolean;
}

/**
 * LLM configuration page — the user enters a custom OpenAI-compatible
 * endpoint, API key, and model name. Values are persisted in localStorage
 * and written to the Rust core via RPC so the inference provider uses
 * the user's own LLM.
 */
const LlmSetup = ({ onComplete, mode = 'setup', compact = false }: LlmSetupProps) => {
  const [inferenceUrl, setInferenceUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [model, setModel] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  // Seed from previously saved settings, then fall back to build-time env defaults.
  useEffect(() => {
    const stored = getStoredLlmSettings();
    setInferenceUrl(stored?.inferenceUrl ?? ENV_DEFAULTS.inferenceUrl);
    setApiKey(stored?.apiKey ?? ENV_DEFAULTS.apiKey);
    setModel(stored?.model ?? ENV_DEFAULTS.model);
  }, []);

  const isFormValid =
    inferenceUrl.trim().length > 0 && apiKey.trim().length > 0 && model.trim().length > 0;

  const handleSave = useCallback(async () => {
    if (!isFormValid) return;

    setIsSaving(true);
    setError(null);

    const settings: LlmSettings = {
      inferenceUrl: inferenceUrl.trim(),
      apiKey: apiKey.trim(),
      model: model.trim(),
    };

    try {
      // Persist locally first so subsequent launches pick up these values
      // even if the RPC write fails (the Rust core may not be running yet).
      storeLlmSettings(settings);
      log('LLM settings saved to localStorage');

      // Try to push settings to the Rust core via RPC so the inference
      // provider picks them up immediately.
      try {
        const { callCoreRpc } = await import('../services/coreRpcClient');
        await callCoreRpc({
          method: 'openhuman.inference_update_model_settings',
          params: {
            inference_url: settings.inferenceUrl,
            api_key: settings.apiKey,
            default_model: settings.model,
          },
        });
        log('LLM settings pushed to core via RPC');
      } catch (rpcError) {
        // Core may not be running yet during first setup — that's fine.
        // The env vars and config.toml will pick it up on next launch.
        log('RPC push skipped (core may not be running): %O', rpcError);
      }

      onComplete(settings);
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : String(saveError);
      log('Failed to save LLM settings: %s', message);
      setError(message || 'Failed to save settings. Please try again.');
    } finally {
      setIsSaving(false);
    }
  }, [inferenceUrl, apiKey, model, isFormValid, onComplete]);

  const isEditMode = mode === 'edit';
  const submitLabel = isSaving ? 'Saving...' : isEditMode ? 'Save' : 'Save & Continue';

  const formContent = (
    <>
      {error && (
        <div
          role="alert"
          className="mb-4 rounded-lg border border-red-200 bg-red-50 dark:bg-red-900/20 dark:border-red-800 px-3 py-2 text-sm text-red-700 dark:text-red-400">
          {error}
        </div>
      )}

      <form
        onSubmit={event => {
          event.preventDefault();
          void handleSave();
        }}
        className="space-y-4">
        {/* Base URL */}
        <div>
          <label
            htmlFor="llm-inference-url"
            className="block text-sm font-medium text-stone-700 dark:text-neutral-300 mb-1">
            Base URL (Path)
          </label>
          <input
            id="llm-inference-url"
            type="url"
            value={inferenceUrl}
            onChange={event => setInferenceUrl(event.target.value)}
            placeholder="https://api.example.com/v1"
            className="w-full rounded-lg border border-stone-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 px-3 py-2 text-sm text-stone-900 dark:text-neutral-100 placeholder:text-stone-400 dark:placeholder:text-neutral-500 focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/25"
            required
          />
        </div>

        {/* API Key */}
        <div>
          <label
            htmlFor="llm-api-key"
            className="block text-sm font-medium text-stone-700 dark:text-neutral-300 mb-1">
            API Key (AK)
          </label>
          <input
            id="llm-api-key"
            type="password"
            value={apiKey}
            onChange={event => setApiKey(event.target.value)}
            placeholder="sk-..."
            className="w-full rounded-lg border border-stone-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 px-3 py-2 text-sm text-stone-900 dark:text-neutral-100 placeholder:text-stone-400 dark:placeholder:text-neutral-500 focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/25"
            required
          />
        </div>

        {/* Model */}
        <div>
          <label
            htmlFor="llm-model"
            className="block text-sm font-medium text-stone-700 dark:text-neutral-300 mb-1">
            Model
          </label>
          <input
            id="llm-model"
            type="text"
            value={model}
            onChange={event => setModel(event.target.value)}
            placeholder="gpt-4o"
            className="w-full rounded-lg border border-stone-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 px-3 py-2 text-sm text-stone-900 dark:text-neutral-100 placeholder:text-stone-400 dark:placeholder:text-neutral-500 focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/25"
            required
          />
        </div>

        {/* Submit */}
        <Button
          type="submit"
          variant="primary"
          size={compact ? 'md' : 'lg'}
          disabled={!isFormValid || isSaving}
          className="w-full mt-2">
          {isSaving ? (
            <span className="flex items-center justify-center gap-2">
              <span className="h-4 w-4 animate-spin rounded-full border-2 border-white border-t-transparent" />
              Saving...
            </span>
          ) : (
            submitLabel
          )}
        </Button>
      </form>
    </>
  );

  // Compact mode: just the form with a small heading, no outer page chrome.
  if (compact) {
    return (
      <div className="space-y-4">
        <h2 className="text-lg font-semibold text-stone-900 dark:text-neutral-100">
          LLM Configuration
        </h2>
        {formContent}
      </div>
    );
  }

  return (
    <div className="min-h-full flex flex-col items-center justify-center p-4">
      <div className="max-w-md w-full">
        <div className="bg-white dark:bg-neutral-900 rounded-2xl shadow-soft border border-stone-200 dark:border-neutral-800 p-8 animate-fade-up">
          {/* Logo — only in setup mode */}
          {!isEditMode && (
            <div className="flex justify-center mb-6">
              <div className="h-20 w-20">
                <RotatingTetrahedronCanvas />
              </div>
            </div>
          )}

          <h1 className="text-2xl font-bold text-stone-900 dark:text-neutral-100 text-center mb-2">
            {isEditMode ? 'LLM Configuration' : 'Configure Your LLM'}
          </h1>

          <p className="text-sm text-stone-500 dark:text-neutral-400 text-center mb-6 leading-relaxed">
            {isEditMode
              ? 'Update your OpenAI-compatible endpoint details below.'
              : 'Enter your OpenAI-compatible endpoint details to get started.'}
          </p>

          {formContent}

          <p className="mt-4 text-center text-[11px] leading-5 text-stone-400 dark:text-neutral-500">
            Settings are stored locally and used to connect to your LLM provider.
          </p>
        </div>
      </div>
    </div>
  );
};

export default LlmSetup;
