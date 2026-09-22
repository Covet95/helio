// @vitest-environment jsdom
/**
 * `App` 的接线测试。
 *
 * 审查点名：路由、`profile-switched` 事件监听、全局错误横幅三项零覆盖。
 * 其中事件监听最值得钉——它把「状态栏切换了 profile」变成「界面刷新」，
 * 断了不会报错，只会让界面停在旧数据上。
 *
 * 事件监听只在 `isTauri()` 为真时注册，所以这里要分别测两种环境。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';

const listen = vi.hoisted(() => vi.fn());
const isTauri = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ isTauri }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

const store = vi.hoisted(() => ({
  fetchProfiles: vi.fn(),
  fetchStatus: vi.fn(),
  refresh: vi.fn(),
  lastError: null as string | null,
  clearError: vi.fn(),
}));
vi.mock('./store', () => ({
  useStore: (selector: (s: unknown) => unknown) => selector(store),
}));
vi.mock('zustand/react/shallow', () => ({ useShallow: (f: unknown) => f }));

// 各页面都是 lazy 的；用最轻的桩避免把它们整棵拉进来。
vi.mock('./components/layout/Sidebar', () => ({ default: () => React.createElement('nav') }));
vi.mock('./pages/ProfilesPage', () => ({ default: () => React.createElement('div', null, 'ProfilesPage') }));
vi.mock('./pages/ConfigPage', () => ({ default: () => React.createElement('div', null, 'ConfigPage') }));
vi.mock('./pages/StatusPage', () => ({ default: () => React.createElement('div', null, 'StatusPage') }));
vi.mock('./pages/ExportPage', () => ({ default: () => React.createElement('div', null, 'ExportPage') }));
vi.mock('./pages/ImportPage', () => ({ default: () => React.createElement('div', null, 'ImportPage') }));
vi.mock('./pages/HistoryPage', () => ({ default: () => React.createElement('div', null, 'HistoryPage') }));

beforeEach(() => {
  vi.resetAllMocks();
  store.lastError = null;
  store.fetchProfiles.mockResolvedValue(undefined);
  store.fetchStatus.mockResolvedValue(undefined);
  store.refresh.mockResolvedValue(undefined);
  isTauri.mockReturnValue(true);
  listen.mockResolvedValue(vi.fn());
  window.location.hash = '';
});

afterEach(cleanup);

async function renderApp() {
  const { default: App } = await import('./App');
  render(React.createElement(App));
}

describe('App — 启动', () => {
  it('挂载时拉取档案与状态', async () => {
    await renderApp();
    await waitFor(() => expect(store.fetchProfiles).toHaveBeenCalled());
    expect(store.fetchStatus).toHaveBeenCalled();
  });

  it('默认路由重定向到档案页', async () => {
    await renderApp();
    expect(await screen.findByText('ProfilesPage')).toBeTruthy();
  });

  it('未知路由也回落到档案页', async () => {
    window.location.hash = '#/no-such-page';
    await renderApp();
    expect(await screen.findByText('ProfilesPage')).toBeTruthy();
  });

  it('各路由渲染对应页面', async () => {
    for (const [hash, text] of [
      ['#/config', 'ConfigPage'],
      ['#/status', 'StatusPage'],
      ['#/import', 'ImportPage'],
      ['#/export', 'ExportPage'],
      ['#/history', 'HistoryPage'],
    ] as const) {
      window.location.hash = hash;
      await renderApp();
      expect(await screen.findByText(text), hash).toBeTruthy();
      cleanup();
    }
  });
});

describe('App — profile-switched 事件', () => {
  it('Tauri 环境下注册监听，事件触发时刷新', async () => {
    let handler: (() => void) | undefined;
    listen.mockImplementation((_event: string, cb: () => void) => {
      handler = cb;
      return Promise.resolve(vi.fn());
    });
    await renderApp();

    await waitFor(() => expect(listen).toHaveBeenCalledWith('profile-switched', expect.any(Function)));
    expect(store.refresh).not.toHaveBeenCalled();

    handler!();
    await waitFor(() => expect(store.refresh).toHaveBeenCalledTimes(1));
  });

  it('非 Tauri 环境（浏览器里跑）不注册监听', async () => {
    isTauri.mockReturnValue(false);
    await renderApp();

    await waitFor(() => expect(store.fetchProfiles).toHaveBeenCalled());
    expect(listen).not.toHaveBeenCalled();
  });

  it('卸载时取消监听（否则热重载会累积监听器）', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    await renderApp();
    await waitFor(() => expect(listen).toHaveBeenCalled());

    cleanup();
    await waitFor(() => expect(unlisten).toHaveBeenCalled());
  });

  it('listen 注册失败时不崩，只记日志', async () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    listen.mockRejectedValue(new Error('no ipc'));
    await renderApp();

    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(screen.getByText('ProfilesPage')).toBeTruthy();
    spy.mockRestore();
  });
});

describe('App — 全局错误横幅', () => {
  it('lastError 存在时显示，可关闭', async () => {
    store.lastError = '切换失败：目标配置文件被占用';
    await renderApp();

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('切换失败');
    fireEvent.click(screen.getByLabelText('关闭错误提示'));
    expect(store.clearError).toHaveBeenCalled();
  });

  it('无错误时不显示横幅', async () => {
    await renderApp();
    await waitFor(() => expect(store.fetchProfiles).toHaveBeenCalled());
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
