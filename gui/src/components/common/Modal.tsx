import { type ReactNode, type InputHTMLAttributes, useState } from 'react';
import { X, AlertTriangle, Eye, EyeOff } from 'lucide-react';

export function Modal({
  title,
  onClose,
  children,
  footer,
  size = 'md',
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: 'md' | 'lg' | 'xl';
}) {
  const maxW = size === 'xl' ? 'max-w-2xl' : size === 'lg' ? 'max-w-lg' : 'max-w-md';
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/35 animate-fade-up p-4">
      <div className={`flex w-full ${maxW} max-h-[90vh] flex-col rounded-lg border border-line bg-card shadow-card`}>
        <div className="flex shrink-0 items-center justify-between border-b border-line/70 px-5 py-3.5">
          <h3 className="text-[15px] font-semibold text-ink">{title}</h3>
          <button
            onClick={onClose}
            className="grid h-7 w-7 place-items-center rounded-md text-ink-faint hover:text-ink hover:bg-elevated transition-colors"
          >
            <X size={16} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-5">{children}</div>
        {footer && (
          <div className="flex shrink-0 justify-end gap-2.5 border-t border-line/70 bg-card px-5 py-3">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}

export function ConfirmDialog({
  title,
  message,
  confirmText = '确定',
  cancelText = '取消',
  danger,
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="fixed inset-0 z-[60] grid place-items-center bg-black/35 animate-fade-up">
      <div className="w-full max-w-sm rounded-lg border border-line bg-card p-5 shadow-card">
        <div className="flex items-start gap-3.5">
          <div className={`grid h-9 w-9 shrink-0 place-items-center rounded-md ${danger ? 'bg-danger/10 text-danger' : 'bg-ink/5 text-ink-dim'}`}>
            <AlertTriangle size={20} />
          </div>
          <div className="flex-1">
            <h3 className="text-[15px] font-semibold text-ink">{title}</h3>
            <p className="mt-1 text-[13px] text-ink-dim leading-relaxed">{message}</p>
          </div>
        </div>
        <div className="mt-5 flex justify-end gap-2.5">
          <button
            onClick={onCancel}
            className="no-drag rounded-md px-4 py-2 text-sm font-medium text-ink-dim hover:bg-elevated hover:text-ink transition-colors"
          >
            {cancelText}
          </button>
          <button
            onClick={onConfirm}
            className={`no-drag rounded-md border px-4 py-2 text-sm font-medium transition-colors active:scale-[0.98] ${
              danger
                ? 'border-danger/30 bg-card text-danger hover:bg-danger/8'
                : 'border-ink bg-ink text-white hover:bg-[#2F2F2C]'
            }`}
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}

interface FieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  mono?: boolean;
}

export function Field({ label, mono, className, type, ...props }: FieldProps) {
  const [revealed, setRevealed] = useState(false);
  const isPassword = type === 'password';
  const inputType = isPassword && revealed ? 'text' : type;

  return (
    <label className="block">
      <span className="block mb-1.5 text-[12px] font-medium text-ink-dim">{label}</span>
      <div className="relative">
        <input
          type={inputType}
          className={`w-full rounded-md border border-line bg-card px-3 py-2 text-[13.5px] text-ink outline-none transition-all placeholder:text-ink-faint focus:border-accent/60 focus:ring-2 focus:ring-accent/15 disabled:opacity-50 ${
            mono ? 'font-mono' : ''
          } ${isPassword ? 'pr-10' : ''} ${className || ''}`}
          {...props}
        />
        {isPassword && (
          <button
            type="button"
            tabIndex={-1}
            onClick={() => setRevealed((v) => !v)}
            aria-label={revealed ? '隐藏' : '显示'}
            className="absolute right-2 top-1/2 -translate-y-1/2 grid h-7 w-7 place-items-center rounded text-ink-faint hover:text-ink hover:bg-elevated"
          >
            {revealed ? <EyeOff size={15} /> : <Eye size={15} />}
          </button>
        )}
      </div>
    </label>
  );
}
