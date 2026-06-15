import { useState, useEffect } from 'react';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import {
  RefreshCw, Boxes, Sparkles, Webhook, ShieldCheck, ChevronDown, Terminal, Globe, AlertCircle,
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
}

export default function ConfigPage() {
  const [targetApp, setTargetApp] = useState<TargetApp>('claude-code');
  const [info, setInfo] = useState<LocalInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [showRaw, setShowRaw] = useState(false);
  const [openHooks, setOpenHooks] = useState(false);
  const [openPerms, setOpenPerms] = useState(false);

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
