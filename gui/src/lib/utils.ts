import { clsx, type ClassValue } from 'clsx';

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
 * 把后端 / Tauri 抛出的原始错误转成对用户友好的中文提示,
 * 避免把 `TypeError: Cannot read properties of undefined (reading 'invoke')`
 * 这类技术堆栈直接显示给用户。
 *
 * - 在浏览器里跑(非 Tauri 容器)时 `invoke` 不存在,识别为「桌面环境不可用」
 * - 其余情况尽量提取可读信息,实在没有再退回原始字符串
 */
export function humanizeError(err: unknown): string {
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
  return raw.replace(/^\s*(TypeError|Error):\s*/i, '').trim() || '发生未知错误';
}
