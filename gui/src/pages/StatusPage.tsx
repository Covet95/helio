import { useEffect } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { RefreshCw, HardDrive, Layers, FolderOpen } from 'lucide-react';
import { formatBytes } from '../lib/utils';
import { SUPPORTED_TOOLS } from '../types';
import type { StatusInfo, TargetStatus, ToolInfo } from '../types';

function statusForTool(status: StatusInfo | null, id: string): TargetStatus | undefined {
  if (!status) return undefined;
  const key = id.replace('-', '_') as keyof StatusInfo;
  return status[key] as TargetStatus | undefined;
}

export default function StatusPage() {
  const { status, loadingStatus, fetchStatus } = useStore();

  useEffect(() => { fetchStatus(); }, []);

  return (
    <div className="min-h-full">
      <PageHeader
        title="Status"
        subtitle="各工具当前激活的 Profile 与连接状态"
        actions={
          <Button variant="secondary" onClick={fetchStatus}>
            <RefreshCw size={15} className={loadingStatus ? 'animate-spin' : ''} />
            刷新
          </Button>
        }
      />

      <div className="px-8 py-6 max-w-4xl">
        {loadingStatus && !status ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : (
          <>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              {SUPPORTED_TOOLS.map((tool, i) => (
                <ToolCard key={tool.id} tool={tool} status={statusForTool(status, tool.id)} index={i} />
              ))}
            </div>

            {/* database panel */}
            <div className="mt-6 rounded-xl border border-line bg-card p-5 animate-fade-up" style={{ animationDelay: '200ms' }}>
              <div className="flex items-center gap-2 mb-4">
                <HardDrive size={16} className="text-accent" />
                <h3 className="text-[14px] font-semibold text-ink">数据库</h3>
              </div>
              <div className="grid grid-cols-3 gap-4">
                <Stat icon={<HardDrive size={16} />} value={status?.database ? formatBytes(status.database.size) : '—'} label="大小" />
                <Stat icon={<Layers size={16} />} value={String(status?.database?.profile_count ?? 0)} label="Profiles" />
                <div className="rounded-lg border border-line bg-surface p-3.5">
                  <div className="flex items-center gap-1.5 text-ink-faint mb-1.5">
                    <FolderOpen size={14} /><span className="text-[11px] font-medium">路径</span>
                  </div>
                  <div className="font-mono text-[11px] text-ink-dim break-all leading-snug">
                    {status?.database?.path ?? '—'}
                  </div>
                </div>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function ToolCard({ tool, status, index }: { tool: ToolInfo; status?: TargetStatus; index: number }) {
  const active = !!status?.profile;
  const connected = status?.connected;
  return (
    <div
      className="group relative overflow-hidden rounded-xl border border-line bg-card p-4 transition-all duration-300 hover:border-line-strong animate-fade-up"
      style={{ animationDelay: `${index * 60}ms` }}
    >
      <div
        className={`pointer-events-none absolute inset-0 bg-gradient-to-br to-transparent transition-opacity duration-500 ${active ? 'opacity-100' : 'opacity-0'}`}
        style={{ backgroundImage: `linear-gradient(135deg, ${tool.color}14, transparent 70%)` }}
      />
      <div className="relative flex items-start justify-between">
        <div className="flex items-center gap-3">
          <div className="grid place-items-center h-10 w-10 rounded-xl border font-mono text-[12px] font-bold transition-transform duration-300 group-hover:scale-105"
               style={{ background: `${tool.color}1a`, color: tool.color, borderColor: `${tool.color}33` }}>
            {tool.short}
          </div>
          <div>
            <div className="text-[14px] font-semibold text-ink">{tool.displayName}</div>
            <div className="font-mono text-[10px] text-ink-faint">{tool.format}</div>
          </div>
        </div>
        {/* status dot */}
        <div className="flex items-center gap-1.5">
          <span className={`h-2 w-2 rounded-full ${active ? (connected ? 'bg-ok animate-breathe' : 'bg-warn') : 'bg-ink-faint/40'}`} />
          <span className={`text-[11px] font-medium ${active ? (connected ? 'text-ok' : 'text-warn') : 'text-ink-faint'}`}>
            {active ? (connected ? '在线' : '未连接') : '未设置'}
          </span>
        </div>
      </div>

      {status?.profile ? (
        <div className="relative mt-4 space-y-1.5">
          <Row label="Profile" value={status.profile.name} strong />
          <Row label="Provider" value={status.profile.provider} />
          <Row label="URL" value={status.profile.api_url} mono />
        </div>
      ) : (
        <div className="relative mt-4 text-[12px] text-ink-faint">尚未为该工具切换任何 Profile</div>
      )}
    </div>
  );
}

function Row({ label, value, mono, strong }: { label: string; value: string; mono?: boolean; strong?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-3 text-[12.5px]">
      <span className="text-ink-faint">{label}</span>
      <span className={`truncate ${mono ? 'font-mono' : ''} ${strong ? 'text-ink font-medium' : 'text-ink-dim'}`}>{value}</span>
    </div>
  );
}

function Stat({ icon, value, label }: { icon: React.ReactNode; value: string; label: string }) {
  return (
    <div className="rounded-lg border border-line bg-surface p-3.5">
      <div className="flex items-center gap-1.5 text-ink-faint mb-1.5">{icon}<span className="text-[11px] font-medium">{label}</span></div>
      <div className="text-[20px] font-semibold text-ink tabular-nums">{value}</div>
    </div>
  );
}
