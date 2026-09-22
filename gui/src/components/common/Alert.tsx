import type { ReactNode } from 'react';
import { cn } from '@/lib/utils';

/**
 * 语义提示条：成功 / 错误 / 警告 / 信息。
 *
 * 此前全库有 14 处手写横幅，class 串几乎逐字相同却微妙不一致——
 * `bg-danger/8` 与 `/10` 混用、字号在 `12px`/`12.5px`/`13px` 之间漂移。
 * 收敛到一处后，语义色与排版只在一个地方定义。
 *
 * `role="alert"` 只在错误时加：成功提示不需要打断屏幕阅读器。
 */
export type AlertTone = 'success' | 'error' | 'warning' | 'info';

interface AlertProps {
  tone: AlertTone;
  children: ReactNode;
  className?: string;
  /** 右侧操作区（如「重试」按钮）。 */
  action?: ReactNode;
}

/** 语义 → class 的唯一映射。透明度统一用 /10，字号统一 12px。 */
const TONE_CLASS: Record<AlertTone, string> = {
  success: 'border-ok/30 bg-ok/10 text-ok',
  error: 'border-danger/30 bg-danger/10 text-danger',
  warning: 'border-warn/30 bg-warn/10 text-warn',
  info: 'border-line bg-card/50 text-ink-dim',
};

export function Alert({ tone, children, className, action }: AlertProps) {
  return (
    <div
      role={tone === 'error' ? 'alert' : undefined}
      className={cn(
        'flex items-start gap-2 rounded-md border px-3 py-2 text-[12px] break-words',
        TONE_CLASS[tone],
        className,
      )}
    >
      <div className="min-w-0 flex-1">{children}</div>
      {action && <div className="shrink-0">{action}</div>}
    </div>
  );
}
