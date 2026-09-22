// @vitest-environment jsdom
/**
 * 共享配置页的两处保护。
 *
 * 1. **原始 TOML 编辑器的未保存改动**：点「取消」原先直接 `setEditing(false)`，
 *    用户手写的整段 config.toml 改动无声消失。
 * 2. **版本回滚的确认**：原先用 `window.confirm`——样式与全站不一致、
 *    在 Tauri 里阻塞 webview，而且确认框里没把「要覆盖哪个文件」讲清楚。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';

const api = vi.hoisted(() => ({
  getLocalConfigInfo: vi.fn(),
  listConfigBackups: vi.fn(),
  updateCodexFields: vi.fn(),
  readCodexConfigRaw: vi.fn(),
  saveCodexConfigRaw: vi.fn(),
  restoreConfigBackup: vi.fn(),
}));
vi.mock('../lib/tauri', () => ({ tauriApi: api }));

const RAW = '# 我手写的注释\nmodel = "gpt-5"\n';

beforeEach(() => {
  vi.resetAllMocks();
  api.getLocalConfigInfo.mockResolvedValue({
    mcp_servers: {}, skills: [], hooks: {}, permissions: {}, other: {},
  });
  api.listConfigBackups.mockResolvedValue([
    { path: '/Users/x/.codex/backups/config.toml.20260101_120000.bak', time: '2026-01-01 12:00', target: 'codex' },
  ]);
  api.readCodexConfigRaw.mockResolvedValue(RAW);
  api.saveCodexConfigRaw.mockResolvedValue(undefined);
  api.restoreConfigBackup.mockResolvedValue('/tmp/backup');
});

afterEach(cleanup);

/** 渲染共享配置页（codex）。 */
async function renderConfigPage() {
  const { useStore } = await import('../store');
  useStore.setState({ selectedTool: 'codex' as never });
  const { default: ConfigPage } = await import('./ConfigPage');
  render(React.createElement(ConfigPage));
  await screen.findByText('编辑 config.toml');
}

/** 进入原始编辑器。 */
async function enterRawEditor() {
  fireEvent.click(screen.getByRole('button', { name: /编辑/ }));
  return (await screen.findByRole('textbox')) as HTMLTextAreaElement;
}

describe('原始 TOML 编辑器 — 未保存改动守卫', () => {
  it('没有改动时点取消直接退出', async () => {
    await renderConfigPage();
    await enterRawEditor();
    fireEvent.click(screen.getByRole('button', { name: /取消/ }));

    expect(screen.queryByRole('alertdialog')).toBeNull();
    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
  });

  it('有改动时点取消先问一句，不直接丢弃', async () => {
    await renderConfigPage();
    const textarea = await enterRawEditor();
    fireEvent.change(textarea, { target: { value: RAW + 'model_reasoning_effort = "high"\n' } });
    fireEvent.click(screen.getByRole('button', { name: /取消/ }));

    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('尚未保存');
    // 编辑器还在，改动也还在
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toContain('model_reasoning_effort');
  });

  it('确认放弃后退出编辑', async () => {
    await renderConfigPage();
    const textarea = await enterRawEditor();
    fireEvent.change(textarea, { target: { value: 'model = "changed"' } });
    fireEvent.click(screen.getByRole('button', { name: /取消/ }));

    const dialog = await screen.findByRole('alertdialog');
    const discard = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('放弃修改'),
    )!;
    fireEvent.click(discard);

    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
  });

  it('选择继续编辑则留在编辑器', async () => {
    await renderConfigPage();
    const textarea = await enterRawEditor();
    fireEvent.change(textarea, { target: { value: 'model = "changed"' } });
    fireEvent.click(screen.getByRole('button', { name: /取消/ }));

    const dialog = await screen.findByRole('alertdialog');
    const keep = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('继续编辑'),
    )!;
    fireEvent.click(keep);

    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('model = "changed"');
  });

  it('保存成功后不再提示未保存', async () => {
    await renderConfigPage();
    const textarea = await enterRawEditor();
    fireEvent.change(textarea, { target: { value: 'model = "saved"' } });
    // 页面上别处也有「保存」（Codex 行为设置），按 DOM 邻近取编辑器那一个。
    const editorSave = Array.from(
      textarea.parentElement!.parentElement!.querySelectorAll('button'),
    ).find((b) => b.textContent?.trim() === '保存')!;
    fireEvent.click(editorSave);
    await waitFor(() => expect(api.saveCodexConfigRaw).toHaveBeenCalledWith('model = "saved"'));

    // 已保存 → 编辑器关闭，无需再问
    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
    expect(screen.queryByRole('alertdialog')).toBeNull();
  });
});

describe('版本回滚 — 用应用内确认框', () => {
  it('点恢复弹出应用自己的确认框，并写明要覆盖的文件', async () => {
    await renderConfigPage();
    fireEvent.click(await screen.findByRole('button', { name: /恢复/ }));

    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('覆盖当前配置');
    expect(dialog.textContent).toContain('config.toml.20260101_120000.bak');
    expect(api.restoreConfigBackup).not.toHaveBeenCalled();
  });

  it('确认后才真正恢复', async () => {
    await renderConfigPage();
    fireEvent.click(await screen.findByRole('button', { name: /恢复/ }));
    const dialog = await screen.findByRole('alertdialog');
    const confirm = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('恢复'),
    )!;
    fireEvent.click(confirm);

    await waitFor(() =>
      expect(api.restoreConfigBackup).toHaveBeenCalledWith(
        'codex',
        '/Users/x/.codex/backups/config.toml.20260101_120000.bak',
      ),
    );
  });

  it('取消则不调用恢复', async () => {
    await renderConfigPage();
    fireEvent.click(await screen.findByRole('button', { name: /恢复/ }));
    const dialog = await screen.findByRole('alertdialog');
    const cancel = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('取消'),
    )!;
    fireEvent.click(cancel);

    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(api.restoreConfigBackup).not.toHaveBeenCalled();
  });
});
