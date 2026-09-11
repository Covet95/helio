import type { TargetApp } from '@/types';

export interface ProviderPreset {
  id: string;
  label: string;
  provider: string;
  api_url: string;
  /** 默认模型建议 */
  model?: string;
  category: 'official' | 'third_party' | 'custom';
}

/** 常用 provider 预设（选中后填充表单，可覆盖） */
export const PROVIDER_PRESETS: Record<TargetApp, ProviderPreset[]> = {
  'claude-code': [
    { id: 'anthropic', label: 'Anthropic 官方', provider: 'anthropic', api_url: 'https://api.anthropic.com', category: 'official' },
    { id: 'deepseek', label: 'DeepSeek', provider: 'anthropic', api_url: 'https://api.deepseek.com/anthropic', model: 'deepseek-chat', category: 'third_party' },
    { id: 'glm', label: '智谱 GLM', provider: 'anthropic', api_url: 'https://open.bigmodel.cn/api/anthropic', model: 'glm-4', category: 'third_party' },
    { id: 'kimi', label: 'Kimi', provider: 'anthropic', api_url: 'https://api.moonshot.cn/anthropic', category: 'third_party' },
    { id: 'custom', label: '自定义', provider: 'anthropic', api_url: '', category: 'custom' },
  ],
  codex: [
    { id: 'openai', label: 'OpenAI 官方', provider: 'openai', api_url: 'https://api.openai.com/v1', model: 'gpt-5.5', category: 'official' },
    { id: 'custom', label: '自定义中转', provider: 'openai', api_url: '', model: 'gpt-5.5', category: 'custom' },
  ],
  pi: [
    { id: 'anthropic', label: 'Anthropic 官方', provider: 'anthropic', api_url: 'https://api.anthropic.com', model: 'claude-sonnet-4-5', category: 'official' },
    { id: 'openai', label: 'OpenAI 官方', provider: 'openai', api_url: 'https://api.openai.com/v1', model: 'gpt-5.5', category: 'official' },
    { id: 'google', label: 'Google 官方', provider: 'google', api_url: 'https://generativelanguage.googleapis.com', model: 'gemini-2.0-flash', category: 'official' },
    { id: 'custom', label: '自定义 endpoint', provider: 'custom', api_url: '', category: 'custom' },
  ],
  opencode: [
    { id: 'anthropic', label: 'Anthropic', provider: 'anthropic', api_url: 'https://api.anthropic.com', category: 'official' },
    { id: 'openai', label: 'OpenAI', provider: 'openai', api_url: 'https://api.openai.com/v1', category: 'official' },
    { id: 'local', label: '本地中转', provider: 'cpa', api_url: 'http://127.0.0.1:8317/v1', category: 'third_party' },
    { id: 'custom', label: '自定义', provider: 'custom', api_url: '', category: 'custom' },
  ],
  hermes: [
    { id: 'custom', label: 'Custom endpoint', provider: 'custom', api_url: 'https://api.example.com/v1', model: 'gpt-5.5', category: 'custom' },
    { id: 'freemodel', label: 'FreeModel 示例', provider: 'freemodel', api_url: 'https://api.freemodel.dev/v1', model: 'gpt-5.5', category: 'third_party' },
    { id: 'local', label: '本地中转', provider: 'cpa', api_url: 'http://127.0.0.1:8317/v1', model: 'claude-opus-4-8', category: 'third_party' },
  ],
  openclaw: [
    { id: 'cpa', label: '本地中转 CPA', provider: 'cpa', api_url: 'http://127.0.0.1:8317/v1', model: 'claude-opus-4-8', category: 'third_party' },
    { id: 'custom', label: 'Custom provider', provider: 'custom', api_url: 'https://api.example.com/v1', model: 'gpt-5.5', category: 'custom' },
  ],
  zcode: [
    { id: 'anthropic', label: 'Anthropic 官方', provider: 'anthropic', api_url: 'https://api.anthropic.com', category: 'official' },
    { id: 'deepseek', label: 'DeepSeek', provider: 'anthropic', api_url: 'https://api.deepseek.com/anthropic', model: 'deepseek-chat', category: 'third_party' },
    { id: 'glm', label: '智谱 GLM', provider: 'anthropic', api_url: 'https://open.bigmodel.cn/api/anthropic', model: 'glm-4', category: 'third_party' },
    { id: 'kimi', label: 'Kimi', provider: 'anthropic', api_url: 'https://api.moonshot.cn/anthropic', category: 'third_party' },
    { id: 'custom', label: '自定义', provider: 'anthropic', api_url: '', category: 'custom' },
  ],
};

/** 推理强度选项（Codex 顶层 model_reasoning_effort，官方 5 档） */
export const REASONING_LEVELS = [
  { value: '', label: '默认' },
  { value: 'minimal', label: '极简' },
  { value: 'low', label: '低' },
  { value: 'medium', label: '中' },
  { value: 'high', label: '高' },
  { value: 'xhigh', label: '极高' },
];

/**
 * Catalog（model_catalog.json）侧推理档位：官方新模型（如 gpt-5.6-sol）还有
 * none/max/ultra，用户显式声明时透传；顶层仍只认 REASONING_LEVELS 的 5 档。
 */
export const CODEX_CATALOG_LEVELS = [
  'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra',
] as const;

/** Service Tier 选项（Codex）：fast 为 legacy 别名，官方主推 flex / priority */
export const SERVICE_TIERS = [
  { value: '', label: '默认' },
  { value: 'flex', label: 'flex' },
  { value: 'priority', label: 'priority' },
  { value: 'fast', label: 'fast（兼容）' },
];

/** 推理摘要选项（Codex model_reasoning_summary） */
export const REASONING_SUMMARIES = [
  { value: '', label: '默认' },
  { value: 'auto', label: 'auto' },
  { value: 'concise', label: 'concise' },
  { value: 'detailed', label: 'detailed' },
  { value: 'none', label: 'none' },
];

/** Verbosity 选项（Codex model_verbosity） */
export const VERBOSITY_LEVELS = [
  { value: '', label: '默认' },
  { value: 'low', label: 'low' },
  { value: 'medium', label: 'medium' },
  { value: 'high', label: 'high' },
];
