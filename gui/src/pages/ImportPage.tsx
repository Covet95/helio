import { useState } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { Field } from '../components/common/Modal';
import {
  Search, FileDown, KeyRound, Boxes, Sparkles, Webhook, ShieldCheck, Check, FileWarning,
} from 'lucide-react';
import { SUPPORTED_TOOLS } from '../types';
import type { TargetApp } from '../types';
import type { CcSwitchProvider } from '../lib/tauri';
import { cn, maskApiKey } from '../lib/utils';

interface Scanned {
  found: boolean;
  api_url: string;
  api_key: string;
  provider: string;
  model?: string;
  reasoning_effort?: string;
  context_1m?: boolean;
  source: string;
}
interface LocalInfo {
  mcp_servers: Record<string, any>;
  skills: string[];
  hooks: any;
  permissions: any;
}
type Feedback = { text: string; kind: 'success' | 'error' | 'info' };

export default function ImportPage() {
  const { addProfile } = useStore();
  const [tool, setTool] = useState<TargetApp>('claude-code');
  const [scanning, setScanning] = useState(false);
  const [api, setApi] = useState<Scanned | null>(null);
  const [info, setInfo] = useState<LocalInfo | null>(null);
  const [name, setName] = useState('');
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [importedShared, setImportedShared] = useState(false);
  const [ccProviders, setCcProviders] = useState<CcSwitchProvider[] | null>(null);
  const [ccScanning, setCcScanning] = useState(false);
  const [ccSelected, setCcSelected] = useState<Set<number>>(new Set());

  const meta = SUPPORTED_TOOLS.find((t) => t.id === tool)!;

  const scanCc = async () => {
    setCcScanning(true); setFeedback(null); setCcProviders(null); setCcSelected(new Set());
    try {
      const { tauriApi } = await import('../lib/tauri');
      const appType = tool === 'claude-code' ? 'claude' : tool;
      const list = await tauriApi.scanCcSwitch(appType);
      setCcProviders(list);
      setCcSelected(new Set(list.map((_, i) => i)));
    } catch (e) {
      setFeedback({ text: 'cc-switch 扫描失败: ' + e, kind: 'error' });
    } finally {
      setCcScanning(false);
    }
  };

  const importCc = async () => {
    if (!ccProviders) return;
    const chosen = ccProviders.filter((_, i) => ccSelected.has(i));
    if (chosen.length === 0) { setFeedback({ text: '请至少选择一个', kind: 'info' }); return; }
    try {
      const { tauriApi } = await import('../lib/tauri');
      const n = await tauriApi.importCcSwitch(tool, chosen);
      setFeedback({ text: `已从 cc-switch 导入 ${n} 个配置档案`, kind: 'success' });
      setCcProviders(null);
    } catch (e) {
      setFeedback({ text: 'cc-switch 导入失败: ' + e, kind: 'error' });
    }
  };

  const scan = async () => {
    setScanning(true); setFeedback(null); setApi(null); setInfo(null); setImportedShared(false);
    try {
      const { tauriApi } = await import('../lib/tauri');
      const [a, i] = await Promise.all([
        tauriApi.scanLocalApi(tool),
        tauriApi.getLocalConfigInfo(tool),
      ]);
      setApi(a); setInfo(i);
      setName(`${tool}-local`);
    } catch (e) {
      setFeedback({ text: '扫描失败: ' + e, kind: 'error' });
    } finally {
      setScanning(false);
    }
  };

  const importProfile = async () => {
    if (!api) return;
    try {
      await addProfile({
        name: name.trim(),
        provider: api.provider,
        api_url: api.api_url,
        api_key: api.api_key,
        model: api.model,
        reasoning_effort: api.reasoning_effort,
        context_1m: api.context_1m,
        target_app: tool,
      });
      setFeedback({ text: `已导入为配置档案「${name.trim()}」`, kind: 'success' });
    } catch (e) {
      const msg = String(e);
      const friendly = /UNIQUE constraint failed/i.test(msg)
        ? `已存在同名档案「${name.trim()}」，请改个名字再导入`
        : '导入失败: ' + msg;
      setFeedback({ text: friendly, kind: 'error' });
    }
  };

  const importShared = async () => {
    try {
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.importSharedConfig(tool);
      setImportedShared(true);
      setFeedback({ text: `已导入 ${meta.displayName} 的共享配置`, kind: 'success' });
    } catch (e) {
      setFeedback({ text: '导入共享配置失败: ' + e, kind: 'error' });
    }
  };

  const mcpCount = info ? Object.keys(info.mcp_servers || {}).length : 0;
  const skillCount = info ? (info.skills?.length || 0) : 0;
  const hasHooks = info && info.hooks && Object.keys(info.hooks).length > 0;
  const hasPerms = info && info.permissions && Object.keys(info.permissions).length > 0;

  return (
    <div className="min-h-full">
      <PageHeader
        title="从本地导入"
        actions={
          <Button onClick={scan} disabled={scanning}>
            <Search size={15} className={scanning ? 'animate-spin' : ''} />
            {scanning ? '扫描中…' : '扫描'}
          </Button>
        }
      />

      <div className="max-w-4xl space-y-4 px-4 py-4 sm:px-7 sm:py-5">
        <div className="flex w-fit max-w-full flex-wrap items-center gap-1 rounded-lg border border-line bg-surface p-1">
          {SUPPORTED_TOOLS.map((t) => {
            const active = tool === t.id;
            return (
              <button
                key={t.id}
                onClick={() => { setTool(t.id); setApi(null); setInfo(null); setFeedback(null); setCcProviders(null); }}
                className={cn(
                  'flex shrink-0 items-center gap-2 whitespace-nowrap rounded-md px-3 py-1.5 text-[13px] font-medium transition-colors',
                  active ? 'bg-card text-ink shadow-soft' : 'text-ink-dim hover:text-ink',
                )}
              >
                <span className="h-2 w-2 rounded-full" style={{ background: t.color }} />
                {t.displayName}
              </button>
            );
          })}
        </div>

        <section className="overflow-hidden rounded-lg border border-line bg-card">
          <div className="flex items-center justify-between border-b border-line px-4 py-3">
            <div className="flex items-center gap-2">
              <Boxes size={15} className="text-accent" />
              <span className="text-[14px] font-semibold text-ink">从 cc-switch 导入</span>
            </div>
            <Button variant="secondary" onClick={scanCc} disabled={ccScanning}>
              <Search size={14} className={ccScanning ? 'animate-spin' : ''} />
              {ccScanning ? '扫描中…' : '扫描'}
            </Button>
          </div>
          {ccProviders && (
            <div className="p-3">
              {ccProviders.length === 0 ? (
                <div className="text-[13px] text-ink-dim py-2">未找到 {meta.displayName} 的 provider</div>
              ) : (
                <>
                  <div className="mb-3 overflow-hidden rounded-md border border-line">
                    {ccProviders.map((p, i) => (
                      <label
                        key={i}
                        className={cn(
                          'flex cursor-pointer items-center gap-3 border-b border-line px-3 py-2.5 transition-colors last:border-b-0 hover:bg-elevated/45',
                          ccSelected.has(i) && 'bg-accent/5',
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={ccSelected.has(i)}
                          onChange={() => {
                            const s = new Set(ccSelected);
                            s.has(i) ? s.delete(i) : s.add(i);
                            setCcSelected(s);
                          }}
                          className="accent-accent"
                        />
                        <div className="flex-1 min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="text-[13.5px] font-medium text-ink">{p.name}</span>
                            {p.is_current && <Badge tone="ok">当前</Badge>}
                            {p.context_1m && <Badge tone="accent">1M</Badge>}
                            {p.reasoning_effort && <Badge>{p.reasoning_effort}</Badge>}
                          </div>
                          <div className="flex items-center gap-2 mt-0.5 text-[11.5px] text-ink-faint">
                            <span className="truncate font-mono">{p.api_url}</span>
                            {p.model && <span className="shrink-0">· {p.model}</span>}
                          </div>
                        </div>
                      </label>
                    ))}
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="text-[12px] text-ink-faint">已选 {ccSelected.size} / {ccProviders.length}</span>
                    <Button onClick={importCc} disabled={ccSelected.size === 0}>
                      <FileDown size={15} />导入选中
                    </Button>
                  </div>
                </>
              )}
            </div>
          )}
        </section>

        {feedback && (
          <div className={`rounded-md border px-3 py-2 text-[13px] animate-fade-up ${
            feedback.kind === 'success' ? 'border-ok/30 bg-ok/10 text-ok'
            : feedback.kind === 'error' ? 'border-danger/30 bg-danger/10 text-danger'
            : 'border-line bg-surface text-ink-dim'
          }`}>{feedback.text}</div>
        )}

        {scanning && <div className="grid place-items-center py-16"><Spinner size="lg" /></div>}

        {!scanning && !api && (
          <div className="rounded-lg border border-dashed border-line bg-surface/50 px-4 py-10 text-center">
            <FileDown size={20} className="mx-auto mb-2 text-ink-faint" />
            <p className="text-[13px] text-ink-faint">{meta.displayName} 未扫描</p>
          </div>
        )}

        {!scanning && api && (
          <>
            <section className="overflow-hidden rounded-lg border border-line bg-card animate-fade-up">
              <div className="flex items-center gap-2 border-b border-line/70 px-4 py-3">
                <KeyRound size={15} className="text-accent" />
                <span className="text-[14px] font-semibold text-ink">导入 API 为配置档案</span>
              </div>
              <div className="space-y-4 p-4">
                {api.found ? (
                  <>
                    <div className="grid grid-cols-1 gap-3">
                      <Field label="档案名称" value={name} onChange={(e) => setName(e.target.value)} />
                      <div className="grid grid-cols-2 gap-3">
                        <ReadField label="API URL" value={api.api_url || '—'} />
                        <ReadField label="API Key" value={api.api_key ? maskApiKey(api.api_key) : '—'} />
                        {api.model && <ReadField label="默认模型" value={api.model} />}
                        {api.reasoning_effort && <ReadField label="推理强度" value={api.reasoning_effort} />}
                        {api.context_1m !== undefined && <ReadField label="1M 上下文" value={api.context_1m ? '启用' : '关闭'} />}
                      </div>
                    </div>
                    <div className="flex items-center justify-between gap-3">
                      <span className="truncate font-mono text-[11px] text-ink-faint">{api.source}</span>
                      <Button onClick={importProfile} disabled={!name.trim() || !api.api_url}>
                        <FileDown size={15} />导入为配置档案
                      </Button>
                    </div>
                    {!api.api_key && (
                      <div className="flex items-center gap-2 rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-[12px] text-warn">
                        <FileWarning size={14} />
                        未在配置文件中找到 API Key，可先导入 URL，稍后再补填
                      </div>
                    )}
                  </>
                ) : (
                  <div className="flex items-center gap-2.5 text-[13px] text-ink-dim">
                    <FileWarning size={16} className="text-warn" />
                    未在 <span className="font-mono text-ink">{api.source}</span> 中找到 API 凭据
                  </div>
                )}
              </div>
            </section>

            <section className="overflow-hidden rounded-lg border border-line bg-card animate-fade-up" style={{ animationDelay: '60ms' }}>
              <div className="flex items-center justify-between border-b border-line/70 px-4 py-3">
                <div className="flex items-center gap-2">
                  <ShieldCheck size={15} className="text-accent" />
                  <span className="text-[14px] font-semibold text-ink">共享配置</span>
                </div>
                <Button variant={importedShared ? 'success' : 'secondary'} onClick={importShared}>
                  {importedShared ? <><Check size={15} />已导入</> : <><FileDown size={15} />导入共享配置</>}
                </Button>
              </div>
              <div className="grid grid-cols-2 gap-3 p-4 sm:grid-cols-4">
                <InfoTile icon={<Boxes size={16} />} count={mcpCount} label="MCP Servers" tint="#4F8DF6"
                          detail={info && mcpCount ? Object.keys(info.mcp_servers).slice(0, 3).join(', ') : '无'} />
                <InfoTile icon={<Sparkles size={16} />} count={skillCount} label="Skills" tint="#4B5563"
                          detail={info && skillCount ? info.skills.slice(0, 3).join(', ') : '无'} />
                <InfoTile icon={<Webhook size={16} />} count={hasHooks ? 1 : 0} label="Hooks" tint="#8A5A44"
                          detail={hasHooks ? '已配置' : '无'} boolean />
                <InfoTile icon={<ShieldCheck size={16} />} count={hasPerms ? 1 : 0} label="Permissions" tint="#27A644"
                          detail={hasPerms ? '已配置' : '无'} boolean />
              </div>
            </section>
          </>
        )}
      </div>
    </div>
  );
}

function ReadField({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">{label}</span>
      <div className="w-full truncate rounded-md border border-line bg-surface px-3 py-2 font-mono text-[12.5px] text-ink">{value}</div>
    </div>
  );
}

function InfoTile({ icon, count, label, detail, tint, boolean }: {
  icon: React.ReactNode; count: number; label: string; detail: string; tint: string; boolean?: boolean;
}) {
  const has = count > 0;
  return (
    <div className="rounded-md border border-line bg-surface p-3">
      <div className="flex items-center gap-2 mb-2" style={{ color: has ? tint : undefined }}>
        <span className={has ? '' : 'text-ink-faint'}>{icon}</span>
        {!boolean && <span className={`text-[18px] font-semibold tabular-nums ${has ? 'text-ink' : 'text-ink-faint'}`}>{count}</span>}
      </div>
      <div className="text-[12px] font-medium text-ink-dim">{label}</div>
      <div className="mt-0.5 text-[11px] text-ink-faint truncate">{detail}</div>
    </div>
  );
}

function Badge({ children, tone }: { children: React.ReactNode; tone?: 'ok' | 'accent' }) {
  return (
    <span className={cn(
      'rounded px-1.5 py-0.5 text-[10px] font-medium',
      tone === 'ok' ? 'bg-ok/10 text-ok' : tone === 'accent' ? 'bg-accent/10 text-accent' : 'bg-line text-ink-dim',
    )}>
      {children}
    </span>
  );
}
