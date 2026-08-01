import { useState, useEffect, useMemo } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { ConfirmDialog } from '../components/common/Modal';
import { Plus, Search } from 'lucide-react';
import type { ApiProfile, TargetApp } from '../types';
import { toolById } from '../types';
import { cn, humanizeError } from '../lib/utils';
import { contextBadgeLabel } from '../lib/contextWindow';
import { profileApiCredentialsText } from '../lib/profileCopy';
import { copyText } from '../lib/clipboard';
import { ProfileCard } from './profiles/ProfileCard';
import { ProfileModal } from './profiles/ProfileFormModal';
import {
  EmptyState,
  activeProfileFor,
  AppSelector,
} from './profiles/helpers';

export default function ProfilesPage() {
  const {
    profiles, status, loadingProfiles, lastError, clearError,
    fetchProfiles, fetchStatus, addProfile, updateProfile, deleteProfile, switchProfile,
  } = useStore();
  const [targetApp, setTargetApp] = useState<TargetApp>('claude-code');
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<ApiProfile | null>(null);
  const [switched, setSwitched] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [feedback, setFeedback] = useState<{ kind: 'success' | 'error' | 'info'; text: string } | null>(null);
  const [dedupConfirm, setDedupConfirm] = useState(false);

  useEffect(() => {
    fetchProfiles();
    fetchStatus();
  }, [fetchProfiles, fetchStatus]);

  const selectedTool = toolById(targetApp)!;
  const activeProfile = activeProfileFor(status, targetApp);
  const normalizedQuery = query.trim().toLowerCase();
  const toolProfiles = useMemo(() => {
    return profiles.filter((p) => p.target_app === targetApp);
  }, [profiles, targetApp]);
  const filteredProfiles = useMemo(() => {
    let list = toolProfiles;
    if (normalizedQuery) {
      list = list.filter((p) => (
        p.name.toLowerCase().includes(normalizedQuery) ||
        p.provider.toLowerCase().includes(normalizedQuery) ||
        p.api_url.toLowerCase().includes(normalizedQuery) ||
        (p.model || '').toLowerCase().includes(normalizedQuery)
      ));
    }
    return list;
  }, [toolProfiles, normalizedQuery]);

  // 去重：优先保留当前启用，其次 updated_at 最新
  const dupPlan = useMemo(() => {
    const groups = new Map<string, ApiProfile[]>();
    for (const p of toolProfiles) {
      const key = `${p.target_app ?? 'shared'}::${(p.api_url || '').replace(/\/+$/, '')}::${p.api_key || ''}`;
      const arr = groups.get(key);
      if (arr) arr.push(p);
      else groups.set(key, [p]);
    }
    const keep: ApiProfile[] = [];
    const remove: ApiProfile[] = [];
    const activeName = activeProfile?.name;
    for (const arr of groups.values()) {
      if (arr.length <= 1) continue;
      const sorted = [...arr].sort((a, b) => {
        if (activeName && a.name === activeName) return -1;
        if (activeName && b.name === activeName) return 1;
        return (b.updated_at ?? 0) - (a.updated_at ?? 0);
      });
      keep.push(sorted[0]);
      remove.push(...sorted.slice(1));
    }
    return { keep, remove };
  }, [toolProfiles, activeProfile?.name]);

  const runDedup = async () => {
    setFeedback(null);
    try {
      for (const p of dupPlan.remove) {
        await deleteProfile(p.target_app ?? targetApp, p.name);
      }
      setFeedback({
        kind: 'success',
        text: `已清理 ${dupPlan.remove.length} 个重复档案（保留：${dupPlan.keep.map((p) => p.name).join('、') || '—'}）`,
      });
    } catch (e) {
      setFeedback({ kind: 'error', text: `去重失败：${humanizeError(e)}` });
    } finally {
      setDedupConfirm(false);
    }
  };

  const handleSwitch = async (name: string) => {
    setFeedback(null);
    try {
      await switchProfile(targetApp, name);
      setSwitched(`${name}→${targetApp}`);
      setFeedback({ kind: 'success', text: `已启用 ${name}（已写入本地 ${selectedTool.displayName} 配置）` });
      setTimeout(() => setSwitched(null), 1600);
    } catch (error) {
      setFeedback({ kind: 'error', text: `启用失败：${humanizeError(error)}` });
    }
  };

  const handleCopy = async (label: string, text: string) => {
    setFeedback(null);
    try {
      await copyText(text);
      setFeedback({ kind: 'success', text: `已复制${label}` });
    } catch (error) {
      setFeedback({ kind: 'error', text: `复制${label}失败：${humanizeError(error, '剪贴板不可用')}` });
    }
  };

  const activeCtx = activeProfile
    ? contextBadgeLabel(activeProfile.context_1m, activeProfile.model, { tool: targetApp })
    : null;

  return (
    <div className="min-h-full">
      <PageHeader
        title="配置档案"
        actions={
          <div className="flex items-center gap-2">
            {dupPlan.remove.length > 0 && (
              <Button variant="secondary" onClick={() => setDedupConfirm(true)}>
                去重 ({dupPlan.remove.length})
              </Button>
            )}
            <Button onClick={() => { setEditing(null); setShowModal(true); }}>
              <Plus size={16} strokeWidth={2.5} />
              新建档案
            </Button>
          </div>
        }
      />

      <div className="px-4 py-4 sm:px-7 sm:py-5">
        <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
          <AppSelector value={targetApp} onChange={setTargetApp} />
          <div className="relative w-full max-w-[260px]">
            <Search size={14} className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-faint" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="h-9 w-full rounded-md border border-line bg-card pl-8 pr-3 text-[13px] text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent/50"
              placeholder="搜索 name / model / url"
            />
          </div>
        </div>

        <div className="mb-4 flex items-center justify-between rounded-lg border border-line bg-card px-3.5 py-2.5">
          <div className="flex min-w-0 items-center gap-2.5">
            <span className="grid h-7 w-7 place-items-center rounded-md font-mono text-[10px] font-bold"
                  style={{ background: `${selectedTool.color}1f`, color: selectedTool.color }}>
              {selectedTool.short}
            </span>
            <div className="min-w-0">
              <div className="text-[13px] font-semibold text-ink">{selectedTool.displayName}</div>
              <div className="truncate font-mono text-[11px] text-ink-faint">
                {activeProfile
                  ? `${activeProfile.name} · ${activeProfile.model || '—'} · ctx ${activeCtx} · ${activeProfile.api_url}`
                  : '未设置'}
              </div>
            </div>
          </div>
          <span className={cn(
            'shrink-0 rounded-md border px-2 py-1 text-[11px] font-medium',
            activeProfile ? 'border-ok/25 bg-ok/8 text-ok' : 'border-line bg-surface text-ink-faint',
          )}>
            {activeProfile ? '当前使用' : '未启用'}
          </span>
        </div>

        {(feedback || lastError) && (
          <div className={cn(
            'mb-3 rounded-md border px-3 py-2 text-[13px]',
            (feedback?.kind === 'success') ? 'border-ok/30 bg-ok/8 text-ok'
              : (feedback?.kind === 'info') ? 'border-line bg-surface text-ink-dim'
              : 'border-danger/30 bg-danger/8 text-danger',
          )}>
            <div className="flex items-start justify-between gap-2">
              <span>{feedback?.text || lastError}</span>
              {lastError && !feedback && (
                <button type="button" className="shrink-0 text-[11px] underline" onClick={clearError}>关闭</button>
              )}
            </div>
          </div>
        )}

        {loadingProfiles ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : profiles.length === 0 ? (
          <EmptyState />
        ) : toolProfiles.length === 0 ? (
          <EmptyState toolLabel={selectedTool.displayName} />
        ) : filteredProfiles.length === 0 ? (
          <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center text-[13px] text-ink-faint">没有匹配的档案</div>
        ) : (
          <div className="max-w-5xl overflow-hidden rounded-lg border border-line bg-card">
            {filteredProfiles.map((p) => (
              <ProfileCard
                key={p.id ?? `${p.target_app}:${p.name}`}
                profile={p}
                active={activeProfile?.name === p.name}
                justSwitched={switched?.startsWith(`${p.name}→`) ?? false}
                onEdit={() => { setEditing(p); setShowModal(true); }}
                onDelete={() => setDeleting(p.name)}
                onCopyCredentials={() => handleCopy('URL + Key', profileApiCredentialsText(p))}
                onSwitch={() => handleSwitch(p.name)}
              />
            ))}
          </div>
        )}
      </div>

      {showModal && (
        <ProfileModal
          profile={editing}
          initialTool={targetApp}
          onClose={() => setShowModal(false)}
          onSave={async (p) => {
            try {
              const wasActive = !!(editing && activeProfile && editing.name === activeProfile.name);
              if (editing) await updateProfile(p);
              else await addProfile(p);
              if (editing && wasActive) {
                setFeedback({
                  kind: 'success',
                  text: `已保存「${p.name}」并 re-apply 到本地 ${selectedTool.displayName}`,
                });
              } else if (editing) {
                setFeedback({
                  kind: 'info',
                  text: `已保存「${p.name}」到 Helio；未启用，不会改本地 ${selectedTool.displayName}（点启用才写入）`,
                });
              } else {
                setFeedback({
                  kind: 'info',
                  text: `已创建「${p.name}」；点「启用」才会写入本地 ${selectedTool.displayName} 配置`,
                });
              }
            } catch (e) {
              const msg = humanizeError(e);
              const friendly = /UNIQUE constraint failed/i.test(String(e))
                ? `已存在同名档案「${p.name}」，请换个名字`
                : `保存失败：${msg}`;
              setFeedback({ kind: 'error', text: friendly });
              return;
            }
            setShowModal(false);
          }}
        />
      )}

      {deleting && (
        <ConfirmDialog
          title="删除配置档案"
          message={`确定要删除「${deleting}」吗？此操作不可撤销。`}
          confirmText="删除"
          danger
          onCancel={() => setDeleting(null)}
          onConfirm={async () => {
            try {
              await deleteProfile(targetApp, deleting);
              setFeedback({ kind: 'success', text: `已删除「${deleting}」` });
            } catch (e) {
              setFeedback({ kind: 'error', text: `删除失败：${humanizeError(e)}` });
            }
            setDeleting(null);
          }}
        />
      )}

      {dedupConfirm && (
        <ConfirmDialog
          title="清理重复档案"
          message={`将删除 ${dupPlan.remove.length} 个重复档案：${dupPlan.remove.map((p) => p.name).join('、')}。保留：${dupPlan.keep.map((p) => p.name).join('、') || '—'}（优先保留当前启用）。不可撤销。`}
          confirmText={`删除 ${dupPlan.remove.length} 个`}
          danger
          onCancel={() => setDedupConfirm(false)}
          onConfirm={runDedup}
        />
      )}
    </div>
  );
}
