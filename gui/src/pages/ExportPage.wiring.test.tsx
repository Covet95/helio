// @vitest-environment jsdom
/**
 * `ExportPage` 的接线测试。
 *
 * 这个页面此前整页零覆盖，而它是**唯一会把明文 key 写进文件**的入口
 * （便携备份 / 数据库导出），接错命令或漏判取消的代价很高。
 *
 * 分支逻辑已在 `exportFlow.test.ts` / `exportMessages.test.ts` 里逐条覆盖；
 * 这里只钉「按钮 → 命令 → 反馈」这条线是否接对。
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';

const dialog = vi.hoisted(() => ({ save: vi.fn(), open: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => dialog);

const api = vi.hoisted(() => ({
  exportDatabase: vi.fn(),
  importDatabase: vi.fn(),
  exportPortableBackup: vi.fn(),
  importPortableBackup: vi.fn(),
  exportSkills: vi.fn(),
  importSkills: vi.fn(),
}));
vi.mock('../lib/tauri', () => ({ tauriApi: api }));

const store = vi.hoisted(() => ({ fetchProfiles: vi.fn(), fetchStatus: vi.fn() }));
vi.mock('../store', () => ({
  useStore: { getState: () => store },
}));

beforeAll(() => {
  // ConfirmDialog 依赖 <dialog>，jsdom 不实现 showModal。
  if (!HTMLDialogElement.prototype.showModal) {
    HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    };
    HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
      this.removeAttribute('open');
    };
  }
});

beforeEach(() => {
  vi.resetAllMocks();
  dialog.save.mockResolvedValue('/tmp/out.db');
  dialog.open.mockResolvedValue('/tmp/in.db');
  store.fetchProfiles.mockResolvedValue(undefined);
  store.fetchStatus.mockResolvedValue(undefined);
});

// 仓库未启用 setupFiles/globals，testing-library 的自动清理不会生效——
// 不显式 cleanup 的话上一条用例的 DOM 会残留，选择器全部命中多个元素。
afterEach(cleanup);

/** 渲染页面。 */
async function renderPage() {
  const { default: ExportPage } = await import('./ExportPage');
  render(React.createElement(ExportPage));
  return screen;
}

/** 找到某标题所在 ActionRow 里的按钮（标题唯一，按钮文案会重复）。 */
function rowButton(title: string, label: string): HTMLButtonElement {
  // 从标题向上走到第一个含按钮的祖先，就是这一行。
  let node: HTMLElement | null = screen.getByRole('heading', { name: title });
  while (node && !node.querySelector('button')) node = node.parentElement;
  return Array.from(node!.querySelectorAll('button')).find((b) =>
    b.textContent?.includes(label),
  ) as HTMLButtonElement;
}

/** 确认对话框里的按钮——与页面上的同名按钮区分开。 */
async function confirmDialogButton(label: string): Promise<HTMLButtonElement> {
  const dialogEl = await screen.findByRole('alertdialog');
  return Array.from(dialogEl.querySelectorAll('button')).find((b) =>
    b.textContent?.includes(label),
  ) as HTMLButtonElement;
}

describe('ExportPage — 导出接线', () => {
  it('便携备份导出：save 的路径交给 exportPortableBackup，并报告 Skills 数', async () => {
    api.exportPortableBackup.mockResolvedValue({
      path: '/tmp/out.db',
      skills: { apps: [], total: 4, path: '/tmp/s' },
    });
    await renderPage();
    fireEvent.click(rowButton('导出便携备份', '导出'));

    await waitFor(() => expect(api.exportPortableBackup).toHaveBeenCalledWith('/tmp/out.db'));
    expect(await screen.findByText(/Skills 4 个/)).toBeTruthy();
  });

  it('便携备份导出使用 tar.gz 过滤器', async () => {
    api.exportPortableBackup.mockResolvedValue({
      path: '/tmp/out.db',
      skills: { apps: [], total: 0, path: '' },
    });
    await renderPage();
    fireEvent.click(rowButton('导出便携备份', '导出'));

    await waitFor(() => expect(dialog.save).toHaveBeenCalled());
    const request = dialog.save.mock.calls[0][0];
    expect(request.filters[0].extensions).toEqual(['tar.gz', 'tgz']);
    expect(request.defaultPath).toMatch(/^helio-portable-\d+\.tar\.gz$/);
  });

  it('取消导出时不调用任何命令', async () => {
    dialog.save.mockResolvedValue(null);
    await renderPage();
    fireEvent.click(rowButton('导出便携备份', '导出'));

    expect(await screen.findByText('导出已取消')).toBeTruthy();
    expect(api.exportPortableBackup).not.toHaveBeenCalled();
  });

  it('导出失败时显示错误反馈，不显示成功', async () => {
    api.exportPortableBackup.mockRejectedValue(new Error('disk full'));
    await renderPage();
    fireEvent.click(rowButton('导出便携备份', '导出'));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('便携备份导出失败');
    expect(alert.textContent).toContain('disk full');
  });

  it('Skills 导出为 0 时用 info 语气而非 success', async () => {
    api.exportSkills.mockResolvedValue({ apps: [], total: 0, path: '/tmp/s' });
    await renderPage();
    fireEvent.click(rowButton('导出 Skills', '导出'));

    expect(await screen.findByText('未发现任何 Skills')).toBeTruthy();
  });
});

