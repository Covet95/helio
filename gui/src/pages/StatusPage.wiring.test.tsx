// @vitest-environment jsdom
/**
 * `StatusPage` 的接线测试。
 *
 * 分支推导已在 `statusBadge.test.ts` 覆盖；这里钉住
 * 「探测按钮 → probeActiveProfiles → 徽标更新」这条线，以及本机扫描
 * 只在有档案的工具上跑（对未配置工具扫盘是无意义的 IO）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';

const api = vi.hoisted(() => ({
  probeActiveProfiles: vi.fn(),
  scanLocalApi: vi.fn(),
}));
vi.mock('../lib/tauri', () => ({ tauriApi: api }));

const store = vi.hoisted(() => ({
  fetchStatus: vi.fn(),
  status: null as unknown,
  loadingStatus: false,
}));
vi.mock('../store', () => ({
  useStore: (selector: (s: unknown) => unknown) => selector(store),
}));
vi.mock('zustand/react/shallow', () => ({ useShallow: (f: unknown) => f }));

beforeEach(() => {
  vi.resetAllMocks();
  store.status = null;
  store.loadingStatus = false;
  store.fetchStatus.mockResolvedValue(undefined);
  api.probeActiveProfiles.mockResolvedValue([]);
  api.scanLocalApi.mockResolvedValue({ found: false, api_url: '', api_key: '', provider: '' });
});

afterEach(cleanup);

async function renderPage() {
  const { default: StatusPage } = await import('./StatusPage');
  render(React.createElement(StatusPage));
}

describe('StatusPage — 探测', () => {
  it('点检测后调用 probeActiveProfiles 并展示延迟', async () => {
    api.probeActiveProfiles.mockResolvedValue([
      {
        target_app: 'codex',
        configured: true,
        ok: true,
        latency_ms: 88,
        probed_at: 1,
      },
    ]);
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /检测连通性/ }));

    await waitFor(() => expect(api.probeActiveProfiles).toHaveBeenCalled());
    expect(await screen.findByText('可达 88ms')).toBeTruthy();
  });

  it('探测失败时显示错误条', async () => {
    api.probeActiveProfiles.mockRejectedValue(new Error('offline'));
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /检测连通性/ }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('offline');
  });

  it('托管工具显示「工具托管」而非可达', async () => {
    api.probeActiveProfiles.mockResolvedValue([
      { target_app: 'codex', configured: true, ok: false, managed: true, probed_at: 1 },
    ]);
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /检测连通性/ }));

    expect(await screen.findByText('工具托管')).toBeTruthy();
  });
});

describe('StatusPage — 本机扫描', () => {
  it('只为有档案的工具扫描（未配置工具不做无意义 IO）', async () => {
    store.status = {
      codex: { connected: true, profile: { name: 'p', provider: 'x', api_url: 'u', model: 'm' } },
      database: { size: 0, profile_count: 1, path: '/tmp/x.db' },
    };
    await renderPage();

    await waitFor(() => expect(api.scanLocalApi).toHaveBeenCalled());
    const scanned = api.scanLocalApi.mock.calls.map((c) => c[0]);
    expect(scanned).toEqual(['codex']);
  });

  it('无档案时不扫描任何工具', async () => {
    store.status = { database: { size: 0, profile_count: 0, path: '' } };
    await renderPage();

    await waitFor(() => expect(store.fetchStatus).toHaveBeenCalled());
    expect(api.scanLocalApi).not.toHaveBeenCalled();
  });

  it('扫描失败不影响页面渲染（失败即跳过）', async () => {
    store.status = {
      codex: { connected: true, profile: { name: 'p', provider: 'x', api_url: 'u', model: 'm' } },
      database: { size: 0, profile_count: 1, path: '/tmp/x.db' },
    };
    api.scanLocalApi.mockRejectedValue(new Error('permission denied'));
    await renderPage();

    expect(await screen.findByText('Codex')).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

describe('StatusPage — 数据库信息', () => {
  it('展示档案数与路径', async () => {
    store.status = {
      database: { size: 2048, profile_count: 7, path: '/Users/x/.helio/helio.db' },
    };
    await renderPage();

    expect(await screen.findByText('7')).toBeTruthy();
    expect(screen.getByText('/Users/x/.helio/helio.db')).toBeTruthy();
  });

  it('路径为空时显示「未初始化」', async () => {
    store.status = { database: { size: 0, profile_count: 0, path: '' } };
    await renderPage();

    expect(await screen.findByText('未初始化')).toBeTruthy();
  });
});
