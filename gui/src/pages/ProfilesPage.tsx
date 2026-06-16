import { useState, useEffect, useMemo } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { Modal, Field, ConfirmDialog } from '../components/common/Modal';
import { Plus, Pencil, Trash2, Check, Layers, Search, Copy, Eye, EyeOff } from 'lucide-react';
import type { ApiProfile, FetchedModel, StatusInfo, TargetApp } from '../types';
import { SUPPORTED_TOOLS, toolById } from '../types';
import { cn, maskApiKey } from '../lib/utils';
import { PROVIDER_PRESETS, REASONING_LEVELS } from '../lib/presets';
import { nextCopyName } from '../lib/profileNames';

export default function ProfilesPage() {
  const { profiles, status, loadingProfiles, fetchProfiles, fetchStatus, addProfile, updateProfile, deleteProfile, switchProfile } = useStore();
  const [targetApp, setTargetApp] = useState<TargetApp>('claude-code');
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<ApiProfile | null>(null);
  const [switched, setSwitched] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [feedback, setFeedback] = useState<{ kind: 'success' | 'error'; text: string } | null>(null);
  const [dedupConfirm, setDedupConfirm] = useState(false);

  useEffect(() => {
    fetchProfiles();
    fetchStatus();
  }, [fetchProfiles, fetchStatus]);

  const selectedTool = toolById(targetApp)!;
  const activeProfile = activeProfileFor(status, targetApp);
  const normalizedQuery = query.trim().toLowerCase();
  const toolProfiles = useMemo(() => {
    // 只看当前工具绑定的档案（每条档案必须明确归属某工具，无"通用"）
    return profiles.filter((p) => p.target_app === targetApp);
  }, [profiles, targetApp]);
  const filteredProfiles = useMemo(() => {
    let list = toolProfiles;
    if (normalizedQuery) {
      list = list.filter((p) => (
        p.name.toLowerCase().includes(normalizedQuery) ||
        p.provider.toLowerCase().includes(normalizedQuery) ||
        p.api_url.toLowerCase().includes(normalizedQuery)
      ));
    }
    return list;
  }, [toolProfiles, normalizedQuery]);

  // 重复检测：只在当前工具可见范围内，按 target_app + api_url + api_key 分组
  const dupExtras = useMemo(() => {
    const groups = new Map<string, ApiProfile[]>();
    for (const p of toolProfiles) {
      const key = `${p.target_app ?? 'shared'}::${(p.api_url || '').replace(/\/+$/, '')}::${p.api_key || ''}`;
      const arr = groups.get(key);
      if (arr) arr.push(p);
      else groups.set(key, [p]);
    }
    const extras: ApiProfile[] = [];
    for (const arr of groups.values()) {
      if (arr.length > 1) extras.push(...arr.slice(1));
    }
    return extras;
  }, [toolProfiles]);

  const runDedup = async () => {
    setFeedback(null);
    try {
      for (const p of dupExtras) {
        await deleteProfile(p.name);
      }
      setFeedback({ kind: 'success', text: `已清理 ${dupExtras.length} 个重复档案` });
    } catch (e) {
      setFeedback({ kind: 'error', text: `去重失败：${e}` });
    } finally {
      setDedupConfirm(false);
    }
  };

  const handleSwitch = async (name: string) => {
    setFeedback(null);
    try {
      await switchProfile(targetApp, name);
      setSwitched(`${name}→${targetApp}`);
      setFeedback({ kind: 'success', text: `已启用 ${name}` });
      setTimeout(() => setSwitched(null), 1600);
    } catch (error) {
      setFeedback({ kind: 'error', text: `启用失败：${error}` });
    }
  };

  const handleDuplicate = async (profile: ApiProfile) => {
    const copy: ApiProfile = {
      ...profile,
      id: undefined,
      name: nextCopyName(profile.name, profiles.map((item) => item.name)),
      created_at: undefined,
      updated_at: undefined,
    };

    setFeedback(null);
    try {
      await addProfile(copy);
      setEditing(copy);
      setShowModal(true);
      setFeedback({ kind: 'success', text: `已复制为 ${copy.name}` });
    } catch (error) {
      setFeedback({ kind: 'error', text: `复制失败：${error}` });
    }
  };

  return (
    <div className="min-h-full">
      <PageHeader
        title="配置档案"
        actions={
          <div className="flex items-center gap-2">
            {dupExtras.length > 0 && (
              <Button variant="secondary" onClick={() => setDedupConfirm(true)}>
                去重 ({dupExtras.length})
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
              placeholder="搜索"
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
                {activeProfile ? `${activeProfile.name} · ${activeProfile.api_url}` : '未设置'}
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

        {feedback && (
          <div className={cn(
            'mb-3 rounded-md border px-3 py-2 text-[13px]',
            feedback.kind === 'success' ? 'border-ok/30 bg-ok/8 text-ok' : 'border-danger/30 bg-danger/8 text-danger',
          )}>
            {feedback.text}
          </div>
        )}

        {loadingProfiles ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : profiles.length === 0 ? (
          <EmptyState />
        ) : filteredProfiles.length === 0 ? (
          <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center text-[13px] text-ink-faint">没有匹配的档案</div>
        ) : (
          <div className="max-w-5xl overflow-hidden rounded-lg border border-line bg-card">
            {filteredProfiles.map((p) => (
              <ProfileCard
                key={p.name}
                profile={p}
                active={activeProfile?.name === p.name}
                justSwitched={switched?.startsWith(`${p.name}→`) ?? false}
                onEdit={() => { setEditing(p); setShowModal(true); }}
                onDelete={() => setDeleting(p.name)}
                onDuplicate={() => handleDuplicate(p)}
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
            if (editing) await updateProfile(p); else await addProfile(p);
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
              await deleteProfile(deleting);
            } catch (e) {
              alert('删除失败：' + e);
            }
            setDeleting(null);
          }}
        />
      )}

      {dedupConfirm && (
        <ConfirmDialog
          title="清理重复档案"
          message={`将删除 ${dupExtras.length} 个当前工具内的重复档案（同一工具下 API URL + Key 相同、名字不同）：${dupExtras.map((p) => p.name).join('、')}。每组保留首个，不可撤销。`}
          confirmText={`删除 ${dupExtras.length} 个`}
          danger
          onCancel={() => setDedupConfirm(false)}
          onConfirm={runDedup}
        />
      )}
    </div>
  );
}

function EmptyState() {
  return (
    <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center">
      <Layers size={20} className="mx-auto mb-2 text-ink-faint" />
      <p className="text-[13px] text-ink-faint">暂无配置档案</p>
    </div>
  );
}

function ProfileCard({
  profile, active, justSwitched, onEdit, onDelete, onDuplicate, onSwitch,
}: {
  profile: ApiProfile;
  active: boolean;
  justSwitched: boolean;
  onEdit: () => void;
  onDelete: () => void;
  onDuplicate: () => void;
  onSwitch: () => void;
}) {
  const tint = providerTint(profile.provider);
  const [keyRevealed, setKeyRevealed] = useState(false);

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
          <div className="flex items-center gap-2">
            <h3 className="truncate text-[14px] font-semibold text-ink">{profile.name}</h3>
            <span className="rounded border border-line bg-surface px-1.5 py-0.5 text-[10px] font-medium text-ink-dim">
              {profile.provider}
            </span>
            {(active || justSwitched) && (
              <span className="inline-flex items-center gap-1 text-[11px] font-medium text-ok">
                <Check size={12} />{active ? '当前使用' : '已切换'}
              </span>
            )}
          </div>
          <div className="mt-1 flex items-center gap-3 text-[12px] text-ink-dim">
            <span className="truncate font-mono">{profile.api_url}</span>
            <span className="inline-flex shrink-0 items-center gap-1 font-mono text-ink-faint">
              <span className="break-all">{keyRevealed ? profile.api_key : maskApiKey(profile.api_key)}</span>
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

        <div className="flex items-center gap-1.5">
          <IconBtn label="编辑" onClick={onEdit}><Pencil size={15} /></IconBtn>
          <IconBtn label="复制" onClick={onDuplicate}><Copy size={15} /></IconBtn>
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

function AppSelector({ value, onChange }: { value: TargetApp; onChange: (value: TargetApp) => void }) {
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

function IconBtn({ children, label, danger, onClick }: { children: React.ReactNode; label: string; danger?: boolean; onClick: () => void }) {
  return (
    <button
      title={label}
      onClick={onClick}
      className={`no-drag grid place-items-center h-8 w-8 rounded-lg border border-line bg-surface transition-all hover:bg-elevated ${
        danger ? 'text-ink-faint hover:text-danger hover:border-danger/40' : 'text-ink-faint hover:text-ink'
      }`}
    >
      {children}
    </button>
  );
}

function providerTint(provider: string): string {
  const p = provider.toLowerCase();
  if (p.includes('anthropic')) return '#8A5A44';
  if (p.includes('openai')) return '#10B981';
  if (p.includes('google')) return '#4F8DF6';
  return '#4B5563';
}

function activeProfileFor(status: StatusInfo | null, targetApp: TargetApp): ApiProfile | undefined {
  if (!status) return undefined;
  const key = targetApp.replace('-', '_') as keyof StatusInfo;
  const targetStatus = status[key];
  if (!targetStatus || !('profile' in targetStatus)) return undefined;
  return targetStatus.profile;
}

function ProfileModal({
  profile, initialTool, onClose, onSave,
}: {
  profile: ApiProfile | null;
  initialTool: TargetApp;
  onClose: () => void;
  onSave: (p: ApiProfile) => void;
}) {
  const initialModalTool = profile?.target_app ?? initialTool;
  const [tool, setTool] = useState<TargetApp>(initialModalTool);
  const [form, setForm] = useState<ApiProfile>(
    profile || emptyProfileForTool(initialModalTool),
  );
  const [models, setModels] = useState<FetchedModel[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelErr, setModelErr] = useState('');

  const loadModels = async () => {
    if (!form.api_url.trim() || !form.api_key.trim()) {
      setModelErr('先填 API URL 和 API Key');
      return;
    }
    setLoadingModels(true);
    setModelErr('');
    try {
      const { tauriApi } = await import('../lib/tauri');
      const list = await tauriApi.fetchModels(form.api_url, form.api_key);
      setModels(list);
      if (list.length === 0) setModelErr('该端点没有返回模型');
    } catch (e) {
      setModelErr(String(e));
      setModels([]);
    } finally {
      setLoadingModels(false);
    }
  };

  const presets = PROVIDER_PRESETS[tool];
  const showModelParams = tool === 'codex' || tool === 'claude-code' || tool === 'opencode';

  const applyPreset = (p: typeof presets[number]) => {
    setForm((f) => ({
      ...f,
      provider: p.provider,
      api_url: p.api_url || f.api_url,
      model: p.model ?? f.model,
    }));
  };

  return (
    <Modal
      title={profile ? '编辑配置档案' : '新建配置档案'}
      onClose={onClose}
      size="lg"
      footer={
        <>
          <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
          <Button type="submit" form="helio-profile-form">{profile ? '保存' : '创建'}</Button>
        </>
      }
    >
      <form
        id="helio-profile-form"
        onSubmit={(e) => { e.preventDefault(); onSave({ ...form, target_app: tool }); }}
        className="space-y-4"
      >
          {!profile && (
            <div>
              <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">目标工具</span>
              <div className="flex flex-wrap gap-1.5">
                {SUPPORTED_TOOLS.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    onClick={() => setTool(t.id)}
                    className={`whitespace-nowrap rounded-md px-3 py-1.5 text-[12.5px] font-medium border transition-all ${
                      tool === t.id ? 'border-accent text-accent bg-accent/8' : 'border-line text-ink-dim hover:border-line-strong'
                    }`}
                  >
                    {t.displayName}
                  </button>
                ))}
              </div>
            </div>
          )}

          {!profile && (
            <div>
              <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">Provider 预设</span>
              <div className="flex flex-wrap gap-1.5">
                {presets.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    onClick={() => applyPreset(p)}
                    className="whitespace-nowrap rounded-md border border-line px-3 py-1.5 text-[12.5px] text-ink-dim transition-all hover:border-accent hover:text-accent"
                  >
                    {p.label}
                  </button>
                ))}
              </div>
            </div>
          )}

          <Field label="名称" value={form.name} required
                 onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="my-proxy" />
          <Field label="Provider" value={form.provider} required
                 onChange={(e) => setForm({ ...form, provider: e.target.value })} placeholder="anthropic / openai / google" />
          <Field label="API URL" type="url" value={form.api_url} required mono
                 onChange={(e) => setForm({ ...form, api_url: e.target.value })} />
          <Field label="API Key" type="password" value={form.api_key} required mono
                 onChange={(e) => setForm({ ...form, api_key: e.target.value })} placeholder="sk-..." />

          {showModelParams && (
            <div className="rounded-lg border border-line bg-surface/60 p-3.5 space-y-3.5">
              <div className="text-[12px] font-medium text-ink-dim">模型参数</div>
              <Field label="默认模型" value={form.model || ''} mono
                     onChange={(e) => setForm({ ...form, model: e.target.value })} placeholder="gpt-5.5 / claude-opus-4" />
              <div>
                <div className="flex items-center gap-2">
                  <Button type="button" variant="secondary" size="sm" onClick={loadModels} disabled={loadingModels}>
                    {loadingModels ? '加载中…' : '加载模型列表'}
                  </Button>
                  {models.length > 0 && (
                    <select
                      value={form.model || ''}
                      onChange={(e) => setForm({ ...form, model: e.target.value })}
                      className="h-8 flex-1 rounded-md border border-line bg-card px-2 text-[12.5px] text-ink outline-none focus:border-accent/50"
                    >
                      <option value="">（选择模型）</option>
                      {models.map((m) => (
                        <option key={m.id} value={m.id}>{m.id}</option>
                      ))}
                    </select>
                  )}
                </div>
                {modelErr && <div className="mt-1 text-[11px] text-danger">{modelErr}</div>}
                {models.length > 0 && <div className="mt-1 text-[11px] text-ink-faint">已加载 {models.length} 个模型；也可在上方输入框自定义</div>}
              </div>

              {tool === 'opencode' && models.length > 0 && (
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between">
                    <span className="text-[12px] font-medium text-ink-dim">
                      provider 模型 <span className="font-normal text-ink-faint">（勾选要挂载到 OpenCode 的模型）</span>
                    </span>
                    <span className="text-[11px] text-ink-faint">已选 {(form.models || []).length}</span>
                  </div>
                  <div className="max-h-40 overflow-y-auto rounded-md border border-line bg-card p-1.5">
                    {models.map((m) => {
                      const selected = (form.models || []).includes(m.id);
                      const toggle = () => {
                        const cur = form.models || [];
                        setForm({
                          ...form,
                          models: selected ? cur.filter((x) => x !== m.id) : [...cur, m.id],
                        });
                      };
                      return (
                        <label key={m.id} className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 hover:bg-elevated/60">
                          <input type="checkbox" checked={selected} onChange={toggle} />
                          <span className="truncate font-mono text-[12px] text-ink">{m.id}</span>
                        </label>
                      );
                    })}
                  </div>
                </div>
              )}

              {tool === 'claude-code' && (
                <div className="space-y-2">
                  <div className="text-[12px] font-medium text-ink-dim">
                    模型角色映射 <span className="font-normal text-ink-faint">（Sonnet/Opus/Haiku → 实际模型；写入 ANTHROPIC_DEFAULT_*_MODEL）</span>
                  </div>
                  {(['sonnet', 'opus', 'haiku'] as const).map((role) => {
                    const labels: Record<string, string> = { sonnet: 'Sonnet', opus: 'Opus', haiku: 'Haiku' };
                    const mm = form.model_mapping || {};
                    const setMM = (k: string, v: string) => setForm({ ...form, model_mapping: { ...mm, [k]: v } });
                    return (
                      <div key={role} className="flex items-center gap-2">
                        <span className="w-14 shrink-0 text-[12px] font-medium text-ink">{labels[role]}</span>
                        <input
                          list={`helio-models-${role}`}
                          value={mm[`${role}_model`] || ''}
                          onChange={(e) => setMM(`${role}_model`, e.target.value)}
                          placeholder="实际模型"
                          className="h-8 flex-1 rounded-md border border-line bg-card px-2 font-mono text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <input
                          value={mm[`${role}_name`] || ''}
                          onChange={(e) => setMM(`${role}_name`, e.target.value)}
                          placeholder="显示名"
                          className="h-8 w-28 rounded-md border border-line bg-card px-2 text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <label className="flex shrink-0 items-center gap-1 text-[11px] text-ink-dim">
                          <input
                            type="checkbox"
                            checked={mm[`${role}_one_m`] === 'true'}
                            onChange={(e) => setMM(`${role}_one_m`, e.target.checked ? 'true' : '')}
                          />
                          1M
                        </label>
                        <datalist id={`helio-models-${role}`}>
                          {models.map((m) => <option key={m.id} value={m.id} />)}
                        </datalist>
                      </div>
                    );
                  })}
                </div>
              )}

              {tool === 'codex' && (
                <div>
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">推理强度</span>
                  <div className="flex gap-1.5">
                    {REASONING_LEVELS.map((r) => (
                      <button
                        key={r.value}
                        type="button"
                        onClick={() => setForm({ ...form, reasoning_effort: r.value || undefined })}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          (form.reasoning_effort || '') === r.value
                            ? 'border-accent text-accent bg-accent/8'
                            : 'border-line text-ink-dim hover:border-line-strong'
                        }`}
                      >
                        {r.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {(tool === 'claude-code' || tool === 'codex') && (
                <label className="flex cursor-pointer items-center justify-between">
                  <div>
                    <div className="text-[13px] font-medium text-ink">1M 上下文窗口</div>
                    <div className="text-[11px] text-ink-faint">model_context_window</div>
                  </div>
                  <button
                    type="button"
                    onClick={() => setForm({ ...form, context_1m: !form.context_1m })}
                    className={`relative h-6 w-11 rounded-full transition-colors ${form.context_1m ? 'bg-accent' : 'bg-line-strong'}`}
                  >
                    <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-soft transition-transform ${form.context_1m ? 'translate-x-[22px]' : 'translate-x-0.5'}`} />
                  </button>
                </label>
              )}
            </div>
          )}
      </form>
    </Modal>
  );
}

function emptyProfileForTool(tool: TargetApp): ApiProfile {
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
