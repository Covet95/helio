/**
 * `runTransfer` 的真实依赖：Tauri 文件对话框 + 错误格式化。
 *
 * 单独成文件，好让 `exportFlow` 保持纯净（可测），而这里集中处理
 * 插件动态导入——原先这段 `await import('@tauri-apps/plugin-dialog')`
 * 在 `ExportPage` 里重复了 6 遍。
 *
 * 保持**动态**导入：对话框插件只在用户真的点按钮时才加载。
 */
import { humanizeError } from '@/lib/utils';
import type { FileRequest, TransferDeps } from './exportFlow';

/** 构造一次流程所需的依赖。 */
export function tauriTransferDeps(): TransferDeps {
  return {
    save: async (request: FileRequest) => {
      const { save } = await import('@tauri-apps/plugin-dialog');
      return save({
        defaultPath: request.defaultPath,
        filters: [{ name: request.filterName, extensions: request.extensions }],
      });
    },
    open: async (request: FileRequest) => {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = await open({
        multiple: false,
        filters: [{ name: request.filterName, extensions: request.extensions }],
      });
      // open 的返回类型含数组分支，但 multiple:false 时只可能是单值。
      return typeof picked === 'string' ? picked : null;
    },
    humanize: humanizeError,
  };
}
