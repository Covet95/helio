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
  gemini: [
    { id: 'google', label: 'Google 官方', provider: 'google', api_url: 'https://generativelanguage.googleapis.com', model: 'gemini-2.0-flash', category: 'official' },
    { id: 'custom', label: '自定义', provider: 'google', api_url: '', category: 'custom' },
  ],
  opencode: [
    { id: 'anthropic', label: 'Anthropic', provider: 'anthropic', api_url: 'https://api.anthropic.com', category: 'official' },
    { id: 'openai', label: 'OpenAI', provider: 'openai', api_url: 'https://api.openai.com/v1', category: 'official' },
    { id: 'custom', label: '自定义', provider: 'custom', api_url: '', category: 'custom' },
  ],
};

/** 推理强度选项（Codex） */
export const REASONING_LEVELS = [
  { value: '', label: '默认' },
  { value: 'low', label: '低' },
  { value: 'medium', label: '中' },
  { value: 'high', label: '高' },
  { value: 'xhigh', label: '极高' },
];
