import { type ReactNode } from 'react';

export function PageHeader({
  title,
  subtitle,
  actions,
}: {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
}) {
  return (
    <div className="drag-region flex min-h-[64px] items-center justify-between gap-3 border-b border-line/70 px-4 py-3 sm:min-h-[72px] sm:px-7 sm:py-4">
      <div className="min-w-0">
        <h1 className="truncate text-[18px] font-semibold text-ink sm:text-[19px]">{title}</h1>
        {subtitle && <p className="mt-1 text-[13px] text-ink-dim">{subtitle}</p>}
      </div>
      <div className="no-drag flex items-center gap-2.5">{actions}</div>
    </div>
  );
}
