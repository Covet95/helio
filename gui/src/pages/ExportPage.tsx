import { useState } from 'react';
import { Button } from '../components/common/Button';
import { PageHeader } from '../components/common/PageHeader';
import { Download, Upload } from 'lucide-react';
import { ConfirmDialog } from '../components/common/Modal';
import { humanizeError } from '../lib/utils';

type Feedback = { text: string; kind: 'success' | 'error' | 'info' };

export default function ExportPage() {
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [confirmImport, setConfirmImport] = useState(false);

  const handleExport = async () => {
    try {
      setExporting(true);
      setFeedback(null);
      const { save } = await import('@tauri-apps/plugin-dialog');
      const filePath = await save({
        defaultPath: `helio-backup-${Date.now()}.db`,
        filters: [{ name: 'Database', extensions: ['db', 'sqlite'] }],
      });
      if (!filePath) { setFeedback({ text: '导出已取消', kind: 'info' }); return; }
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.exportDatabase(filePath);
      setFeedback({ text: '数据库导出成功', kind: 'success' });
    } catch (err) {
      setFeedback({ text: `导出失败: ${humanizeError(err)}`, kind: 'error' });
    } finally {
      setExporting(false);
    }
  };

  const handleImport = async () => {
    try {
      setImporting(true);
      setConfirmImport(false);
      setFeedback(null);
      const { open } = await import('@tauri-apps/plugin-dialog');
      const filePath = await open({
        multiple: false,
        filters: [{ name: 'Database', extensions: ['db', 'sqlite'] }],
      });
      if (!filePath) { setFeedback({ text: '导入已取消', kind: 'info' }); return; }
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.importDatabase(filePath as string);
      setFeedback({ text: '数据库导入成功，正在刷新…', kind: 'success' });
      // soft reload app data (avoid full document reload in Tauri)
      try {
        const { useStore } = await import('../store');
        await useStore.getState().fetchProfiles();
        await useStore.getState().fetchStatus();
      } catch {
        window.location.reload();
      }
    } catch (err) {
      setFeedback({ text: `导入失败: ${humanizeError(err)}`, kind: 'error' });
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="min-h-full">
      <PageHeader title="备份 / 恢复" />

      <div className="max-w-3xl px-4 py-4 sm:px-7 sm:py-5">
        {feedback && (
          <div className={`mb-4 rounded-md border px-3 py-2 text-[13px] ${
            feedback.kind === 'success' ? 'border-ok/30 bg-ok/10 text-ok'
            : feedback.kind === 'error' ? 'border-danger/30 bg-danger/10 text-danger'
            : 'border-line bg-surface text-ink-dim'
          }`}>
            {feedback.text}
          </div>
        )}

        <div className="overflow-hidden rounded-lg border border-line bg-card">
          <ActionRow
            icon={<Download size={20} className="text-accent" />}
            title="导出数据库"
            meta=".db / .sqlite"
            button={<Button onClick={handleExport} disabled={exporting}><Download size={16} />{exporting ? '导出中…' : '导出'}</Button>}
          />
          <ActionRow
            icon={<Upload size={20} className="text-opencode" />}
            title="导入数据库"
            meta="覆盖前自动备份"
            button={<Button variant="secondary" onClick={() => setConfirmImport(true)} disabled={importing}><Upload size={16} />{importing ? '导入中…' : '导入'}</Button>}
          />
        </div>
      </div>

      {confirmImport && (
        <ConfirmDialog
          title="导入数据库"
          message="当前数据库会被覆盖。导入前会自动备份当前库（带时间戳保留，可回退），且仅在所选文件是合法数据库时才执行。"
          confirmText="导入"
          danger
          onCancel={() => setConfirmImport(false)}
          onConfirm={handleImport}
        />
      )}
    </div>
  );
}

function ActionRow({ icon, title, meta, button }: {
  icon: React.ReactNode; title: string; meta: string; button: React.ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-4 border-b border-line px-4 py-3.5 last:border-b-0">
      <div className="flex min-w-0 items-center gap-3">
        <div className="grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line bg-surface">{icon}</div>
        <div className="min-w-0">
          <h3 className="truncate text-[14px] font-semibold text-ink">{title}</h3>
          <p className="truncate text-[12px] text-ink-faint">{meta}</p>
        </div>
      </div>
      {button}
    </div>
  );
}
