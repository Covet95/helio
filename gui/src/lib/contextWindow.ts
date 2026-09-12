import type { ApiProfile, TargetApp } from '@/types';

/** Align with Helio Rust adapters: Grok 500k / others 200k / 1M toggle. */
export const CONTEXT_1M = 1_000_000;
export const CONTEXT_GROK = 500_000;
export const CONTEXT_STANDARD = 200_000;

export type ContextMode = 'unset' | 'standard' | '1m';

export function isGrokModel(model?: string | null): boolean {
  return !!model?.trim() && model.trim().toLowerCase().includes('grok');
}

export function standardContextTokens(model?: string | null): number {
  return isGrokModel(model) ? CONTEXT_GROK : CONTEXT_STANDARD;
}

export function contextModeFromBool(v?: boolean | null): ContextMode {
  if (v === true) return '1m';
  if (v === false) return 'standard';
  return 'unset';
}

export function contextModeToBool(mode: ContextMode): boolean | undefined {
  if (mode === '1m') return true;
  if (mode === 'standard') return false;
  return undefined;
}

/** Tokens that will be written when mode is 1m/standard. unset → null. */
export function resolvedContextTokens(
  context1m: boolean | undefined | null,
  model?: string | null,
): number | null {
  if (context1m === true) return CONTEXT_1M;
  if (context1m === false) return standardContextTokens(model);
  return null;
}

/** Short badge for list/status cards. */
export function contextBadgeLabel(
  context1m: boolean | undefined | null,
  model?: string | null,
  opts?: { tool?: TargetApp },
): string {
  const tokens = resolvedContextTokens(context1m, model);
  if (tokens === CONTEXT_1M) return '1M';
  if (tokens === CONTEXT_GROK) return '500k';
  if (tokens === CONTEXT_STANDARD) return '200k';
  // Hermes/OpenClaw default when unset: adapters may still pick defaults on None differently
  if (opts?.tool === 'openclaw') return '默认·1M';
  if (opts?.tool === 'hermes') return '默认·不写';
  return '—';
}

export function contextPreviewLine(
  context1m: boolean | undefined | null,
  model?: string | null,
  tool?: TargetApp,
): string {
  const tokens = resolvedContextTokens(context1m, model);
  if (tokens != null) {
    const note =
      tokens === CONTEXT_GROK
        ? '（Grok 标准）'
        : tokens === CONTEXT_STANDARD
          ? '（非 Grok 标准）'
          : tokens === CONTEXT_1M
            ? '（1M）'
            : '';
    return `将写入 ${tokens.toLocaleString()}${note}`;
  }
  if (tool === 'openclaw') return '不强制覆盖；OpenClaw 无现值时偏 1M 默认';
  if (tool === 'hermes') return '不写入 context_length（保留本地现值）';
  return '不修改上下文窗口字段';
}

/** Tool → StatusInfo key.显式映射：加新工具时编译器会提醒补全，而不是运行时拼字符串。 */
export type ToolStatusKey = 'claude_code' | 'codex' | 'pi' | 'opencode' | 'hermes' | 'openclaw' | 'zcode';

const STATUS_KEY_BY_TOOL: Record<TargetApp, ToolStatusKey> = {
  'claude-code': 'claude_code',
  codex: 'codex',
  pi: 'pi',
  opencode: 'opencode',
  hermes: 'hermes',
  openclaw: 'openclaw',
  zcode: 'zcode',
};

export function statusKeyFor(targetApp: TargetApp): ToolStatusKey {
  return STATUS_KEY_BY_TOOL[targetApp];
}

export function profileKeyCount(p: ApiProfile): number {
  if (p.api_keys && p.api_keys.length > 0) return p.api_keys.length;
  return p.api_key?.trim() ? 1 : 0;
}

export function activeKeyLabel(p: ApiProfile): string | undefined {
  const active = p.api_keys?.find((k) => k.is_active);
  return active?.label || undefined;
}
