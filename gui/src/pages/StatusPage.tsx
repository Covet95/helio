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
        title="状态"
        actions={
          <Button variant="secondary" onClick={fetchStatus}>
            <RefreshCw size={15} className={loadingStatus ? 'animate-spin' : ''} />
            刷新
          </Button>
        }
      />

      <div className="max-w-5xl px-4 py-4 sm:px-7 sm:py-5">
        {loadingStatus && !status ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : (
          <>
            <div className="overflow-hidden rounded-lg border border-line bg-card">
              {SUPPORTED_TOOLS.map((tool) => (
                <ToolCard key={tool.id} tool={tool} status={statusForTool(status, tool.id)} />
              ))}
            </div>

            <div className="mt-4 rounded-lg border border-line bg-card p-4">
              <div className="mb-3 flex items-center gap-2">
                <HardDrive size={16} className="text-accent" />
                <h3 className="text-[14px] font-semibold text-ink">数据库</h3>
              </div>
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                <Stat icon={<HardDrive size={16} />} value={status?.database ? formatBytes(status.database.size) : '—'} label="大小" />
                <Stat icon={<Layers size={16} />} value={String(status?.database?.profile_count ?? 0)} label="档案" />
                <div className="rounded-md border border-line bg-surface p-3 sm:col-span-2 lg:col-span-1">
                  <div className="flex items-center gap-1.5 text-ink-faint mb-1.5">
                    <FolderOpen size={14} /><span className="text-[11px] font-medium">路径</span>
                  </div>
                  <div className={`font-mono text-[11px] break-all leading-snug ${status?.database?.path ? 'text-ink-dim' : 'text-ink-faint italic'}`}>
                    {status?.database?.path ?? '未初始化'}
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

function ToolCard({ tool, status }: { tool: ToolInfo; status?: TargetStatus }) {
  const active = !!status?.profile;
  const connected = status?.connected;
  return (
    <div
      className="group relative border-b border-line bg-card px-3.5 py-3 transition-colors duration-150 last:border-b-0 hover:bg-elevated/45"
    >
      <div className="relative flex items-start justify-between">
        <div className="flex items-center gap-3">
          <div className="grid h-9 w-9 place-items-center rounded-md border font-mono text-[12px] font-bold"
               style={{ background: `${tool.color}1a`, color: tool.color, borderColor: `${tool.color}33` }}>
            {tool.short}
          </div>
          <div>
            <div className="text-[14px] font-semibold text-ink">{tool.displayName}</div>
            <div className="font-mono text-[10px] text-ink-faint">{tool.format}</div>
          </div>
        </div>
        <div className="flex items-center gap-1.5">
          <span className={`h-2 w-2 rounded-full ${active ? (connected ? 'bg-ok' : 'bg-warn') : 'bg-ink-faint/40'}`} />
          <span className={`text-[11px] font-medium ${active ? (connected ? 'text-ok' : 'text-warn') : 'text-ink-faint'}`}>
            {active ? (connected ? '在线' : '未连接') : '未设置'}
          </span>
        </div>
      </div>

      {status?.profile ? (
        <div className="relative mt-3 grid grid-cols-1 gap-1.5 sm:grid-cols-2 lg:grid-cols-3">
          <Row label="Profile" value={status.profile.name} strong />
          <Row label="Provider" value={status.profile.provider} />
          <Row label="URL" value={status.profile.api_url} mono />
        </div>
      ) : (
        <div className="relative mt-4 text-[12px] text-ink-faint">未设置</div>
      )}
    </div>
  );
}

function Row({ label, value, mono, strong }: { label: string; value: string; mono?: boolean; strong?: boolean }) {
  return (
    <div className="min-w-0 text-[12.5px]">
      <span className="text-ink-faint">{label}</span>
      <div className={`truncate ${mono ? 'font-mono' : ''} ${strong ? 'text-ink font-medium' : 'text-ink-dim'}`}>{value}</div>
    </div>
  );
}

function Stat({ icon, value, label }: { icon: React.ReactNode; value: string; label: string }) {
  return (
    <div className="rounded-md border border-line bg-surface p-3">
      <div className="flex items-center gap-1.5 text-ink-faint mb-1.5">{icon}<span className="text-[11px] font-medium">{label}</span></div>
      <div className="text-[18px] font-semibold text-ink tabular-nums">{value}</div>
    </div>
  );
}