describe('ExportPage — 导入接线', () => {
  it('便携备份恢复需确认，确认后调 importPortableBackup 并刷新应用数据', async () => {
    api.importPortableBackup.mockResolvedValue({
      restored_targets: ['codex', 'pi'],
      skills: { restored: 2, skipped: 0, skipped_names: [] },
    });
    await renderPage();
    fireEvent.click(rowButton('恢复便携备份', '恢复'));

    // 确认对话框先出现，此时命令尚未调用。
    const confirm = await confirmDialogButton('恢复');
    expect(api.importPortableBackup).not.toHaveBeenCalled();
    fireEvent.click(confirm);

    await waitFor(() => expect(api.importPortableBackup).toHaveBeenCalledWith('/tmp/in.db'));
    expect(await screen.findByText(/已恢复 2 个工具配置/)).toBeTruthy();
    await waitFor(() => expect(store.fetchProfiles).toHaveBeenCalled());
  });

  it('取消确认对话框不触发命令', async () => {
    await renderPage();
    fireEvent.click(rowButton('恢复便携备份', '恢复'));
    fireEvent.click(await confirmDialogButton('取消'));

    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(api.importPortableBackup).not.toHaveBeenCalled();
  });

  it('数据库导入成功后刷新应用数据', async () => {
    api.importDatabase.mockResolvedValue(undefined);
    await renderPage();
    fireEvent.click(rowButton('导入数据库', '导入'));
    fireEvent.click(await confirmDialogButton('导入'));

    await waitFor(() => expect(api.importDatabase).toHaveBeenCalledWith('/tmp/in.db'));
    expect(await screen.findByText(/正在刷新/)).toBeTruthy();
    await waitFor(() => expect(store.fetchStatus).toHaveBeenCalled());
  });

  it('Skills 导入报告跳过的同名项', async () => {
    api.importSkills.mockResolvedValue({
      restored: 1,
      skipped: 2,
      skipped_names: ['alpha', 'beta'],
    });
    await renderPage();
    fireEvent.click(rowButton('导入 Skills', '导入'));
    fireEvent.click(await confirmDialogButton('导入'));

    const text = await screen.findByText(/跳过同名 2 个/);
    expect(text.textContent).toContain('alpha、beta');
  });

  it('导入取消（未选文件）时不调命令，提示「导入已取消」', async () => {
    dialog.open.mockResolvedValue(null);
    await renderPage();
    fireEvent.click(rowButton('导入数据库', '导入'));
    fireEvent.click(await confirmDialogButton('导入'));

    expect(await screen.findByText('导入已取消')).toBeTruthy();
    expect(api.importDatabase).not.toHaveBeenCalled();
  });
});

describe('ExportPage — 并发防护', () => {
  it('一个传输进行中时，其余按钮全部禁用', async () => {
    // 便携备份挂起：模拟正在写一个大归档。
    api.exportPortableBackup.mockImplementation(() => new Promise(() => {}));
    await renderPage();
    fireEvent.click(rowButton('导出便携备份', '导出'));
    await waitFor(() => expect(api.exportPortableBackup).toHaveBeenCalled());

    for (const [title, label] of [
      ['导出便携备份', '导出'],
      ['恢复便携备份', '恢复'],
      ['导出数据库', '导出'],
      ['导入数据库', '导入'],
      ['导出 Skills', '导出'],
      ['导入 Skills', '导入'],
    ] as const) {
      expect(rowButton(title, label).disabled, title).toBe(true);
    }
  });

  it('传输结束后按钮恢复可用', async () => {
    api.exportPortableBackup.mockResolvedValue({
      path: '/tmp/out.db',
      skills: { apps: [], total: 0, path: '' },
    });
    await renderPage();
    fireEvent.click(rowButton('导出便携备份', '导出'));
    await screen.findByText(/Skills 0 个/);

    expect(rowButton('导出数据库', '导出').disabled).toBe(false);
  });
});
