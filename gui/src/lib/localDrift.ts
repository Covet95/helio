/** 磁盘 API 与启用档案的一致性判定（纯函数，可单测）。 */

export interface LocalApiSnapshot {
  found: boolean;
  api_url: string;
  api_key: string;
}

export interface ProfileApiRef {
  api_url: string;
  api_key: string;
}

export type LocalDrift = 'url' | 'key' | 'consistent';

/** 归一 URL：去首尾空白与末尾斜杠。只为发现手改，不做语义等价。 */
export function normalizeApiUrl(url: string): string {
  return (url || '').trim().replace(/\/+$/, '');
}

/**
 * 磁盘未检出凭据时返回 null（不误报，调用方直接不展示）。
 * Key 仅在两侧都非空才比较：Codex 等凭据可放 env 或 auth 命令，单侧为空不算偏离。
 */
export function describeLocalDrift(
  profile: ProfileApiRef,
  scanned: LocalApiSnapshot | null | undefined,
): LocalDrift | null {
  if (!scanned || !scanned.found) return null;
  if (normalizeApiUrl(profile.api_url) !== normalizeApiUrl(scanned.api_url)) return 'url';
  const a = (profile.api_key || '').trim();
  const b = (scanned.api_key || '').trim();
  if (a && b && a !== b) return 'key';
  return 'consistent';
}
