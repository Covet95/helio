import { useState, useEffect } from 'react';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { PageHeader } from '../components/common/PageHeader';
import { Save, CheckCircle2, AlertCircle } from 'lucide-react';
import type { TargetApp } from '../types';
import { SUPPORTED_TOOLS } from '../types';

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

  useEffect(() => { loadConfig(); }, [targetApp]);

  const handleSave = async () => {
    try {
      const parsed = JSON.parse(config);
      setSaving(true);
      const { tauriApi } = await import('../lib/tauri');
      await tauriApi.saveSharedConfig(targetApp, parsed);
    } catch (err) {
      if (err instanceof SyntaxError) setError('JSON 格式错误: ' + err.message);
      else setError('保存失败: ' + err);
    } finally {
      setSaving(false);
    }
  };

  const valid = (() => { try { JSON.parse(config); return true; } catch { return false; } })();
  const current = SUPPORTED_TOOLS.find((t) => t.id === targetApp)!;

  return (
    <div className="flex flex-col h-full">
      <PageHeader
        title="Shared Config"
        subtitle="permissions / hooks / MCP / skills — 切换 Profile 时这些配置完整保留"
        actions={
          <Button onClick={handleSave} disabled={!valid || saving}>
            <Save size={15} />{saving ? '保存中…' : '保存'}
          </Button>
        }
      />

      <div className="px-8 py-5 flex-1 flex flex-col min-h-0">
        {/* segmented tool switcher */}
        <div className="inline-flex w-fit items-center gap-1 rounded-xl border border-line bg-surface p-1 mb-4">
          {SUPPORTED_TOOLS.map((tool) => {
            const active = targetApp === tool.id;
            return (
              <button
                key={tool.id}
                onClick={() => setTargetApp(tool.id)}
                className={`relative flex items-center gap-2 rounded-lg px-3.5 py-1.5 text-[13px] font-medium transition-all ${
                  active ? 'text-ink' : 'text-ink-dim hover:text-ink'
                }`}
                style={active ? { background: `${tool.color}1f` } : undefined}
              >
                <span className="h-2 w-2 rounded-full" style={{ background: tool.color }} />
                {tool.displayName}
              </button>
            );
          })}
        </div>

        {error && (
          <div className="mb-3 flex items-center gap-2 rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-[13px] text-danger">
            <AlertCircle size={15} />{error}
          </div>
        )}

        {/* editor */}
        <div className="relative flex-1 min-h-0 rounded-xl border border-line bg-[#0B0B0E] overflow-hidden">
          {/* editor chrome */}
          <div className="flex items-center justify-between border-b border-line/70 px-4 py-2">
            <div className="flex items-center gap-2 font-mono text-[11px] text-ink-faint">
              <span className="h-2.5 w-2.5 rounded-full bg-danger/60" />
              <span className="h-2.5 w-2.5 rounded-full bg-warn/60" />
              <span className="h-2.5 w-2.5 rounded-full bg-ok/60" />
              <span className="ml-2">{current.displayName.toLowerCase().replace(/\s/g, '-')}.shared.{current.format === '.env' ? 'json' : 'json'}</span>
            </div>
            {!loading && (
              valid
                ? <span className="flex items-center gap-1 text-[11px] text-ok"><CheckCircle2 size={13} />valid json</span>
                : <span className="flex items-center gap-1 text-[11px] text-danger"><AlertCircle size={13} />invalid json</span>
            )}
          </div>
          {loading ? (
            <div className="grid place-items-center h-full"><Spinner size="lg" /></div>
          ) : (
            <textarea
              value={config}
              onChange={(e) => { setConfig(e.target.value); setError(''); }}
              spellCheck={false}
              className="h-[calc(100%-37px)] w-full resize-none bg-transparent p-4 font-mono text-[12.5px] leading-relaxed text-ink-dim outline-none placeholder:text-ink-faint"
              placeholder="{ }"
            />
          )}
        </div>
      </div>
    </div>
  );
}
