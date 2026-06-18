import { duplicateProfileDraft, profileApiConfigText, profileApiUrlText } from './profileCopy';

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

equal(profileApiUrlText(source), 'https://api.example.com');

const copiedConfig = JSON.parse(profileApiConfigText({
  ...source,
  model: 'claude-sonnet',
  models: ['claude-sonnet', 'claude-opus'],
  reasoning_effort: 'high',
  context_1m: true,
}));

equal(copiedConfig.name, 'prod');
equal(copiedConfig.target_app, 'opencode');
equal(copiedConfig.provider, 'anthropic');
equal(copiedConfig.api_url, 'https://api.example.com');
equal(copiedConfig.api_key, 'sk-test');
equal(copiedConfig.model, 'claude-sonnet');
equal(copiedConfig.models.length, 2);
equal(copiedConfig.reasoning_effort, 'high');
equal(copiedConfig.context_1m, true);
equal(copiedConfig.id, undefined);
equal(copiedConfig.created_at, undefined);
equal(copiedConfig.updated_at, undefined);
