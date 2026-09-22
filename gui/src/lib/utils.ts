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

/**
 * 把备份文件名里的时间戳（`YYYYmmdd_HHMMSS_ffffff`）转成可读时间。
 *
 * 后端用这种格式命名，因为它字典序即时间序、便于排序；但直接展示给用户
 * 就是 `20260101_120000_000000` 这样一串数字，读不出「什么时候」。
 *
 * 解析失败时原样返回——这是展示层的便利函数，不该因为格式意外而让整页报错。
 */
export function formatBackupTime(stamp: string): string {
  const match = /^(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})/.exec(stamp);
  if (!match) return stamp;
  const [, y, mo, d, h, mi, s] = match;
  return `${y}-${mo}-${d} ${h}:${mi}:${s}`;
}

/**
 * 掩码 API key：保留前 10 / 后 5 个**字符**，中间省略。
 *
 * 按码点切而不是 `slice`：`slice` 以 UTF-16 码元为单位，key 里若有 emoji
 * 之类的星平面字符（占两个码元），会从中间切开留下孤立代理项——
 * 显示成 `�`，严重时后续处理会抛错。用 `Array.from` 按码点分。
 */
export function maskApiKey(key: string): string {
  const chars = Array.from(key);
  if (chars.length <= 15) return '***';
  return `${chars.slice(0, 10).join('')}...${chars.slice(-5).join('')}`;
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
 * - **结构化错误（全部命令）**:后端已经把「机器可读的类别」和「给人看的文案」
 *   分开返回了,直接用 `message`,**不再**按文案做正则猜测。
 * - 在浏览器里跑(非 Tauri 容器)时 `invoke` 不存在,识别为「桌面环境不可用」
 * - 其余情况尽量提取可读信息,实在没有再退回原始字符串
 *
 * 历史上这里有三条正则,用来把**尚未迁移**、仍返回字符串的命令的英文报错
 * 翻译成中文。它们天生不可靠:`Profile id=42 不存在` 会被 `/不存在/` 命中,
 * 改写成「未找到对应数据,可能尚未初始化」——既丢了 id,又给了错误的排查方向
 * (实际是 profile 被删了,不是「尚未初始化」)。现在所有 `#[tauri::command]`
 * 都返回 `AppError`,后端文案本身就是中文且准确,这三条正则已全部删除。
 * 新增命令请直接返回 `AppError`,不要试图在这里加回文案匹配。
 */
export function humanizeError(err: unknown, fallback = '发生未知错误'): string {
  if (isAppError(err)) {
    const message = err.message || fallback;
    // detail 只含 anyhow 链上「message 之外」的部分（根因 + 中间层），
    // 拼起来正好是完整信息且不重复。必须真的显示出来——否则后端的根因
    // （如 `no such table: profiles`）对用户就是不可见的，
    // 那等于把「失败原因」藏起来，比不做结构化还糟。
    const detail = errorDetail(err);
    return detail ? `${message}：${detail}` : message;
  }

  const raw =
    err instanceof Error ? err.message : typeof err === 'string' ? err : safeStringify(err);

  // 没有 Tauri runtime —— 通常是在普通浏览器里打开了前端。
  // 这**不是**后端返回的错误，走不到上面的 isAppError 分支，必须单独识别。
  if (/invoke|__TAURI__|is not a function|reading 'invoke'/i.test(raw)) {
    return '无法连接到桌面后端(请在 Helio 应用内打开,而非普通浏览器)';
  }
  // 退回:去掉冗长的 "TypeError:/Error:" 前缀,保留核心信息
  return raw.replace(/^\s*(TypeError|Error):\s*/i, '').trim() || fallback;
}

/**
 * 非 Error/非字符串输入转可读文本。JSON.stringify 优先，避免普通对象
 * 被 String() 变成毫无信息的 `[object Object]`；无法序列化时退回空串，
 * 由调用方的 fallback 兜底。
 */
function safeStringify(value: unknown): string {
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value) ?? '';
  } catch {
    return '';
  }
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
