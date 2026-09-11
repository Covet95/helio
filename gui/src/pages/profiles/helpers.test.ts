import { describe, expect, it } from 'vitest';
import {
  emptyProfileForTool,
  normalizeCodexCatalogModels,
  normalizeOpenCodeModelConfigs,
  profileConfigFingerprint,
  providerTint,
} from './helpers';
import { humanizeError } from '../../lib/utils';

describe('profile helpers', () => {
  it('creates tool-specific defaults', () => {
    expect(emptyProfileForTool('openclaw')).toMatchObject({
      target_app: 'openclaw',
      context_1m: true,
      max_tokens: 128000,
      api_mode: 'chat_completions',
    });
    expect(emptyProfileForTool('hermes')).toMatchObject({
      target_app: 'hermes',
      context_1m: false,
      api_mode: 'chat_completions',
    });
    expect(emptyProfileForTool('opencode')).toMatchObject({
      target_app: 'opencode',
      opencode_api_mode: "",
    });
    expect(emptyProfileForTool('zcode')).toMatchObject({
      target_app: 'zcode',
      provider: 'anthropic',
      api_url: 'https://api.anthropic.com',
    });
    expect(emptyProfileForTool('zcode', {
      name: 'cc',
      provider: 'anthropic',
      api_url: 'https://api.deepseek.com/anthropic',
      api_key: 'sk-from-claude',
      model: 'deepseek-chat',
      model_mapping: { sonnet_model: 'deepseek-chat' },
      target_app: 'claude-code',
    })).toMatchObject({
      target_app: 'zcode',
      provider: 'anthropic',
      api_url: 'https://api.deepseek.com/anthropic',
      api_key: 'sk-from-claude',
      model: 'deepseek-chat',
      model_mapping: { sonnet_model: 'deepseek-chat' },
    });
  });

  it('normalizes presentation helpers', () => {
    expect(providerTint('OpenAI compatible')).toBe('#10B981');
    expect(humanizeError(new Error('TypeError: unavailable'))).toBe('unavailable');
    expect(humanizeError(new Error('TypeError: unavailable'), '剪贴板不可用')).toBe('unavailable');
    expect(humanizeError('TypeError: nothing')).toBe('nothing');
    expect(humanizeError('')).toBe('发生未知错误');
  });

  it('normalizes Codex catalog reasoning levels without writing the legacy flag', () => {
    expect(normalizeCodexCatalogModels([
      {
        slug: 'proxy-model',
        supports_reasoning: true,
        reasoning_levels: ['XHIGH', 'low', 'xhigh', 'max', 'ultra', 'none', 'unsupported'],
        supports_web_search: true,
      },
      {
        slug: 'legacy-model',
        supports_reasoning: true,
      },
      { slug: '   ' },
    ])).toEqual([
      {
        slug: 'proxy-model',
        reasoning_levels: ['xhigh', 'low', 'max', 'ultra', 'none'],
        supports_web_search: true,
      },
      {
        slug: 'legacy-model',
        reasoning_levels: ['minimal', 'low', 'medium', 'high', 'xhigh'],
      },
    ]);
  });

  it('normalizes OpenCode model configs and keeps variants', () => {
    expect(normalizeOpenCodeModelConfigs({
      '  gpt-5  ': {
        options: { reasoningEffort: 'high' },
        variants: {
          low: { reasoningEffort: 'low' },
          max: { thinking: { type: 'enabled', budgetTokens: 32000 } },
        },
      },
      '   ': {},
      empty: {},
    })).toEqual({
      'gpt-5': {
        options: { reasoningEffort: 'high' },
        variants: {
          low: { reasoningEffort: 'low' },
          max: { thinking: { type: 'enabled', budgetTokens: 32000 } },
        },
      },
    });
    expect(normalizeOpenCodeModelConfigs(undefined)).toBeUndefined();
  });

  it('deduplicates only equivalent configuration, not intentional variants', () => {
    const base = {
      name: 'one',
      provider: 'proxy',
      api_url: 'https://proxy.example/v1',
      api_key: 'sk-live',
      target_app: 'codex' as const,
      model: 'gpt-5',
      wire_api: 'responses',
      api_keys: [{
        id: 'generated-a',
        label: 'primary',
        key: 'sk-live',
        is_active: true,
        last_probe_ok: true,
        last_probed_at: 10,
      }],
    };
    const same = {
      ...base,
      name: 'two',
      id: 99,
      updated_at: 20,
      api_keys: [{ ...base.api_keys[0], id: 'generated-b', last_probe_ok: false, last_probed_at: 99 }],
    };
    const flex = { ...base, name: 'flex', service_tier: 'flex' };

    expect(profileConfigFingerprint(base)).toBe(profileConfigFingerprint(same));
    expect(profileConfigFingerprint(base)).not.toBe(profileConfigFingerprint(flex));
  });
});
