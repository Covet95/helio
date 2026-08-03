import { useState } from 'react';
import type { ApiKeyEntry, ApiProfile, FetchedModel, TargetApp } from '../../types';
import { SUPPORTED_TOOLS } from '../../types';
import { Button } from '../../components/common/Button';
import { Modal, Field } from '../../components/common/Modal';
import { PROVIDER_PRESETS, REASONING_LEVELS } from '../../lib/presets';
import { cn, maskApiKey, humanizeError } from '../../lib/utils';
import {
  contextModeFromBool,
  contextModeToBool,
  contextPreviewLine,
  type ContextMode,
} from '../../lib/contextWindow';
import { emptyProfileForTool, normalizeCodexCatalogModels } from './helpers';

function newKeyId(): string {
  return `k${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
}

function ensureKeyPool(p: ApiProfile): ApiKeyEntry[] {
  if (p.api_keys && p.api_keys.length > 0) {
    return p.api_keys.map((e) => ({ ...e }));
  }
  if (p.api_key?.trim()) {
    return [
      {
        id: newKeyId(),
        label: 'default',
        key: p.api_key,
        is_active: true,
      },
    ];
  }
  return [
    {
      id: newKeyId(),
      label: 'default',
      key: '',
      is_active: true,
    },
  ];
}

function withActiveKey(p: ApiProfile, keys: ApiKeyEntry[]): ApiProfile {
  const active = keys.find((k) => k.is_active) || keys[0];
  return {
    ...p,
    api_keys: keys,
    api_key: active?.key ?? p.api_key,
  };
}

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
  const [form, setForm] = useState<ApiProfile>(() => {
    const base = initialProfile || emptyProfileForTool(initialModalTool);
    return withActiveKey(base, ensureKeyPool(base));
  });
  const [models, setModels] = useState<FetchedModel[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [checkingApi, setCheckingApi] = useState(false);
  const [apiHealth, setApiHealth] = useState<{ kind: 'success' | 'error'; text: string } | null>(null);
  const [modelErr, setModelErr] = useState('');
  const [formErr, setFormErr] = useState('');
  const [multiKeyMode, setMultiKeyMode] = useState(
    () => (initialProfile?.api_keys?.length ?? 0) > 1,
  );

  const keys = form.api_keys && form.api_keys.length > 0 ? form.api_keys : ensureKeyPool(form);
  const activeKey =
    keys.find((k) => k.is_active)?.key?.trim() || form.api_key.trim();
  const isBedrock = tool === 'codex' && form.provider.trim().toLowerCase() === 'amazon-bedrock';

  const setKeys = (next: ApiKeyEntry[]) => {
    setForm((f) => withActiveKey(f, next));
    setApiHealth(null);
  };

  const loadModels = async () => {
    if (isBedrock) {
      setModelErr('Amazon Bedrock 使用 Codex 内置 AWS 认证，Helio 无法加载其模型列表');
      return;
    }
    if (!form.api_url.trim() || !activeKey) {
      setModelErr('先填 API URL 和 API Key');
      return;
    }
    setLoadingModels(true);
    setModelErr('');
    try {
      const { tauriApi } = await import('../../lib/tauri');
      const list = await tauriApi.fetchModels(form.api_url, activeKey);
      setModels(list);
      if (list.length === 0) setModelErr('该端点没有返回模型');
    } catch (e) {
      setModelErr(humanizeError(e));
      setModels([]);
    } finally {
      setLoadingModels(false);
    }
  };

  const runProbe = async (apiKey: string, keyLabel?: string) => {
    const { tauriApi } = await import('../../lib/tauri');
    const model = form.model?.trim() || form.models?.[0]?.trim() || '';
    if (isBedrock) {
      throw new Error('Amazon Bedrock 使用 Codex 内置 AWS 认证，Helio 无法执行 HTTP 模型探活');
    }
    if (!form.api_url.trim() || !apiKey.trim()) {
      throw new Error('先填 API URL 和 API Key');
    }
    if (!model) {
      throw new Error('先选择或填写模型');
    }
    return tauriApi.testModel({
      targetApp: tool,
      apiUrl: form.api_url,
      apiKey,
      model,
      envKey: form.env_key,
      wireApi: form.wire_api,
      apiMode: form.api_mode,
      experimentalBearerToken: form.experimental_bearer_token,
      keyLabel,
    });
  };

  const testConnection = async () => {
    setCheckingApi(true);
    setApiHealth(null);
    setModelErr('');
    try {
      const result = await runProbe(activeKey);
      const proto = result.protocol ? ` · ${result.protocol}` : '';
      setApiHealth({
        kind: 'success',
        text: `模型 ${result.model} 可用${proto}`,
      });
    } catch (error) {
      setApiHealth({ kind: 'error', text: humanizeError(error) });
    } finally {
      setCheckingApi(false);
    }
  };

  const testAllKeys = async () => {
    setCheckingApi(true);
    setApiHealth(null);
    const pool = keys.filter((k) => k.key.trim());
    if (pool.length === 0) {
      setApiHealth({ kind: 'error', text: '没有可测试的 Key' });
      setCheckingApi(false);
      return;
    }
    const lines: string[] = [];
    let anyFail = false;
    let activeFailed = false;
    const activeId = keys.find((k) => k.is_active)?.id;
    for (const k of pool) {
      try {
        const r = await runProbe(k.key, k.label || k.id);
        lines.push(`✓ ${k.label || k.id} · ${r.protocol || 'ok'}`);
      } catch (e) {
        anyFail = true;
        if (k.id === activeId) activeFailed = true;
        const msg = humanizeError(e);
        lines.push(`✗ ${k.label || k.id} · ${msg.slice(0, 80)}`);
      }
    }
    setApiHealth({
      kind: anyFail ? 'error' : 'success',
      text: lines.join('；') + (activeFailed ? ' · 可点「Failover」激活可用 Key' : ''),
    });
    setCheckingApi(false);
  };

  const failoverKeys = async () => {
    if (!form.name.trim()) {
      setApiHealth({ kind: 'error', text: '请先保存档案名称后再 failover' });
      return;
    }
    setCheckingApi(true);
    setApiHealth(null);
    try {
      const { tauriApi } = await import('../../lib/tauri');
      // 未保存的多 key 先保存由用户负责；这里对 DB 中档案 failover
      const r = await tauriApi.failoverProfileKeys(tool, form.name);
      if (r.success) {
        setApiHealth({
          kind: 'success',
          text: `Failover 成功 → ${r.active_label || r.active_key_id || 'key'}${r.re_switched ? '（已 re-switch）' : ''}`,
        });
        // 刷新表单活跃标记（按 label 匹配）
        if (r.active_key_id && form.api_keys) {
          setKeys(form.api_keys.map((k) => ({ ...k, is_active: k.id === r.active_key_id || k.label === r.active_label })));
        }
      } else {
        setApiHealth({
          kind: 'error',
          text: `全部 Key 失败：${r.tried.map((t) => t.error || 'fail').join('；')}`,
        });
      }
    } catch (e) {
      setApiHealth({ kind: 'error', text: humanizeError(e) });
    } finally {
      setCheckingApi(false);
    }
  };

  const presets = PROVIDER_PRESETS[tool];
  const showModelParams = tool === 'codex' || tool === 'claude-code' || tool === 'pi' || tool === 'opencode' || tool === 'hermes' || tool === 'openclaw';

  const applyPreset = (p: typeof presets[number]) => {
    setForm((f) => ({
      ...f,
      provider: p.provider,
      api_url: p.api_url || f.api_url,
      model: p.model ?? f.model,
    }));
  };

  const submit = () => {
    const normalized = withActiveKey(form, ensureKeyPool(form));
    const usesCodexEnv = tool === 'codex' && Boolean(normalized.env_key?.trim());
    const usesBedrock = tool === 'codex' && normalized.provider.trim().toLowerCase() === 'amazon-bedrock';
    if (!normalized.name.trim() || !normalized.provider.trim() || (!usesBedrock && (!normalized.api_url.trim() || (!usesCodexEnv && !normalized.api_key.trim())))) {
      setFormErr('请填写名称、Provider、API URL，并提供 API Key 或 Codex 环境变量名');
      return;
    }
    setFormErr('');
    let catalog_models = normalized.catalog_models;
    if (tool === 'codex' && catalog_models) {
      catalog_models = normalizeCodexCatalogModels(catalog_models);
    } else if (tool !== 'codex') {
      catalog_models = undefined;
    }
    onSave({
      ...normalized,
      api_url: usesBedrock ? '' : normalized.api_url,
      api_key: usesBedrock ? '' : normalized.api_key,
      api_keys: usesBedrock ? undefined : normalized.api_keys,
      env_key: usesBedrock ? undefined : normalized.env_key,
      supports_standalone_web_search: usesBedrock
        ? undefined
        : normalized.supports_standalone_web_search || undefined,
      target_app: tool,
      catalog_models,
    });
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
                    onClick={() => {
                      setTool(t.id);
                      if (!initialProfile) {
                        const base = emptyProfileForTool(t.id);
                        setForm(withActiveKey(base, ensureKeyPool(base)));
                        setModels([]);
                        setModelErr('');
                        setApiHealth(null);
                        setMultiKeyMode(false);
                      }
                    }}
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
          {isBedrock ? (
            <div className="space-y-3 rounded-md border border-line bg-surface/60 p-3">
              <div className="text-[12px] text-ink-dim">
                Amazon Bedrock 使用 Codex 内置 AWS 认证，不写入 API URL 或 API Key。
              </div>
              <Field label="AWS Profile（可选）" value={form.aws_profile || ''} mono
                     onChange={(e) => setForm({ ...form, aws_profile: e.target.value.trim() || undefined })} />
              <Field label="AWS Region（可选）" value={form.aws_region || ''} mono
                     onChange={(e) => setForm({ ...form, aws_region: e.target.value.trim() || undefined })} />
            </div>
          ) : (
            <Field label="API URL" type="url" value={form.api_url} required mono
                   onChange={(e) => { setForm({ ...form, api_url: e.target.value }); setApiHealth(null); }} />
          )}

          {!isBedrock && !multiKeyMode ? (
            <div className="space-y-1.5">
              <Field
                label="API Key"
                type="password"
                value={activeKey}
                required={tool !== 'codex' || !form.env_key?.trim()}
                mono
                onChange={(e) => {
                  const v = e.target.value;
                  const next = keys.map((k) =>
                    k.is_active ? { ...k, key: v } : k,
                  );
                  if (!next.some((k) => k.is_active) && next[0]) {
                    next[0] = { ...next[0], key: v, is_active: true };
                  }
                  setKeys(next.length ? next : [{ id: newKeyId(), label: 'default', key: v, is_active: true }]);
                }}
                placeholder="sk-..."
              />
              <button
                type="button"
                className="text-[11px] text-accent hover:underline"
                onClick={() => setMultiKeyMode(true)}
              >
                同一 API 配置多把 Key…
              </button>
            </div>
          ) : !isBedrock ? (
            <div className="space-y-2 rounded-lg border border-line bg-surface/60 p-3">
              <div className="flex items-center justify-between">
                <span className="text-[12px] font-medium text-ink-dim">
                  API Keys <span className="font-normal text-ink-faint">（仅活跃 key 会在 switch 时写入）</span>
                </span>
                <button
                  type="button"
                  className="text-[11px] text-ink-faint hover:text-ink"
                  onClick={() => setMultiKeyMode(false)}
                >
                  折叠为单 Key
                </button>
              </div>
              <div className="space-y-2">
                {keys.map((k) => (
                  <div key={k.id} className="flex flex-wrap items-center gap-2 rounded-md border border-line bg-card px-2 py-1.5">
                    <button
                      type="button"
                      title="设为活跃"
                      onClick={() =>
                        setKeys(keys.map((x) => ({ ...x, is_active: x.id === k.id })))
                      }
                      className={cn(
                        'h-4 w-4 shrink-0 rounded-full border',
                        k.is_active ? 'border-accent bg-accent' : 'border-line-strong',
                      )}
                    />
                    <input
                      value={k.label}
                      onChange={(e) =>
                        setKeys(keys.map((x) => (x.id === k.id ? { ...x, label: e.target.value } : x)))
                      }
                      placeholder="备注"
                      className="h-7 w-20 shrink-0 rounded border border-line bg-surface px-1.5 text-[12px] text-ink outline-none focus:border-accent/50"
                    />
                    <input
                      type="password"
                      value={k.key}
                      onChange={(e) =>
                        setKeys(keys.map((x) => (x.id === k.id ? { ...x, key: e.target.value } : x)))
                      }
                      placeholder="sk-..."
                      className="h-7 min-w-0 flex-1 rounded border border-line bg-surface px-1.5 font-mono text-[12px] text-ink outline-none focus:border-accent/50"
                    />
                    <span className="hidden font-mono text-[10px] text-ink-faint sm:inline">
                      {k.key ? maskApiKey(k.key) : ''}
                    </span>
                    <button
                      type="button"
                      className="text-[11px] text-danger hover:underline disabled:opacity-40"
                      disabled={keys.length <= 1}
                      onClick={() => {
                        if (keys.length <= 1) return;
                        let next = keys.filter((x) => x.id !== k.id);
                        if (!next.some((x) => x.is_active) && next[0]) {
                          next = next.map((x, i) => ({ ...x, is_active: i === 0 }));
                        }
                        setKeys(next);
                      }}
                    >
                      删
                    </button>
                  </div>
                ))}
              </div>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() =>
                  setKeys([
                    ...keys,
                    {
                      id: newKeyId(),
                      label: `key-${keys.length + 1}`,
                      key: '',
                      is_active: false,
                    },
                  ])
                }
              >
                + 添加 Key
              </Button>
            </div>
          ) : null}

          {!isBedrock && (
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" variant="secondary" size="sm" onClick={testConnection} disabled={checkingApi}>
              {checkingApi ? '测试中…' : multiKeyMode ? '测试活跃模型' : '测试模型'}
            </Button>
            {multiKeyMode && (
              <Button type="button" variant="ghost" size="sm" onClick={testAllKeys} disabled={checkingApi}>
                测试全部 Key
              </Button>
            )}
            {multiKeyMode && (
              <Button type="button" variant="ghost" size="sm" onClick={failoverKeys} disabled={checkingApi}>
                Failover
              </Button>
            )}
            {apiHealth && (
              <span className={cn(
                'text-[12px]',
                apiHealth.kind === 'success' ? 'text-ok' : 'text-danger',
              )}>
                {apiHealth.text}
              </span>
            )}
          </div>
          )}

          {showModelParams && (
            <div className="rounded-lg border border-line bg-surface/60 p-3.5 space-y-3.5">
              <div className="text-[12px] font-medium text-ink-dim">模型参数</div>
              <Field label="默认模型" value={form.model || ''} mono
                     onChange={(e) => setForm({ ...form, model: e.target.value })} placeholder="gpt-5.5 / claude-opus-4" />
              <div>
                <div className="flex items-center gap-2">
                  <Button type="button" variant="secondary" size="sm" onClick={loadModels} disabled={loadingModels || isBedrock}>
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

              {tool === 'codex' && (
                <div className="space-y-2">
                  <div className="text-[12px] font-medium text-ink-dim">
                    模型目录 <span className="font-normal text-ink-faint">（生成 model_catalog.json，供 Codex /model 显示；slug 原样保存）</span>
                  </div>
                  <div className="space-y-1.5">
                    {(form.catalog_models || []).map((entry, idx) => (
                      <div key={idx} className="space-y-1.5 rounded-md border border-line bg-card/50 p-2">
                        <div className="flex flex-wrap items-center gap-2">
                        <input
                          value={entry.slug}
                          onChange={(e) => {
                            const next = [...(form.catalog_models || [])];
                            next[idx] = { ...next[idx], slug: e.target.value };
                            setForm({ ...form, catalog_models: next });
                          }}
                          placeholder="slug（必填）"
                          className="h-8 min-w-0 flex-1 rounded-md border border-line bg-card px-2 font-mono text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <input
                          value={entry.display_name || ''}
                          onChange={(e) => {
                            const next = [...(form.catalog_models || [])];
                            next[idx] = {
                              ...next[idx],
                              display_name: e.target.value || undefined,
                            };
                            setForm({ ...form, catalog_models: next });
                          }}
                          placeholder="显示名"
                          className="h-8 w-28 shrink-0 rounded-md border border-line bg-card px-2 text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <button
                          type="button"
                          title="上移"
                          disabled={idx === 0}
                          onClick={() => {
                            if (idx === 0) return;
                            const next = [...(form.catalog_models || [])];
                            [next[idx - 1], next[idx]] = [next[idx], next[idx - 1]];
                            setForm({ ...form, catalog_models: next });
                          }}
                          className="h-8 w-8 shrink-0 rounded-md border border-line text-[11px] text-ink-dim disabled:opacity-40"
                        >
                          ↑
                        </button>
                        <button
                          type="button"
                          title="下移"
                          disabled={idx >= (form.catalog_models || []).length - 1}
                          onClick={() => {
                            const list = form.catalog_models || [];
                            if (idx >= list.length - 1) return;
                            const next = [...list];
                            [next[idx], next[idx + 1]] = [next[idx + 1], next[idx]];
                            setForm({ ...form, catalog_models: next });
                          }}
                          className="h-8 w-8 shrink-0 rounded-md border border-line text-[11px] text-ink-dim disabled:opacity-40"
                        >
                          ↓
                        </button>
                        <button
                          type="button"
                          title="删除"
                          onClick={() => {
                            const next = (form.catalog_models || []).filter((_, i) => i !== idx);
                            setForm({
                              ...form,
                              catalog_models: next.length ? next : undefined,
                            });
                          }}
                          className="h-8 w-8 shrink-0 rounded-md border border-line text-[11px] text-danger"
                        >
                          ×
                        </button>
                        </div>
                        <div className="flex flex-wrap items-center gap-3 text-[11px] text-ink-dim">
                          <input
                            type="number"
                            min={1}
                            value={entry.context_window || ''}
                            onChange={(e) => {
                              const next = [...(form.catalog_models || [])];
                              next[idx] = { ...next[idx], context_window: e.target.value ? Number(e.target.value) : undefined };
                              setForm({ ...form, catalog_models: next });
                            }}
                            placeholder="上下文窗口"
                            className="h-7 w-28 rounded border border-line bg-surface px-1.5 font-mono text-[11px]"
                          />
                          {[
                            ['supports_images', '图片'],
                            ['supports_tool_calls', '工具调用'],
                            ['supports_web_search', '联网搜索'],
                          ].map(([key, label]) => (
                            <label key={key} className="flex items-center gap-1">
                              <input
                                type="checkbox"
                                checked={Boolean(entry[key as keyof typeof entry])}
                                onChange={(e) => {
                                  const next = [...(form.catalog_models || [])];
                                  next[idx] = { ...next[idx], [key]: e.target.checked || undefined };
                                  setForm({ ...form, catalog_models: next });
                                }}
                              />
                              {key === 'supports_web_search' ? `${label}（需 Provider 支持）` : label}
                            </label>
                          ))}
                        </div>
                        <div className="flex flex-wrap items-center gap-2 text-[11px] text-ink-dim">
                          <span>推理等级</span>
                          {['minimal', 'low', 'medium', 'high', 'xhigh'].map((level) => {
                            const levels = entry.reasoning_levels
                              ?? (entry.supports_reasoning ? ['low', 'medium', 'high'] : []);
                            const checked = levels.includes(level);
                            return (
                              <label key={level} className="flex items-center gap-1">
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  onChange={(e) => {
                                    const next = [...(form.catalog_models || [])];
                                    const updated = e.target.checked
                                      ? [...levels, level]
                                      : levels.filter((item) => item !== level);
                                    next[idx] = {
                                      ...next[idx],
                                      reasoning_levels: Array.from(new Set(updated)),
                                      supports_reasoning: undefined,
                                    };
                                    setForm({ ...form, catalog_models: next });
                                  }}
                                />
                                {level}
                              </label>
                            );
                          })}
                        </div>
                      </div>
                    ))}
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      onClick={() =>
                        setForm({
                          ...form,
                          catalog_models: [...(form.catalog_models || []), { slug: '' }],
                        })
                      }
                    >
                      添加模型
                    </Button>
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      onClick={() => {
                        const m = form.model?.trim();
                        if (!m) return;
                        const cur = form.catalog_models || [];
                        if (cur.some((e) => e.slug === m)) return;
                        setForm({
                          ...form,
                          catalog_models: [{ slug: m }, ...cur],
                        });
                      }}
                    >
                      将默认模型加入目录
                    </Button>
                    {models.length > 0 && (
                      <Button
                        type="button"
                        variant="secondary"
                        size="sm"
                        onClick={() => {
                          const cur = form.catalog_models || [];
                          const have = new Set(cur.map((e) => e.slug));
                          const add = models
                            .filter((m) => !have.has(m.id))
                            .map((m) => ({ slug: m.id }));
                          if (!add.length) return;
                          setForm({ ...form, catalog_models: [...cur, ...add] });
                        }}
                      >
                        从已加载列表全部加入
                      </Button>
                    )}
                  </div>
                  <div className="text-[11px] text-ink-faint">
                    切换档案时整表覆盖 ~/.codex/model_catalog.json 并设置 model_catalog_json；未配置则不改本机 catalog。修改后需重启 Codex 才能刷新 /model。
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
                  <div>
                    <div className="mb-1.5 text-[13px] font-medium text-ink">上下文窗口 (context_length)</div>
                    <div className="mb-2 flex gap-1.5">
                      {([
                        { mode: '1m' as ContextMode, label: '1M' },
                        { mode: 'standard' as ContextMode, label: '标准' },
                        { mode: 'unset' as ContextMode, label: '不修改' },
                      ]).map((opt) => {
                        const cur = contextModeFromBool(form.context_1m);
                        return (
                          <button
                            key={opt.mode}
                            type="button"
                            onClick={() => setForm({ ...form, context_1m: contextModeToBool(opt.mode) })}
                            className={`flex-1 rounded-md border px-2 py-1.5 text-[12px] font-medium transition-all ${
                              cur === opt.mode
                                ? 'border-accent bg-accent/8 text-accent'
                                : 'border-line text-ink-dim hover:border-line-strong'
                            }`}
                          >
                            {opt.label}
                          </button>
                        );
                      })}
                    </div>
                    <div className="text-[11px] text-ink-faint">
                      标准：Grok <code className="font-mono">500000</code> / 其它 <code className="font-mono">200000</code>
                      ；1M → <code className="font-mono">1000000</code>
                      。{contextPreviewLine(form.context_1m, form.model, 'hermes')}
                    </div>
                  </div>
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
                  <div>
                    <div className="mb-1.5 text-[13px] font-medium text-ink">上下文窗口 (contextWindow)</div>
                    <div className="mb-2 flex gap-1.5">
                      {([
                        { mode: '1m' as ContextMode, label: '1M' },
                        { mode: 'standard' as ContextMode, label: '标准' },
                        { mode: 'unset' as ContextMode, label: '不修改' },
                      ]).map((opt) => {
                        const cur = contextModeFromBool(form.context_1m);
                        return (
                          <button
                            key={opt.mode}
                            type="button"
                            onClick={() => setForm({ ...form, context_1m: contextModeToBool(opt.mode) })}
                            className={`flex-1 rounded-md border px-2 py-1.5 text-[12px] font-medium transition-all ${
                              cur === opt.mode
                                ? 'border-accent bg-accent/8 text-accent'
                                : 'border-line text-ink-dim hover:border-line-strong'
                            }`}
                          >
                            {opt.label}
                          </button>
                        );
                      })}
                    </div>
                    <div className="text-[11px] text-ink-faint">
                      写入 models[].contextWindow 与 agents.defaults.contextTokens。
                      {contextPreviewLine(form.context_1m, form.model, 'openclaw')}
                    </div>
                  </div>
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

              {tool === 'codex' && !isBedrock && (
                <label className="flex cursor-pointer items-center justify-between">
                  <div>
                    <div className="text-[13px] font-medium text-ink">Provider 独立联网搜索</div>
                    <div className="text-[11px] text-ink-faint">supports_standalone_web_search；模型目录也需启用联网搜索</div>
                  </div>
                  <input
                    type="checkbox"
                    checked={form.supports_standalone_web_search === true}
                    onChange={(e) => setForm({
                      ...form,
                      supports_standalone_web_search: e.target.checked || undefined,
                    })}
                  />
                </label>
              )}

              {tool === 'codex' && !isBedrock && (
                <Field label="API Key 环境变量" value={form.env_key || ''} mono
                       onChange={(e) => setForm({ ...form, env_key: e.target.value.trim() || undefined })}
                       placeholder="留空则由 Helio 安全写入 auth.json；例如 OPENAI_API_KEY" />
              )}

              {tool === 'codex' && (
                <Field label="Service Tier" value={form.service_tier || ''} mono
                       onChange={(e) => setForm({ ...form, service_tier: e.target.value || undefined })}
                       placeholder="留空 / fast" />
              )}

              {/* Claude Code / Codex only — not shared with Hermes/OpenClaw */}
              {(tool === 'claude-code' || tool === 'codex') && (
                <div>
                  <div className="mb-1.5 text-[13px] font-medium text-ink">1M 上下文窗口</div>
                  <div className="mb-1 flex gap-1.5">
                    {([
                      { mode: '1m' as ContextMode, label: '开启 1M' },
                      { mode: 'standard' as ContextMode, label: '关闭' },
                      { mode: 'unset' as ContextMode, label: '不修改' },
                    ]).map((opt) => {
                      const cur = contextModeFromBool(form.context_1m);
                      return (
                        <button
                          key={opt.mode}
                          type="button"
                          onClick={() => setForm({ ...form, context_1m: contextModeToBool(opt.mode) })}
                          className={`flex-1 rounded-md border px-2 py-1.5 text-[12px] font-medium transition-all ${
                            cur === opt.mode
                              ? 'border-accent bg-accent/8 text-accent'
                              : 'border-line text-ink-dim hover:border-line-strong'
                          }`}
                        >
                          {opt.label}
                        </button>
                      );
                    })}
                  </div>
                  <div className="text-[11px] text-ink-faint">model_context_window · {contextPreviewLine(form.context_1m, form.model, tool)}</div>
                </div>
              )}
            </div>
          )}
      </form>
    </Modal>
  );
}
