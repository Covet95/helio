import { clsx, type ClassValue } from 'clsx';
import type { AppError } from '@/types';

export function cn(...inputs: ClassValue[]) {
  return clsx(inputs);
}

export function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

export function maskApiKey(key: string): string {
  if (key.length <= 15) return '***';
  return `${key.slice(0, 10)}...${key.slice(-5)}`;
}

/**
 * 判断后端抛出的错误是否为结构化错误（`AppError`）。
 *
 * 未迁移的命令仍返回**字符串**，所以两种形状都要能认。判据必须是
 * `kind` + `message` 同时存在：普通 `Error` 只有 `message`，
 * 不会被误判进来。
 */
export function isAppError(err: unknown): err is AppError {
  if (typeof err !== 'object' || err === null) return false;
  const candidate = err as { kind?: unknown; message?: unknown };
  return typeof candidate.kind === 'string' && typeof candidate.message === 'string';
}

/**
 * 把后端 / Tauri 抛出的原始错误转成对用户友好的中文提示,
 * 避免把 `TypeError: Cannot read properties of undefined (reading 'invoke')`
 * 这类技术堆栈直接显示给用户。
 *
 * - **结构化错误（新命令）**:后端已经把「机器可读的类别」和「给人看的文案」
 *   分开返回了,直接用 `message`,**不再**按文案做正则猜测。
 * - 在浏览器里跑(非 Tauri 容器)时 `invoke` 不存在,识别为「桌面环境不可用」
 * - 其余情况尽量提取可读信息,实在没有再退回原始字符串
 *
 * 下面那几条正则只服务于**尚未迁移**、仍返回字符串的命令。它们天生不可靠:
 * `Profile id=42 不存在` 会被 `/不存在/` 命中,改写成
 * 「未找到对应数据,可能尚未初始化」——既丢了 id,又给了错误的排查方向
 * (实际是 profile 被删了,不是「尚未初始化」)。迁移完成后可以整段删掉。
 */
export function humanizeError(err: unknown, fallback = '发生未知错误'): string {
  if (isAppError(err)) return err.message || fallback;

  const raw = err instanceof Error ? err.message : String(err);

  // 没有 Tauri runtime —— 通常是在普通浏览器里打开了前端
  if (/invoke|__TAURI__|is not a function|reading 'invoke'/i.test(raw)) {
    return '无法连接到桌面后端(请在 Helio 应用内打开,而非普通浏览器)';
  }
  if (/not found|不存在|no such/i.test(raw)) {
    return '未找到对应数据,可能尚未初始化';
  }
  if (/permission|denied|EACCES/i.test(raw)) {
    return '权限不足,无法访问该资源';
  }
  // 退回:去掉冗长的 "TypeError:/Error:" 前缀,保留核心信息
  return raw.replace(/^\s*(TypeError|Error):\s*/i, '').trim() || fallback;
}

/**
 * 结构化错误携带的原始技术细节(anyhow 链 / sqlite 报错 / 路径),
 * 供界面上的「详情」折叠展示。没有则返回 `null`。
 *
 * 与 `humanizeError` 分开是有意的:`message` 是给用户的一句话,
 * `detail` 是给排查问题的人看的,两者不应混在一行里。
 */
export function errorDetail(err: unknown): string | null {
  if (!isAppError(err)) return null;
  const detail = err.detail?.trim();
  return detail ? detail : null;
}
