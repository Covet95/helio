import { describe, expect, it } from 'vitest';
import { describeLocalDrift, normalizeApiUrl } from './localDrift';

describe('normalizeApiUrl', () => {
  it('ignores trailing slashes and whitespace', () => {
    expect(normalizeApiUrl('https://api.example.com/v1///  ')).toBe('https://api.example.com/v1');
  });
});

describe('describeLocalDrift', () => {
  it('returns null when nothing was found on disk', () => {
    expect(
      describeLocalDrift({ api_url: 'https://a', api_key: 'k' }, { found: false, api_url: '', api_key: '' }),
    ).toBeNull();
    expect(describeLocalDrift({ api_url: 'https://a', api_key: 'k' }, null)).toBeNull();
  });

  it('reports consistent for equal endpoints', () => {
    expect(
      describeLocalDrift(
        { api_url: 'https://api.example.com/v1', api_key: 'k' },
        { found: true, api_url: 'https://api.example.com/v1/', api_key: 'k' },
      ),
    ).toBe('consistent');
  });

  it('reports url drift', () => {
    expect(
      describeLocalDrift(
        { api_url: 'https://old.example.com', api_key: 'k' },
        { found: true, api_url: 'https://new.example.com', api_key: 'k' },
      ),
    ).toBe('url');
  });

  it('reports key drift only when both sides are non-empty', () => {
    expect(
      describeLocalDrift(
        { api_url: 'https://a', api_key: 'old' },
        { found: true, api_url: 'https://a', api_key: 'new' },
      ),
    ).toBe('key');
    expect(
      describeLocalDrift(
        { api_url: 'https://a', api_key: '' },
        { found: true, api_url: 'https://a', api_key: 'new' },
      ),
    ).toBe('consistent');
  });
});
