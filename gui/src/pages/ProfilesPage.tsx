import { lazy, Suspense, useState, useEffect, useMemo, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { useShallow } from 'zustand/react/shallow';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { ConfirmDialog } from '../components/common/Modal';
import { Plus, Search, X, FileDown } from 'lucide-react';
import type { ApiProfile, TargetApp } from '../types';
import { SUPPORTED_TOOLS, toolById } from '../types';
import { cn, humanizeError } from '../lib/utils';
import { tauriApi } from '../lib/tauri';
import { readSwitchProbe, writeSwitchProbe } from '../lib/settings';
import { contextBadgeLabel } from '../lib/contextWindow';
import { profileApiCredentialsText } from '../lib/profileCopy';
import { copyText } from '../lib/clipboard';
import { ProfileCard } from './profiles/ProfileCard';
import {
  EmptyState,
  activeProfileFor,
  AppSelector,
  profileConfigFingerprint,
} from './profiles/helpers';

const ProfileModal = lazy(() => import('./profiles/ProfileFormModal').then((module) => ({ default: module.ProfileModal })));

export default function ProfilesPage() {
  const {
    profiles, status, loadingProfiles,
    fetchProfiles, fetchStatus, addProfile, updateProfile, deleteProfile, switchProfile,
    sharedTool, setSelectedTool,
  } = useStore(useShallow((state) => ({
    profiles: state.profiles, status: state.status, loadingProfiles: state.loadingProfiles,
    fetchProfiles: state.fetchProfiles, fetchStatus: state.fetchStatus,
    addProfile: state.addProfile, updateProfile: state.updateProfile,
    deleteProfile: state.deleteProfile, switchProfile: state.switchProfile,
    sharedTool: state.selectedTool, setSelectedTool: state.setSelectedTool,
  })));
  const targetApp = sharedTool;
  const setTargetApp = setSelectedTool;
  const navigate = useNavigate();
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<ApiProfile | null>(null);
  const [switched, setSwitched] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<{ name: string; tool: TargetApp } | null>(null);
  const [query, setQuery] = useState('');
  const [feedback, setFeedback] = useState<{ kind: 'success' | 'error' | 'info'; text: string } | null>(null);
  const [dedupConfirm, setDedupConfirm] = useState(false);
  const [deletingLegacy, setDeletingLegacy] = useState<ApiProfile | null>(null);
  const [switching, setSwitching] = useState<string | null>(null);
  const switchPending = useRef(false);
  // 每个 legacy 行的目标工具选择（默认当前页工具）
  const [legacyTool, setLegacyTool] = useState<Record<number, TargetApp>>({});
  // 启用时先探活：key 全挂则后端拒绝写入配置。偏好持久化到 localStorage。
  const [switchProbe, setSwitchProbe] = useState(() => readSwitchProbe());

  useEffect(() => {
    if (!switched) return;
    const timer = window.setTimeout(() => setSwitched(null), 1600);
    return () => window.clearTimeout(timer);
  }, [switched]);

  const selectedTool = toolById(targetApp)!;
  const activeProfile = activeProfileFor(status, targetApp);
  const claudeSeed = activeProfileFor(status, 'claude-code')
    || profiles.find((p) => p.target_app === 'claude-code');
  const normalizedQuery = query.trim().toLowerCase();
  const toolProfiles = useMemo(() => {
    return profiles.filter((p) => p.target_app === targetApp);
  }, [profiles, targetApp]);
  // 未归属档案（target_app 为空的历史遗留）：各工具页都过滤掉了，单独露出认领入口
  const legacyProfiles = useMemo(() => {
    return profiles.filter((p) => !p.target_app);
  }, [profiles]);
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
      const key = profileConfigFingerprint(p);
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
        await tauriApi.deleteProfile(p.target_app ?? targetApp, p.name);
      }
      setFeedback({
        kind: 'success',
        text: `已清理 ${dupPlan.remove.length} 个重复档案（保留：${dupPlan.keep.map((p) => p.name).join('、') || '—'}）`,
      });
    } catch (e) {
      setFeedback({ kind: 'error', text: `去重失败：${humanizeError(e)}` });
    } finally {
      await useStore.getState().refresh();
      setDedupConfirm(false);
    }
  };

  const handleSwitch = async (tool: TargetApp, name: string) => {
    if (switchPending.current) return;
    switchPending.current = true;
    const switchKey = `${tool}:${name}`;
    setSwitching(switchKey);
    setFeedback(null);
    try {
      await switchProfile(tool, name, switchProbe || undefined);
      setSwitched(switchKey);
      const toolLabel = toolById(tool)?.displayName ?? tool;
      setFeedback({
        kind: 'success',
        text: switchProbe
          ? `已探活并启用 ${name}（已写入本地 ${toolLabel} 配置）`
          : `已启用 ${name}（已写入本地 ${toolLabel} 配置）`,
      });
    } catch (error) {
      setFeedback({ kind: 'error', text: `启用失败：${humanizeError(error)}` });
    } finally {
      switchPending.current = false;
      setSwitching(null);
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

  const claimLegacy = async (p: ApiProfile) => {
    if (p.id == null) return;
    const t = legacyTool[p.id] ?? targetApp;
    setFeedback(null);
    try {
      await tauriApi.assignLegacyProfile(p.id, t);
      setFeedback({ kind: 'success', text: `已将「${p.name}」归属到 ${toolById(t)?.displayName ?? t}` });
      await fetchProfiles();
      await fetchStatus();
    } catch (e) {
      setFeedback({ kind: 'error', text: `认领失败：${humanizeError(e)}` });
    }
  };

  const claimAllLegacy = async () => {
    setFeedback(null);
    try {
      let n = 0;
      for (const p of legacyProfiles) {
        if (p.id == null) continue;
        await tauriApi.assignLegacyProfile(p.id, legacyTool[p.id] ?? targetApp);
        n += 1;
      }
      setFeedback({ kind: 'success', text: `已认领 ${n} 个档案到 ${toolById(targetApp)?.displayName ?? targetApp}（可在各行下拉框里单独改目标）` });
      await fetchProfiles();
      await fetchStatus();
    } catch (e) {
      setFeedback({ kind: 'error', text: `认领失败：${humanizeError(e)}` });
    }
  };

  const dropLegacy = async () => {
    const p = deletingLegacy;
    setDeletingLegacy(null);
    if (!p || p.id == null) return;
    setFeedback(null);
    try {
      await tauriApi.deleteLegacyProfile(p.id);
      setFeedback({ kind: 'success', text: `已删除未归属档案「${p.name}」` });
      await fetchProfiles();
      await fetchStatus();
    } catch (e) {
      setFeedback({ kind: 'error', text: `删除失败：${humanizeError(e)}` });
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
            <Button variant="secondary" onClick={() => navigate('/import')}>
              <FileDown size={15} />
              从本机扫描
            </Button>
            <Button onClick={() => { setEditing(null); setShowModal(true); }}>
              <Plus size={16} strokeWidth={2.5} />
              新建档案
            </Button>
          </div>
        }
      />

      <div className="px-4 py-4 sm:px-7 sm:py-5">
        <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
          <AppSelector value={targetApp} onChange={(tool) => { setTargetApp(tool); setFeedback(null); }} disabled={switching !== null || deleting !== null || dedupConfirm || deletingLegacy !== null} />
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex shrink-0 cursor-pointer items-center gap-1.5 text-[12px] text-ink-dim" title="启用档案前先探活 key，全部失败则不写入本地配置">
              <input
                type="checkbox"
                checked={switchProbe}
                onChange={(e) => {
                  setSwitchProbe(e.target.checked);
                  writeSwitchProbe(e.target.checked);
                }}
              />
              启用时先探活
            </label>
            <div className="relative w-full max-w-[260px]">
              <Search size={14} className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-faint" />
              <input
                value={query}
                aria-label="搜索配置档案"
                onChange={(event) => setQuery(event.target.value)}
                className="h-9 w-full rounded-md border border-line bg-card pl-8 pr-3 text-[13px] text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent/50"
                placeholder="搜索名称、模型或 URL"
              />
            </div>
          </div>
        </div>

        <div className="mb-4 flex items-center justify-between gap-3 border-y border-line px-1 py-3">
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

        {legacyProfiles.length > 0 && (
          <div className="mb-4 overflow-hidden rounded-lg border border-warn/30 bg-warn/5">
            <div className="flex flex-wrap items-center gap-2 border-b border-warn/20 px-3.5 py-2.5">
              <span className="text-[13px] font-semibold text-ink">未归属档案（{legacyProfiles.length}）</span>
              <span className="font-normal text-[11px] text-ink-faint">旧版本遗留，先认领到工具再使用</span>
              <Button size="sm" variant="secondary" onClick={claimAllLegacy} className="ml-auto">全部认领到{toolById(targetApp)?.displayName ?? targetApp}</Button>
            </div>
            {legacyProfiles.map((p) => (
              <div key={p.id ?? p.name} className="flex flex-wrap items-center gap-2 border-b border-line/60 px-3.5 py-2 last:border-b-0">
                <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink" title={p.name}>{p.name}</span>
                <span className="max-w-[220px] truncate font-mono text-[11px] text-ink-faint" title={`${p.provider} ${p.api_url}`}>
                  {p.provider} · {p.api_url}
                </span>
                <select
                  value={p.id != null ? (legacyTool[p.id] ?? targetApp) : targetApp}
                  onChange={(e) => {
                    if (p.id == null) return;
                    const t = e.target.value as TargetApp;
                    setLegacyTool((prev) => ({ ...prev, [p.id as number]: t }));
                  }}
                  className="h-8 rounded-md border border-line bg-card px-2 text-[12px] text-ink outline-none focus:border-accent/50"
                  aria-label={`「${p.name}」归属工具`}
                >
                  {SUPPORTED_TOOLS.map((t) => (
                    <option key={t.id} value={t.id}>{t.displayName}</option>
                  ))}
                </select>
                <Button size="sm" variant="secondary" onClick={() => claimLegacy(p)}>认领</Button>
                <Button size="sm" variant="ghost" onClick={() => setDeletingLegacy(p)}>删除</Button>
              </div>
            ))}
          </div>
        )}

        {feedback && (
          <div role={feedback.kind === 'error' ? 'alert' : 'status'} className={cn(
            'mb-3 rounded-md border px-3 py-2 text-[13px]',
            (feedback?.kind === 'success') ? 'border-ok/30 bg-ok/8 text-ok'
              : (feedback?.kind === 'info') ? 'border-line bg-surface text-ink-dim'
              : 'border-danger/30 bg-danger/8 text-danger',
          )}>
            <div className="flex items-start justify-between gap-2">
              <span className="min-w-0 break-words">{feedback.text}</span>
              <button type="button" className="icon-button" title="关闭提示" aria-label="关闭提示" onClick={() => setFeedback(null)}><X size={14} /></button>
            </div>
          </div>
        )}

        {loadingProfiles && profiles.length === 0 ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : profiles.length === 0 ? (
          <EmptyState />
        ) : toolProfiles.length === 0 ? (
          <EmptyState toolLabel={selectedTool.displayName} />
        ) : filteredProfiles.length === 0 ? (
          <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center text-[13px] text-ink-faint">没有匹配的档案</div>
        ) : (
          <div className="max-w-5xl border-y border-line bg-card" aria-busy={loadingProfiles}>
            {filteredProfiles.map((p) => (
              <ProfileCard
                key={p.id ?? `${p.target_app}:${p.name}`}
                profile={p}
                active={activeProfile?.name === p.name}
                justSwitched={switched === `${p.target_app ?? targetApp}:${p.name}`}
                onEdit={() => { setEditing(p); setShowModal(true); }}
                onDelete={() => setDeleting({ name: p.name, tool: p.target_app ?? targetApp })}
                onCopyCredentials={() => handleCopy('URL + Key', profileApiCredentialsText(p))}
                onSwitch={() => handleSwitch(p.target_app ?? targetApp, p.name)}
                switching={switching === `${p.target_app ?? targetApp}:${p.name}`}
                busy={switching !== null}
              />
            ))}
          </div>
        )}
      </div>

      {showModal && (
        <Suspense fallback={<div role="status" aria-label="加载表单" className="fixed inset-0 z-50 grid place-items-center bg-black/20"><Spinner size="lg" /></div>}>
        <ProfileModal
          profile={editing}
          initialTool={targetApp}
          seedFrom={claudeSeed}
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
              throw new Error(friendly);
            }
            setShowModal(false);
          }}
        />
        </Suspense>
      )}

      {deleting && (
        <ConfirmDialog
          title="删除配置档案"
          message={`确定要删除「${deleting.name}」（${toolById(deleting.tool)?.displayName ?? deleting.tool}）吗？此操作不可撤销。`}
          confirmText="删除"
          danger
          onCancel={() => setDeleting(null)}
          onConfirm={async () => {
            const target = deleting;
            try {
              await deleteProfile(target.tool, target.name);
              setFeedback({ kind: 'success', text: `已删除「${target.name}」` });
            } catch (e) {
              setFeedback({ kind: 'error', text: `删除失败：${humanizeError(e)}` });
            }
            setDeleting(null);
          }}
        />
      )}

      {deletingLegacy && (
        <ConfirmDialog
          title="删除未归属档案"
          message={`确定要删除未归属档案「${deletingLegacy.name}」吗？此操作不可撤销。`}
          confirmText="删除"
          danger
          onCancel={() => setDeletingLegacy(null)}
          onConfirm={dropLegacy}
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
