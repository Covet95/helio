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
};

/** 推理强度选项（Codex） */
export const REASONING_LEVELS = [
  { value: '', label: '默认' },
  { value: 'minimal', label: '极简' },
  { value: 'low', label: '低' },
  { value: 'medium', label: '中' },
  { value: 'high', label: '高' },
  { value: 'xhigh', label: '极高' },
];
