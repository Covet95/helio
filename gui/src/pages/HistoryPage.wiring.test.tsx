// @vitest-environment jsdom
/**
 * `HistoryPage` 的接线测试。
 *
 * 这个页面此前零覆盖，而它做的是**删除**：单删、批量删、按天清理。
 * 走的是系统垃圾桶，但仍需要用户确认两次（勾选 + 对话框），
 * 且「失败了几个」必须如实报出来。
 *
 * 文案/校验分支已在 `historyMessages.test.ts` 覆盖；这里钉接线。
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';

const api = vi.hoisted(() => ({
  listSessions: vi.fn(),
  readSessionPreview: vi.fn(),
  deleteSession: vi.fn(),
  deleteSessions: vi.fn(),
  cleanupSessions: vi.fn(),
}));
vi.mock('../lib/tauri', () => ({ tauriApi: api }));

beforeAll(() => {
  if (!HTMLDialogElement.prototype.showModal) {
    HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    };
    HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
      this.removeAttribute('open');
    };
  }
});

/** 造一条会话元数据。 */
function session(id: string, overrides = {}) {
  return {
    id,
    tool: 'codex',
    title: `会话 ${id}`,
    cwd: `/tmp/${id}`,
    modified_at: 1700000000,
    size_bytes: 1024,
    message_count: 3,
    parseable: true,
    ...overrides,
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  api.listSessions.mockResolvedValue([session('a'), session('b')]);
  api.readSessionPreview.mockResolvedValue([{ role: 'user', text: '你好' }]);
  api.deleteSession.mockResolvedValue({ ok: true });
  api.deleteSessions.mockResolvedValue([{ ok: true }, { ok: true }]);
  api.cleanupSessions.mockResolvedValue([]);
});

afterEach(cleanup);

async function renderPage() {
  const { default: HistoryPage } = await import('./HistoryPage');
  render(React.createElement(HistoryPage));
  await screen.findByText('会话 a');
}

/** 确认对话框里的按钮。 */
async function confirmButton(label: string): Promise<HTMLButtonElement> {
  const dialogEl = await screen.findByRole('alertdialog');
  return Array.from(dialogEl.querySelectorAll('button')).find((b) =>
    b.textContent?.includes(label),
  ) as HTMLButtonElement;
}

describe('HistoryPage — 列表', () => {
  it('加载并展示会话', async () => {
    await renderPage();
    expect(screen.getByText('会话 a')).toBeTruthy();
    expect(screen.getByText('会话 b')).toBeTruthy();
  });

  it('加载失败时显示错误条并清空列表', async () => {
    api.listSessions.mockRejectedValue(new Error('db locked'));
    const { default: HistoryPage } = await import('./HistoryPage');
    render(React.createElement(HistoryPage));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('db locked');
    expect(screen.getByText('没有会话')).toBeTruthy();
  });

  it('切换工具筛选后重新加载', async () => {
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Claude Code' }));

    await waitFor(() =>
      expect(api.listSessions).toHaveBeenCalledWith('claude-code', undefined),
    );
  });

  it('搜索带 250ms 防抖', async () => {
    await renderPage();
    fireEvent.change(screen.getByPlaceholderText('搜索 cwd 或标题…'), {
      target: { value: 'foo' },
    });

    // 防抖窗口内不应立刻再查一次
    expect(api.listSessions).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(api.listSessions).toHaveBeenCalledWith(undefined, 'foo'), {
      timeout: 1000,
    });
  });
});

describe('HistoryPage — 预览', () => {
  it('点预览拉取消息并展示角色', async () => {
    await renderPage();
    fireEvent.click(screen.getAllByTitle('预览')[0]);

    expect(await screen.findByText('user')).toBeTruthy();
    expect(screen.getByText('你好')).toBeTruthy();
  });

  it('预览加载失败时显示错误条', async () => {
    api.readSessionPreview.mockRejectedValue(new Error('损坏的 JSONL'));
    await renderPage();
    fireEvent.click(screen.getAllByTitle('预览')[0]);

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('损坏的 JSONL');
  });
});

