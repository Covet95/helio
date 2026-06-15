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
          'no-drag inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md font-medium',
          'border transition-[background-color,box-shadow,border-color,color] duration-150 outline-none',
          'focus-visible:ring-2 focus-visible:ring-accent/40',
          'disabled:opacity-40 disabled:pointer-events-none active:translate-y-px',
          {
            'border-ink bg-ink text-white shadow-[0_1px_2px_rgba(10,16,14,0.18),inset_0_1px_0_rgba(255,255,255,0.10)] hover:bg-[#2B3430]':
              variant === 'primary',
            'border-line bg-card text-ink-dim shadow-[inset_0_1px_0_rgba(255,255,255,0.85)] hover:border-line-strong hover:bg-elevated/70 hover:text-ink':
              variant === 'secondary',
            'border-transparent bg-transparent text-ink-dim hover:bg-elevated/60 hover:text-ink':
              variant === 'ghost',
            'border-ok/35 bg-card text-ok hover:bg-ok/8':
              variant === 'success',
            'border-danger/35 bg-card text-danger hover:bg-danger/8':
              variant === 'danger',
          },
          {
            'px-3 py-1.5 text-[13px]': size === 'sm',
            'px-3.5 py-2 text-[13px]': size === 'md',
            'px-5 py-2.5 text-sm': size === 'lg',
          },
          className,
        )}
        {...props}
      />
    );
  },
);

Button.displayName = 'Button';
