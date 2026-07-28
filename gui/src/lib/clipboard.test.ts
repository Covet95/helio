import { copyText } from './clipboard';
import { describe, expect, it } from 'vitest';

describe('copyText', () => {
  it('uses the supplied native clipboard writer', async () => {
    let copied = '';

    await copyText('native clipboard text', async (text) => {
      copied = text;
    });

    expect(copied).toBe('native clipboard text');
  });
});
