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
    <header className="drag-region flex min-h-[64px] flex-wrap items-center justify-between gap-3 border-b border-line bg-card px-4 py-3 sm:min-h-[72px] sm:px-7 sm:py-4">
      <div className="min-w-0">
        <h1 className="break-words text-[18px] font-semibold text-ink">{title}</h1>
        {subtitle && <p className="mt-1 text-[13px] text-ink-dim">{subtitle}</p>}
      </div>
      {actions && <div className="page-actions no-drag flex max-w-full flex-wrap items-center gap-2.5">{actions}</div>}
    </header>
  );
}
