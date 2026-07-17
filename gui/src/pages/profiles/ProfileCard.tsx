import { useState } from 'react';
import type { ApiProfile, TargetApp } from '../../types';
import { Button } from '../../components/common/Button';
import { cn, maskApiKey } from '../../lib/utils';
import {
  contextBadgeLabel,
  profileKeyCount,
  activeKeyLabel,
} from '../../lib/contextWindow';
import { Pencil, Trash2, Check, Eye, EyeOff, Link2 } from 'lucide-react';
import { IconBtn, providerTint } from './helpers';

export function ProfileCard({
  profile, active, justSwitched, onEdit, onDelete, onCopyCredentials, onSwitch,
}: {
  profile: ApiProfile;
  active: boolean;
  justSwitched: boolean;
  onEdit: () => void;
  onDelete: () => void;
  onCopyCredentials: () => void;
  onSwitch: () => void;
}) {
  const tint = providerTint(profile.provider);
  const [keyRevealed, setKeyRevealed] = useState(false);
  const tool = (profile.target_app || 'claude-code') as TargetApp;
  const ctx = contextBadgeLabel(profile.context_1m, profile.model, { tool });
  const keyN = profileKeyCount(profile);
  const keyLabel = activeKeyLabel(profile);

  return (
    <div
      className={cn(
        'group relative border-b border-line px-3.5 py-3 transition-colors duration-150 last:border-b-0 hover:bg-elevated/45',
        active && 'bg-accent/5',
      )}
    >
      <div className="relative flex items-center gap-3">
        <div
          className="grid h-9 w-9 shrink-0 place-items-center rounded-md border font-mono text-[12px] font-bold"
          style={{ background: `${tint}1a`, color: tint, borderColor: `${tint}33` }}
        >
          {profile.name.slice(0, 2).toUpperCase()}
        </div>

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate text-[14px] font-semibold text-ink">{profile.name}</h3>
            <span className="rounded border border-line bg-surface px-1.5 py-0.5 text-[10px] font-medium text-ink-dim">
              {profile.provider}
            </span>
            {profile.model && (
              <span className="max-w-[160px] truncate rounded border border-line bg-surface px-1.5 py-0.5 font-mono text-[10px] text-ink-dim" title={profile.model}>
                {profile.model}
              </span>
            )}
            <span
              className={cn(
                'rounded border px-1.5 py-0.5 text-[10px] font-medium',
                ctx === '1M' ? 'border-accent/30 bg-accent/8 text-accent' : 'border-line bg-surface text-ink-dim',
              )}
              title="上下文窗口"
            >
              ctx {ctx}
            </span>
            {profile.api_mode && (tool === 'hermes' || tool === 'openclaw') && (
              <span className="rounded border border-line bg-surface px-1.5 py-0.5 font-mono text-[10px] text-ink-faint">
                {profile.api_mode}
              </span>
            )}
            {keyN > 1 && (
              <span className="rounded border border-line bg-surface px-1.5 py-0.5 text-[10px] text-ink-faint">
                keys {keyN}{keyLabel ? `·${keyLabel}` : ''}
              </span>
            )}
            {(active || justSwitched) && (
              <span className="inline-flex items-center gap-1 text-[11px] font-medium text-ok">
                <Check size={12} />{active ? '当前使用' : '已切换'}
              </span>
            )}
          </div>
          <div className="mt-1 flex min-w-0 items-center gap-3 text-[12px] text-ink-dim">
            <span className="min-w-0 flex-1 truncate font-mono">{profile.api_url}</span>
            <span className="inline-flex shrink-0 items-center gap-1 font-mono text-ink-faint">
              <span className="max-w-[160px] truncate">{keyRevealed ? profile.api_key : maskApiKey(profile.api_key)}</span>
              {profile.api_key && (
                <button
                  type="button"
                  onClick={() => setKeyRevealed((v) => !v)}
                  aria-label={keyRevealed ? '隐藏 Key' : '显示 Key'}
                  className="grid h-5 w-5 place-items-center rounded text-ink-faint hover:text-ink hover:bg-elevated"
                >
                  {keyRevealed ? <EyeOff size={13} /> : <Eye size={13} />}
                </button>
              )}
            </span>
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          <IconBtn label="编辑" onClick={onEdit}><Pencil size={15} /></IconBtn>
          <IconBtn label="复制 URL + Key" onClick={onCopyCredentials}><Link2 size={15} /></IconBtn>
          <IconBtn label="删除" danger onClick={onDelete}><Trash2 size={15} /></IconBtn>
          {active ? (
            <span className="ml-1 rounded-md border border-ok/25 bg-ok/8 px-2.5 py-1.5 text-[12px] font-medium text-ok">当前</span>
          ) : (
            <Button size="sm" variant="secondary" onClick={onSwitch}>启用</Button>
          )}
        </div>
      </div>
    </div>
  );
}
