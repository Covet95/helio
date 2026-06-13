import { type ReactNode, type InputHTMLAttributes } from 'react';
import { X } from 'lucide-react';

export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm animate-fade-up">
      <div className="w-full max-w-md rounded-2xl border border-line bg-card shadow-card">
        <div className="flex items-center justify-between px-6 py-4 border-b border-line/70">
          <h3 className="text-[15px] font-semibold text-ink">{title}</h3>
          <button
            onClick={onClose}
            className="grid place-items-center h-7 w-7 rounded-md text-ink-faint hover:text-ink hover:bg-elevated transition-colors"
          >
            <X size={16} />
          </button>
        </div>
        <div className="p-6">{children}</div>
      </div>
    </div>
  );
}

interface FieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  mono?: boolean;
}

export function Field({ label, mono, className, ...props }: FieldProps) {
  return (
    <label className="block">
      <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">{label}</span>
      <input
        className={`w-full rounded-lg border border-line bg-surface px-3 py-2 text-[13.5px] text-ink placeholder:text-ink-faint outline-none transition-all focus:border-accent/60 focus:ring-2 focus:ring-accent/20 disabled:opacity-50 ${
          mono ? 'font-mono' : ''
        } ${className || ''}`}
        {...props}
      />
    </label>
  );
}
