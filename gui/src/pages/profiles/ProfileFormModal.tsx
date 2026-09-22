import { useRef, useState } from 'react';
import { Alert } from '@/components/common/Alert';
import type {
  ApiKeyEntry,
  ApiProfile,
  FetchedModel,
  OpenCodeModelConfig,
  OpenCodeVariantConfig,
  TargetApp,
} from '../../types';
import { SUPPORTED_TOOLS } from '../../types';
import { Button } from '../../components/common/Button';
import { Modal, Field } from '../../components/common/Modal';
import { PROVIDER_PRESETS, REASONING_LEVELS, SERVICE_TIERS, REASONING_SUMMARIES, VERBOSITY_LEVELS, CODEX_CATALOG_LEVELS } from '../../lib/presets';
import { cn, maskApiKey, humanizeError } from '../../lib/utils';
import { tauriApi } from '../../lib/tauri';
import {
  contextModeFromBool,
  contextModeToBool,
  contextPreviewLine,
  type ContextMode,
} from '../../lib/contextWindow';
import { ApiModeSelector, emptyProfileForTool } from './helpers';
import {
  ensureKeyPool,
  newKeyId,
  normalizeSubmit,
  withActiveKey,
} from './submitNormalize';

// OpenCode 推理强度档位：variant 快捷添加与 reasoningEffort 下拉共用同一组官方档位。
const OPENCODE_EFFORT_LEVELS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];


