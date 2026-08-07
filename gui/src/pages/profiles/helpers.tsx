import type {
  ApiProfile,
  CodexCatalogModel,
  OpenCodeModelConfig,
  StatusInfo,
  TargetApp,
} from '../../types';
import { SUPPORTED_TOOLS } from '../../types';
import { PROVIDER_PRESETS } from '../../lib/presets';
import { cn } from '../../lib/utils';
import { statusKeyFor } from '../../lib/contextWindow';
import { Layers } from 'lucide-react';
import React from 'react';

export function EmptyState({ toolLabel }: { toolLabel?: string }) {
  return (
    <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center">
      <Layers size={20} className="mx-auto mb-2 text-ink-faint" />
      <p className="text-[13px] text-ink-faint">
        {toolLabel ? `${toolLabel} 暂无配置档案` : '暂无配置档案'}
      </p>
      <p className="mt-1 text-[11px] text-ink-faint">点右上角「新建档案」开始</p>
    </div>
  );
}

export function AppSelector({ value, onChange }: { value: TargetApp; onChange: (value: TargetApp) => void }) {
  return (
    <div className="flex w-fit max-w-full flex-wrap items-center gap-1 rounded-lg border border-line bg-surface p-1">
      {SUPPORTED_TOOLS.map((tool) => {
        const active = value === tool.id;
        return (
          <button
            key={tool.id}
            type="button"
            onClick={() => onChange(tool.id)}
            className={cn(
              'no-drag flex shrink-0 items-center gap-2 whitespace-nowrap rounded-md px-3 py-1.5 text-[13px] font-medium transition-colors',
              active ? 'bg-card text-ink shadow-soft' : 'text-ink-dim hover:text-ink',
            )}
          >
            <span className="h-2 w-2 rounded-full" style={{ background: tool.color }} />
            {tool.displayName}
          </button>
        );
      })}
    </div>
  );
}

export function IconBtn({ children, label, danger, onClick }: { children: React.ReactNode; label: string; danger?: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`no-drag grid place-items-center h-8 w-8 rounded-lg border border-line bg-surface transition-all hover:bg-elevated ${
        danger ? 'text-ink-faint hover:text-danger hover:border-danger/40' : 'text-ink-faint hover:text-ink'
      }`}
    >
      {children}
    </button>
  );
}

export function providerTint(provider: string): string {
  const p = provider.toLowerCase();
  if (p.includes('anthropic')) return '#8A5A44';
  if (p.includes('openai')) return '#10B981';
  if (p.includes('google')) return '#4F8DF6';
  if (p.includes('grok') || p.includes('xai')) return '#CA8A04';
  return '#4B5563';
}

export function activeProfileFor(status: StatusInfo | null, targetApp: TargetApp): ApiProfile | undefined {
  if (!status) return undefined;
  const key = statusKeyFor(targetApp) as keyof StatusInfo;
  const targetStatus = status[key];
  if (!targetStatus || typeof targetStatus !== 'object' || !('profile' in targetStatus)) return undefined;
  return (targetStatus as { profile?: ApiProfile }).profile;
}

export function emptyProfileForTool(tool: TargetApp): ApiProfile {
  const preset = PROVIDER_PRESETS[tool]?.[0];
  const base: ApiProfile = {
    name: '',
    provider: preset?.provider ?? 'anthropic',
    api_url: preset?.api_url ?? '',
    api_key: '',
    model: preset?.model,
    target_app: tool,
  };
  if (tool === 'openclaw') {
    return {
      ...base,
      context_1m: true,
      max_tokens: 128000,
      api_mode: 'chat_completions',
    };
  }
  if (tool === 'opencode') {
    return {
      ...base,
      opencode_api_mode: 'chat_completions',
    };
  }
  if (tool === 'hermes') {
    return {
      ...base,
      context_1m: false,
      api_mode: 'chat_completions',
    };
  }
  return base;
}

export function normalizeOpenCodeModelConfigs(
  configs: Record<string, OpenCodeModelConfig> | undefined,
): Record<string, OpenCodeModelConfig> | undefined {
  const cleaned = Object.fromEntries(
    Object.entries(configs || {})
      .map(([model, config]) => [model.trim(), config] as const)
      .filter(([model, config]) => model.length > 0 && config && Object.keys(config).length > 0),
  );
  return Object.keys(cleaned).length > 0 ? cleaned : undefined;
}

export function normalizeCodexCatalogModels(
  catalogModels: CodexCatalogModel[] | undefined,
): CodexCatalogModel[] | undefined {
  const cleaned = (catalogModels || [])
    .map((entry) => ({
      slug: entry.slug,
      display_name: entry.display_name?.trim() ? entry.display_name : undefined,
      context_window: entry.context_window && entry.context_window > 0
        ? entry.context_window
        : undefined,
      reasoning_levels: Array.from(new Set(
        (entry.reasoning_levels ?? (
          entry.supports_reasoning
            ? ['minimal', 'low', 'medium', 'high', 'xhigh']
            : []
        ))
          .map((level) => level.trim().toLowerCase())
          .filter((level) => ['minimal', 'low', 'medium', 'high', 'xhigh'].includes(level)),
      )),
      supports_images: entry.supports_images || undefined,
      supports_tool_calls: entry.supports_tool_calls || undefined,
      supports_web_search: entry.supports_web_search || undefined,
    }))
    .filter((entry) => entry.slug.trim().length > 0);
  return cleaned.length ? cleaned : undefined;
}
