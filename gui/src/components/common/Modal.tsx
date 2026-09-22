import { type ReactNode, type InputHTMLAttributes, useEffect, useId, useRef, useState } from 'react';
import { X, AlertTriangle, Eye, EyeOff } from 'lucide-react';
import { humanizeError } from '../../lib/utils';

export function Modal({
  title,
  onClose,
  children,
  footer,
  size = 'md',
  busy = false,
  role = 'dialog',
  descriptionId,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: 'md' | 'lg' | 'xl';
  busy?: boolean;
  role?: 'dialog' | 'alertdialog';
  descriptionId?: string;
}) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  // Esc 关闭。注意：这里刻意不用原生 <dialog>——旧版 WebKit 里顶层 dialog
  // 按 fit-content 收缩，纵向 flex 的内容区会被压到只剩一行；普通 overlay
  // div + 定高面板在所有引擎表现一致。
  //
  // **只有最上层的对话框响应 Esc**：监听挂在 document 上，嵌套时（例如表单
  // 上再叠一个「放弃修改？」确认框）两个监听器都会收到事件——外层先注册、
  // 先执行，会把确认框关掉的同时触发它自己的 onClose。用「DOM 里有没有更晚
  // 出现的同级面板」判断谁在最上层，只让最上层处理。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || busy) return;
      const panels = document.querySelectorAll('[data-modal-panel]');
      if (panels.length > 0 && panels[panels.length - 1] !== panelRef.current) return;
      onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose, busy]);
  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    return () => {
      if (trigger?.isConnected) trigger.focus();
    };
  }, []);
  const maxW = size === 'xl' ? 'max-w-2xl' : size === 'lg' ? 'max-w-lg' : 'max-w-md';
  return (
    <div className="fixed inset-0 z-50 grid place-items-center overflow-y-auto bg-black/40 p-4">
      <div
        ref={panelRef}
        data-modal-panel=""
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        aria-busy={busy}
        className={`app-dialog flex max-h-full w-full ${maxW} flex-col overflow-hidden rounded-lg border border-line bg-card p-0 text-ink shadow-card`}
      >
        <div className="flex shrink-0 items-center justify-between border-b border-line/70 px-5 py-3.5">
          <h3 id={titleId} className="min-w-0 break-words text-[15px] font-semibold text-ink">{title}</h3>
          <button
            type="button"
            title="关闭"
            aria-label="关闭"
            disabled={busy}
            onClick={onClose}
            className="icon-button shrink-0"
          >
            <X size={16} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-5">{children}</div>
        {footer && (
          <div className="flex shrink-0 flex-wrap justify-end gap-2.5 border-t border-line/70 bg-card px-5 py-3">
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
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const pendingRef = useRef(false);
  const messageId = useId();
  const confirm = async () => {
    if (pendingRef.current) return;
    pendingRef.current = true;
    setPending(true);
    setError('');
    try {
      await onConfirm();
    } catch (cause) {
      setError(humanizeError(cause));
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  };
  return (
    <Modal title={title} onClose={onCancel} busy={pending} role="alertdialog" descriptionId={messageId}
      footer={
        <>
          <button
            type="button"
            autoFocus
            disabled={pending}
            onClick={onCancel}
            className="no-drag rounded-md px-4 py-2 text-sm font-medium text-ink-dim hover:bg-elevated hover:text-ink transition-colors disabled:opacity-40"
          >
            {cancelText}
          </button>
          <button
            type="button"
            disabled={pending}
            onClick={confirm}
            className={`no-drag min-w-20 rounded-md border px-4 py-2 text-sm font-medium transition-colors disabled:opacity-40 ${
              danger
                ? 'border-danger/30 bg-card text-danger hover:bg-danger/8'
                : 'border-ink bg-ink text-white hover:bg-[#2F2F2C]'
            }`}
          >
            {pending ? '处理中…' : confirmText}
          </button>
        </>
      }
    >
      <div className="flex items-start gap-3">
        <AlertTriangle size={20} className={`shrink-0 ${danger ? 'text-danger' : 'text-warn'}`} />
        <p id={messageId} className="min-w-0 whitespace-pre-wrap break-words text-[13px] leading-relaxed text-ink-dim">{message}</p>
      </div>
      {error && <p role="alert" className="mt-3 break-words text-[13px] text-danger">{error}</p>}
    </Modal>
  );
}

interface FieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  mono?: boolean;
}

export function Field({ label, mono, className, type, id, ...props }: FieldProps) {
  const generatedId = useId();
  const inputId = id || generatedId;
  const [revealed, setRevealed] = useState(false);
  const isPassword = type === 'password';
  const inputType = isPassword && revealed ? 'text' : type;

  return (
    <div className="block">
      <label htmlFor={inputId} className="block mb-1.5 text-[12px] font-medium text-ink-dim">{label}</label>
      <div className="relative">
        <input
          id={inputId}
          type={inputType}
          className={`w-full rounded-md border border-line bg-card px-3 py-2 text-[13.5px] text-ink outline-none transition-all placeholder:text-ink-faint focus:border-accent/60 focus:ring-2 focus:ring-accent/15 disabled:opacity-50 ${
            mono ? 'font-mono' : ''
          } ${isPassword ? 'pr-10' : ''} ${className || ''}`}
          {...props}
        />
        {isPassword && (
          <button
            type="button"
            onClick={() => setRevealed((v) => !v)}
            aria-label={revealed ? '隐藏' : '显示'}
            aria-pressed={revealed}
            title={revealed ? '隐藏' : '显示'}
            className="absolute right-2 top-1/2 -translate-y-1/2 grid h-7 w-7 place-items-center rounded text-ink-faint hover:text-ink hover:bg-elevated"
          >
            {revealed ? <EyeOff size={15} /> : <Eye size={15} />}
          </button>
        )}
      </div>
    </div>
  );
}
