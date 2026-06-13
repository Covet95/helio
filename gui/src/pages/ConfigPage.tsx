import { useState, useEffect } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import type { TargetApp } from '../types';

export default function ConfigPage() {
  const [targetApp, setTargetApp] = useState<TargetApp>('claude-code');
  const [config, setConfig] = useState('');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  const loadConfig = async () => {
    setLoading(true);
    setError('');
    try {
      const { tauriApi } = await import('../lib/tauri');
      const result = await tauriApi.getSharedConfig(targetApp);
      setConfig(result ? JSON.stringify(result, null, 2) : '{}');
    } catch (err) {
      setError('加载配置失败: ' + err);
      setConfig('{}');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadConfig();
  }, [targetApp]);

  const handleSave = async () => {
    try {
      const parsed = JSON.parse(config);
      setSaving(true);
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.saveSharedConfig(targetApp, parsed);
      alert('配置保存成功！');
    } catch (err) {
      if (err instanceof SyntaxError) {
        setError('JSON 格式错误: ' + err.message);
      } else {
        setError('保存失败: ' + err);
      }
    } finally {
      setSaving(false);
    }
  };

  const isValidJson = () => {
    try {
      JSON.parse(config);
      return true;
    } catch {
      return false;
    }
  };

  return (
    <div className="p-8 h-full flex flex-col">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-2xl font-bold text-gray-900">共享配置</h2>

        <div className="flex gap-4 items-center">
          <div className="flex gap-2">
            <button
              onClick={() => setTargetApp('claude-code')}
              className={`px-4 py-2 rounded-lg transition-colors ${
                targetApp === 'claude-code'
                  ? 'bg-primary text-white'
                  : 'bg-gray-200 text-gray-700 hover:bg-gray-300'
              }`}
            >
              Claude Code
            </button>
            <button
              onClick={() => setTargetApp('codex')}
              className={`px-4 py-2 rounded-lg transition-colors ${
                targetApp === 'codex'
                  ? 'bg-primary text-white'
                  : 'bg-gray-200 text-gray-700 hover:bg-gray-300'
              }`}
            >
              Codex
            </button>
          </div>

          <Button
            onClick={handleSave}
            disabled={!isValidJson() || saving}
          >
            {saving ? '保存中...' : '保存配置'}
          </Button>
        </div>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-50 border border-red-200 text-red-700 rounded-lg">
          {error}
        </div>
      )}

      {loading ? (
        <div className="flex-1 flex items-center justify-center">
          <Spinner size="lg" />
        </div>
      ) : (
        <div className="flex-1 flex flex-col">
          <textarea
            value={config}
            onChange={(e) => {
              setConfig(e.target.value);
              setError('');
            }}
            className="flex-1 p-4 border rounded-lg font-mono text-sm resize-none focus:ring-2 focus:ring-primary focus:border-transparent"
            placeholder="输入 JSON 配置..."
            spellCheck={false}
          />

          <div className="mt-2 text-sm text-gray-500">
            {isValidJson() ? (
              <span className="text-green-600">✓ JSON 格式正确</span>
            ) : (
              <span className="text-red-600">✗ JSON 格式错误</span>
            )}
          </div>
        </div>
      )}

      <div className="mt-4 p-4 bg-blue-50 border border-blue-200 rounded-lg">
        <h3 className="font-semibold text-blue-900 mb-2">💡 提示</h3>
        <ul className="text-sm text-blue-800 space-y-1">
          <li>• 这里配置的是 permissions、hooks、MCP 等共享配置</li>
          <li>• 切换 Profile 时，API 配置会替换，但这些共享配置会保留</li>
          <li>• 支持标准的 Claude Code / Codex 配置格式</li>
        </ul>
      </div>
    </div>
  );
}
