import {
  contextBadgeLabel,
  contextModeFromBool,
  contextModeToBool,
  isGrokModel,
  resolvedContextTokens,
  standardContextTokens,
  statusKeyFor,
} from './contextWindow';
import { describe, expect, it } from 'vitest';

describe('contextWindow', () => {
  it('recognizes model families and standard contexts', () => {
    expect(isGrokModel('grok-4.5')).toBe(true);
    expect(isGrokModel('xai/grok-4')).toBe(true);
    expect(isGrokModel('claude-opus-4-8')).toBe(false);
    expect(standardContextTokens('grok-4.5')).toBe(500_000);
    expect(standardContextTokens('gpt-5.5')).toBe(200_000);
  });

  it('maps context modes and labels', () => {
    expect(contextModeFromBool(true)).toBe('1m');
    expect(contextModeFromBool(false)).toBe('standard');
    expect(contextModeFromBool(undefined)).toBe('unset');
    expect(contextModeToBool('1m')).toBe(true);
    expect(contextModeToBool('standard')).toBe(false);
    expect(contextModeToBool('unset')).toBeUndefined();
    expect(resolvedContextTokens(true, 'x')).toBe(1_000_000);
    expect(resolvedContextTokens(false, 'grok-4.5')).toBe(500_000);
    expect(resolvedContextTokens(false, 'claude')).toBe(200_000);
    expect(resolvedContextTokens(undefined, 'grok')).toBeNull();
    expect(contextBadgeLabel(false, 'grok-4.5', { tool: 'hermes' })).toBe('500k');
    expect(contextBadgeLabel(true, 'x', { tool: 'hermes' })).toBe('1M');
    expect(statusKeyFor('claude-code')).toBe('claude_code');
  });
});
