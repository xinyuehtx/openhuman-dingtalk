import { useCallback, useEffect, useMemo, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

import ChannelSetupModal from '../components/channels/ChannelSetupModal';
import DwsSetupCard from '../components/dws/DwsSetupCard';
import { ToastContainer } from '../components/intelligence/Toast';
import AutocompleteSetupModal from '../components/skills/AutocompleteSetupModal';
import CreateSkillModal from '../components/skills/CreateSkillModal';
import InstallSkillDialog from '../components/skills/InstallSkillDialog';
import ScreenIntelligenceSetupModal from '../components/skills/ScreenIntelligenceSetupModal';
import UnifiedSkillCard from '../components/skills/SkillCard';
import type { SkillCategory } from '../components/skills/skillCategories';
import SkillDetailDrawer from '../components/skills/SkillDetailDrawer';
import {
  BUILT_IN_SKILL_ICONS,
  CHANNEL_ICONS,
  skillCategoryHeadingClassName,
  SkillCategoryIcon,
} from '../components/skills/skillIcons';
import UninstallSkillConfirmDialog from '../components/skills/UninstallSkillConfirmDialog';
import VoiceSetupModal from '../components/skills/VoiceSetupModal';
import { useAutocompleteSkillStatus } from '../features/autocomplete/useAutocompleteSkillStatus';
import { useScreenIntelligenceSkillStatus } from '../features/screen-intelligence/useScreenIntelligenceSkillStatus';
import { useVoiceSkillStatus } from '../features/voice/useVoiceSkillStatus';
import { useChannelDefinitions } from '../hooks/useChannelDefinitions';
import { useT } from '../lib/i18n/I18nContext';
import { skillsApi, type SkillSummary } from '../services/api/skillsApi';
import { useAppSelector } from '../store/hooks';
import type { ChannelConnectionStatus, ChannelDefinition, ChannelType } from '../types/channels';
import type { ToastNotification } from '../types/intelligence';
import { subconsciousEscalationsDismiss } from '../utils/tauriCommands';

function channelStatusLabel(status: ChannelConnectionStatus, t: (key: string) => string): string {
  switch (status) {
    case 'connected':
      return t('skills.connected');
    case 'connecting':
      return t('channels.status.connecting');
    case 'error':
      return t('common.error');
    default:
      return t('channels.status.notConfigured');
  }
}

function channelStatusColor(status: ChannelConnectionStatus): string {
  switch (status) {
    case 'connected':
      return 'text-sage-600 dark:text-sage-300';
    case 'connecting':
      return 'text-amber-600 dark:text-amber-300';
    case 'error':
      return 'text-coral-600 dark:text-coral-300';
    default:
      return 'text-stone-400 dark:text-neutral-500';
  }
}

interface ChannelTileProps {
  def: ChannelDefinition;
  status: ChannelConnectionStatus;
  icon: React.ReactNode;
  onOpen: () => void;
}

function ChannelTile({ def, status, icon, onOpen }: ChannelTileProps) {
  const { t } = useT();
  const isConnected = status === 'connected';
  const isPending = status === 'connecting';
  const isError = status === 'error';
  const statusLabel = channelStatusLabel(status, t);
  const ctaLabel = isConnected ? t('skills.configure') : t('channels.setup');

  return (
    <button
      type="button"
      onClick={onOpen}
      title={`${def.display_name} — ${def.description}`}
      aria-label={`${def.display_name}, ${statusLabel}. ${ctaLabel}.`}
      className={`group flex flex-col items-center gap-2 rounded-2xl border p-3 pb-3 text-center transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-primary-500/40 ${
        isConnected
          ? 'border-sage-300 bg-sage-50/80 shadow-[0_0_0_1px_rgba(34,197,94,0.12)] hover:bg-sage-50 dark:border-sage-500/30 dark:bg-sage-500/10 dark:hover:bg-sage-500/15'
          : isPending
            ? 'border-amber-200 bg-amber-50/40 hover:bg-amber-50/70 dark:border-amber-500/30 dark:bg-amber-500/10 dark:hover:bg-amber-500/15'
            : isError
              ? 'border-coral-200 bg-coral-50/30 hover:bg-coral-50/50 dark:border-coral-500/30 dark:bg-coral-500/10 dark:hover:bg-coral-500/15'
              : 'border-stone-200 bg-white hover:bg-stone-50 dark:border-neutral-800 dark:bg-neutral-900 dark:hover:bg-neutral-800/60'
      }`}>
      <div className="relative flex h-12 w-12 flex-shrink-0 items-center justify-center text-stone-700 dark:text-neutral-200 [&>span]:h-12 [&>span]:w-12 [&>span]:rounded-2xl [&_svg]:h-7 [&_svg]:w-7">
        {icon}
      </div>
      <div className="flex min-h-[2.5rem] w-full min-w-0 flex-col items-center justify-start gap-0.5">
        <span className="line-clamp-2 text-[11px] font-semibold leading-tight text-stone-900 dark:text-neutral-100">
          {def.display_name}
        </span>
        <span className={`line-clamp-1 text-[10px] font-medium ${channelStatusColor(status)}`}>
          {statusLabel}
        </span>
      </div>
    </button>
  );
}

// ─── Built-in skill definitions ────────────────────────────────────────────────

const BUILT_IN_SKILLS: Array<{
  id: string;
  title: string;
  description: string;
  route: string;
  icon: React.ReactNode;
}> = [
  // Hidden — not active yet. Uncomment to re-enable.
  // {
  //   id: 'screen-intelligence',
  //   title: 'Screen Intelligence',
  //   description:
  //     'Capture windows, summarize what is on screen, and feed useful context into memory.',
  //   route: '/settings/screen-intelligence',
  //   icon: BUILT_IN_SKILL_ICONS.screenIntelligence,
  // },
  // text-autocomplete + voice-stt hidden per #717 (modals/status hooks retained for re-enable).
];

// ─── Item type for unified list ────────────────────────────────────────────────

interface SkillItem {
  id: string;
  name: string;
  description: string;
  category: SkillCategory;
  kind: 'builtin' | 'channel' | 'discovered';
  // For built-in
  route?: string;
  icon?: React.ReactNode;
  // For channel
  channelDef?: ChannelDefinition;
  channelStatus?: ChannelConnectionStatus;
  // For discovered SKILL.md skills
  discoveredSkill?: SkillSummary;
}

// ─── Main Skills Page ──────────────────────────────────────────────────────────

export default function Skills() {
  const { t } = useT();
  const location = useLocation();
  const navigate = useNavigate();
  const { definitions: channelDefs } = useChannelDefinitions();
  const channelConnections = useAppSelector(state => state.channelConnections);

  const [channelModalDef, setChannelModalDef] = useState<ChannelDefinition | null>(null);
  const [screenIntelligenceModalOpen, setScreenIntelligenceModalOpen] = useState(false);
  const [autocompleteModalOpen, setAutocompleteModalOpen] = useState(false);
  const [voiceModalOpen, setVoiceModalOpen] = useState(false);
  const screenIntelligenceStatus = useScreenIntelligenceSkillStatus();
  const autocompleteStatus = useAutocompleteSkillStatus();
  const voiceStatus = useVoiceSkillStatus();

  const [discoveredSkills, setDiscoveredSkills] = useState<SkillSummary[]>([]);
  const [selectedSkill, setSelectedSkill] = useState<SkillSummary | null>(null);
  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [installDialogOpen, setInstallDialogOpen] = useState(false);
  const [uninstallCandidate, setUninstallCandidate] = useState<SkillSummary | null>(null);
  const [toasts, setToasts] = useState<ToastNotification[]>([]);
  const addToast = useCallback((toast: Omit<ToastNotification, 'id'>) => {
    const newToast: ToastNotification = { ...toast, id: `toast-${Date.now()}-${Math.random()}` };
    setToasts(prev => [...prev, newToast]);
  }, []);
  const removeToast = useCallback((id: string) => {
    setToasts(prev => prev.filter(toast => toast.id !== id));
  }, []);
  const pendingEscalationId =
    location.state &&
    typeof location.state === 'object' &&
    'subconsciousEscalationId' in location.state &&
    typeof location.state.subconsciousEscalationId === 'string'
      ? location.state.subconsciousEscalationId
      : null;

  const clearPendingEscalationState = useCallback(() => {
    navigate(location.pathname, { replace: true, state: null });
  }, [location.pathname, navigate]);

  // Kept around for parity with the previous Composio drawer that used to dismiss
  // open subconscious escalations on connect — still useful for channel modals.
  const dismissPendingEscalationIfResolved = useCallback(
    async (resolution: string) => {
      if (!pendingEscalationId) return;
      try {
        await subconsciousEscalationsDismiss(pendingEscalationId);
      } catch (error) {
        console.debug('[skills][subconscious] dismiss escalation:error', {
          escalationId: pendingEscalationId,
          resolution,
          error: error instanceof Error ? error.message : String(error),
        });
        return;
      }
      clearPendingEscalationState();
    },
    [clearPendingEscalationState, pendingEscalationId]
  );

  const refreshDiscoveredSkills = useCallback(async (): Promise<SkillSummary[]> => {
    try {
      const skills = await skillsApi.listSkills();
      setDiscoveredSkills(skills);
      return skills;
    } catch (err) {
      console.debug('[skills][discovered] listSkills error', {
        error: err instanceof Error ? err.message : String(err),
      });
      return [];
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const skills = await refreshDiscoveredSkills();
      if (cancelled) return;
      void skills;
    })();
    return () => {
      cancelled = true;
    };
  }, [refreshDiscoveredSkills]);

  const bestChannelStatus = (channelId: ChannelType): ChannelConnectionStatus => {
    const conns = channelConnections.connections[channelId];
    if (!conns) return 'disconnected';
    const statuses = Object.values(conns).map(c => c?.status ?? 'disconnected');
    if (statuses.includes('connected')) return 'connected';
    if (statuses.includes('connecting')) return 'connecting';
    if (statuses.includes('error')) return 'error';
    return 'disconnected';
  };

  const configurableChannels = useMemo(
    () => channelDefs.filter(d => d.id !== 'web'),
    [channelDefs]
  );

  // Unified item list
  const allItems: SkillItem[] = useMemo(() => {
    const items: SkillItem[] = [];

    for (const s of BUILT_IN_SKILLS) {
      items.push({
        id: s.id,
        name: s.title,
        description: s.description,
        category: 'Built-in',
        kind: 'builtin',
        route: s.route,
        icon: s.icon,
      });
    }

    for (const def of configurableChannels) {
      items.push({
        id: `channel-${def.id}`,
        name: def.display_name,
        description: def.description,
        category: 'Channels',
        kind: 'channel',
        channelDef: def,
        channelStatus: bestChannelStatus(def.id as ChannelType),
        icon: CHANNEL_ICONS[def.icon],
      });
    }

    // Discovered SKILL.md skills — surface each as a card whose CTA opens
    // the detail drawer. They live under the generic "Other" category so
    // they don't displace hand-curated built-ins or Channels.
    for (const skill of discoveredSkills) {
      items.push({
        id: `discovered-${skill.id}`,
        name: skill.name,
        description: skill.description,
        category: 'Other',
        kind: 'discovered',
        icon: BUILT_IN_SKILL_ICONS.screenIntelligence,
        discoveredSkill: skill,
      });
    }

    return items;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [configurableChannels, channelConnections, discoveredSkills]);

  const filteredItems = useMemo(() => {
    return allItems.filter(item => {
      return item.category !== 'Channels';
    });
  }, [allItems]);

  // Underscore prefix tells TS to ignore the unused-binding warning.
  // Kept as a stable computation so downstream callers that may rely
  // on it via future PR conflicts don't silently lose it.
  const _groupedItems = useMemo(() => {
    const groups = new Map<SkillCategory, SkillItem[]>();
    for (const item of filteredItems) {
      const existing = groups.get(item.category);
      if (existing) {
        existing.push(item);
      } else {
        groups.set(item.category, [item]);
      }
    }
    return Array.from(groups.entries()).map(([category, items]) => ({ category, items }));
  }, [filteredItems]);
  void _groupedItems;

  const channelsGroup = useMemo(() => {
    const items = allItems.filter(item => item.category === 'Channels');
    return items.length > 0 ? { category: 'Channels' as SkillCategory, items } : undefined;
  }, [allItems]);

  const otherGroups = useMemo(() => {
    const groups = new Map<SkillCategory, SkillItem[]>();
    for (const item of allItems) {
      if (item.category === 'Channels') continue;
      const list = groups.get(item.category);
      if (list) list.push(item);
      else groups.set(item.category, [item]);
    }
    return Array.from(groups.entries()).map(([category, items]) => ({ category, items }));
  }, [allItems]);

  const renderGroup = ({ category, items }: { category: SkillCategory; items: SkillItem[] }) => (
    <div
      key={category}
      className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-3 shadow-soft animate-fade-up">
      <div className="px-1 pb-3 pt-1">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-stone-900 dark:text-neutral-100">
          <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-stone-100 dark:bg-neutral-800">
            <SkillCategoryIcon
              category={category}
              className={skillCategoryHeadingClassName(category)}
            />
          </span>
          {category}
        </h2>
      </div>
      <div className="space-y-2">
        {items.map(item => {
          if (item.kind === 'builtin') {
            /* v8 ignore start */
            if (item.id === 'screen-intelligence') {
              return (
                <UnifiedSkillCard
                  key={item.id}
                  icon={item.icon}
                  title={item.name}
                  description={item.description}
                  statusLabel={screenIntelligenceStatus.statusLabel}
                  statusColor={screenIntelligenceStatus.statusColor}
                  ctaLabel={screenIntelligenceStatus.ctaLabel}
                  ctaVariant={screenIntelligenceStatus.ctaVariant}
                  onCtaClick={() => {
                    if (screenIntelligenceStatus.platformUnsupported) {
                      navigate(item.route!);
                      return;
                    }
                    if (
                      screenIntelligenceStatus.connectionStatus === 'connected' ||
                      screenIntelligenceStatus.connectionStatus === 'disconnected'
                    ) {
                      navigate(item.route!);
                      return;
                    }
                    setScreenIntelligenceModalOpen(true);
                  }}
                />
              );
            }
            if (item.id === 'text-autocomplete') {
              return (
                <UnifiedSkillCard
                  key={item.id}
                  icon={item.icon}
                  title={item.name}
                  description={item.description}
                  statusLabel={autocompleteStatus.statusLabel}
                  statusColor={autocompleteStatus.statusColor}
                  ctaLabel={autocompleteStatus.ctaLabel}
                  ctaVariant={autocompleteStatus.ctaVariant}
                  onCtaClick={() => {
                    if (
                      autocompleteStatus.platformUnsupported ||
                      autocompleteStatus.connectionStatus === 'connected' ||
                      autocompleteStatus.connectionStatus === 'disconnected'
                    ) {
                      navigate(item.route!);
                      return;
                    }
                    setAutocompleteModalOpen(true);
                  }}
                />
              );
            }
            if (item.id === 'voice-stt') {
              return (
                <UnifiedSkillCard
                  key={item.id}
                  icon={item.icon}
                  title={item.name}
                  description={item.description}
                  statusLabel={voiceStatus.statusLabel}
                  statusColor={voiceStatus.statusColor}
                  ctaLabel={voiceStatus.ctaLabel}
                  ctaVariant={voiceStatus.ctaVariant}
                  onCtaClick={() => {
                    if (
                      voiceStatus.connectionStatus === 'connected' ||
                      voiceStatus.connectionStatus === 'connecting' ||
                      voiceStatus.connectionStatus === 'disconnected'
                    ) {
                      navigate(item.route!);
                      return;
                    }
                    setVoiceModalOpen(true);
                  }}
                />
              );
            }
            return (
              <UnifiedSkillCard
                key={item.id}
                icon={item.icon}
                title={item.name}
                description={item.description}
                ctaLabel={t('nav.settings')}
                onCtaClick={() => navigate(item.route!)}
              />
            );
            /* v8 ignore stop */
          }
          if (item.kind === 'discovered') {
            const skill = item.discoveredSkill!;
            const scopeLabel = skill.legacy
              ? t('scope.legacy')
              : skill.scope === 'user'
                ? t('scope.user')
                : skill.scope === 'project'
                  ? t('scope.project')
                  : t('scope.legacy');
            const scopeColor = skill.legacy
              ? 'text-stone-600 dark:text-neutral-300'
              : skill.scope === 'user'
                ? 'text-sage-600'
                : skill.scope === 'project'
                  ? 'text-amber-600'
                  : 'text-stone-600 dark:text-neutral-300';
            const canUninstall = skill.scope === 'user' && !skill.legacy;
            return (
              <UnifiedSkillCard
                key={item.id}
                icon={item.icon}
                title={item.name}
                description={item.description}
                statusLabel={scopeLabel}
                statusColor={scopeColor}
                ctaLabel={t('common.seeAll')}
                onCtaClick={() => setSelectedSkill(skill)}
                secondaryActions={
                  canUninstall
                    ? [
                        {
                          label: t('skills.disconnect'),
                          testId: `uninstall-skill-${skill.id}`,
                          icon: (
                            <svg
                              className="h-3.5 w-3.5"
                              fill="none"
                              stroke="currentColor"
                              strokeWidth="2"
                              viewBox="0 0 24 24">
                              <path
                                strokeLinecap="round"
                                strokeLinejoin="round"
                                d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14z"
                              />
                            </svg>
                          ),
                          onClick: () => setUninstallCandidate(skill),
                        },
                      ]
                    : undefined
                }
              />
            );
          }
        })}
      </div>
    </div>
  );

  return (
    <div className="min-h-full">
      <div className="min-h-full flex flex-col">
        <div className="flex-1 flex items-start justify-center p-4 pt-6">
          <div className="w-full max-w-3xl space-y-4">
            {/* DingTalk DWS — primary integration for this fork. */}
            <div
              className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-3 shadow-soft animate-fade-up"
              data-walkthrough="skills-dws">
              <div className="px-1 pb-3 pt-1">
                <h2 className="flex items-center gap-2 text-sm font-semibold text-stone-900 dark:text-neutral-100">
                  <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-primary-100 dark:bg-primary-500/20 text-primary-600 dark:text-primary-300">
                    🟦
                  </span>
                  钉钉工作台 (DWS)
                </h2>
                <p className="mt-0.5 text-[11px] leading-relaxed text-stone-500 dark:text-neutral-400">
                  通过 DingTalk Workspace CLI 管理钉钉全产品能力 — AI
                  表格、日历、通讯录、待办、审批、考勤、文档、云盘等。
                </p>
              </div>
              <div className="px-1 pb-1">
                <DwsSetupCard />
              </div>
            </div>

            {channelsGroup && (
              <div className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-3 shadow-soft animate-fade-up">
                <div className="px-1 pb-3 pt-1">
                  <h2
                    className="flex items-center gap-2 text-sm font-semibold text-stone-900 dark:text-neutral-100"
                    data-walkthrough="skills-channels">
                    <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-stone-100 dark:bg-neutral-800">
                      <SkillCategoryIcon
                        category="Channels"
                        className={skillCategoryHeadingClassName('Channels')}
                      />
                    </span>
                    {t('skills.channels')}
                  </h2>
                  <p className="mt-0.5 text-[11px] leading-relaxed text-stone-500 dark:text-neutral-400">
                    {t('channels.defaultMessaging')}
                  </p>
                </div>
                <div
                  className="grid gap-2 sm:gap-3"
                  style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(5.5rem, 1fr))' }}>
                  {channelsGroup.items.map(item => (
                    <ChannelTile
                      key={item.id}
                      def={item.channelDef!}
                      status={item.channelStatus!}
                      icon={item.icon}
                      onOpen={() => setChannelModalDef(item.channelDef!)}
                    />
                  ))}
                </div>
              </div>
            )}

            {otherGroups.map(group => renderGroup(group))}
            {
              <>
                {channelsGroup && (
                  <div className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-3 shadow-soft animate-fade-up">
                    <div className="px-1 pb-3 pt-1">
                      <h2
                        className="flex items-center gap-2 text-sm font-semibold text-stone-900 dark:text-neutral-100"
                        data-walkthrough="skills-channels">
                        <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-stone-100 dark:bg-neutral-800">
                          <SkillCategoryIcon
                            category="Channels"
                            className={skillCategoryHeadingClassName('Channels')}
                          />
                        </span>
                        {t('skills.channels')}
                      </h2>
                      <p className="mt-0.5 text-[11px] leading-relaxed text-stone-500 dark:text-neutral-400">
                        {t('channels.defaultMessaging')}
                      </p>
                    </div>
                    <div
                      className="grid gap-2 sm:gap-3"
                      style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(5.5rem, 1fr))' }}>
                      {channelsGroup.items.map(item => (
                        <ChannelTile
                          key={item.id}
                          def={item.channelDef!}
                          status={item.channelStatus!}
                          icon={item.icon}
                          onOpen={() => setChannelModalDef(item.channelDef!)}
                        />
                      ))}
                    </div>
                  </div>
                )}

                {/* <MeetingBotsCard onToast={addToast} /> */}

                {/* DingTalk DWS Integration */}
                <div className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-3 shadow-soft animate-fade-up">
                  <div className="px-1 pb-3 pt-1">
                    <h2
                      className="flex items-center gap-2 text-sm font-semibold text-stone-900 dark:text-neutral-100"
                      data-walkthrough="skills-dws">
                      <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-stone-100 dark:bg-neutral-800">
                        🔔
                      </span>
                      钉钉工作台 (DWS)
                    </h2>
                    <p className="mt-0.5 text-[11px] leading-relaxed text-stone-500 dark:text-neutral-400">
                      通过 DingTalk Workspace CLI 管理钉钉全产品能力
                    </p>
                  </div>
                  <div className="px-1 pb-1">
                    <DwsSetupCard />
                  </div>
                </div>

                {otherGroups.map(group => renderGroup(group))}
              </>
            }
          </div>
        </div>
      </div>

      {channelModalDef && (
        <ChannelSetupModal
          definition={channelModalDef}
          onClose={() => {
            void dismissPendingEscalationIfResolved(`channel:${channelModalDef.id}`);
            setChannelModalDef(null);
          }}
        />
      )}

      {screenIntelligenceModalOpen && (
        <ScreenIntelligenceSetupModal
          onClose={() => setScreenIntelligenceModalOpen(false)}
          initialStep={screenIntelligenceStatus.allPermissionsGranted ? 'enable' : 'permissions'}
        />
      )}

      {autocompleteModalOpen && (
        <AutocompleteSetupModal onClose={() => setAutocompleteModalOpen(false)} />
      )}

      {voiceModalOpen && (
        <VoiceSetupModal onClose={() => setVoiceModalOpen(false)} skillStatus={voiceStatus} />
      )}

      {selectedSkill && (
        <SkillDetailDrawer skill={selectedSkill} onClose={() => setSelectedSkill(null)} />
      )}

      {createModalOpen && (
        <CreateSkillModal
          onClose={() => setCreateModalOpen(false)}
          onCreated={skill => {
            setCreateModalOpen(false);
            setDiscoveredSkills(prev =>
              prev.some(s => s.id === skill.id) ? prev : [...prev, skill]
            );
            setSelectedSkill(skill);
            void refreshDiscoveredSkills();
          }}
        />
      )}

      {installDialogOpen && (
        <InstallSkillDialog
          onClose={() => setInstallDialogOpen(false)}
          onInstalled={result => {
            void (async () => {
              const skills = await refreshDiscoveredSkills();
              const firstNewId = result.newSkills[0];
              if (firstNewId) {
                const match = skills.find(s => s.id === firstNewId);
                if (match) setSelectedSkill(match);
              }
            })();
          }}
        />
      )}

      {uninstallCandidate && (
        <UninstallSkillConfirmDialog
          skill={uninstallCandidate}
          onClose={() => setUninstallCandidate(null)}
          onUninstalled={result => {
            addToast({
              type: 'success',
              title: t('skills.disconnect'),
              message: `"${result.name}" ${t('common.success')}`,
            });
            setSelectedSkill(prev => (prev && prev.id === result.name ? null : prev));
            setDiscoveredSkills(prev => prev.filter(s => s.id !== result.name));
            void refreshDiscoveredSkills();
          }}
        />
      )}
      <ToastContainer notifications={toasts} onRemove={removeToast} />
    </div>
  );
}
