import { duplicateProfileDraft } from './profileCopy';

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

const draft = duplicateProfileDraft(source, ['prod', 'prod-copy']);

equal(draft.id, undefined);
equal(draft.name, 'prod-copy-2');
equal(draft.target_app, 'opencode');
equal(draft.created_at, undefined);
equal(draft.updated_at, undefined);
equal(source.id, 42);
