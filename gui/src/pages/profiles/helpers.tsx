import type { ApiProfile, StatusInfo, TargetApp } from '../../types';
import { SUPPORTED_TOOLS } from '../../types';
import { PROVIDER_PRESETS } from '../../lib/presets';
import { cn } from '../../lib/utils';
import { Layers } from 'lucide-react';
import React from 'react';

export function EmptyState() {
  return (
    <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center">
      <Layers size={20} className="mx-auto mb-2 text-ink-faint" />
      <p className="text-[13px] text-ink-faint">暂无配置档案</p>
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
  return '#4B5563';
}

export function humanizeCopyError(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  return raw.replace(/^\s*(TypeError|Error):\s*/i, '').trim() || '剪贴板不可用';
}

export function activeProfileFor(status: StatusInfo | null, targetApp: TargetApp): ApiProfile | undefined {
  if (!status) return undefined;
  const key = targetApp.replace('-', '_') as keyof StatusInfo;
  const targetStatus = status[key];
  if (!targetStatus || !('profile' in targetStatus)) return undefined;
  return targetStatus.profile;
}


export function emptyProfileForTool(tool: TargetApp): ApiProfile {
  const preset = PROVIDER_PRESETS[tool]?.[0];
  return {
    name: '',
    provider: preset?.provider ?? 'anthropic',
    api_url: preset?.api_url ?? '',
    api_key: '',
    model: preset?.model,
    target_app: tool,
  };
}
