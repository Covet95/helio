import { useEffect } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { RefreshCw, Database, HardDrive, CheckCircle, XCircle } from 'lucide-react';
import { formatBytes } from '../lib/utils';

export default function StatusPage() {
  const { status, loadingStatus, fetchStatus } = useStore();

  useEffect(() => {
    fetchStatus();
  }, []);

  const handleRefresh = () => {
    fetchStatus();
  };

  if (loadingStatus) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size="lg" />
      </div>
    );
  }

  return (
    <div className="p-8">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-2xl font-bold text-gray-900">系统状态</h2>
        <Button onClick={handleRefresh} variant="secondary" className="gap-2">
          <RefreshCw size={18} />
          刷新
        </Button>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mb-6">
        {/* Claude Code Status */}
        <div className="bg-white rounded-lg shadow p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">Claude Code</h3>
            {status?.claude_code?.connected ? (
              <CheckCircle className="text-success" size={24} />
            ) : (
              <XCircle className="text-gray-400" size={24} />
            )}
          </div>

          {status?.claude_code?.profile ? (
            <div className="space-y-2 text-sm">
              <div className="flex justify-between">
                <span className="text-gray-600">当前 Profile:</span>
                <span className="font-medium">{status.claude_code.profile.name}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-600">Provider:</span>
                <span className="font-medium">{status.claude_code.profile.provider}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-600">API URL:</span>
                <span className="font-medium truncate ml-2">{status.claude_code.profile.api_url}</span>
              </div>
            </div>
          ) : (
            <p className="text-gray-500 text-sm">未配置</p>
          )}
        </div>

        {/* Codex Status */}
        <div className="bg-white rounded-lg shadow p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">Codex</h3>
            {status?.codex?.connected ? (
              <CheckCircle className="text-success" size={24} />
            ) : (
              <XCircle className="text-gray-400" size={24} />
            )}
          </div>

          {status?.codex?.profile ? (
            <div className="space-y-2 text-sm">
              <div className="flex justify-between">
                <span className="text-gray-600">当前 Profile:</span>
                <span className="font-medium">{status.codex.profile.name}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-600">Provider:</span>
                <span className="font-medium">{status.codex.profile.provider}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-600">API URL:</span>
                <span className="font-medium truncate ml-2">{status.codex.profile.api_url}</span>
              </div>
            </div>
          ) : (
            <p className="text-gray-500 text-sm">未配置</p>
          )}
        </div>
      </div>

      {/* Database Info */}
      <div className="bg-white rounded-lg shadow p-6">
        <div className="flex items-center gap-2 mb-4">
          <Database size={24} className="text-primary" />
          <h3 className="text-lg font-semibold">数据库信息</h3>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
          <div className="text-center p-4 bg-gray-50 rounded-lg">
            <HardDrive size={32} className="mx-auto mb-2 text-gray-600" />
            <div className="text-2xl font-bold text-gray-900">
              {status?.database ? formatBytes(status.database.size) : '-'}
            </div>
            <div className="text-sm text-gray-600">数据库大小</div>
          </div>

          <div className="text-center p-4 bg-gray-50 rounded-lg">
            <Database size={32} className="mx-auto mb-2 text-gray-600" />
            <div className="text-2xl font-bold text-gray-900">
              {status?.database?.profile_count ?? 0}
            </div>
            <div className="text-sm text-gray-600">Profiles 数量</div>
          </div>

          <div className="col-span-1 md:col-span-1 p-4 bg-gray-50 rounded-lg">
            <div className="text-sm text-gray-600 mb-1">数据库路径:</div>
            <div className="text-xs font-mono text-gray-800 break-all">
              {status?.database?.path ?? '-'}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