export function ProfileModal({
  profile, initialTool, seedFrom, onClose, onSave,
}: {
  profile: ApiProfile | null;
  initialTool: TargetApp;
  seedFrom?: ApiProfile;
  onClose: () => void;
  onSave: (p: ApiProfile) => Promise<void>;
}) {
  const initialProfile = profile;
  const initialModalTool = initialProfile?.target_app ?? initialTool;
  const [tool, setTool] = useState<TargetApp>(initialModalTool);
  const [form, setForm] = useState<ApiProfile>(() => {
    const base = initialProfile || emptyProfileForTool(initialModalTool, seedFrom);
    return withActiveKey(base, ensureKeyPool(base));
  });
  const [models, setModels] = useState<FetchedModel[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [checkingApi, setCheckingApi] = useState(false);
  const [apiHealth, setApiHealth] = useState<{ kind: 'success' | 'error'; text: string } | null>(null);
  const [modelErr, setModelErr] = useState('');
  const [formErr, setFormErr] = useState('');
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [multiKeyMode, setMultiKeyMode] = useState(
    () => (initialProfile?.api_keys?.length ?? 0) > 1,
  );
  const [variantDrafts, setVariantDrafts] = useState<Record<string, string>>({});
  const [variantNameDrafts, setVariantNameDrafts] = useState<Record<string, string>>({});
  const [optionsDrafts, setOptionsDrafts] = useState<Record<string, string>>({});
  const [optionsErrors, setOptionsErrors] = useState<Record<string, string>>({});

  const keys = form.api_keys && form.api_keys.length > 0 ? form.api_keys : ensureKeyPool(form);
  const activeKey =
    keys.find((k) => k.is_active)?.key?.trim() || form.api_key.trim();
  const isBedrock = tool === 'codex' && form.provider.trim().toLowerCase() === 'amazon-bedrock';
  const usesAuthCommand = tool === 'codex' && Boolean(form.auth_command?.trim());

  const setKeys = (next: ApiKeyEntry[]) => {
    setForm((f) => withActiveKey(f, next));
    setApiHealth(null);
  };

  const loadModels = async () => {
    if (isBedrock) {
      setModelErr('Amazon Bedrock 使用 Codex 内置 AWS 认证，Helio 无法加载其模型列表');
      return;
    }
    const hasDiscoveryCredential =
      Boolean(activeKey)
      || (tool === 'codex' && Boolean(form.env_key?.trim()))
      || (tool === 'codex' && Boolean(form.experimental_bearer_token?.trim()))
      || (tool === 'codex' && Boolean(form.auth_command?.trim()));
    if (!form.api_url.trim() || !hasDiscoveryCredential) {
      setModelErr('先填 API URL 和 API Key、环境变量名、Bearer Token 或 Auth 命令');
      return;
    }
    setLoadingModels(true);
    setModelErr('');
    try {
      const list = await tauriApi.fetchModels({
        targetApp: tool,
        provider: form.provider,
        apiUrl: form.api_url,
        apiKey: activeKey,
        envKey: form.env_key,
        apiMode: tool === 'opencode' ? form.opencode_api_mode : form.api_mode,
        experimentalBearerToken: form.experimental_bearer_token,
        awsProfile: form.aws_profile,
        awsRegion: form.aws_region,
        hasCommandAuth: usesAuthCommand || undefined,
      });
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
    const model = form.model?.trim() || form.models?.[0]?.trim() || '';
    if (isBedrock) {
      throw new Error('Amazon Bedrock 使用 Codex 内置 AWS 认证，Helio 无法执行 HTTP 模型探活');
    }
    // Bearer 模式：活跃 key 为空时用 bearer token 探活。
    const effectiveKey = apiKey.trim()
      || (tool === 'codex' ? form.experimental_bearer_token?.trim() || '' : '');
    if (!form.api_url.trim() || !effectiveKey) {
      if (tool === 'codex' && form.auth_command?.trim()) {
        throw new Error('该档案使用 auth 命令获取 token，Helio 不执行外部命令，无法探活');
      }
      throw new Error('先填 API URL 和 API Key');
    }
    if (!model) {
      throw new Error('先选择或填写模型');
    }
    return tauriApi.testModel({
      targetApp: tool,
      apiUrl: form.api_url,
      apiKey: effectiveKey,
      model,
      envKey: form.env_key,
      wireApi: form.wire_api,
      apiMode: tool === 'opencode' ? form.opencode_api_mode : form.api_mode,
      experimentalBearerToken: form.experimental_bearer_token,
      keyLabel,
      hasCommandAuth: usesAuthCommand || undefined,
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

  const savedName = initialProfile?.name.trim() ?? '';
  const nameDirty = form.name.trim() !== savedName;
  const failoverKeys = async () => {
    if (!form.name.trim()) {
      setApiHealth({ kind: 'error', text: '请先保存档案名称后再 failover' });
      return;
    }
    // 用已入库的名字做 failover：表单改名未保存时按新名查库会命中错误档案。
    if (!initialProfile || nameDirty) {
      setApiHealth({ kind: 'error', text: '档案名称已修改，请先保存后再 failover' });
      return;
    }
    const targetName = initialProfile.name;
    setCheckingApi(true);
    setApiHealth(null);
    try {
      // 未保存的多 key 先保存由用户负责；这里对 DB 中档案 failover
      const r = await tauriApi.failoverProfileKeys(tool, targetName);
      if (r.success) {
        // 刷新表单活跃标记：函数式更新，只改 is_active，不覆盖等待期间的用户编辑。
        if (r.active_key_id) {
          const activeId = r.active_key_id;
          const activeLabel = r.active_label;
          setForm((f) => {
            if (!f.api_keys) return f;
            return withActiveKey(
              f,
              f.api_keys.map((k) => ({ ...k, is_active: k.id === activeId || k.label === activeLabel })),
            );
          });
        }
        setApiHealth({
          kind: 'success',
          text: `Failover 成功 → ${r.active_label || r.active_key_id || 'key'}${r.re_switched ? '（已 re-switch）' : ''}`,
        });
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
  // OpenCode 模型的单一读写口：三片存储（默认模型 / 挂载列表 / 逐模型配置）在渲染时
  // 归一为条目列表，结构性修改（挂载、打补丁、删除）只走 setOpenCodeModels 写回。
  // 默认模型输入框仍直写 form.model（单字段、无结构分叉，归一时自动纳入）。
  type OpenCodeModelEntry = {
    id: string;
    mounted: boolean;
    isDefault: boolean;
    config: OpenCodeModelConfig | undefined;
  };
  const openCodeModels: OpenCodeModelEntry[] = Array.from(new Set([
    ...(form.models || []),
    form.model?.trim() || '',
    ...Object.keys(form.model_configs || {}),
  ].filter(Boolean))).map((id) => ({
    id,
    mounted: (form.models || []).includes(id),
    isDefault: (form.model || '').trim() === id,
    config: form.model_configs?.[id],
  }));

  const setOpenCodeModels = (entries: OpenCodeModelEntry[]) => {
    const nextConfigs: Record<string, OpenCodeModelConfig> = {};
    for (const e of entries) {
      if (e.config) nextConfigs[e.id] = e.config;
    }
    const nextModel = entries.find((e) => e.isDefault)?.id ?? '';
    const nextModels = entries.filter((e) => e.mounted).map((e) => e.id);
    const nextModelConfigs = Object.keys(nextConfigs).length > 0 ? nextConfigs : undefined;
    setForm((f) => ({
      ...f,
      model: nextModel,
      models: nextModels,
      model_configs: nextModelConfigs,
    }));
  };

  const patchOpenCodeModelConfig = (modelId: string, patch: Partial<OpenCodeModelConfig>) => {
    // 函数式写回：直接基于最新 form 合并，避免用渲染时刻 openCodeModels 覆盖并发编辑。
    setForm((f) => {
      const prev = f.model_configs?.[modelId] || {};
      const merged = { ...prev, ...patch };
      return {
        ...f,
        model_configs: { ...(f.model_configs || {}), [modelId]: merged },
      };
    });
  };

  // 彻底移除一个 OpenCode 模型：同时清理 models 挂载、model_configs 卡片和默认引用。
  // 若删的是默认模型，自动把剩余并集里的第一个提升为默认，避免顶层 model 悬空。
  const removeOpenCodeModel = (modelId: string) => {
    const rest = openCodeModels.filter((e) => e.id !== modelId);
    if (rest.length > 0 && !rest.some((e) => e.isDefault)) {
      rest[0] = { ...rest[0], isDefault: true };
    }
    setOpenCodeModels(rest);
    setVariantDrafts((drafts) => {
      const next = { ...drafts };
      delete next[modelId];
      return next;
    });
    setVariantNameDrafts((drafts) => {
      const next = { ...drafts };
      for (const key of Object.keys(next)) {
        if (key === modelId || key.startsWith(`${modelId}:`)) delete next[key];
      }
      return next;
    });
  };

  const applyPreset = (p: typeof presets[number]) => {
    setForm((f) => ({
      ...f,
      provider: p.provider,
      api_url: p.api_url || f.api_url,
      model: p.model ?? f.model,
    }));
  };

  const submit = async () => {
    if (savingRef.current) return;

    // 字段归一与互斥清洗已抽到 `submitNormalize`（纯函数、有表驱动单测）——
    // 这段逻辑此前埋在这里，是全应用最难测也最容易出错的部分。
    const result = normalizeSubmit(form, tool);
    if (!result.ok) {
      setFormErr(result.error);
      return;
    }
    setFormErr('');

    savingRef.current = true;
    setSaving(true);
    try {
      await onSave(result.value);
    } catch (error) {
      setFormErr(humanizeError(error));
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  return (
    <Modal
      title={profile ? '编辑配置档案' : '新建配置档案'}
      onClose={onClose}
      busy={saving}
      size="xl"
      footer={
        <>
          <Button type="button" variant="ghost" disabled={saving} onClick={onClose}>取消</Button>
          <Button type="button" disabled={saving} className="min-w-20" onClick={submit}>{saving ? '保存中…' : profile ? '保存' : '创建'}</Button>
        </>
      }
    >
      <form
        id="helio-profile-form"
        onSubmit={(e) => { e.preventDefault(); void submit(); }}
      >
        <fieldset disabled={saving} className="min-w-0 space-y-4">
          {formErr && (
            <Alert tone="error">{formErr}</Alert>
          )}
          {!initialProfile && (
            <div>
              <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">目标工具</span>
              <div className="flex flex-wrap gap-1.5">
                {SUPPORTED_TOOLS.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    disabled={loadingModels || checkingApi}
                    onClick={() => {
                      setTool(t.id);
                      if (!initialProfile) {
                        const base = emptyProfileForTool(t.id, seedFrom);
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
                 onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} placeholder="my-proxy" />
          <Field label="Provider" value={form.provider} required
                 onChange={(e) => setForm((f) => ({ ...f, provider: e.target.value }))} placeholder="anthropic / openai / google" />
          {isBedrock ? (
            <div className="space-y-3 rounded-md border border-line bg-surface/60 p-3">
              <div className="text-[12px] text-ink-dim">
                Amazon Bedrock 使用 Codex 内置 AWS 认证，不写入 API URL 或 API Key。
              </div>
              <Field label="AWS Profile（可选）" value={form.aws_profile || ''} mono
                     onChange={(e) => setForm((f) => ({ ...f, aws_profile: e.target.value.trim() || undefined }))} />
              <Field label="AWS Region（可选）" value={form.aws_region || ''} mono
                     onChange={(e) => setForm((f) => ({ ...f, aws_region: e.target.value.trim() || undefined }))} />
            </div>
          ) : (
            <Field label="API URL" type="url" value={form.api_url} required mono
                   onChange={(e) => { setForm((f) => ({ ...f, api_url: e.target.value })); setApiHealth(null); }} />
          )}

          {!isBedrock && !multiKeyMode ? (
            <div className="space-y-1.5">
              <Field
                label="API Key"
                type="password"
                value={activeKey}
                required={tool !== 'codex' || (!form.env_key?.trim() && !form.auth_command?.trim() && !form.experimental_bearer_token?.trim())}
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
                    <span className="hidden font-mono text-[10px] text-ink-faint md:inline">
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

          {usesAuthCommand && !isBedrock && (
            <div className="text-[11px] text-ink-faint">
              Auth 命令模式下 API Key 不会被写入配置，仅保留为备注。
            </div>
          )}

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
            {/* Failover 操作库里档案的 key，新建未入库时点它必报错，故仅编辑时显示 */}
            {multiKeyMode && initialProfile && (
              <Button type="button" variant="ghost" size="sm" onClick={failoverKeys} disabled={checkingApi} title={nameDirty ? '档案名称已修改，请先保存后再 failover' : undefined}>
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

          {(
            <div className="rounded-lg border border-line bg-surface/60 p-3.5 space-y-3.5">
              <div className="text-[12px] font-medium text-ink-dim">模型参数</div>
              <Field label="默认模型" value={form.model || ''} mono
                     onChange={(e) => setForm((f) => ({ ...f, model: e.target.value }))} placeholder="gpt-5.5 / claude-opus-4" />
              <div>
                <div className="flex items-center gap-2">
                  <Button type="button" variant="secondary" size="sm" onClick={loadModels} disabled={loadingModels || isBedrock}>
                    {loadingModels ? '加载中…' : '加载模型列表'}
                  </Button>
                  {models.length > 0 && (
                    <select
                      value={form.model || ''}
                      onChange={(e) => setForm((f) => ({ ...f, model: e.target.value }))}
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

              {tool === 'opencode' && (models.length > 0 || openCodeModels.some((e) => !models.some((m) => m.id === e.id))) && (
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between">
                    <span className="text-[12px] font-medium text-ink-dim">
                      provider 模型 <span className="font-normal text-ink-faint">（勾选挂载；取消勾选保留配置，下方卡片删除按钮可彻底移除）</span>
                    </span>
                    <span className="text-[11px] text-ink-faint">已选 {(form.models || []).length}</span>
                  </div>
                  <div className="max-h-40 overflow-y-auto rounded-md border border-line bg-card p-1.5">
                    {[...openCodeModels, ...models.filter((m) => !openCodeModels.some((e) => e.id === m.id)).map((m) => ({ id: m.id, mounted: false, isDefault: false, config: undefined }) as OpenCodeModelEntry)].map((entry) => {
                      const toggle = () => {
                        if (entry.isDefault) return;
                        if (openCodeModels.some((e) => e.id === entry.id)) {
                          setOpenCodeModels(openCodeModels.map((e) =>
                            e.id === entry.id ? { ...e, mounted: !e.mounted } : e,
                          ));
                        } else {
                          // 拉取列表里尚未纳管的模型：勾选即纳入并挂载。
                          setOpenCodeModels([...openCodeModels, { ...entry, mounted: true }]);
                        }
                      };
                      return (
                        <label
                          key={entry.id}
                          title={entry.isDefault ? '默认模型不可取消挂载，可更改默认模型或用下方卡片删除按钮移除' : undefined}
                          className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 hover:bg-elevated/60"
                        >
                          <input
                            type="checkbox"
                            checked={entry.mounted}
                            disabled={entry.isDefault}
                            onChange={toggle}
                            aria-label={`挂载模型 ${entry.id}`}
                          />
                          <span className="truncate font-mono text-[12px] text-ink">{entry.id}</span>
                        </label>
                      );
                    })}
                  </div>
                </div>
              )}

              {tool === 'opencode' && (
                <div className="space-y-3 rounded-lg border border-line/80 bg-card/40 p-3">
                  <div className="text-[12px] font-semibold text-ink-dim">OpenCode 模型配置</div>
                  <div>
                    <span className="mb-1.5 block text-[12px] font-medium text-ink-dim">协议模式</span>
                    <div className="flex gap-1.5">
                      {[
                        { value: "", label: "默认" },
                        { value: 'chat_completions', label: 'Chat Completions' },
                        { value: 'responses', label: 'Responses' },
                      ].map((mode) => (
                        <button
                          key={mode.value}
                          type="button"
                          onClick={() => setForm((f) => ({ ...f, opencode_api_mode: mode.value }))}
                          className={`flex-1 rounded-md border px-2 py-1.5 text-[12px] font-medium transition-all ${
                            (form.opencode_api_mode || "") === mode.value
                              ? 'border-accent bg-accent/8 text-accent'
                              : 'border-line text-ink-dim hover:border-line-strong'
                          }`}
                        >
                          {mode.label}
                        </button>
                      ))}
                    </div>
                    <div className="mt-1 text-[11px] text-ink-faint">
                      Chat 使用 <code className="font-mono">@ai-sdk/openai-compatible</code>；
                      Responses 使用 <code className="font-mono">@ai-sdk/openai</code>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <div className="text-[12px] font-medium text-ink-dim">
                      模型行为 <span className="font-normal text-ink-faint">（默认模型、勾选模型和配置模型都会写入）</span>
                    </div>
                    {openCodeModels.length === 0 ? (
                      <div className="rounded-md border border-dashed border-line px-3 py-2 text-[11px] text-ink-faint">
                        先填写默认模型，或加载模型列表后勾选模型
                      </div>
                    ) : (
                      openCodeModels.map((entry) => {
                        const modelId = entry.id;
                        const config: OpenCodeModelConfig = entry.config || {};
                        const limit = config.limit || {};
                        const variants = config.variants || {};
                        const setConfig = (patch: Partial<OpenCodeModelConfig>) => {
                          patchOpenCodeModelConfig(modelId, patch);
                        };
                        const setLimit = (key: "context" | "input" | "output", value: string) => {
                          const n = Number(value);
                          const next = { ...limit };
                          if (!value || !Number.isFinite(n) || n <= 0) delete next[key];
                          else next[key] = Math.floor(n);
                          setConfig({ limit: Object.keys(next).length > 0 ? next : undefined });
                        };
                        const updateVariant = (
                          variantId: string,
                          patch: Partial<OpenCodeVariantConfig>,
                        ) => {
                          setConfig({
                            variants: {
                              ...variants,
                              [variantId]: { ...variants[variantId], ...patch },
                            },
                          });
                        };
                        const renameVariant = (oldId: string, rawId: string) => {
                          const nextId = rawId.trim();
                          if (!nextId || nextId === oldId || variants[nextId]) return;
                          const next = { ...variants, [nextId]: variants[oldId] };
                          delete next[oldId];
                          setConfig({ variants: next });
                        };
                        const addVariant = (variantId: string) => {
                          const id = variantId.trim();
                          if (!id || variants[id]) return;
                          setConfig({
                            variants: {
                              ...variants,
                              [id]: {},
                            },
                          });
                        };
                        const setVariantReasoning = (variantId: string, value: string) => {
                          const variant = { ...variants[variantId] };
                          if (value) variant.reasoningEffort = value;
                          else delete variant.reasoningEffort;
                          updateVariant(variantId, variant);
                        };
                        return (
                          <div key={modelId} className="space-y-2 rounded-md border border-line bg-card/50 p-2.5">
                            <div className="flex flex-wrap items-center gap-2">
                              <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-ink">{modelId}</span>
                              <button
                                type="button"
                                aria-label={`删除模型 ${modelId}`}
                                title="彻底移除该模型（挂载、配置与默认引用）"
                                onClick={() => removeOpenCodeModel(modelId)}
                                className="text-[11px] text-danger hover:underline"
                              >
                                删除
                              </button>
                            </div>
                            <div className="flex flex-wrap items-center gap-2 text-[11px] text-ink-dim">
                              <span>限制</span>
                              <input
                                type="number"
                                min={1}
                                value={limit.context ?? ''}
                                onChange={(e) => setLimit('context', e.target.value)}
                                placeholder="context"
                                className="h-7 w-24 rounded border border-line bg-surface px-1.5 font-mono"
                              />
                              <input
                                type="number"
                                min={1}
                                value={limit.output ?? ''}
                                onChange={(e) => setLimit('output', e.target.value)}
                                placeholder="output"
                                className="h-7 w-24 rounded border border-line bg-surface px-1.5 font-mono"
                              />
                                <input
                                  type="number"
                                  min={1}
                                  value={limit.input ?? ""}
                                  onChange={(e) => setLimit("input", e.target.value)}
                                  placeholder="input"
                                  className="h-7 w-24 rounded border border-line bg-surface px-1.5 font-mono"
                                />
                            </div>
                            <div className="space-y-1.5">
                              <div className="flex flex-wrap items-center gap-1.5 text-[11px] font-medium text-ink-dim">
                                <span>Variants</span>
                                {OPENCODE_EFFORT_LEVELS.map((variantId) => (
                                  <button
                                    key={variantId}
                                    type="button"
                                    onClick={() => addVariant(variantId)}
                                    disabled={Boolean(variants[variantId])}
                                    className="rounded border border-line px-1.5 py-0.5 disabled:opacity-40"
                                  >
                                    + {variantId}
                                  </button>
                                ))}
                              </div>
                              {Object.entries(variants).map(([variantId, variant]) => (
                                <div key={variantId} className="flex flex-wrap items-center gap-1.5">
                                  <input
                                    value={variantNameDrafts[`${modelId}:${variantId}`] ?? variantId}
                                    onChange={(e) => setVariantNameDrafts({
                                      ...variantNameDrafts,
                                      [`${modelId}:${variantId}`]: e.target.value,
                                    })}
                                    onBlur={(e) => {
                                      const draftKey = `${modelId}:${variantId}`;
                                      renameVariant(variantId, e.target.value);
                                      setVariantNameDrafts((drafts) => {
                                        const next = { ...drafts };
                                        delete next[draftKey];
                                        return next;
                                      });
                                    }}
                                    className="h-7 w-20 rounded border border-line bg-surface px-1.5 font-mono text-[11px]"
                                    aria-label={`Variant ${variantId} 名称`}
                                  />
                                  <select
                                    value={String(variant.reasoningEffort || '')}
                                    onChange={(e) => setVariantReasoning(variantId, e.target.value)}
                                    title="reasoningEffort：该 variant 的推理强度（官方自定义 variant 写法）"
                                    aria-label={`Variant ${variantId} 推理强度`}
                                    className="h-7 rounded border border-line bg-surface px-1.5 text-[11px]"
                                  >
                                    <option value="">effort 不设置</option>
                                    {OPENCODE_EFFORT_LEVELS.map((level) => (
                                      <option key={level} value={level}>{level}</option>
                                    ))}
                                  </select>
                                  <label className="flex items-center gap-1 text-[11px] text-ink-dim">
                                    <input
                                      type="checkbox"
                                      checked={variant.disabled === true}
                                      onChange={(e) => updateVariant(variantId, { disabled: e.target.checked || undefined })}
                                    />
                                    disabled
                                  </label>
                                  <button
                                    type="button"
                                    title="删除 Variant"
                                    onClick={() => {
                                      const next = { ...variants };
                                      delete next[variantId];
                                      setConfig({ variants: Object.keys(next).length > 0 ? next : undefined });
                                      setVariantNameDrafts((drafts) => {
                                        const nextDrafts = { ...drafts };
                                        delete nextDrafts[`${modelId}:${variantId}`];
                                        return nextDrafts;
                                      });
                                    }}
                                    className="text-[11px] text-danger hover:underline"
                                  >
                                    删除
                                  </button>
                                </div>
                              ))}
                              <div className="space-y-1">
                                <div className="text-[11px] font-medium text-ink-dim">options 高级 JSON（temperature / thinking 等，留空不管）</div>
                                <textarea
                                  value={optionsDrafts[modelId] ?? (config.options && Object.keys(config.options).length > 0 ? JSON.stringify(config.options, null, 2) : "")}
                                  onChange={(e) => {
                                    setOptionsDrafts({ ...optionsDrafts, [modelId]: e.target.value });
                                    setOptionsErrors((errs) => {
                                      const next = { ...errs };
                                      delete next[modelId];
                                      return next;
                                    });
                                  }}
                                  onBlur={(e) => {
                                    const raw = e.target.value.trim();
                                    if (!raw) {
                                      setConfig({ options: undefined });
                                      setOptionsDrafts((drafts) => {
                                        const nd = { ...drafts };
                                        delete nd[modelId];
                                        return nd;
                                      });
                                      return;
                                    }
                                    try {
                                      const parsed = JSON.parse(raw);
                                      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("bad shape");
                                      setConfig({ options: parsed });
                                      setOptionsDrafts((drafts) => {
                                        const nd = { ...drafts };
                                        delete nd[modelId];
                                        return nd;
                                      });
                                    } catch {
                                      setOptionsErrors({ ...optionsErrors, [modelId]: "JSON 解析失败，未保存" });
                                    }
                                  }}
                                  placeholder="如 temperature / thinking，须为 JSON 对象"
                                  rows={2}
                                  spellCheck={false}
                                  className="w-full rounded border border-line bg-surface px-1.5 py-1 font-mono text-[11px]"
                                />
                                {optionsErrors[modelId] && (
                                  <div className="text-[11px] text-danger">{optionsErrors[modelId]}</div>
                                )}
                              </div>
                              <div className="flex items-center gap-1.5">
                                <input
                                  value={variantDrafts[modelId] || ''}
                                  onChange={(e) => setVariantDrafts({ ...variantDrafts, [modelId]: e.target.value })}
                                  placeholder="自定义 Variant 名称"
                                  className="h-7 w-40 rounded border border-line bg-surface px-1.5 text-[11px]"
                                />
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="sm"
                                  onClick={() => {
                                    addVariant(variantDrafts[modelId] || '');
                                    setVariantDrafts({ ...variantDrafts, [modelId]: '' });
                                  }}
                                >
                                  添加
                                </Button>
                              </div>
                            </div>
                          </div>
                        );
                      })
                    )}
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
                            const v = e.target.value;
                            setForm((f) => {
                              const next = [...(f.catalog_models || [])];
                              next[idx] = { ...next[idx], slug: v };
                              return { ...f, catalog_models: next };
                            });
                          }}
                          placeholder="slug（必填）"
                          className="h-8 min-w-0 flex-1 rounded-md border border-line bg-card px-2 font-mono text-[12px] text-ink outline-none focus:border-accent/50"
                        />
                        <input
                          value={entry.display_name || ''}
                          onChange={(e) => {
                            const v = e.target.value || undefined;
                            setForm((f) => {
                              const next = [...(f.catalog_models || [])];
                              next[idx] = { ...next[idx], display_name: v };
                              return { ...f, catalog_models: next };
                            });
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
                            setForm((f) => {
                              const next = [...(f.catalog_models || [])];
                              if (idx === 0 || idx >= next.length) return f;
                              [next[idx - 1], next[idx]] = [next[idx], next[idx - 1]];
                              return { ...f, catalog_models: next };
                            });
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
                            setForm((f) => {
                              const list = f.catalog_models || [];
                              if (idx >= list.length - 1) return f;
                              const next = [...list];
                              [next[idx], next[idx + 1]] = [next[idx + 1], next[idx]];
                              return { ...f, catalog_models: next };
                            });
                          }}
                          className="h-8 w-8 shrink-0 rounded-md border border-line text-[11px] text-ink-dim disabled:opacity-40"
                        >
                          ↓
                        </button>
                        <button
                          type="button"
                          title="删除"
                          onClick={() => {
                            setForm((f) => {
                              const next = (f.catalog_models || []).filter((_, i) => i !== idx);
                              return { ...f, catalog_models: next.length ? next : undefined };
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
                              const v = e.target.value ? Number(e.target.value) : undefined;
                              setForm((f) => {
                                const next = [...(f.catalog_models || [])];
                                next[idx] = { ...next[idx], context_window: v };
                                return { ...f, catalog_models: next };
                              });
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
                                  const v = e.target.checked || undefined;
                                  setForm((f) => {
                                    const next = [...(f.catalog_models || [])];
                                    next[idx] = { ...next[idx], [key]: v };
                                    return { ...f, catalog_models: next };
                                  });
                                }}
                              />
                              {key === 'supports_web_search' ? `${label}（需 Provider 支持）` : label}
                            </label>
                          ))}
                        </div>
                        <div className="flex flex-wrap items-center gap-2 text-[11px] text-ink-dim">
                          <span>推理等级</span>
                          {CODEX_CATALOG_LEVELS.map((level) => {
                            const levels = entry.reasoning_levels
                              ?? (
                                entry.supports_reasoning
                                  ? ['minimal', 'low', 'medium', 'high', 'xhigh']
                                  : []
                              );
                            const checked = levels.includes(level);
                            return (
                              <label key={level} className="flex items-center gap-1">
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  onChange={(e) => {
                                    const checked = e.target.checked;
                                    setForm((f) => {
                                      const next = [...(f.catalog_models || [])];
                                      const cur = next[idx];
                                      const curLevels = cur?.reasoning_levels
                                        ?? (cur?.supports_reasoning
                                          ? ['minimal', 'low', 'medium', 'high', 'xhigh']
                                          : []);
                                      const updated = checked
                                        ? [...curLevels, level]
                                        : curLevels.filter((item) => item !== level);
                                      next[idx] = {
                                        ...next[idx],
                                        reasoning_levels: Array.from(new Set(updated)),
                                        supports_reasoning: undefined,
                                      };
                                      return { ...f, catalog_models: next };
                                    });
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
                        setForm((f) => ({
                          ...f,
                          catalog_models: [...(f.catalog_models || []), { slug: '' }],
                        }))
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
                        setForm((f) => {
                          const cur = f.catalog_models || [];
                          if (cur.some((e) => e.slug === m)) return f;
                          return { ...f, catalog_models: [{ slug: m }, ...cur] };
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
                          setForm((f) => {
                            const cur = f.catalog_models || [];
                            const have = new Set(cur.map((e) => e.slug));
                            const add = models
                              .filter((m) => !have.has(m.id))
                              .map((m) => ({ slug: m.id }));
                            if (!add.length) return f;
                            return { ...f, catalog_models: [...cur, ...add] };
                          });
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

              {(tool === 'claude-code' || tool === 'zcode') && (
                <div className="space-y-2">
                  <div className="text-[12px] font-medium text-ink-dim">
                    模型角色映射 <span className="font-normal text-ink-faint">（Sonnet/Opus/Fable/Haiku → 实际模型；Claude Code 写 env，ZCode 写入 provider.models）</span>
                  </div>
                  {(['sonnet', 'opus', 'fable', 'haiku'] as const).map((role) => {
                    const labels: Record<string, string> = { sonnet: 'Sonnet', opus: 'Opus', fable: 'Fable', haiku: 'Haiku' };
                    const mm = form.model_mapping || {};
                    const setMM = (k: string, v: string) => setForm((f) => ({ ...f, model_mapping: { ...(f.model_mapping || {}), [k]: v } }));
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
                <div className="space-y-2">
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">推理强度</span>
                  <div className="flex gap-1.5">
                    {REASONING_LEVELS.map((r) => (
                      <button
                        key={r.value}
                        type="button"
                        onClick={() => setForm((f) => ({ ...f, reasoning_effort: r.value || undefined }))}
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
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">推理摘要（model_reasoning_summary）</span>
                  <div className="flex gap-1.5">
                    {REASONING_SUMMARIES.map((r) => (
                      <button
                        key={r.value}
                        type="button"
                        onClick={() => setForm((f) => ({ ...f, reasoning_summary: r.value || undefined }))}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          (form.reasoning_summary || '') === r.value
                            ? 'border-accent text-accent bg-accent/8'
                            : 'border-line text-ink-dim hover:border-line-strong'
                        }`}
                      >
                        {r.label}
                      </button>
                    ))}
                  </div>
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">Verbosity（model_verbosity）</span>
                  <div className="flex gap-1.5">
                    {VERBOSITY_LEVELS.map((r) => (
                      <button
                        key={r.value}
                        type="button"
                        onClick={() => setForm((f) => ({ ...f, verbosity: r.value || undefined }))}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          (form.verbosity || '') === r.value
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
                    <ApiModeSelector
                      value={form.api_mode}
                      onChange={(api_mode) => setForm((f) => ({ ...f, api_mode }))}
                    />
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
                            onClick={() => setForm((f) => ({ ...f, context_1m: contextModeToBool(opt.mode) }))}
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
                    <ApiModeSelector
                      value={form.api_mode}
                      onChange={(api_mode) => setForm((f) => ({ ...f, api_mode }))}
                    />
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
                            onClick={() => setForm((f) => ({ ...f, context_1m: contextModeToBool(opt.mode) }))}
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
                        setForm((f) => ({ ...f, max_tokens: undefined }));
                        return;
                      }
                      const n = Number(v);
                      const next = Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined;
                      setForm((f) => ({ ...f, max_tokens: next }));
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
                    onChange={(e) => {
                      const v = e.target.checked || undefined;
                      setForm((f) => ({ ...f, supports_standalone_web_search: v }));
                    }}
                  />
                </label>
              )}

              {tool === 'codex' && !isBedrock && !usesAuthCommand && (
                <Field label="API Key 环境变量" value={form.env_key || ''} mono
                       onChange={(e) => setForm((f) => ({ ...f, env_key: e.target.value.trim() || undefined }))}
                       placeholder="留空则由 Helio 安全写入 auth.json；例如 OPENAI_API_KEY" />
              )}

              {tool === 'codex' && !isBedrock && !usesAuthCommand && (
                <Field label="Bearer Token" type="password" value={form.experimental_bearer_token || ''} mono
                       onChange={(e) => setForm((f) => ({ ...f, experimental_bearer_token: e.target.value.trim() || undefined }))}
                       placeholder="写入 provider 的 experimental_bearer_token（不推荐，能用环境变量就用环境变量）" />
              )}

              {tool === 'codex' && !isBedrock && !usesAuthCommand && (
                <div>
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">OpenAI 鉴权（requires_openai_auth）</span>
                  <div className="flex gap-1.5">
                    {[
                      { value: undefined as boolean | undefined, label: '默认' },
                      { value: true as boolean | undefined, label: '是' },
                      { value: false as boolean | undefined, label: '否' },
                    ].map((o) => (
                      <button
                        key={o.label}
                        type="button"
                        onClick={() => setForm((f) => ({ ...f, requires_openai_auth: o.value }))}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          form.requires_openai_auth === o.value
                            ? 'border-accent text-accent bg-accent/8'
                            : 'border-line text-ink-dim hover:border-line-strong'
                        }`}
                      >
                        {o.label}
                      </button>
                    ))}
                  </div>
                  <div className="mt-1 text-[11px] text-ink-faint">
                    默认 = 自动推导：环境变量 / Bearer 模式为否，auth.json 写 key 模式为是。
                  </div>
                </div>
              )}

              {tool === 'codex' && !isBedrock && (
                <div className="space-y-2">
                  <div className="text-[12px] font-medium text-ink-dim">
                    Auth 命令 <span className="font-normal text-ink-faint">（命令式 token，写入 [model_providers] auth 块；与环境变量 / API Key 互斥；Helio 不执行该命令）</span>
                  </div>
                  <Field label="Command" value={form.auth_command || ''} mono
                         onChange={(e) => setForm((f) => ({ ...f, auth_command: e.target.value.trim() || undefined }))}
                         placeholder="例如 gcloud auth print-access-token" />
                  {usesAuthCommand && (
                    <>
                      <Field label="Args（空格分隔）" value={(form.auth_args || []).join(' ')} mono
                             onChange={(e) => {
                               const args = e.target.value.split(/\s+/).map((a) => a.trim()).filter(Boolean);
                               setForm((f) => ({ ...f, auth_args: args.length ? args : undefined }));
                             }}
                             placeholder="例如 auth print-access-token" />
                      <div className="grid grid-cols-3 gap-2">
                        <Field label="超时 ms" value={form.auth_timeout_ms != null ? String(form.auth_timeout_ms) : ''} mono
                               onChange={(e) => {
                                 const n = Number(e.target.value.trim());
                                 setForm((f) => ({ ...f, auth_timeout_ms: e.target.value.trim() && Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined }));
                               }}
                               placeholder="5000" />
                        <Field label="刷新间隔 ms" value={form.auth_refresh_interval_ms != null ? String(form.auth_refresh_interval_ms) : ''} mono
                               onChange={(e) => {
                                 const n = Number(e.target.value.trim());
                                 setForm((f) => ({ ...f, auth_refresh_interval_ms: e.target.value.trim() && Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined }));
                               }}
                               placeholder="300000" />
                        <Field label="工作目录" value={form.auth_cwd || ''} mono
                               onChange={(e) => setForm((f) => ({ ...f, auth_cwd: e.target.value.trim() || undefined }))}
                               placeholder="可选" />
                      </div>
                    </>
                  )}
                </div>
              )}

              {tool === 'codex' && (
                <div>
                  <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">Service Tier</span>
                  <div className="flex gap-1.5">
                    {SERVICE_TIERS.map((r) => (
                      <button
                        key={r.value}
                        type="button"
                        onClick={() => setForm((f) => ({ ...f, service_tier: r.value || undefined }))}
                        className={`flex-1 rounded-md px-2 py-1.5 text-[12px] font-medium border transition-all ${
                          (form.service_tier || '') === r.value
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

              {/* Claude Code / Codex only — not shared with Hermes/OpenClaw */}
              {(tool === 'claude-code' || tool === 'codex' || tool === 'zcode') && (
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
                          onClick={() => setForm((f) => ({ ...f, context_1m: contextModeToBool(opt.mode) }))}
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
                    {tool === 'zcode'
                      ? 'provider.models.*.limit.context · '
                      : tool === 'claude-code'
                        ? 'Claude env [1M] 后缀 · '
                        : 'model_context_window · '}
                    {contextPreviewLine(form.context_1m, form.model, tool)}
                  </div>
                </div>
              )}
            </div>
          )}
        </fieldset>
      </form>
    </Modal>
  );
}
