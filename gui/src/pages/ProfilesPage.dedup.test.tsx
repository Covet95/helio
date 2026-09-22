// @vitest-environment jsdom
/**
 * `runDedup` 的部分失败语义。
 *
 * 审查点名这段「循环删除 N 个档案 + 部分失败语义」无测试。
 *
 * 原实现遇到第一个失败就跳出，只报「去重失败：<原因>」——而前面几个
 * **已经删掉了**。用户看到「失败」，以为什么都没变，实际上档案已经少了几
 * 个且不可撤销。这里钉住新语义：逐个删、如实报出删了几个、哪几个失败。
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import React from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { ApiProfile } from '../types';

/** 三个同配置的 codex 档案：去重会保留最新的一个，删掉另外两个。 */
const profiles = [
  {
    id: 1, name: 'old', provider: 'openai', api_url: 'https://x/v1', api_key: 'sk-1',
    model: 'm', target_app: 'codex', created_at: 1, updated_at: 1,
  },
  {
    id: 2, name: 'mid', provider: 'openai', api_url: 'https://x/v1', api_key: 'sk-1',
    model: 'm', target_app: 'codex', created_at: 1, updated_at: 5,
  },
  {
    id: 3, name: 'newest', provider: 'openai', api_url: 'https://x/v1', api_key: 'sk-1',
    model: 'm', target_app: 'codex', created_at: 1, updated_at: 9,
  },
] as unknown as ApiProfile[];

const api = vi.hoisted(() => ({
  listProfiles: vi.fn(), getStatus: vi.fn(), addProfile: vi.fn(),
  updateProfile: vi.fn(), deleteProfile: vi.fn(), switchProfile: vi.fn(),
  scanLocalApi: vi.fn(),
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

beforeEach(() => {
  vi.resetAllMocks();
  api.listProfiles.mockResolvedValue(profiles);
  api.getStatus.mockResolvedValue({ database: { size: 1, profile_count: 3, path: '/tmp/x' } });
  api.deleteProfile.mockResolvedValue(true);
});

afterEach(cleanup);

/** 渲染并切到 codex 页签。 */
async function renderCodexTab() {
  const { default: ProfilesPage } = await import('./ProfilesPage');
  const { useStore } = await import('../store');
  useStore.setState({ selectedTool: 'codex' as never });
  await useStore.getState().fetchProfiles(true);
  await useStore.getState().fetchStatus(true);
  render(React.createElement(MemoryRouter, null, React.createElement(ProfilesPage)));
  await waitFor(() => expect(screen.getByText('newest')).toBeTruthy());
}

/** 点「去重 (N)」并在确认框里确认。 */
async function confirmDedup() {
  fireEvent.click(await screen.findByRole('button', { name: /去重 \(/ }));
  const dialog = await screen.findByRole('alertdialog');
  fireEvent.click(within(dialog).getByRole('button', { name: /删除 \d+ 个/ }));
}

describe('runDedup', () => {
  it('保留最新的一条，删掉其余', async () => {
    await renderCodexTab();
    await confirmDedup();

    await waitFor(() => expect(api.deleteProfile).toHaveBeenCalledTimes(2));
    const deleted = api.deleteProfile.mock.calls.map((c) => c[1]);
    expect(deleted.sort()).toEqual(['mid', 'old']);
  });

  it('全部成功时报删除数与保留名单', async () => {
    await renderCodexTab();
    await confirmDedup();

    const alert = await screen.findByText(/已清理 2 个重复档案/);
    expect(alert.textContent).toContain('newest');
  });

  it('中途失败时不谎报「全部失败」——已删的要如实计入', async () => {
    // 第二个删失败；第一个已经删掉了。
    api.deleteProfile
      .mockResolvedValueOnce(true)
      .mockRejectedValueOnce(new Error('文件被占用'));
    await renderCodexTab();
    await confirmDedup();

    const alert = await screen.findByText(/个失败/);
    expect(alert.textContent).toContain('已清理 1 个');
    expect(alert.textContent).toContain('1 个失败');
    expect(alert.textContent).toContain('文件被占用');
    // 失败项要点名，用户才知道还剩哪个
    expect(alert.textContent).toContain('old');
  });

  it('失败时仍然刷新列表（否则界面与磁盘不一致）', async () => {
    api.deleteProfile.mockRejectedValue(new Error('boom'));
    await renderCodexTab();
    const before = api.listProfiles.mock.calls.length;
    await confirmDedup();

    await waitFor(() => expect(api.listProfiles.mock.calls.length).toBeGreaterThan(before));
  });

  it('无论成败都关闭确认框', async () => {
    api.deleteProfile.mockRejectedValue(new Error('boom'));
    await renderCodexTab();
    await confirmDedup();

    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
  });
});
