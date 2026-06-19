import { profileApiCredentialsText } from './profileCopy';

function equal(actual: unknown, expected: unknown) {
  if (actual !== expected) {
    throw new Error(`expected ${String(expected)}, got ${String(actual)}`);
  }
}

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

equal(profileApiCredentialsText(source), 'API URL: https://api.example.com\nAPI Key: sk-test');
