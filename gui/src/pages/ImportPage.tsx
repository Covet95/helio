import { useRef, useState } from 'react';
import { Alert } from '@/components/common/Alert';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { Field } from '../components/common/Modal';
import {
  Search, FileDown, KeyRound, Boxes, FileWarning,
} from 'lucide-react';
import { SUPPORTED_TOOLS } from '../types';
import type { OpenCodeModelConfig, TargetApp } from '../types';
import { tauriApi, type CcSwitchProvider } from '../lib/tauri';
import { AppSelector } from './profiles/helpers';
import { cn, humanizeError, maskApiKey } from '../lib/utils';

interface Scanned {
  found: boolean;
  api_url: string;
  api_key: string;
  provider: string;
  model?: string;
  model_mapping?: Record<string, string>;
  reasoning_effort?: string;
  reasoning_summary?: string;
  verbosity?: string;
  context_1m?: boolean;
  wire_api?: string;
  env_key?: string;
  requires_openai_auth?: boolean;
  experimental_bearer_token?: string;
  service_tier?: string;
  supports_standalone_web_search?: boolean;
  aws_profile?: string;
  aws_region?: string;
  auth_command?: string;
  auth_args?: string[];
  auth_timeout_ms?: number;
  auth_refresh_interval_ms?: number;
  auth_cwd?: string;
  api_mode?: string;
  opencode_api_mode?: string;
  opencode_models?: string[];
  opencode_model_configs?: Record<string, OpenCodeModelConfig>;
  max_tokens?: number;
  source: string;
}
type Feedback = { text: string; kind: 'success' | 'error' | 'info' };

export default function ImportPage() {
  const tool = useStore((state) => state.selectedTool);
  const setTool = useStore((state) => state.setSelectedTool);
  return <ImportToolPage key={tool} tool={tool} onToolChange={setTool} />;
}

