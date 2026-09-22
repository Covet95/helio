/**
 * 导出/导入的「选路径 → 执行 → 汇报」流程。
 *
 * `ExportPage` 里 6 个 handler 各写了一遍同样的骨架（每份约 20 行）：
 * 弹对话框、判空取消、调命令、拼反馈、catch 兜底。差异只在
 * 「用 save 还是 open」「过滤器是什么」「执行哪条命令」「成功说什么」。
 *
 * 抽成一个函数并把对话框与错误格式化作为依赖注入，就可以不起 jsdom、
 * 不碰 Tauri 地测全部分支——包括最容易写错的取消路径与异常路径。
 */
import { cancelledMessage, failureMessage, type Feedback } from './exportMessages';

/** 保存/打开对话框的参数（与 `@tauri-apps/plugin-dialog` 的入参同形）。 */
export interface FileRequest {
  /** 默认文件名，含扩展名。仅导出（`save`）用得上，导入不必给。 */
  defaultPath?: string;
  /** 文件类型过滤器的显示名。 */
  filterName: string;
  /** 允许的扩展名。 */
  extensions: string[];
}

/** 流程依赖：由调用方注入真实实现，测试里换成桩。 */
export interface TransferDeps {
  save(request: FileRequest): Promise<string | null>;
  open(request: FileRequest): Promise<string | null>;
  humanize(error: unknown): string;
}

/** 成功时的反馈由调用方给出——文案各命令不同。 */
export type TransferOutcome = Feedback;

export interface TransferSpec {
  /** 导出用 `save`（选新路径），导入用 `open`（选已有文件）。 */
  mode: 'export' | 'import';
  request: FileRequest;
  /** 失败文案前缀，例如「便携备份恢复」。 */
  action: string;
  /** 拿到路径后执行的实际命令。 */
  execute: (path: string) => Promise<TransferOutcome>;
}

/**
 * 走完一次文件传输。
 *
 * 三条出口：取消（info）、失败（error）、成功（由 `execute` 决定）。
 * 对话框本身抛错（例如插件加载失败）也算失败——与原实现一致，
 * 原先把 `import(...)` 放在 try 内。
 */
export async function runTransfer(
  deps: TransferDeps,
  spec: TransferSpec,
): Promise<TransferOutcome> {
  let path: string | null;
  try {
    path =
      spec.mode === 'export'
        ? await deps.save(spec.request)
        : await deps.open(spec.request);
  } catch (error) {
    return { text: failureMessage(spec.action, error, deps.humanize), kind: 'error' };
  }
  if (!path) {
    return { text: cancelledMessage(spec.mode === 'export' ? '导出' : '导入'), kind: 'info' };
  }
  try {
    return await spec.execute(path);
  } catch (error) {
    return { text: failureMessage(spec.action, error, deps.humanize), kind: 'error' };
  }
}
