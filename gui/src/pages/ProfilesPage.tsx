import { useState, useRef, useEffect } from 'react';import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { Modal, Field } from '../components/common/Modal';
import { Plus, Pencil, Trash2, ArrowLeftRight, Check, Layers } from 'lucide-react';
import type { ApiProfile, TargetApp } from '../types';
import { SUPPORTED_TOOLS } from '../types';
import { maskApiKey } from '../lib/utils';

export default function ProfilesPage() {
  const { profiles, loadingProfiles, fetchProfiles, addProfile, updateProfile, deleteProfile, switchProfile } = useStore();
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<ApiProfile | null>(null);
  const [switched, setSwitched] = useState<string | null>(null);

  useEffect(() => { fetchProfiles(); }, [fetchProfiles]);

  const handleSwitch = async (app: TargetApp, name: string) => {
    await switchProfile(app, name);
    setSwitched(`${name}→${app}`);
    setTimeout(() => setSwitched(null), 1600);
  };

  return (
    <div className="min-h-full">
      <PageHeader
        title="API Profiles"
        subtitle="切换时只替换 API URL 与 Key，其余共享配置（permissions / hooks / MCP / skills）完整保留"
        actions={
          <Button onClick={() => { setEditing(null); setShowModal(true); }}>
            <Plus size={16} strokeWidth={2.5} />
            New Profile
          </Button>
        }
      />

      <div className="px-8 py-6">
        {loadingProfiles ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : profiles.length === 0 ? (
          <EmptyState onAdd={() => { setEditing(null); setShowModal(true); }} />
        ) : (
          <div className="space-y-3 max-w-4xl">
            {profiles.map((p, i) => (
              <ProfileCard
                key={p.name}
                profile={p}
                index={i}
                justSwitched={switched?.startsWith(`${p.name}→`) ?? false}
                onEdit={() => { setEditing(p); setShowModal(true); }}
                onDelete={async () => {
                  if (confirm(`删除 Profile "${p.name}"？`)) await deleteProfile(p.name);
                }}
                onSwitch={handleSwitch}
              />
            ))}
          </div>
        )}
      </div>

      {showModal && (
        <ProfileModal
          profile={editing}
          onClose={() => setShowModal(false)}
          onSave={async (p) => {
            if (editing) await updateProfile(p); else await addProfile(p);
            setShowModal(false);
          }}
        />
      )}
    </div>
  );
}

function EmptyState({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="max-w-4xl rounded-2xl border border-dashed border-line bg-surface/40 py-20 text-center">
      <div className="mx-auto grid place-items-center h-14 w-14 rounded-2xl bg-elevated border border-line mb-5">
        <Layers size={24} className="text-ink-faint" />
      </div>
      <p className="text-ink font-medium">还没有任何 Profile</p>
      <p className="mt-1.5 text-[13px] text-ink-dim">添加一个 API Profile 开始管理多工具切换</p>
      <Button onClick={onAdd} className="mt-6 mx-auto"><Plus size={16} strokeWidth={2.5} />New Profile</Button>
    </div>
  );
}

