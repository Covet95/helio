import { useState } from 'react';
import { Button } from '../components/common/Button';
import { PageHeader } from '../components/common/PageHeader';
import { Download, Upload, Database, ShieldCheck } from 'lucide-react';

export default function ExportPage() {
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [message, setMessage] = useState('');

  const handleExport = async () => {
    try {
      setExporting(true);
      setMessage('');
      const { save } = await import('@tauri-apps/plugin-dialog');
      const filePath = await save({
        defaultPath: `switch-api-backup-${Date.now()}.db`,
        filters: [{ name: 'Database', extensions: ['db', 'sqlite'] }],
      });
      if (!filePath) { setMessage('导出已取消'); return; }
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.exportDatabase(filePath);
      setMessage('✅ 数据库导出成功');
    } catch (err) {
      setMessage('❌ 导出失败: ' + err);
    } finally {
      setExporting(false);
    }
  };

  const handleImport = async () => {
    if (!confirm('导入将覆盖当前数据库（会自动备份）。是否继续？')) return;
    try {
      setImporting(true);
      setMessage('');
      const { open } = await import('@tauri-apps/plugin-dialog');
      const filePath = await open({
        multiple: false,
        filters: [{ name: 'Database', extensions: ['db', 'sqlite'] }],
      });
      if (!filePath) { setMessage('导入已取消'); return; }
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.importDatabase(filePath as string);
      setMessage('✅ 数据库导入成功，正在刷新…');
      setTimeout(() => window.location.reload(), 1500);
    } catch (err) {
      setMessage('❌ 导入失败: ' + err);
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="min-h-full">
      <PageHeader title="Import / Export" subtitle="单文件数据库，便于备份、迁移与团队共享" />

      <div className="px-8 py-6 max-w-3xl">
        {message && (
          <div className={`mb-5 rounded-lg border px-4 py-2.5 text-[13px] animate-fade-up ${
            message.startsWith('✅') ? 'border-ok/30 bg-ok/10 text-ok'
            : message.startsWith('❌') ? 'border-danger/30 bg-danger/10 text-danger'
            : 'border-line bg-surface text-ink-dim'
          }`}>
            {message}
          </div>
        )}

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <ActionCard
            icon={<Download size={20} className="text-accent" />}
            tint="#3B82F6"
            title="导出数据库"
            desc="将所有 Profiles 与共享配置导出为单个 .db 文件"
            button={<Button onClick={handleExport} disabled={exporting} className="w-full"><Download size={16} />{exporting ? '导出中…' : '导出'}</Button>}
          />
          <ActionCard
            icon={<Upload size={20} className="text-opencode" />}
            tint="#A78BFA"
            title="导入数据库"
            desc="从备份文件恢复，当前数据库会自动备份为 .backup"
            button={<Button variant="secondary" onClick={handleImport} disabled={importing} className="w-full"><Upload size={16} />{importing ? '导入中…' : '导入'}</Button>}
          />
        </div>

        <div className="mt-4 rounded-xl border border-line bg-surface/50 p-4">
          <div className="flex items-center gap-2 mb-2.5">
            <ShieldCheck size={15} className="text-ok" />
            <span className="text-[13px] font-medium text-ink">安全说明</span>
          </div>
          <ul className="space-y-1.5 text-[12.5px] text-ink-dim">
            <li className="flex gap-2"><Dot />数据库包含所有 API Profiles 与各工具共享配置</li>
            <li className="flex gap-2"><Dot />导出的 .db 文件可在任意设备导入，适合团队分享</li>
            <li className="flex gap-2"><Dot />导入前自动备份当前库，操作可回滚</li>
          </ul>
        </div>
      </div>
    </div>
  );
}

function ActionCard({ icon, title, desc, button, tint }: {
  icon: React.ReactNode; title: string; desc: string; button: React.ReactNode; tint: string;
}) {
  return (
    <div className="group relative overflow-hidden rounded-xl border border-line bg-card p-5 transition-all duration-300 hover:border-line-strong">
      <div className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-500 group-hover:opacity-100"
           style={{ backgroundImage: `linear-gradient(135deg, ${tint}10, transparent 70%)` }} />
      <div className="relative">
        <div className="grid place-items-center h-11 w-11 rounded-xl border border-line bg-surface mb-3.5">{icon}</div>
        <h3 className="text-[14.5px] font-semibold text-ink mb-1">{title}</h3>
        <p className="text-[12.5px] text-ink-dim mb-4 leading-relaxed">{desc}</p>
        {button}
      </div>
    </div>
  );
}

function Dot() {
  return <span className="mt-1.5 h-1 w-1 shrink-0 rounded-full bg-ink-faint" />;
}

// silence unused import in case Database icon wanted later
void Database;
