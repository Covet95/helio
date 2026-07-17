import {
  contextBadgeLabel,
  contextModeFromBool,
  contextModeToBool,
  isGrokModel,
  resolvedContextTokens,
  standardContextTokens,
  statusKeyFor,
} from './contextWindow';

function equal(actual: unknown, expected: unknown, label?: string) {
  if (actual !== expected) {
    throw new Error(`${label ?? 'assert'}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

equal(isGrokModel('grok-4.5'), true, 'grok');
equal(isGrokModel('xai/grok-4'), true, 'xai/grok');
equal(isGrokModel('claude-opus-4-8'), false, 'claude');
equal(standardContextTokens('grok-4.5'), 500_000, 'std grok');
equal(standardContextTokens('gpt-5.5'), 200_000, 'std other');
equal(contextModeFromBool(true), '1m');
equal(contextModeFromBool(false), 'standard');
equal(contextModeFromBool(undefined), 'unset');
equal(contextModeToBool('1m'), true);
equal(contextModeToBool('standard'), false);
equal(contextModeToBool('unset'), undefined);
equal(resolvedContextTokens(true, 'x'), 1_000_000);
equal(resolvedContextTokens(false, 'grok-4.5'), 500_000);
equal(resolvedContextTokens(false, 'claude'), 200_000);
equal(resolvedContextTokens(undefined, 'grok'), null);
equal(contextBadgeLabel(false, 'grok-4.5', { tool: 'hermes' }), '500k');
equal(contextBadgeLabel(true, 'x', { tool: 'hermes' }), '1M');
equal(statusKeyFor('claude-code'), 'claude_code');

console.log('contextWindow.test.ts: ok');