describe('HistoryPage — 删除', () => {
  it('单删需确认，确认后调 deleteSession 并刷新', async () => {
    await renderPage();
    fireEvent.click(screen.getAllByTitle('删除')[0]);
    expect(api.deleteSession).not.toHaveBeenCalled();

    fireEvent.click(await confirmButton('确认'));
    await waitFor(() => expect(api.deleteSession).toHaveBeenCalledWith('codex', 'a'));
    await waitFor(() => expect(api.listSessions).toHaveBeenCalledTimes(2));
  });

  it('单删失败时显示后端原因', async () => {
    api.deleteSession.mockResolvedValue({ ok: false, error: '文件被占用' });
    await renderPage();
    fireEvent.click(screen.getAllByTitle('删除')[0]);
    fireEvent.click(await confirmButton('确认'));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('文件被占用');
  });

  it('勾选后才出现批量删除按钮，且带选中数', async () => {
    await renderPage();
    expect(screen.queryByRole('button', { name: /删除选中/ })).toBeNull();

    const boxes = screen.getAllByRole('checkbox');
    fireEvent.click(boxes[0]);
    expect(await screen.findByRole('button', { name: /删除选中 \(1\)/ })).toBeTruthy();

    fireEvent.click(boxes[1]);
    expect(await screen.findByRole('button', { name: /删除选中 \(2\)/ })).toBeTruthy();
  });

  it('批量删除只提交选中的项', async () => {
    await renderPage();
    fireEvent.click(screen.getAllByRole('checkbox')[1]);
    fireEvent.click(await screen.findByRole('button', { name: /删除选中/ }));
    fireEvent.click(await confirmButton('确认'));

    await waitFor(() =>
      expect(api.deleteSessions).toHaveBeenCalledWith([{ tool: 'codex', id: 'b' }]),
    );
  });

  it('批量删除部分失败时如实报出 N/M 与原因', async () => {
    api.deleteSessions.mockResolvedValue([
      { ok: true },
      { ok: false, error: '权限不足' },
    ]);
    await renderPage();
    fireEvent.click(screen.getAllByRole('checkbox')[0]);
    fireEvent.click(screen.getAllByRole('checkbox')[1]);
    fireEvent.click(await screen.findByRole('button', { name: /删除选中/ }));
    fireEvent.click(await confirmButton('确认'));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('批量删除失败 1/2 个');
    expect(alert.textContent).toContain('权限不足');
  });

  it('全部成功时不显示错误条', async () => {
    await renderPage();
    fireEvent.click(screen.getAllByRole('checkbox')[0]);
    fireEvent.click(await screen.findByRole('button', { name: /删除选中/ }));
    fireEvent.click(await confirmButton('确认'));

    await waitFor(() => expect(api.deleteSessions).toHaveBeenCalled());
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('删除后清空勾选（避免对已删项再次操作）', async () => {
    await renderPage();
    fireEvent.click(screen.getAllByRole('checkbox')[0]);
    fireEvent.click(await screen.findByRole('button', { name: /删除选中/ }));
    fireEvent.click(await confirmButton('确认'));

    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /删除选中/ })).toBeNull(),
    );
  });
});

describe('HistoryPage — 快捷清理', () => {
  it('输入非法天数时报错且不弹确认框', async () => {
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /快捷清理/ }));
    fireEvent.change(screen.getByLabelText('清理多少天前的会话'), {
      target: { value: '0' },
    });
    fireEvent.click(screen.getByRole('button', { name: '继续' }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('请输入有效的天数');
    expect(api.cleanupSessions).not.toHaveBeenCalled();
  });

  it('合法天数走确认框，确认后按当前筛选清理', async () => {
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /快捷清理/ }));
    fireEvent.change(screen.getByLabelText('清理多少天前的会话'), {
      target: { value: '7' },
    });
    fireEvent.click(screen.getByRole('button', { name: '继续' }));
    fireEvent.click(await confirmButton('确认'));

    await waitFor(() => expect(api.cleanupSessions).toHaveBeenCalledWith(undefined, 7));
  });

  it('清理失败时汇总原因', async () => {
    api.cleanupSessions.mockResolvedValue([{ ok: false, error: '磁盘只读' }]);
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /快捷清理/ }));
    fireEvent.click(screen.getByRole('button', { name: '继续' }));
    fireEvent.click(await confirmButton('确认'));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('清理失败 1/1 个');
    expect(alert.textContent).toContain('磁盘只读');
  });
});