function ProfileCard({
  profile, index, justSwitched, onEdit, onDelete, onSwitch,
}: {
  profile: ApiProfile;
  index: number;
  justSwitched: boolean;
  onEdit: () => void;
  onDelete: () => void;
  onSwitch: (app: TargetApp, name: string) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener('mousedown', onDoc);
    return () => document.removeEventListener('mousedown', onDoc);
  }, []);

  // derive an icon mark + tint from the profile's provider, fallback to neutral
  const tint = providerTint(profile.provider);

  return (
    <div
      className="group relative overflow-hidden rounded-xl border border-line bg-card px-4 py-3.5 transition-all duration-300 hover:border-line-strong hover:bg-elevated/40 animate-fade-up"
      style={{ animationDelay: `${index * 45}ms` }}
    >
      {/* success flash overlay */}
      <div className={`pointer-events-none absolute inset-0 bg-gradient-to-r from-ok/10 to-transparent transition-opacity duration-500 ${justSwitched ? 'opacity-100' : 'opacity-0'}`} />

      <div className="relative flex items-center gap-4">
        {/* icon tile */}
        <div
          className="grid place-items-center h-11 w-11 shrink-0 rounded-xl border border-line font-mono text-[13px] font-bold transition-transform duration-300 group-hover:scale-105"
          style={{ background: `${tint}1a`, color: tint, borderColor: `${tint}33` }}
        >
          {profile.name.slice(0, 2).toUpperCase()}
        </div>

        {/* info */}
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h3 className="truncate text-[15px] font-semibold text-ink">{profile.name}</h3>
            <span className="rounded-md border border-line bg-surface px-1.5 py-0.5 text-[10px] font-medium text-ink-dim">
              {profile.provider}
            </span>
          </div>
          <div className="mt-1 flex items-center gap-3 text-[12px] text-ink-dim">
            <span className="truncate font-mono">{profile.api_url}</span>
            <span className="shrink-0 font-mono text-ink-faint">{maskApiKey(profile.api_key)}</span>
          </div>
        </div>

        {/* actions: hidden until hover */}
        <div className="flex items-center gap-1.5 opacity-0 pointer-events-none transition-opacity duration-200 group-hover:opacity-100 group-hover:pointer-events-auto">
          <IconBtn label="编辑" onClick={onEdit}><Pencil size={15} /></IconBtn>
          <IconBtn label="删除" danger onClick={onDelete}><Trash2 size={15} /></IconBtn>
        </div>

        {/* switch dropdown — always visible */}
        <div className="relative" ref={ref}>
          <button
            onClick={() => setMenuOpen((v) => !v)}
            className="no-drag flex items-center gap-1.5 rounded-lg border border-line bg-surface px-3 py-2 text-[13px] font-medium text-ink-dim transition-all hover:border-accent/50 hover:text-ink"
          >
            <ArrowLeftRight size={14} />
            Switch
          </button>
          {menuOpen && (
            <div className="absolute right-0 top-full z-20 mt-1.5 w-48 overflow-hidden rounded-xl border border-line bg-card shadow-card animate-fade-up">
              <div className="px-3 py-2 text-[11px] font-medium text-ink-faint border-b border-line/70">应用到工具</div>
              {SUPPORTED_TOOLS.map((tool) => (
                <button
                  key={tool.id}
                  onClick={() => { onSwitch(tool.id, profile.name); setMenuOpen(false); }}
                  className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left text-[13px] text-ink-dim transition-colors hover:bg-elevated hover:text-ink"
                >
                  <span className="grid place-items-center h-6 w-6 rounded-md font-mono text-[10px] font-bold"
                        style={{ background: `${tool.color}22`, color: tool.color }}>
                    {tool.short}
                  </span>
                  <span className="flex-1">{tool.displayName}</span>
                  <span className="font-mono text-[10px] text-ink-faint">{tool.format}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      {justSwitched && (
        <div className="relative mt-2 flex items-center gap-1.5 text-[12px] font-medium text-ok">
          <Check size={13} /> 已切换
        </div>
      )}
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
  if (p.includes('anthropic')) return '#D97757';
  if (p.includes('openai')) return '#10B981';
  if (p.includes('google')) return '#4F8DF6';
  return '#A78BFA';
}

function ProfileModal({
  profile, onClose, onSave,
}: {
  profile: ApiProfile | null;
  onClose: () => void;
  onSave: (p: ApiProfile) => void;
}) {
  const [form, setForm] = useState<ApiProfile>(
    profile || { name: '', provider: 'anthropic', api_url: 'https://api.anthropic.com', api_key: '' },
  );
  return (
    <Modal title={profile ? '编辑 Profile' : '新建 Profile'} onClose={onClose}>
      <form
        onSubmit={(e) => { e.preventDefault(); onSave(form); }}
        className="space-y-4"
      >
        <Field label="名称" value={form.name} disabled={!!profile} required
               onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="my-proxy" />
        <Field label="Provider" value={form.provider} required
               onChange={(e) => setForm({ ...form, provider: e.target.value })} placeholder="anthropic / openai / google" />
        <Field label="API URL" type="url" value={form.api_url} required mono
               onChange={(e) => setForm({ ...form, api_url: e.target.value })} />
        <Field label="API Key" type="password" value={form.api_key} required mono
               onChange={(e) => setForm({ ...form, api_key: e.target.value })} placeholder="sk-..." />
        <div className="flex justify-end gap-2.5 pt-2">
          <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
          <Button type="submit">{profile ? '保存' : '创建'}</Button>
        </div>
      </form>
    </Modal>
  );
}
