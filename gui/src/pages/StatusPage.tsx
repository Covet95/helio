import { useEffect, useState, type ReactNode } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { RefreshCw, HardDrive, Layers, FolderOpen, Activity } from 'lucide-react';
import { formatBytes, humanizeError } from '../lib/utils';
import { contextBadgeLabel, statusKeyFor } from '../lib/contextWindow';
import { tauriApi } from '../lib/tauri';
import { SUPPORTED_TOOLS } from '../types';
import type { StatusInfo, TargetApp, TargetStatus, ToolInfo, ToolProbeResult } from '../types';

function statusForTool(status: StatusInfo | null, id: string): TargetStatus | undefined {
  if (!status) return undefined;
  const key = statusKeyFor(id) as keyof StatusInfo;
  const v = status[key];
  if (!v || typeof v !== 'object' || !('connected' in (v as object) || 'profile' in (v as object))) {
    return undefined;
  }
  return v as TargetStatus;
}

export default function StatusPage() {
  const { status, loadingStatus, fetchStatus, lastError, clearError } = useStore();
  const [probing, setProbing] = useState(false);
  const [probeMap, setProbeMap] = useState<Record<string, ToolProbeResult>>({});
  const [probeErr, setProbeErr] = useState('');

  useEffect(() => { fetchStatus(); }, [fetchStatus]);

  const runProbe = async () => {
    setProbing(true);
    setProbeErr('');
    try {
      const list = await tauriApi.probeActiveProfiles();
      const map: Record<string, ToolProbeResult> = {};
      for (const r of list) map[r.target_app] = r as ToolProbeResult;
      setProbeMap(map);
    } catch (e) {
      setProbeErr(humanizeError(e));
    } finally {
      setProbing(false);
    }
  };

  return (
    <div className="min-h-full">
      <PageHeader
        title="状态"
        actions={
          <div className="flex items-center gap-2">
            <Button variant="secondary" onClick={runProbe} disabled={probing}>
              <Activity size={15} className={probing ? 'animate-pulse' : ''} />
              {probing ? '检测中…' : '检测连通性'}
            </Button>
            <Button variant="secondary" onClick={fetchStatus}>
              <RefreshCw size={15} className={loadingStatus ? 'animate-spin' : ''} />
              刷新
            </Button>
          </div>
        }
      />

      <div className="max-w-5xl px-4 py-4 sm:px-7 sm:py-5">
        {(probeErr || lastError) && (
          <div className="mb-3 rounded-md border border-danger/30 bg-danger/8 px-3 py-2 text-[12.5px] text-danger">
            <div className="flex items-start justify-between gap-2">
              <span>{probeErr || lastError}</span>
              {lastError && !probeErr && (
                <button type="button" className="underline text-[11px]" onClick={clearError}>关闭</button>
              )}
            </div>
          </div>
        )}
        {loadingStatus && !status ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : (
          <>
            <div className="overflow-hidden rounded-lg border border-line bg-card">
              {SUPPORTED_TOOLS.map((tool) => (
                <ToolCard
                  key={tool.id}
                  tool={tool}
                  status={statusForTool(status, tool.id)}
                  probe={probeMap[tool.id]}
                />
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

function ToolCard({
  tool, status, probe,
}: {
  tool: ToolInfo;
  status?: TargetStatus;
  probe?: ToolProbeResult;
}) {
  const configured = !!(status?.profile || status?.connected);
  let badge = configured ? '已配置' : '未设置';
  let badgeClass = configured ? 'text-ok' : 'text-ink-faint';
  let dotClass = configured ? 'bg-ok' : 'bg-ink-faint/40';
  if (probe) {
    const ms = probe.latency_ms != null ? ` ${probe.latency_ms}ms` : '';
    if (probe.ok && probe.status === 'degraded') {
      badge = `较慢${ms}`;
      badgeClass = 'text-warn';
      dotClass = 'bg-warn';
    } else if (probe.ok) {
      badge = `可达${ms}`;
      badgeClass = 'text-ok';
      dotClass = 'bg-ok';
    } else if (probe.configured) {
      badge = '不可达';
      badgeClass = 'text-danger';
      dotClass = 'bg-danger';
    }
  }
  const p = status?.profile;
  const ctx = p ? contextBadgeLabel(p.context_1m, p.model, { tool: tool.id as TargetApp }) : null;
  const protocol = p && tool.id === 'opencode' ? p.opencode_api_mode : p?.api_mode;
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
          <span className={`h-2 w-2 rounded-full ${dotClass}`} />
          <span className={`text-[11px] font-medium ${badgeClass}`}>
            {badge}
          </span>
        </div>
      </div>

      {p ? (
        <div className="relative mt-3 grid grid-cols-1 gap-1.5 sm:grid-cols-2 lg:grid-cols-3">
          <Row label="Profile" value={p.name} strong />
          <Row label="Provider" value={p.provider} />
          <Row label="Model" value={p.model || '—'} mono />
          <Row label="Context" value={ctx ? `ctx ${ctx}` : '—'} />
          {protocol && (
            <Row
              label={tool.id === 'opencode' ? 'opencode_api_mode' : 'api_mode'}
              value={protocol}
              mono
            />
          )}
          <Row label="URL" value={p.api_url} mono />
          {probe?.http_status != null && (
            <Row label="HTTP" value={String(probe.http_status)} mono />
          )}
          {probe?.error && <Row label="错误" value={probe.error} />}
        </div>
      ) : (
        <div className="relative mt-4 text-[12px] text-ink-faint">未设置</div>
      )}
    </div>
  );
}

function Row({ label, value, mono, strong }: { label: string; value: string; mono?: boolean; strong?: boolean }) {
  return (
    <div className="rounded-md border border-line bg-surface px-2.5 py-1.5">
      <div className="text-[10px] font-medium text-ink-faint">{label}</div>
      <div className={`truncate text-[12px] ${strong ? 'font-semibold text-ink' : 'text-ink-dim'} ${mono ? 'font-mono' : ''}`}>
        {value}
      </div>
    </div>
  );
}

function Stat({ icon, value, label }: { icon: ReactNode; value: string; label: string }) {
  return (
    <div className="rounded-md border border-line bg-surface p-3">
      <div className="mb-1.5 flex items-center gap-1.5 text-ink-faint">{icon}<span className="text-[11px] font-medium">{label}</span></div>
      <div className="text-[16px] font-semibold text-ink">{value}</div>
    </div>
  );
}
