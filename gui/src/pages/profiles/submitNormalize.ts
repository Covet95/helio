/**
 * 表单提交前的字段归一与互斥清洗。
 *
 * 从 `ProfileFormModal` 的 `submit()` 里抽出来——那段逻辑此前埋在组件内部，
 * 只能靠渲染测试碰运气，而它是整个应用**最容易出错**的地方：
 * 7 个工具共享一套表单，字段按 `tool` 条件保留/清空，凭据模式之间还有互斥
 * 规则（env_key / Bearer / Auth 命令 / Bedrock）。
 *
 * 抽成纯函数后可以表驱动单测，不需要渲染。
 */
import type { ApiProfile, TargetApp } from '@/types';
import { normalizeCodexCatalogModels, normalizeOpenCodeModelConfigs } from './helpers';

/** 已归一化、可直接提交的表单值。 */
export type NormalizedSubmit = ApiProfile & { target_app: TargetApp };

/** 校验失败时返回的原因（中文，直接展示给用户）。 */
export type ValidationError = string;

/**
 * 校验并归一化表单。
 *
 * 返回 `{ ok: true, value }` 或 `{ ok: false, error }`——用返回值而非抛异常，
 * 便于调用方区分「校验失败」与「保存失败」。
 */
export function normalizeSubmit(
  form: ApiProfile,
  tool: TargetApp,
): { ok: true; value: NormalizedSubmit } | { ok: false; error: ValidationError } {
  const normalized = withActiveKey(form, ensureKeyPool(form));

  const isCodex = tool === 'codex';
  const usesCodexEnv = isCodex && Boolean(normalized.env_key?.trim());
  const usesAuthCmd = isCodex && Boolean(normalized.auth_command?.trim());
  const usesBearer = isCodex && Boolean(normalized.experimental_bearer_token?.trim());
  const usesBedrock =
    isCodex && normalized.provider.trim().toLowerCase() === 'amazon-bedrock';

  // 必填校验：Bedrock 不需要 URL/Key（走 AWS 凭据链）。
  const missingBase = !normalized.name.trim() || !normalized.provider.trim();
  const missingUrl = !usesBedrock && !normalized.api_url.trim();
  const missingCredential =
    !usesBedrock && !usesCodexEnv && !usesAuthCmd && !usesBearer && !normalized.api_key.trim();
  if (missingBase || missingUrl || missingCredential) {
    return {
      ok: false,
      error:
        '请填写名称、Provider、API URL，并提供 API Key、环境变量名、Bearer Token 或 Auth 命令',
    };
  }

  // 凭据模式互斥：env_key 与 Bearer 只能留一个。
  if (usesCodexEnv && usesBearer) {
    return {
      ok: false,
      error: 'Codex 环境变量与 Bearer Token 请只保留一个（与 Auth 命令也互斥）',
    };
  }

  const value: NormalizedSubmit = {
    ...normalized,
    // Bedrock 走 AWS 凭据链，不写 URL/Key/Key 池。
    api_url: usesBedrock ? '' : normalized.api_url,
    api_key: usesBedrock ? '' : normalized.api_key,
    api_keys: usesBedrock ? undefined : normalized.api_keys,
    wire_api: normalizeWireApi(normalized.wire_api, tool),
    env_key: usesBedrock || usesAuthCmd ? undefined : normalized.env_key,
    experimental_bearer_token: usesAuthCmd ? undefined : normalized.experimental_bearer_token,
    requires_openai_auth: usesAuthCmd ? undefined : normalized.requires_openai_auth,
    auth_command: usesAuthCmd ? normalized.auth_command?.trim() || undefined : undefined,
    auth_args: usesAuthCmd ? normalizeAuthArgs(normalized.auth_args) : undefined,
    auth_timeout_ms: usesAuthCmd
      ? positiveIntOrUndefined(normalized.auth_timeout_ms)
      : undefined,
    auth_refresh_interval_ms: usesAuthCmd
      ? positiveIntOrUndefined(normalized.auth_refresh_interval_ms)
      : undefined,
    auth_cwd: usesAuthCmd ? normalized.auth_cwd?.trim() || undefined : undefined,
    supports_standalone_web_search: usesBedrock
      ? undefined
      : normalized.supports_standalone_web_search || undefined,
    target_app: tool,
    catalog_models: normalizeCatalog(normalized.catalog_models, tool),
    model_configs:
      tool === 'opencode' ? normalizeOpenCodeModelConfigs(normalized.model_configs) : undefined,
    opencode_api_mode:
      tool === 'opencode' ? normalized.opencode_api_mode?.trim() || undefined : undefined,
  };

  return { ok: true, value };
}

/** 非 Codex 工具不保存 catalog；Codex 则做去重与档位过滤。 */
function normalizeCatalog(
  catalogModels: ApiProfile['catalog_models'],
  tool: TargetApp,
) {
  if (tool !== 'codex') return undefined;
  return catalogModels ? normalizeCodexCatalogModels(catalogModels) : catalogModels;
}

/**
 * Codex wire 归一：responses 系别名与 chat 系历史值都写成 `responses`，
 * 未知值返回 `undefined`（不保存，后端默认 responses）。
 *
 * 非 Codex 工具一律不保存该字段。
 */
export function normalizeWireApi(
  wireApi: string | undefined,
  tool: TargetApp,
): string | undefined {
  if (tool !== 'codex' || !wireApi?.trim()) return undefined;
  const w = wireApi.trim().toLowerCase();
  return WIRE_API_ALIASES.includes(w) ? 'responses' : undefined;
}

/** 官方已删除 chat 系取值，但历史数据里仍有；一并归一为 responses。 */
const WIRE_API_ALIASES = [
  'responses',
  'openai-responses',
  'openai_responses',
  'codex_responses',
  'chat',
  'chat_completions',
  'openai-chat',
];

/** auth 命令参数：去空白、丢空项；全空则返回 undefined。 */
function normalizeAuthArgs(args: string[] | undefined): string[] | undefined {
  const cleaned = (args || []).map((a) => a.trim()).filter(Boolean);
  return cleaned.length ? cleaned : undefined;
}

/** 正数才保留，否则 undefined（超时/间隔不接受 0 或负数）。 */
function positiveIntOrUndefined(n: unknown): number | undefined {
  return typeof n === 'number' && Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined;
}

/** 生成 key id。时间戳 + 随机后缀，避免同毫秒内碰撞。 */
export function newKeyId(): string {
  return `k${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
}

/**
 * 把档案的 key 池归一成至少一项。
 *
 * 优先用已有的 `api_keys`；否则由单 key 字段造一条；都没有则给一条空 key
 * 供用户填写（保持「永远至少有一个可编辑的 key 行」）。
 */
export function ensureKeyPool(p: ApiProfile) {
  if (p.api_keys && p.api_keys.length > 0) {
    return p.api_keys.map((e) => ({ ...e }));
  }
  if (p.api_key?.trim()) {
    return [{ id: newKeyId(), label: 'default', key: p.api_key, is_active: true }];
  }
  return [{ id: newKeyId(), label: 'default', key: '', is_active: true }];
}

/**
 * 把「活跃 key」同步回 `api_key` 单值字段。
 *
 * 后端只读 `api_key`（适配器不认 key 池），所以提交前必须把池里标记为
 * `is_active` 的那把回填过去；没有标记则取第一把。
 */
export function withActiveKey(p: ApiProfile, keys: ReturnType<typeof ensureKeyPool>): ApiProfile {
  const active = keys.find((k) => k.is_active) || keys[0];
  return {
    ...p,
    api_keys: keys,
    api_key: active?.key ?? p.api_key,
  };
}
