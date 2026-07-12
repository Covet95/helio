import { useState } from 'react';
import type { ApiProfile, FetchedModel, TargetApp } from '../../types';
import { SUPPORTED_TOOLS } from '../../types';
import { Button } from '../../components/common/Button';
import { Modal, Field } from '../../components/common/Modal';
import { PROVIDER_PRESETS, REASONING_LEVELS } from '../../lib/presets';
import { cn } from '../../lib/utils';
import { emptyProfileForTool } from './helpers';

export function ProfileModal({
  profile, initialTool, onClose, onSave,
}: {
  profile: ApiProfile | null;
  initialTool: TargetApp;
  onClose: () => void;
  onSave: (p: ApiProfile) => void;
}) {
  const initialProfile = profile;
  const initialModalTool = initialProfile?.target_app ?? initialTool;
  const [tool, setTool] = useState<TargetApp>(initialModalTool);
  const [form, setForm] = useState<ApiProfile>(
    initialProfile || emptyProfileForTool(initialModalTool),
  );
  const [models, setModels] = useState<FetchedModel[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [checkingApi, setCheckingApi] = useState(false);
  const [apiHealth, setApiHealth] = useState<{ kind: 'success' | 'error'; text: string } | null>(null);
  const [modelErr, setModelErr] = useState('');
  const [formErr, setFormErr] = useState('');

  const loadModels = async () => {
    if (!form.api_url.trim() || !form.api_key.trim()) {
      setModelErr('先填 API URL 和 API Key');
      return;
    }
    setLoadingModels(true);
    setModelErr('');
    try {
      const { tauriApi } = await import('../../lib/tauri');
      const list = await tauriApi.fetchModels(form.api_url, form.api_key);
      setModels(list);
      if (list.length === 0) setModelErr('该端点没有返回模型');
    } catch (e) {
      setModelErr(String(e));
      setModels([]);
    } finally {
      setLoadingModels(false);
    }
  };

  const testConnection = async () => {
    setCheckingApi(true);
    setApiHealth(null);
    setModelErr('');
    try {
      const { tauriApi } = await import('../../lib/tauri');
      const model = form.model?.trim() || form.models?.[0]?.trim() || '';
      if (!form.api_url.trim() || !form.api_key.trim()) {
        setApiHealth({ kind: 'error', text: '先填 API URL 和 API Key' });
        return;
      }
      if (!model) {
        setApiHealth({ kind: 'error', text: '先选择或填写模型' });
        return;
      }
      const result = await tauriApi.testModel(form.api_url, form.api_key, model, form.wire_api);
      const wireLabel = form.wire_api === 'responses' ? ' · Responses' : form.wire_api === 'chat' ? ' · Chat' : '';
      setApiHealth({ kind: 'success', text: `模型 ${result.model} 可用${wireLabel}` });
    } catch (error) {
      setApiHealth({ kind: 'error', text: error instanceof Error ? error.message : String(error) });
    } finally {
      setCheckingApi(false);
    }
  };

  const presets = PROVIDER_PRESETS[tool];
  const showModelParams = tool === 'codex' || tool === 'claude-code' || tool === 'opencode' || tool === 'hermes' || tool === 'openclaw';

  const applyPreset = (p: typeof presets[number]) => {
    setForm((f) => ({
      ...f,
      provider: p.provider,
      api_url: p.api_url || f.api_url,
      model: p.model ?? f.model,
    }));
  };

  // 显式提交：footer 的保存按钮在 <form> 之外（Modal 把 footer 渲染成 form 的兄弟节点），
  // 靠 button[form=id] 跨 DOM 关联在 WKWebView 里不可靠，改为直接调用，并自己做必填校验。
  const submit = () => {
    if (!form.name.trim() || !form.provider.trim() || !form.api_url.trim() || !form.api_key.trim()) {
      setFormErr('请填写名称、Provider、API URL、API Key');
      return;
    }
    setFormErr('');
    onSave({ ...form, target_app: tool });
  };

  return (
    <Modal
      title={profile ? '编辑配置档案' : '新建配置档案'}
      onClose={onClose}
      size="lg"
      footer={
        <>
          <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
          <Button type="button" onClick={submit}>{profile ? '保存' : '创建'}</Button>
        </>
      }
    >
      <form
        id="helio-profile-form"
        onSubmit={(e) => { e.preventDefault(); submit(); }}
        className="space-y-4"
      >
          {formErr && (
            <div className="rounded-md border border-danger/30 bg-danger/8 px-3 py-2 text-[12.5px] text-danger">
              {formErr}
            </div>
          )}
          {!initialProfile && (
            <div>
              <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">目标工具</span>
              <div className="flex flex-wrap gap-1.5">
                {SUPPORTED_TOOLS.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    onClick={() => setTool(t.id)}
                    className={`whitespace-nowrap rounded-md px-3 py-1.5 text-[12.5px] font-medium border transition-all ${
                      tool === t.id ? 'border-accent text-accent bg-accent/8' : 'border-line text-ink-dim hover:border-line-strong'
                    }`}
                  >
                    {t.displayName}
                  </button>
                ))}
              </div>
            </div>
          )}

          {!initialProfile && (
            <div>
              <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">Provider 预设</span>
              <div className="flex flex-wrap gap-1.5">
                {presets.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    onClick={() => applyPreset(p)}
                    className="whitespace-nowrap rounded-md border border-line px-3 py-1.5 text-[12.5px] text-ink-dim transition-all hover:border-accent hover:text-accent"
                  >
                    {p.label}
                  </button>
                ))}
              </div>
            </div>
          )}

          <Field label="名称" value={form.name} required
                 onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="my-proxy" />
          <Field label="Provider" value={form.provider} required
                 onChange={(e) => setForm({ ...form, provider: e.target.value })} placeholder="anthropic / openai / google" />
          <Field label="API URL" type="url" value={form.api_url} required mono
                 onChange={(e) => { setForm({ ...form, api_url: e.target.value }); setApiHealth(null); }} />
          <Field label="API Key" type="password" value={form.api_key} required mono
                 onChange={(e) => { setForm({ ...form, api_key: e.target.value }); setApiHealth(null); }} placeholder="sk-..." />

          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" variant="secondary" size="sm" onClick={testConnection} disabled={checkingApi}>
              {checkingApi ? '测试中…' : '测试模型'}
            </Button>
            {apiHealth && (
              <span className={cn(
                'text-[12px]',
                apiHealth.kind === 'success' ? 'text-ok' : 'text-danger',
              )}>
                {apiHealth.text}
              </span>
            )}
          </div>

          {showModelParams && (
            <div className="rounded-lg border border-line bg-surface/60 p-3.5 space-y-3.5">
              <div className="text-[12px] font-medium text-ink-dim">模型参数</div>
              <Field label="默认模型" value={form.model || ''} mono
                     onChange={(e) => setForm({ ...form, model: e.target.value })} placeholder="gpt-5.5 / claude-opus-4" />
              <div>
                <div className="flex items-center gap-2">
                  <Button type="button" variant="secondary" size="sm" onClick={loadModels} disabled={loadingModels}>
                    {loadingModels ? '加载中…' : '加载模型列表'}
                  </Button>
                  {models.length > 0 && (
                    <select
                      value={form.model || ''}
                      onChange={(e) => setForm({ ...form, model: e.target.value })}
                      className="h-8 flex-1 rounded-md border border-line bg-card px-2 text-[12.5px] text-ink outline-none focus:border-accent/50"
                    >
                      <option value="">（选择模型）</option>
                      {models.map((m) => (
                        <option key={m.id} value={m.id}>{m.id}</option>
                      ))}
                    </select>
                  )}
                </div>
                {modelErr && <div className="mt-1 text-[11px] text-danger">{modelErr}</div>}
                {models.length > 0 && <div className="mt-1 text-[11px] text-ink-faint">已加载 {models.length} 个模型；也可在上方输入框自定义</div>}
              </div>

              {tool === 'opencode' && models.length > 0 && (
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between">
                    <span className="text-[12px] font-medium text-ink-dim">
                      provider 模型 <span className="font-normal text-ink-faint">（勾选要挂载到 OpenCode 的模型）</span>
                    </span>
                    <span className="text-[11px] text-ink-faint">已选 {(form.models || []).length}</span>
                  </div>
                  <div className="max-h-40 overflow-y-auto rounded-md border border-line bg-card p-1.5">
                    {models.map((m) => {
                      const selected = (form.models || []).includes(m.id);
                      const toggle = () => {
                        const cur = form.models || [];
                        setForm({
                          ...form,
                          models: selected ? cur.filter((x) => x !== m.id) : [...cur, m.id],
                        });
                      };
                      return (
                        <label key={m.id} className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 hover:bg-elevated/60">
                          <input type="checkbox" checked={selected} onChange={toggle} />
                          <span className="truncate font-mono text-[12px] text-ink">{m.id}</span>
                        </label>
                      );
                    })}
                  </div>
                </div>
              )}

              {tool === 'claude-code' && (
                <div className="space-y-2">
                  <div className="text-[12px] font-medium text-ink-dim">
                    模型角色映射 <span className="font-normal text-ink-faint">（Sonnet/Opus/Fable/Haiku → 实际模型；写入 ANTHROPIC_DEFAULT_*_MODEL）</span>
                  </div>
                  {(['sonnet', 'opus', 'fable', 'haiku'] as const).map((role) => {
                    const labels: Record<string, string> = { sonnet: 'Sonnet', opus: 'Opus', fable: 'Fable', haiku: 'Haiku' };
                    const mm = form.model_mapping || {};
                    const setMM = (k: string, v: string) => setForm({ ...form, model_mapping: { ...mm, [k]: v } });
                    return (
                      <div key={role} className="flex flex-wrap items-center gap-2">
                        <span className="w-14 shrink-0 text-[12px] font-medium text-ink">{labels[role]}</span>
                        <input
                          list={`helio-models-${role}`}
                          value={mm[`${role}_model`] || ''}
                          onChange={(e) => setMM(`${role}_model`, e.target.value)}
                          placeholder="实际模型"
                          className="h-8 min-w-0 flex-1 rounded-md border border-line bg-card px-2 font-mono text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <input
                          value={mm[`${role}_name`] || ''}
                          onChange={(e) => setMM(`${role}_name`, e.target.value)}
                          placeholder="显示名"
                          className="h-8 w-24 shrink-0 rounded-md border border-line bg-card px-2 text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <label className="flex shrink-0 items-center gap-1 text-[11px] text-ink-dim">
                          <input
                            type="checkbox"
                            checked={mm[`${role}_one_m`] === 'true'}
                            onChange={(e) => setMM(`${role}_one_m`, e.target.checked ? 'true' : '')}
                          />
                          1M
                        </label>
                        <datalist id={`helio-models-${role}`}>
                          {models.map((m) => <option key={m.id} value={m.id} />)}
                        </datalist>
                      </div>
                    );
                  })}
                </div>
              )}

              {tool === 'codex' && (
                <div>
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">推理强度</span>
                  <div className="flex gap-1.5">
                    {REASONING_LEVELS.map((r) => (
                      <button
                        key={r.value}
                        type="button"
                        onClick={() => setForm({ ...form, reasoning_effort: r.value || undefined })}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          (form.reasoning_effort || '') === r.value
                            ? 'border-accent text-accent bg-accent/8'
                            : 'border-line text-ink-dim hover:border-line-strong'
                        }`}
                      >
                        {r.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {tool === 'codex' && (
                <div>
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">Wire 协议</span>
                  <div className="flex gap-1.5">
                    {[
                      { value: '', label: '默认' },
                      { value: 'responses', label: 'Responses' },
                      { value: 'chat', label: 'Chat' },
                    ].map((w) => (
                      <button
                        key={w.value}
                        type="button"
                        onClick={() => setForm({ ...form, wire_api: w.value || undefined })}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          (form.wire_api || '') === w.value
                            ? 'border-accent text-accent bg-accent/8'
                            : 'border-line text-ink-dim hover:border-line-strong'
                        }`}
                      >
                        {w.label}
                      </button>
                    ))}
                  </div>
                  <div className="mt-1 text-[11px] text-ink-faint">wire_api：第三方中转若不支持 Responses 选 Chat</div>
                </div>
              )}

              {/* ── Hermes-only model params ── */}
              {tool === 'hermes' && (
                <div className="space-y-3 rounded-lg border border-line/80 bg-card/40 p-3">
                  <div className="text-[12px] font-semibold text-ink-dim">Hermes 模型参数</div>
                  <div>
                    <span className="mb-1.5 block text-[12px] font-medium text-ink-dim">协议模式 (api_mode)</span>
                    <div className="flex gap-1.5">
                      {[
                        { value: 'chat_completions', label: 'Chat' },
                        { value: 'anthropic_messages', label: 'Anthropic' },
                        { value: 'codex_responses', label: 'Responses' },
                      ].map((w) => (
                        <button
                          key={w.value}
                          type="button"
                          onClick={() => setForm({ ...form, api_mode: w.value })}
                          className={`flex-1 rounded-md border px-2 py-1.5 text-[12px] font-medium transition-all ${
                            (form.api_mode || 'chat_completions') === w.value
                              ? 'border-accent bg-accent/8 text-accent'
                              : 'border-line text-ink-dim hover:border-line-strong'
                          }`}
                        >
                          {w.label}
                        </button>
                      ))}
                    </div>
                    <div className="mt-1 text-[11px] text-ink-faint">
                      写入 <code className="font-mono">model.api_mode</code> 与{' '}
                      <code className="font-mono">custom_providers[].api_mode</code>
                      。Provider 填 custom 名（如 freemodel / cpa）→{' '}
                      <code className="font-mono">model.provider=custom:&lt;name&gt;</code>
                    </div>
                  </div>
                  <label className="flex cursor-pointer items-center justify-between">
                    <div>
                      <div className="text-[13px] font-medium text-ink">1M 上下文 (context_length)</div>
                      <div className="text-[11px] text-ink-faint">
                        开启后写入 <code className="font-mono">model.context_length=1000000</code>
                        ，并镜像到当前 <code className="font-mono">custom_providers[]</code> 条目
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={() => setForm({ ...form, context_1m: !form.context_1m })}
                      className={`relative h-6 w-11 rounded-full transition-colors ${form.context_1m ? 'bg-accent' : 'bg-line-strong'}`}
                    >
                      <span
                        className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-soft transition-transform ${
                          form.context_1m ? 'translate-x-[22px]' : 'translate-x-0.5'
                        }`}
                      />
                    </button>
                  </label>
                </div>
              )}

              {/* ── OpenClaw-only model params ── */}
              {tool === 'openclaw' && (
                <div className="space-y-3 rounded-lg border border-line/80 bg-card/40 p-3">
                  <div className="text-[12px] font-semibold text-ink-dim">OpenClaw 模型参数</div>
                  <div>
                    <span className="mb-1.5 block text-[12px] font-medium text-ink-dim">协议模式 (api)</span>
                    <div className="flex gap-1.5">
                      {[
                        { value: 'chat_completions', label: 'Chat' },
                        { value: 'anthropic_messages', label: 'Anthropic' },
                        { value: 'codex_responses', label: 'Responses' },
                      ].map((w) => (
                        <button
                          key={w.value}
                          type="button"
                          onClick={() => setForm({ ...form, api_mode: w.value })}
                          className={`flex-1 rounded-md border px-2 py-1.5 text-[12px] font-medium transition-all ${
                            (form.api_mode || 'chat_completions') === w.value
                              ? 'border-accent bg-accent/8 text-accent'
                              : 'border-line text-ink-dim hover:border-line-strong'
                          }`}
                        >
                          {w.label}
                        </button>
                      ))}
                    </div>
                    <div className="mt-1 text-[11px] text-ink-faint">
                      写入 <code className="font-mono">models.providers.&lt;id&gt;.api</code>
                      。Provider 填 provider id（如 cpa）；primary ={' '}
                      <code className="font-mono">provider/model</code>
                    </div>
                  </div>
                  <label className="flex cursor-pointer items-center justify-between">
                    <div>
                      <div className="text-[13px] font-medium text-ink">1M 上下文 (contextWindow)</div>
                      <div className="text-[11px] text-ink-faint">
                        开启后写入 <code className="font-mono">models[].contextWindow=1000000</code>
                        与 <code className="font-mono">agents.defaults.contextTokens</code>
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={() => setForm({ ...form, context_1m: !form.context_1m })}
                      className={`relative h-6 w-11 rounded-full transition-colors ${form.context_1m ? 'bg-accent' : 'bg-line-strong'}`}
                    >
                      <span
                        className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-soft transition-transform ${
                          form.context_1m ? 'translate-x-[22px]' : 'translate-x-0.5'
                        }`}
                      />
                    </button>
                  </label>
                  <Field
                    label="Max Tokens (maxTokens)"
                    value={form.max_tokens != null ? String(form.max_tokens) : ''}
                    mono
                    onChange={(e) => {
                      const v = e.target.value.trim();
                      if (!v) {
                        setForm({ ...form, max_tokens: undefined });
                        return;
                      }
                      const n = Number(v);
                      setForm({
                        ...form,
                        max_tokens: Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined,
                      });
                    }}
                    placeholder="默认 128000 → models.providers.<id>.models[].maxTokens"
                  />
                </div>
              )}

              {tool === 'codex' && (
                <label className="flex cursor-pointer items-center justify-between">
                  <div>
                    <div className="text-[13px] font-medium text-ink">思考模式</div>
                    <div className="text-[11px] text-ink-faint">model_thinking_enabled</div>
                  </div>
                  <button
                    type="button"
                    onClick={() => setForm({ ...form, model_thinking_enabled: form.model_thinking_enabled ? undefined : true })}
                    className={`relative h-6 w-11 rounded-full transition-colors ${form.model_thinking_enabled ? 'bg-accent' : 'bg-line-strong'}`}
                  >
                    <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-soft transition-transform ${form.model_thinking_enabled ? 'translate-x-[22px]' : 'translate-x-0.5'}`} />
                  </button>
                </label>
              )}

              {tool === 'codex' && (
                <label className="flex cursor-pointer items-center justify-between">
                  <div>
                    <div className="text-[13px] font-medium text-ink">要求 OpenAI 鉴权</div>
                    <div className="text-[11px] text-ink-faint">requires_openai_auth（留默认即可）</div>
                  </div>
                  <button
                    type="button"
                    onClick={() => setForm({ ...form, requires_openai_auth: form.requires_openai_auth === false ? undefined : false })}
                    className={`relative h-6 w-11 rounded-full transition-colors ${form.requires_openai_auth !== false ? 'bg-accent' : 'bg-line-strong'}`}
                  >
                    <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-soft transition-transform ${form.requires_openai_auth !== false ? 'translate-x-[22px]' : 'translate-x-0.5'}`} />
                  </button>
                </label>
              )}

              {tool === 'codex' && (
                <Field label="Bearer Token" type="password" value={form.experimental_bearer_token || ''} mono
                       onChange={(e) => setForm({ ...form, experimental_bearer_token: e.target.value || undefined })}
                       placeholder="留空 / 部分中转在鉴权失败时需要" />
              )}

              {tool === 'codex' && (
                <Field label="Service Tier" value={form.service_tier || ''} mono
                       onChange={(e) => setForm({ ...form, service_tier: e.target.value || undefined })}
                       placeholder="留空 / fast" />
              )}

              {/* Claude Code / Codex only — not shared with Hermes/OpenClaw */}
              {(tool === 'claude-code' || tool === 'codex') && (
                <label className="flex cursor-pointer items-center justify-between">
                  <div>
                    <div className="text-[13px] font-medium text-ink">1M 上下文窗口</div>
                    <div className="text-[11px] text-ink-faint">model_context_window</div>
                  </div>
                  <button
                    type="button"
                    onClick={() => setForm({ ...form, context_1m: !form.context_1m })}
                    className={`relative h-6 w-11 rounded-full transition-colors ${form.context_1m ? 'bg-accent' : 'bg-line-strong'}`}
                  >
                    <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-soft transition-transform ${form.context_1m ? 'translate-x-[22px]' : 'translate-x-0.5'}`} />
                  </button>
                </label>
              )}
            </div>
          )}
      </form>
    </Modal>
  );
}

