import { describe, expect, it } from 'vitest';
import { emptyProfileForTool, humanizeCopyError, providerTint } from './helpers';

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
  });

  it('normalizes presentation helpers', () => {
    expect(providerTint('OpenAI compatible')).toBe('#10B981');
    expect(humanizeCopyError(new Error('TypeError: unavailable'))).toBe('unavailable');
  });
});
