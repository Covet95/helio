/**
 * 本机扫描结果 → 配置档案的字段映射。
 *
 * 从 `ImportPage.importProfile()` 里抽出来。这是**第二个**「按 tool 门控字段」
 * 的地方（第一个是 `profiles/submitNormalize.ts` 的表单提交路径），
 * 而两处的门控规则**并不一致**——这正是最该被测试钉住的地方：
 * 新增一个字段时漏改一处，就会写进脏数据。
 *
 * 本次抽取**保持 ImportPage 的现有行为**（含它与 submitNormalize 的分歧），
 * 不做「顺手统一」——分歧点已在测试里逐条标注，需要产品决策后再动。
 */
import type { OpenCodeModelConfig, TargetApp } from '@/types';

/** `scan_local_api` 的返回形状（与 Rust 侧 `ScannedApi` 对应）。 */
export interface ScannedApi {
  found: boolean;
  api_url: string;
  api_key: string;
  provider: string;
  model?: string;
  model_mapping?: Record<string, string>;
  reasoning_effort?: string;
  reasoning_summary?: string;
  verbosity?: string;
  context_1m?: boolean;
  wire_api?: string;
  env_key?: string;
  requires_openai_auth?: boolean;
  experimental_bearer_token?: string;
  service_tier?: string;
  supports_standalone_web_search?: boolean;
  aws_profile?: string;
  aws_region?: string;
  auth_command?: string;
  auth_args?: string[];
  auth_timeout_ms?: number;
  auth_refresh_interval_ms?: number;
  auth_cwd?: string;
  api_mode?: string;
  opencode_api_mode?: string;
  opencode_models?: string[];
  opencode_model_configs?: Record<string, OpenCodeModelConfig>;
  max_tokens?: number;
  source: string;
}

/** 提交给 `add_profile` 的载荷。 */
export interface ImportPayload {
  name: string;
  target_app: TargetApp;
  [field: string]: unknown;
}

/** 仅 Codex 读的字段。 */
const CODEX_ONLY = [
  'reasoning_summary',
  'verbosity',
  'env_key',
  'wire_api',
  'requires_openai_auth',
  'experimental_bearer_token',
  'auth_command',
  'auth_args',
  'auth_timeout_ms',
  'auth_refresh_interval_ms',
  'auth_cwd',
] as const;

/** 仅 opencode 读的字段。 */
const OPENCODE_ONLY = [
  'opencode_api_mode',
  'opencode_models',
  'opencode_model_configs',
] as const;

/** 所有工具共用、原样透传的字段。 */
const PASSTHROUGH = [
  'provider',
  'api_url',
  'api_key',
  'model',
  'model_mapping',
  'reasoning_effort',
  'context_1m',
  'service_tier',
  'supports_standalone_web_search',
  'aws_profile',
  'aws_region',
] as const;

/**
 * 把扫描结果映射成档案载荷。
 *
 * 门控规则（**当前行为，非理想设计**）：
 * - Codex 专属 → 仅 codex
 * - `api_mode` → 仅 hermes / openclaw
 * - opencode 专属 → 仅 opencode（`models` 用 opencode 的复数名，档案里叫 `models`）
 * - `max_tokens` → 仅 openclaw
 * - 其余 → 全工具透传
 */
export function buildImportPayload(
  scanned: ScannedApi,
  tool: TargetApp,
  name: string,
): ImportPayload {
  const isCodex = tool === 'codex';
  const payload: ImportPayload = {
    name,
    target_app: tool,
  };

  for (const field of PASSTHROUGH) {
    payload[field] = scanned[field];
  }
  for (const field of CODEX_ONLY) {
    payload[field] = isCodex ? scanned[field] : undefined;
  }
  payload.api_mode = tool === 'hermes' || tool === 'openclaw' ? scanned.api_mode : undefined;
  for (const field of OPENCODE_ONLY) {
    payload[field] = tool === 'opencode' ? scanned[field] : undefined;
  }
  // 档案字段叫 `models`，扫描结果叫 `opencode_models`——名字不同，需显式转。
  payload.models = tool === 'opencode' ? scanned.opencode_models : undefined;
  payload.max_tokens = tool === 'openclaw' ? scanned.max_tokens : undefined;

  return payload;
}

/** 把后端唯一约束报错翻译成用户能照做的提示。 */
export function friendlyImportError(raw: string, name: string, humanized: string): string {
  return /UNIQUE constraint failed/i.test(raw)
    ? `已存在同名档案「${name}」，请改个名字再导入`
    : `导入失败: ${humanized}`;
}
