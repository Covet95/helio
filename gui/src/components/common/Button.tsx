import { type ButtonHTMLAttributes, forwardRef } from 'react';
import { cn } from '@/lib/utils';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'ghost' | 'success' | 'danger';
  size?: 'sm' | 'md' | 'lg';
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', ...props }, ref) => {
    return (
      <button
        ref={ref}
        className={cn(
          'no-drag inline-flex items-center justify-center gap-2 rounded-lg font-medium',
          'transition-all duration-200 outline-none',
          'focus-visible:ring-2 focus-visible:ring-accent/50',
          'disabled:opacity-40 disabled:pointer-events-none active:scale-[0.98]',
          {
            'bg-accent text-white shadow-[0_2px_12px_-2px_rgb(59_130_246/0.5)] hover:bg-accent-soft':
              variant === 'primary',
            'bg-elevated text-ink border border-line hover:border-line-strong hover:bg-line/40':
              variant === 'secondary',
            'bg-transparent text-ink-dim hover:bg-elevated hover:text-ink':
              variant === 'ghost',
            'bg-ok text-white hover:brightness-110': variant === 'success',
            'bg-danger/90 text-white hover:bg-danger': variant === 'danger',
          },
          {
            'px-3 py-1.5 text-[13px]': size === 'sm',
            'px-4 py-2 text-sm': size === 'md',
            'px-6 py-3 text-base': size === 'lg',
          },
          className,
        )}
        {...props}
      />
    );
  },
);

Button.displayName = 'Button';
