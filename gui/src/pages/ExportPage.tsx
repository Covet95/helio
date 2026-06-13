import { useState } from 'react';
import { Button } from '../components/common/Button';
import { Download, Upload, Info } from 'lucide-react';

export default function ExportPage() {
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [message, setMessage] = useState('');

  const handleExport = async () => {
    try {
      setExporting(true);
      setMessage('');

      // 使用 Tauri 的 save 对话框
      const { save } = await import('@tauri-apps/plugin-dialog');
      const filePath = await save({
        defaultPath: `switch-api-backup-${Date.now()}.db`,
        filters: [{
          name: 'Database',
          extensions: ['db', 'sqlite']
        }]
      });

      if (!filePath) {
        setMessage('导出已取消');
        return;
      }

      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.exportDatabase(filePath);
      setMessage('✅ 数据库导出成功！');
    } catch (err) {
      setMessage('❌ 导出失败: ' + err);
    } finally {
      setExporting(false);
    }
  };

  const handleImport = async () => {
    if (!confirm('导入将覆盖当前数据库（会自动备份）。是否继续？')) {
      return;
    }

    try {
      setImporting(true);
      setMessage('');

      // 使用 Tauri 的 open 对话框
      const { open } = await import('@tauri-apps/plugin-dialog');
      const filePath = await open({
        multiple: false,
        filters: [{
          name: 'Database',
          extensions: ['db', 'sqlite']
        }]
      });

      if (!filePath) {
        setMessage('导入已取消');
        return;
      }

      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.importDatabase(filePath as string);
      setMessage('✅ 数据库导入成功！请刷新页面');

      // 刷新页面
      setTimeout(() => window.location.reload(), 2000);
    } catch (err) {
      setMessage('❌ 导入失败: ' + err);
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="p-8">
      <h2 className="text-2xl font-bold text-gray-900 mb-6">数据库管理</h2>

      {message && (
        <div className={`mb-6 p-4 rounded-lg ${
          message.startsWith('✅') ? 'bg-green-50 text-green-800' :
          message.startsWith('❌') ? 'bg-red-50 text-red-800' :
          'bg-blue-50 text-blue-800'
        }`}>
          {message}
        </div>
      )}

      <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
        {/* 导出卡片 */}
        <div className="bg-white rounded-lg shadow p-6">
          <div className="flex items-center gap-3 mb-4">
            <Download className="text-primary" size={24} />
            <h3 className="text-lg font-semibold">导出数据库</h3>
          </div>
          <p className="text-gray-600 mb-4 text-sm">
            将所有 Profiles 和配置导出为单个文件，用于备份或迁移到其他设备
          </p>
          <Button
            onClick={handleExport}
            disabled={exporting}
            className="w-full gap-2"
          >
            <Download size={18} />
            {exporting ? '导出中...' : '导出数据库'}
          </Button>
        </div>

        {/* 导入卡片 */}
        <div className="bg-white rounded-lg shadow p-6">
          <div className="flex items-center gap-3 mb-4">
            <Upload className="text-warning" size={24} />
            <h3 className="text-lg font-semibold">导入数据库</h3>
          </div>
          <p className="text-gray-600 mb-4 text-sm">
            从备份文件恢复数据库。当前数据库会自动备份（.backup 扩展名）
          </p>
          <Button
            onClick={handleImport}
            disabled={importing}
            variant="secondary"
            className="w-full gap-2"
          >
            <Upload size={18} />
            {importing ? '导入中...' : '导入数据库'}
          </Button>
        </div>
      </div>

      {/* 说明 */}
      <div className="mt-6 bg-blue-50 border border-blue-200 rounded-lg p-4">
        <div className="flex gap-3">
          <Info className="text-blue-600 flex-shrink-0" size={20} />
          <div className="text-sm text-blue-900">
            <p className="font-semibold mb-2">📌 使用说明</p>
            <ul className="space-y-1">
              <li>• 数据库包含所有 API Profiles 和共享配置</li>
              <li>• 导出的 .db 文件可以在任何设备上导入</li>
              <li>• 导入会自动备份当前数据库</li>
              <li>• 团队协作：可以导出后分享给团队成员</li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
}