function ImportToolPage({ tool, onToolChange }: { tool: TargetApp; onToolChange: (tool: TargetApp) => void }) {
  const addProfile = useStore((state) => state.addProfile);
  const [importing, setImporting] = useState(false);
  const importingRef = useRef(false);
  const [scanning, setScanning] = useState(false);
  const [api, setApi] = useState<Scanned | null>(null);
  const [name, setName] = useState('');
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [ccProviders, setCcProviders] = useState<CcSwitchProvider[] | null>(null);
  const [ccScanning, setCcScanning] = useState(false);
  const [ccSelected, setCcSelected] = useState<Set<number>>(new Set());

  const meta = SUPPORTED_TOOLS.find((t) => t.id === tool)!;
  const canImportCcSwitch = tool === 'claude-code' || tool === 'codex';

  const scanCc = async () => {
    if (!canImportCcSwitch) {
      setFeedback({ text: '当前仅支持从 cc-switch 导入 Claude Code 和 Codex provider', kind: 'info' });
      return;
    }
    setCcScanning(true); setFeedback(null); setCcProviders(null); setCcSelected(new Set());
    try {
      const appType = tool === 'claude-code' ? 'claude' : tool;
      const list = await tauriApi.scanCcSwitch(appType);
      setCcProviders(list);
      setCcSelected(new Set(list.map((_, i) => i)));
    } catch (e) {
      setFeedback({ text: `cc-switch 扫描失败: ${humanizeError(e)}`, kind: 'error' });
    } finally {
      setCcScanning(false);
    }
  };

  const importCc = async () => {
    if (!ccProviders || importingRef.current) return;
    const chosen = ccProviders.filter((_, i) => ccSelected.has(i));
    if (chosen.length === 0) { setFeedback({ text: '请至少选择一个', kind: 'info' }); return; }
    importingRef.current = true;
    setImporting(true);
    try {
      const n = await tauriApi.importCcSwitch(tool, chosen);
      await useStore.getState().refresh();
      setFeedback({ text: `已从 cc-switch 导入 ${n} 个配置档案`, kind: 'success' });
      setCcProviders(null);
    } catch (e) {
      setFeedback({ text: `cc-switch 导入失败: ${humanizeError(e)}`, kind: 'error' });
    } finally {
      importingRef.current = false;
      setImporting(false);
    }
  };

  const scan = async () => {
    setScanning(true); setFeedback(null); setApi(null);
    try {
      const a = await tauriApi.scanLocalApi(tool);
      setApi(a);
      setName(`${tool}-local`);
    } catch (e) {
      setFeedback({ text: `扫描失败: ${humanizeError(e)}`, kind: 'error' });
    } finally {
      setScanning(false);
    }
  };

  const importProfile = async () => {
    if (!api || !name.trim() || importingRef.current) return;
    importingRef.current = true;
    setImporting(true);
    try {
      await addProfile({
        name: name.trim(),
        provider: api.provider,
        api_url: api.api_url,
        api_key: api.api_key,
        model: api.model,
        model_mapping: api.model_mapping,
        reasoning_effort: api.reasoning_effort,
        reasoning_summary: tool === 'codex' ? api.reasoning_summary : undefined,
        verbosity: tool === 'codex' ? api.verbosity : undefined,
        context_1m: api.context_1m,
        env_key: tool === 'codex' ? api.env_key : undefined,
        wire_api: tool === 'codex' ? api.wire_api : undefined,
        requires_openai_auth: tool === 'codex' ? api.requires_openai_auth : undefined,
        experimental_bearer_token: tool === 'codex' ? api.experimental_bearer_token : undefined,
        auth_command: tool === 'codex' ? api.auth_command : undefined,
        auth_args: tool === 'codex' ? api.auth_args : undefined,
        auth_timeout_ms: tool === 'codex' ? api.auth_timeout_ms : undefined,
        auth_refresh_interval_ms: tool === 'codex' ? api.auth_refresh_interval_ms : undefined,
        auth_cwd: tool === 'codex' ? api.auth_cwd : undefined,
        api_mode: tool === 'hermes' || tool === 'openclaw' ? api.api_mode : undefined,
        opencode_api_mode: tool === 'opencode' ? api.opencode_api_mode : undefined,
        models: tool === 'opencode' ? api.opencode_models : undefined,
        model_configs: tool === 'opencode' ? api.opencode_model_configs : undefined,
        max_tokens: tool === 'openclaw' ? api.max_tokens : undefined,
        service_tier: api.service_tier,
        supports_standalone_web_search: api.supports_standalone_web_search,
        aws_profile: api.aws_profile,
        aws_region: api.aws_region,
        target_app: tool,
      });
      setFeedback({ text: `已导入为配置档案「${name.trim()}」`, kind: 'success' });
    } catch (e) {
      const msg = String(e);
      const friendly = /UNIQUE constraint failed/i.test(msg)
        ? `已存在同名档案「${name.trim()}」，请改个名字再导入`
        : `导入失败: ${humanizeError(e)}`;
      setFeedback({ text: friendly, kind: 'error' });
    } finally {
      importingRef.current = false;
      setImporting(false);
    }
  };

  return (
    <div className="min-h-full">
      <PageHeader
        title="从本地导入"
        actions={
          <Button onClick={scan} disabled={scanning || importing}>
            <Search size={15} className={scanning ? 'animate-spin' : ''} />
            {scanning ? '扫描中…' : '扫描'}
          </Button>
        }
      />

      <div className="max-w-4xl space-y-4 px-4 py-4 sm:px-7 sm:py-5">
        <AppSelector value={tool} onChange={onToolChange} disabled={importing} />

        <p className="text-[12px] leading-relaxed text-ink-faint">
          这里只把本机 API 做成档案。共享配置（MCP / Hooks / 权限）打开应用即自动同步，切换时以本机最新内容为准，无需手动导入。
        </p>

        {canImportCcSwitch && <section className="border-b border-line pb-4">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-line px-4 py-3">
            <div className="flex min-w-0 items-center gap-2">
              <Boxes size={15} className="text-accent" />
              <span className="text-[14px] font-semibold text-ink">从 cc-switch 导入</span>
            </div>
            <Button variant="secondary" onClick={scanCc} disabled={ccScanning || importing}>
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
                    <Button onClick={importCc} disabled={ccSelected.size === 0 || importing}>
                      <FileDown size={15} />导入选中
                    </Button>
                  </div>
                </>
              )}
            </div>
          )}
        </section>}

        {feedback && (
          <Alert
            tone={feedback.kind === 'success' ? 'success' : feedback.kind === 'error' ? 'error' : 'info'}
            className="animate-fade-up"
          >{feedback.text}</Alert>
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
            <section className="border-b border-line pb-4">
              <div className="flex items-center gap-2 border-b border-line/70 px-4 py-3">
                <KeyRound size={15} className="text-accent" />
                <span className="text-[14px] font-semibold text-ink">导入 API 为配置档案</span>
              </div>
              <div className="space-y-4 p-4">
                {api.found ? (
                  <>
                    <div className="grid grid-cols-1 gap-3">
                      <Field label="档案名称" value={name} disabled={importing} onChange={(e) => setName(e.target.value)} />
                      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        <ReadField label="API URL" value={api.api_url || '—'} />
                        <ReadField label="API Key" value={api.api_key ? maskApiKey(api.api_key) : '—'} />
                        {api.model && <ReadField label="默认模型" value={api.model} />}
                        {api.reasoning_effort && <ReadField label="推理强度" value={api.reasoning_effort} />}
                        {api.reasoning_summary && <ReadField label="推理摘要" value={api.reasoning_summary} />}
                        {api.verbosity && <ReadField label="Verbosity" value={api.verbosity} />}
                        {api.service_tier && <ReadField label="Service Tier" value={api.service_tier} />}
                        {api.auth_command && <ReadField label="Auth 命令" value={api.auth_command} />}
                        {api.context_1m !== undefined && <ReadField label="1M 上下文" value={api.context_1m ? '启用' : '关闭'} />}
                        {api.model_mapping && Object.keys(api.model_mapping).length > 0 && (
                          <ReadField
                            label="角色映射"
                            value={(['sonnet', 'opus', 'fable', 'haiku'] as const)
                              .map((r) => {
                                const m = api.model_mapping?.[`${r}_model`];
                                if (!m) return null;
                                const one = api.model_mapping?.[`${r}_one_m`] === 'true' ? ' [1M]' : '';
                                return `${r}→${m}${one}`;
                              })
                              .filter(Boolean)
                              .join('，') || '—'}
                          />
                        )}
                      </div>
                    </div>
                    <div className="flex flex-wrap items-center justify-between gap-3">
                      <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink-faint">{api.source}</span>
                      <Button
                        onClick={importProfile}
                        disabled={importing || !name.trim() || (!api.api_url && !(tool === 'codex' && api.provider === 'amazon-bedrock'))}
                      >
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
      <div title={value} className="w-full break-all rounded-md border border-line bg-card px-3 py-2 font-mono text-[12.5px] text-ink">{value}</div>
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
