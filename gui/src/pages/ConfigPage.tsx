import { useState, useEffect } from 'react';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import {
  RefreshCw, Boxes, Sparkles, Webhook, ShieldCheck, ChevronDown, Terminal, Globe, AlertCircle, Layers, FileCog, Save, X, CheckCircle2, SlidersHorizontal,
} from 'lucide-react';
import type { TargetApp } from '../types';
import { SUPPORTED_TOOLS } from '../types';
import { cn, humanizeError } from '../lib/utils';

interface McpServerCfg {
  command?: string;
  args?: string[];
  url?: string | null;
  env?: Record<string, string> | null;
}
interface LocalInfo {
  mcp_servers: Record<string, McpServerCfg>;
  skills: string[];
  hooks: Record<string, unknown>;
  permissions: Record<string, unknown>;
  other: Record<string, unknown>;
}

export default function ConfigPage() {
  const [targetApp, setTargetApp] = useState<TargetApp>('claude-code');
  const [info, setInfo] = useState<LocalInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [showRaw, setShowRaw] = useState(false);
  const [openHooks, setOpenHooks] = useState(false);
  const [openPerms, setOpenPerms] = useState(false);
  const [openOther, setOpenOther] = useState(false);

  const load = async () => {
    setLoading(true);
    setError('');
    try {
      const { tauriApi } = await import('../lib/tauri');
      const result = (await tauriApi.getLocalConfigInfo(targetApp)) as LocalInfo;
      setInfo(result);
    } catch (err) {
      setError('读取本地配置失败: ' + humanizeError(err));
      setInfo(null);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetApp]);

  const mcpEntries = info ? Object.entries(info.mcp_servers || {}) : [];
  const skills = info?.skills || [];
  const hooks = (info?.hooks || {}) as Record<string, unknown>;
  const perms = (info?.permissions || {}) as Record<string, unknown>;
  const hookKeys = Object.keys(hooks);
  const allowN = Array.isArray(perms.allow) ? perms.allow.length : 0;
  const denyN = Array.isArray(perms.deny) ? perms.deny.length : 0;
  const other = (info?.other || {}) as Record<string, unknown>;
  const otherKeys = Object.keys(other);

  return (
    <div className="flex h-full flex-col">
      <PageHeader
        title="共享配置"
        actions={
          <Button variant="secondary" onClick={load} disabled={loading}>
            <RefreshCw size={15} className={loading ? 'animate-spin' : ''} />
            {loading ? '读取中…' : '刷新'}
          </Button>
        }
      />

      <div className="flex min-h-0 flex-1 flex-col overflow-auto px-4 py-4 sm:px-7 sm:py-5">
        {/* tool selector */}
        <div className="mb-4 flex w-fit max-w-full flex-wrap items-center gap-1 rounded-lg border border-line bg-surface p-1">
          {SUPPORTED_TOOLS.map((t) => {
            const active = targetApp === t.id;
            return (
              <button
                key={t.id}
                onClick={() => setTargetApp(t.id)}
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

        {error && (
          <div className="mb-3 flex items-center gap-2 rounded-md border border-danger/30 bg-danger/8 px-3 py-2 text-[13px] text-danger">
            <AlertCircle size={15} className="shrink-0" />
            <span className="flex-1">{error}</span>
          </div>
        )}

        {loading ? (
          <div className="grid place-items-center py-20">
            <Spinner size="lg" />
          </div>
        ) : info ? (
          <div className="space-y-4">
            <Section icon={<Boxes size={15} className="text-accent" />} title="MCP Servers" count={mcpEntries.length}>
              {mcpEntries.length === 0 ? (
                <Empty>未配置 MCP server</Empty>
              ) : (
                <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                  {mcpEntries.map(([name, cfg]) => (
                    <McpCard key={name} name={name} cfg={cfg} />
                  ))}
                </div>
              )}
            </Section>

            <Section icon={<Sparkles size={15} className="text-accent" />} title="Skills" count={skills.length}>
              {skills.length === 0 ? (
                <Empty>未发现 skills</Empty>
              ) : (
                <div className="flex flex-wrap gap-1.5">
                  {skills.map((s) => (
                    <span
                      key={s}
                      className="rounded-md border border-line bg-surface px-2 py-1 font-mono text-[11.5px] text-ink-dim"
                    >
                      {s}
                    </span>
                  ))}
                </div>
              )}
            </Section>

            <Section icon={<Webhook size={15} className="text-accent" />} title="Hooks" count={hookKeys.length}>
              {hookKeys.length === 0 ? (
                <Empty>无 hooks</Empty>
              ) : (
                <>
                  <div className="flex flex-wrap gap-1.5">
                    {hookKeys.map((k) => (
                      <span
                        key={k}
                        className="rounded-md border border-line bg-surface px-2 py-1 text-[11.5px] text-ink-dim"
                      >
                        {k}
                      </span>
                    ))}
                  </div>
                  <RawToggle open={openHooks} setOpen={setOpenHooks} data={hooks} />
                </>
              )}
            </Section>

            <Section icon={<ShieldCheck size={15} className="text-accent" />} title="Permissions" count={allowN + denyN}>
              <div className="flex flex-wrap items-center gap-1.5">
                {allowN > 0 && (
                  <span className="rounded-md border border-ok/30 bg-ok/8 px-2 py-1 text-[11.5px] text-ok">
                    allow · {allowN}
                  </span>
                )}
                {denyN > 0 && (
                  <span className="rounded-md border border-danger/30 bg-danger/8 px-2 py-1 text-[11.5px] text-danger">
                    deny · {denyN}
                  </span>
                )}
                {allowN + denyN === 0 && <Empty inline>无</Empty>}
              </div>
              {allowN + denyN > 0 && <RawToggle open={openPerms} setOpen={setOpenPerms} data={perms} />}
            </Section>

            <Section icon={<Layers size={15} className="text-accent" />} title="其他同步配置" count={otherKeys.length}>
              {otherKeys.length === 0 ? (
                <Empty>无</Empty>
              ) : (
                <>
                  <div className="flex flex-wrap gap-1.5">
                    {otherKeys.map((k) => (
                      <span
                        key={k}
                        className="rounded-md border border-line bg-surface px-2 py-1 font-mono text-[11.5px] text-ink-dim"
                      >
                        {k}
                      </span>
                    ))}
                  </div>
                  <RawToggle open={openOther} setOpen={setOpenOther} data={other} />
                  <div className="mt-1.5 text-[11px] text-ink-faint">
                    切换 profile 时这些顶层配置会被原样保留同步
                  </div>
                </>
              )}
            </Section>

            {/* Codex 行为设置（仅 Codex） */}
            {targetApp === 'codex' && <CodexBehaviorSettings current={other} onSaved={load} />}

            {/* 编辑 config.toml（仅 Codex） */}
            {targetApp === 'codex' && <CodexConfigEditor onSaved={load} />}

            {/* 原始 JSON（只读，折叠） */}
            <div className="overflow-hidden rounded-lg border border-line bg-card">
              <button
                onClick={() => setShowRaw((v) => !v)}
                className="flex w-full items-center gap-2 px-4 py-3 text-[13px] font-medium text-ink-dim transition-colors hover:text-ink"
              >
                <ChevronDown size={15} className={cn('transition-transform', showRaw && 'rotate-180')} />
                原始 JSON
              </button>
              {showRaw && (
                <pre className="max-h-80 overflow-auto border-t border-line bg-surface px-4 py-3 font-mono text-[11.5px] leading-relaxed text-ink-dim">
                  {JSON.stringify(info, null, 2)}
                </pre>
              )}
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}

// Codex 全局行为字段：官方枚举用下拉，魔改字段标注「非官方」。
const CODEX_SELECT_FIELDS: { key: string; label: string; options: string[] }[] = [
  { key: 'approval_policy', label: 'approval_policy', options: ['untrusted', 'on-failure', 'on-request', 'never'] },
  { key: 'sandbox_mode', label: 'sandbox_mode', options: ['read-only', 'workspace-write', 'danger-full-access'] },
  { key: 'personality', label: 'personality', options: ['none', 'friendly', 'pragmatic'] },
  { key: 'model_reasoning_effort', label: 'model_reasoning_effort', options: ['minimal', 'low', 'medium', 'high', 'xhigh'] },
  { key: 'service_tier', label: 'service_tier', options: ['fast', 'flex'] },
];
const CODEX_BOOL_FIELDS: { key: string; label: string; unofficial?: boolean }[] = [
  { key: 'disable_response_storage', label: 'disable_response_storage' },
  { key: 'enable_workflows', label: 'enable_workflows', unofficial: true },
  { key: 'enable_ultracode_trigger', label: 'enable_ultracode_trigger', unofficial: true },
  { key: 'skip_permission_prompts_for_mcp', label: 'skip_permission_prompts_for_mcp', unofficial: true },
];

// 把 current（来自 get_local_config_info 的 other）里的顶层值转成下拉/文本框用的字符串。
function toStr(v: unknown): string {
  if (v === undefined || v === null) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'number') return String(v);
  return '';
}

function CodexBehaviorSettings({
  current,
  onSaved,
}: {
  current: Record<string, unknown>;
  onSaved: () => void;
}) {
  // 字符串型字段（下拉 + model_auto_compact_token_limit 数字框 + model_effort_level 文本框）
  // 统一存为字符串，'' 表示「不设置」。
  const buildStr = () => {
    const s: Record<string, string> = {};
    for (const f of CODEX_SELECT_FIELDS) s[f.key] = toStr(current[f.key]);
    s.model_auto_compact_token_limit = toStr(current.model_auto_compact_token_limit);
    s.model_effort_level = toStr(current.model_effort_level);
    return s;
  };
  const buildBool = () => {
    const b: Record<string, boolean> = {};
    for (const f of CODEX_BOOL_FIELDS) b[f.key] = current[f.key] === true;
    return b;
  };

  const [strVals, setStrVals] = useState<Record<string, string>>(buildStr);
  const [boolVals, setBoolVals] = useState<Record<string, boolean>>(buildBool);
  const [initStr, setInitStr] = useState<Record<string, string>>(buildStr);
  const [initBool, setInitBool] = useState<Record<string, boolean>>(buildBool);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState('');
  const [saved, setSaved] = useState(false);

  // current 刷新后（load() 之后）重新同步初始值与编辑值。
  useEffect(() => {
    const s = buildStr();
    const b = buildBool();
    setStrVals(s);
    setBoolVals(b);
    setInitStr(s);
    setInitBool(b);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current]);

  const dirty =
    CODEX_SELECT_FIELDS.some((f) => strVals[f.key] !== initStr[f.key]) ||
    strVals.model_auto_compact_token_limit !== initStr.model_auto_compact_token_limit ||
    strVals.model_effort_level !== initStr.model_effort_level ||
    CODEX_BOOL_FIELDS.some((f) => boolVals[f.key] !== initBool[f.key]);

  const dangerCombo =
    strVals.approval_policy === 'never' && strVals.sandbox_mode === 'danger-full-access';

  const setStr = (k: string, v: string) => {
    setSaved(false);
    setStrVals((prev) => ({ ...prev, [k]: v }));
  };
  const setBool = (k: string, v: boolean) => {
    setSaved(false);
    setBoolVals((prev) => ({ ...prev, [k]: v }));
  };

  const save = async () => {
    setErr('');
    // 只发送相对初始值有变化的字段：'' → null（删除），有值 → 写入。
    const fields: Record<string, unknown> = {};

    for (const f of CODEX_SELECT_FIELDS) {
      if (strVals[f.key] !== initStr[f.key]) {
        fields[f.key] = strVals[f.key] === '' ? null : strVals[f.key];
      }
    }
    if (strVals.model_effort_level !== initStr.model_effort_level) {
      const v = strVals.model_effort_level.trim();
      fields.model_effort_level = v === '' ? null : v;
    }
    if (strVals.model_auto_compact_token_limit !== initStr.model_auto_compact_token_limit) {
      const raw = strVals.model_auto_compact_token_limit.trim();
      if (raw === '') {
        fields.model_auto_compact_token_limit = null;
      } else {
        if (!/^\d+$/.test(raw) || Number(raw) <= 0) {
          setErr('model_auto_compact_token_limit 必须是正整数');
          return;
        }
        fields.model_auto_compact_token_limit = Number(raw);
      }
    }
    for (const f of CODEX_BOOL_FIELDS) {
      if (boolVals[f.key] !== initBool[f.key]) {
        fields[f.key] = boolVals[f.key];
      }
    }

    if (Object.keys(fields).length === 0) {
      setSaved(true);
      return;
    }

    setSaving(true);
    try {
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.updateCodexFields(fields);
      setSaved(true);
      onSaved();
    } catch (e) {
      setErr(humanizeError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="overflow-hidden rounded-lg border border-line bg-card">
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5">
        <SlidersHorizontal size={15} className="text-accent" />
        <span className="text-[14px] font-semibold text-ink">Codex 行为设置</span>
        <div className="ml-auto flex items-center gap-2">
          {saved && (
            <span className="flex items-center gap-1 text-[12px] text-ok">
              <CheckCircle2 size={13} /> 已保存
            </span>
          )}
          <Button onClick={save} disabled={saving || !dirty}>
            <Save size={15} />
            {saving ? '保存中…' : '保存'}
          </Button>
        </div>
      </div>
      <div className="space-y-4 p-4">
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
          {CODEX_SELECT_FIELDS.map((f) => (
            <Field key={f.key} label={f.label}>
              <select
                value={strVals[f.key] ?? ''}
                onChange={(e) => setStr(f.key, e.target.value)}
                className="w-full rounded-md border border-line bg-surface px-2.5 py-1.5 text-[13px] text-ink focus:border-accent focus:outline-none"
              >
                <option value="">(不设置)</option>
                {f.options.map((o) => (
                  <option key={o} value={o}>
                    {o}
                  </option>
                ))}
              </select>
            </Field>
          ))}

          <Field label="model_auto_compact_token_limit">
            <input
              type="number"
              min={1}
              step={1}
              value={strVals.model_auto_compact_token_limit}
              onChange={(e) => setStr('model_auto_compact_token_limit', e.target.value)}
              placeholder="(不设置)"
              className="w-full rounded-md border border-line bg-surface px-2.5 py-1.5 text-[13px] text-ink focus:border-accent focus:outline-none"
            />
          </Field>

          <Field label="model_effort_level" unofficial>
            <input
              type="text"
              value={strVals.model_effort_level}
              onChange={(e) => setStr('model_effort_level', e.target.value)}
              placeholder="(不设置)"
              className="w-full rounded-md border border-line bg-surface px-2.5 py-1.5 font-mono text-[13px] text-ink focus:border-accent focus:outline-none"
            />
          </Field>
        </div>

        <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
          {CODEX_BOOL_FIELDS.map((f) => (
            <Toggle
              key={f.key}
              label={f.label}
              unofficial={f.unofficial}
              checked={boolVals[f.key]}
              onChange={(v) => setBool(f.key, v)}
            />
          ))}
        </div>

        {dangerCombo && (
          <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/8 px-3 py-2 text-[12px] text-danger">
            <AlertCircle size={14} className="mt-0.5 shrink-0" />
            <span className="flex-1">
              approval_policy=never 与 sandbox_mode=danger-full-access 组合在官方 Codex 会触发回退，请确认这是你想要的设置。
            </span>
          </div>
        )}

        {err && (
          <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/8 px-3 py-2 text-[12px] text-danger">
            <AlertCircle size={14} className="mt-0.5 shrink-0" />
            <span className="flex-1 whitespace-pre-wrap break-words">{err}</span>
          </div>
        )}

        <div className="text-[11px] text-ink-faint">
          下拉选「(不设置)」会从 config.toml 删除该字段。保存前自动备份并校验 TOML，标注「非官方」的为魔改字段。
        </div>
      </div>
    </section>
  );
}

function Field({
  label,
  unofficial,
  children,
}: {
  label: string;
  unofficial?: boolean;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 flex min-w-0 items-center gap-1.5 font-mono text-[12px] text-ink-dim">
        <span className="truncate">{label}</span>
        {unofficial && (
          <span className="shrink-0 rounded bg-elevated px-1 py-0.5 font-sans text-[10px] text-ink-faint">非官方</span>
        )}
      </span>
      {children}
    </label>
  );
}

function Toggle({
  label,
  unofficial,
  checked,
  onChange,
}: {
  label: string;
  unofficial?: boolean;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      onClick={() => onChange(!checked)}
      className="flex items-center justify-between gap-2 rounded-md border border-line bg-surface px-3 py-2 text-left transition-colors hover:border-accent/50"
    >
      <span className="flex min-w-0 items-center gap-1.5 font-mono text-[12px] text-ink-dim">
        <span className="truncate">{label}</span>
        {unofficial && (
          <span className="shrink-0 rounded bg-elevated px-1 py-0.5 font-sans text-[10px] text-ink-faint">非官方</span>
        )}
      </span>
      <span
        className={cn(
          'relative h-5 w-9 shrink-0 rounded-full transition-colors',
          checked ? 'bg-accent' : 'bg-elevated',
        )}
      >
        <span
          className={cn(
            'absolute top-0.5 h-4 w-4 rounded-full bg-card shadow-soft transition-transform',
            checked ? 'translate-x-4' : 'translate-x-0.5',
          )}
        />
      </span>
    </button>
  );
}

function CodexConfigEditor({ onSaved }: { onSaved: () => void }) {
  const [editing, setEditing] = useState(false);
  const [content, setContent] = useState('');
  const [loadingRaw, setLoadingRaw] = useState(false);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState('');
  const [saved, setSaved] = useState(false);

  const enterEdit = async () => {
    setErr('');
    setSaved(false);
    setLoadingRaw(true);
    try {
      const { tauriApi } = await import('../lib/tauri');
      const raw = await tauriApi.readCodexConfigRaw();
      setContent(raw);
      setEditing(true);
    } catch (e) {
      setErr('读取 config.toml 失败: ' + humanizeError(e));
    } finally {
      setLoadingRaw(false);
    }
  };

  const save = async () => {
    setErr('');
    setSaving(true);
    try {
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.saveCodexConfigRaw(content);
      setEditing(false);
      setSaved(true);
      onSaved();
    } catch (e) {
      // 后端返回的 TOML 语法错误原文要展示出来，别只给泛化错误
      setErr(humanizeError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="overflow-hidden rounded-lg border border-line bg-card">
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5">
        <FileCog size={15} className="text-accent" />
        <span className="text-[14px] font-semibold text-ink">编辑 config.toml</span>
        {!editing && (
          <div className="ml-auto flex items-center gap-2">
            {saved && (
              <span className="flex items-center gap-1 text-[12px] text-ok">
                <CheckCircle2 size={13} /> 已保存
              </span>
            )}
            <Button variant="secondary" onClick={enterEdit} disabled={loadingRaw}>
              {loadingRaw ? '加载中…' : '编辑'}
            </Button>
          </div>
        )}
      </div>
      <div className="p-4">
        {!editing ? (
          <div className="text-[12.5px] text-ink-faint">
            直接编辑 ~/.codex/config.toml 的原始文本。保存会自动备份当前配置，并在校验语法通过后才写入。
          </div>
        ) : (
          <div className="space-y-3">
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              spellCheck={false}
              className="h-96 w-full resize-y overflow-auto rounded-md border border-line bg-surface px-3 py-2 font-mono text-[12px] leading-relaxed text-ink focus:border-accent focus:outline-none"
            />
            {err && (
              <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/8 px-3 py-2 text-[12px] text-danger">
                <AlertCircle size={14} className="mt-0.5 shrink-0" />
                <span className="flex-1 whitespace-pre-wrap break-words font-mono">{err}</span>
              </div>
            )}
            <div className="flex flex-wrap items-center gap-2">
              <Button onClick={save} disabled={saving}>
                <Save size={15} />
                {saving ? '保存中…' : '保存'}
              </Button>
              <Button variant="secondary" onClick={() => { setEditing(false); setErr(''); }} disabled={saving}>
                <X size={15} />
                取消
              </Button>
              <span className="min-w-0 flex-1 text-[11px] text-ink-faint">
                保存前自动备份当前配置，并校验 TOML 语法，通过后才写入
              </span>
            </div>
          </div>
        )}
      </div>
    </section>
  );
}

function Section({
  icon,
  title,
  count,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  count: number;
  children: React.ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-lg border border-line bg-card">
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5">
        {icon}
        <span className="text-[14px] font-semibold text-ink">{title}</span>
        <span className="rounded-md bg-elevated px-1.5 py-0.5 text-[11px] font-medium text-ink-dim">{count}</span>
      </div>
      <div className="p-4">{children}</div>
    </section>
  );
}

function McpCard({ name, cfg }: { name: string; cfg: McpServerCfg }) {
  const hasUrl = !!cfg.url;
  const cmdLine = [cfg.command, ...(cfg.args || [])].filter(Boolean).join(' ');
  const envCount = cfg.env ? Object.keys(cfg.env).length : 0;
  return (
    <div className="rounded-md border border-line bg-surface p-3">
      <div className="flex items-center gap-2">
        {hasUrl ? (
          <Globe size={13} className="shrink-0 text-accent" />
        ) : (
          <Terminal size={13} className="shrink-0 text-accent" />
        )}
        <span className="truncate text-[13px] font-semibold text-ink">{name}</span>
      </div>
      {cmdLine && (
        <div className="mt-1.5 truncate font-mono text-[11px] text-ink-faint" title={cmdLine}>
          {cmdLine}
        </div>
      )}
      {hasUrl && cfg.url && (
        <div className="mt-1.5 truncate font-mono text-[11px] text-ink-faint" title={cfg.url}>
          {cfg.url}
        </div>
      )}
      {envCount > 0 && <div className="mt-1.5 text-[10.5px] text-ink-faint">env · {envCount} 项</div>}
    </div>
  );
}

function RawToggle({
  open,
  setOpen,
  data,
}: {
  open: boolean;
  setOpen: (v: boolean) => void;
  data: unknown;
}) {
  return (
    <div className="mt-2">
      <button
        onClick={() => setOpen(!open)}
        className="flex items-center gap-1 text-[11px] text-ink-faint transition-colors hover:text-ink-dim"
      >
        <ChevronDown size={12} className={cn('transition-transform', open && 'rotate-180')} />
        {open ? '收起原始' : '展开原始'}
      </button>
      {open && (
        <pre className="mt-1.5 max-h-60 overflow-auto rounded-md border border-line bg-surface px-3 py-2 font-mono text-[10.5px] leading-relaxed text-ink-faint">
          {JSON.stringify(data, null, 2)}
        </pre>
      )}
    </div>
  );
}

function Empty({ children, inline }: { children: React.ReactNode; inline?: boolean }) {
  return <div className={cn('text-[12.5px] text-ink-faint', !inline && 'py-1')}>{children}</div>;
}
