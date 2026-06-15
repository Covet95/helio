import { useEffect, useState, useCallback } from 'react';
import { PageHeader } from '../components/common/PageHeader';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { RefreshCw, Trash2, Search, Eye } from 'lucide-react';
import { tauriApi } from '../lib/tauri';
import { formatBytes } from '../lib/utils';
import type { SessionMeta, PreviewMessage } from '../types';

const TOOLS = [
  { id: '', label: '全部' },
  { id: 'codex', label: 'Codex' },
  { id: 'claude-code', label: 'Claude Code' },
];

export default function HistoryPage() {
  const [list, setList] = useState<SessionMeta[]>([]);
  const [loading, setLoading] = useState(false);
  const [tool, setTool] = useState('');
  const [search, setSearch] = useState('');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [preview, setPreview] = useState<{ meta: SessionMeta; msgs: PreviewMessage[] } | null>(null);

  const key = (m: SessionMeta) => `${m.tool}:${m.id}`;

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const data = await tauriApi.listSessions(tool || undefined, search || undefined);
      setList(data);
      setSelected(new Set());
    } finally {
      setLoading(false);
    }
  }, [tool, search]);

  useEffect(() => { load(); }, [tool]);

  const toggle = (m: SessionMeta) => {
    const k = key(m);
    setSelected((s) => {
      const n = new Set(s);
      n.has(k) ? n.delete(k) : n.add(k);
      return n;
    });
  };

  const deleteOne = async (m: SessionMeta) => {
    if (!confirm(`删除会话？\n${m.tool} · ${m.cwd}\n将移到系统垃圾桶（可恢复）`)) return;
    await tauriApi.deleteSession(m.tool, m.id);
    load();
  };

  const deleteSelected = async () => {
    const items = list.filter((m) => selected.has(key(m))).map((m) => ({ tool: m.tool, id: m.id }));
    if (!items.length) return;
    if (!confirm(`批量删除 ${items.length} 个会话？将移到系统垃圾桶（可恢复）`)) return;
    await tauriApi.deleteSessions(items);
    load();
  };

  const cleanup = async () => {
    const days = Number(prompt('清理多少天前的会话？', '30'));
    if (!days || days <= 0) return;
    if (!confirm(`清理 ${days} 天前的会话？将移到系统垃圾桶（可恢复）`)) return;
    await tauriApi.cleanupSessions(tool || undefined, days);
    load();
  };

  const openPreview = async (m: SessionMeta) => {
    const msgs = await tauriApi.readSessionPreview(m.tool, m.id);
    setPreview({ meta: m, msgs });
  };

  return (
    <div className="min-h-full">
      <PageHeader
        title="会话历史"
        actions={
          <div className="flex items-center gap-2">
            <Button variant="secondary" onClick={cleanup}><Trash2 size={15} />快捷清理</Button>
            {selected.size > 0 && (
              <Button variant="danger" onClick={deleteSelected}><Trash2 size={15} />删除选中 ({selected.size})</Button>
            )}
            <Button variant="secondary" onClick={load}>
              <RefreshCw size={15} className={loading ? 'animate-spin' : ''} />刷新
            </Button>
          </div>
        }
      />

      <div className="max-w-5xl px-4 py-4 sm:px-7 sm:py-5">
        <div className="mb-3 flex items-center gap-2">
          <div className="flex rounded-md border border-line bg-surface p-0.5">
            {TOOLS.map((t) => (
              <button key={t.id} onClick={() => setTool(t.id)}
                className={`rounded px-3 py-1 text-[12px] font-medium ${tool === t.id ? 'bg-elevated text-ink' : 'text-ink-dim'}`}>
                {t.label}
              </button>
            ))}
          </div>
          <div className="relative flex-1">
            <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-faint" />
            <input value={search} onChange={(e) => setSearch(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && load()}
              placeholder="搜索 cwd 或标题…"
              className="w-full rounded-md border border-line bg-surface py-1.5 pl-8 pr-3 text-[13px] text-ink outline-none focus:border-accent" />
          </div>
        </div>

        {loading && !list.length ? (
          <div className="grid place-items-center py-32"><Spinner size="lg" /></div>
        ) : !list.length ? (
          <div className="py-24 text-center text-[13px] text-ink-faint">没有会话</div>
        ) : (
          <div className="overflow-hidden rounded-lg border border-line bg-card">
            {list.map((m) => (
              <div key={key(m)} className="flex items-center gap-3 border-b border-line px-3.5 py-2.5 last:border-b-0 hover:bg-elevated/45">
                <input type="checkbox" checked={selected.has(key(m))} onChange={() => toggle(m)} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-[10px] text-accent">{m.tool}</span>
                    <span className="truncate text-[13px] text-ink">{m.title ?? m.cwd ?? m.id}</span>
                    {!m.parseable && <span className="text-[10px] text-warn">不可解析</span>}
                  </div>
                  <div className="truncate font-mono text-[10.5px] text-ink-faint">{m.cwd}</div>
                </div>
                <div className="text-right text-[10.5px] text-ink-faint tabular-nums">
                  <div>{new Date(m.modified_at * 1000).toLocaleString()}</div>
                  <div>{formatBytes(m.size_bytes)} · {m.message_count} 条</div>
                </div>
                <button onClick={() => openPreview(m)} className="rounded p-1.5 text-ink-dim hover:text-ink hover:bg-elevated" title="预览">
                  <Eye size={15} />
                </button>
                <button onClick={() => deleteOne(m)} className="rounded p-1.5 text-ink-dim hover:text-danger hover:bg-elevated" title="删除">
                  <Trash2 size={15} />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>

      {preview && (
        <div className="fixed inset-0 z-50 flex justify-end bg-black/30" onClick={() => setPreview(null)}>
          <div className="h-full w-[480px] overflow-y-auto bg-card p-4 shadow-xl" onClick={(e) => e.stopPropagation()}>
            <div className="mb-3 flex items-center justify-between">
              <div className="text-[13px] font-semibold text-ink">{preview.meta.title ?? preview.meta.cwd}</div>
              <button onClick={() => setPreview(null)} className="text-ink-faint hover:text-ink">✕</button>
            </div>
            <div className="space-y-3">
              {preview.msgs.length === 0 && <div className="text-[12px] text-ink-faint">无可预览内容</div>}
              {preview.msgs.map((msg, i) => (
                <div key={i} className="rounded-md border border-line bg-surface p-2.5">
                  <div className="mb-1 font-mono text-[10px] text-accent">{msg.role}</div>
                  <div className="whitespace-pre-wrap text-[12px] text-ink-dim">{msg.text}</div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
