import { profileApiCredentialsText } from './profileCopy';
import { describe, expect, it } from 'vitest';

describe('profileApiCredentialsText', () => {
  it('copies only the API credentials', () => {
    const source = {
      id: 42,
      name: 'prod',
      provider: 'anthropic',
      api_url: 'https://api.example.com',
      api_key: 'sk-test',
      target_app: 'opencode' as const,
      created_at: 100,
      updated_at: 200,
    };

    expect(profileApiCredentialsText(source)).toBe('API URL: https://api.example.com\nAPI Key: sk-test');
  });
});
