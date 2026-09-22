/**
 * 工具卡片的徽标推导。
 *
 * 从 `StatusPage` 的 `ToolCard` 里抽出来。原先是一串 if/else 赋值给三个
 * 局部变量（文案、文字色、圆点色），六条分支，且**顺序即优先级**：
 * 托管 > 较慢 > 可达 > 不可达 > 默认。
 *
 * 这是给用户看的连通性结论，判错就是误导（例如把「不可达」显示成「已配置」）。
 * 抽成纯函数后可以逐分支钉住，也顺带让优先级显式化。
 */
import type { ToolProbeResult } from '@/types';

export interface Badge {
  text: string;
  /** 文字颜色 class。 */
  textClass: string;
  /** 圆点颜色 class。 */
  dotClass: string;
}

const OK = 'text-ok';
const WARN = 'text-warn';
const DANGER = 'text-danger';
const FAINT = 'text-ink-faint';

/**
 * 推导徽标。
 *
 * `configured` 是「档案已设置或已连接」；有探测结果时以探测为准，
 * 因为探测是刚测出来的事实，档案只是本地记录。
 */
export function toolBadge(configured: boolean, probe?: ToolProbeResult): Badge {
  const fallback: Badge = configured
    ? { text: '已配置', textClass: OK, dotClass: 'bg-ok' }
    : { text: '未设置', textClass: FAINT, dotClass: 'bg-ink-faint/40' };

  if (!probe) return fallback;

  const ms = probe.latency_ms != null ? ` ${probe.latency_ms}ms` : '';
  if (probe.managed) {
    // 工具自管（例如 IDE 插件写自己的配置），Helio 不参与，一律算正常。
    return { text: '工具托管', textClass: OK, dotClass: 'bg-ok' };
  }
  if (probe.ok && probe.status === 'degraded') {
    return { text: `较慢${ms}`, textClass: WARN, dotClass: 'bg-warn' };
  }
  if (probe.ok) {
    return { text: `可达${ms}`, textClass: OK, dotClass: 'bg-ok' };
  }
  if (probe.configured) {
    return { text: '不可达', textClass: DANGER, dotClass: 'bg-danger' };
  }
  // 未配置且探测失败：保持默认徽标，不谎报「不可达」。
  return fallback;
}
