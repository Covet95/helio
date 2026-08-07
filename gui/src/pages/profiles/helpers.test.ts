import { describe, expect, it } from 'vitest';
import {
  emptyProfileForTool,
  normalizeCodexCatalogModels,
  normalizeOpenCodeModelConfigs,
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
      opencode_api_mode: 'chat_completions',
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
        reasoning_levels: ['XHIGH', 'low', 'xhigh', 'unsupported'],
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
        reasoning_levels: ['xhigh', 'low'],
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
});
