import { getStoredLlmSettings } from '../../utils/configPersistence';

interface LlmConfigCardProps {
  /** Called when the user taps the edit button. */
  onEdit: () => void;
}

/** Masks an API key to show only the first 3 and last 4 characters. */
function maskApiKey(key: string): string {
  if (key.length <= 8) return '••••••••';
  return `${key.slice(0, 3)}${'•'.repeat(4)}${key.slice(-4)}`;
}

/** Truncates a URL to at most `maxLength` characters with ellipsis. */
function truncateUrl(url: string, maxLength = 32): string {
  if (url.length <= maxLength) return url;
  return `${url.slice(0, maxLength)}…`;
}

/**
 * Compact card showing the current custom LLM configuration at a glance.
 * Displayed on the Home page when the user is in custom LLM mode.
 */
const LlmConfigCard = ({ onEdit }: LlmConfigCardProps) => {
  const settings = getStoredLlmSettings();
  if (!settings) return null;

  return (
    <div className="mt-3 bg-white dark:bg-neutral-900 rounded-2xl shadow-soft border border-stone-200 dark:border-neutral-800 p-4 animate-fade-up">
      <div className="flex items-center justify-between mb-2">
        <span className="text-[11px] uppercase tracking-wide text-stone-400 dark:text-neutral-500">
          LLM Configuration
        </span>
        <button
          type="button"
          onClick={onEdit}
          aria-label="Edit LLM configuration"
          className="p-1.5 rounded-lg text-stone-400 dark:text-neutral-500 hover:text-primary-500 hover:bg-stone-100 dark:hover:bg-neutral-800 transition-colors">
          <svg
            className="w-4 h-4"
            fill="none"
            stroke="currentColor"
            strokeWidth={2}
            viewBox="0 0 24 24"
            aria-hidden="true">
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M16.862 4.487l1.687-1.688a1.875 1.875 0 112.652 2.652L10.582 16.07a4.5 4.5 0 01-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 011.13-1.897l8.932-8.931z"
            />
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M19.5 7.125L16.862 4.487"
            />
          </svg>
        </button>
      </div>
      <div className="space-y-1.5 text-sm">
        <div className="flex items-center gap-2">
          <span className="text-stone-500 dark:text-neutral-400 min-w-[60px]">Model</span>
          <span className="text-stone-900 dark:text-neutral-100 font-medium truncate">
            {settings.model}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-stone-500 dark:text-neutral-400 min-w-[60px]">Endpoint</span>
          <span className="text-stone-600 dark:text-neutral-300 truncate text-xs font-mono">
            {truncateUrl(settings.inferenceUrl)}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-stone-500 dark:text-neutral-400 min-w-[60px]">API Key</span>
          <span className="text-stone-600 dark:text-neutral-300 text-xs font-mono">
            {maskApiKey(settings.apiKey)}
          </span>
        </div>
      </div>
    </div>
  );
};

export default LlmConfigCard;
