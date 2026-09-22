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
  statusError: null as string | null,
  loadingStatus: false,
  /** 订阅者：让改 store 能触发重渲染（对象桩默认做不到）。 */
  listeners: new Set<() => void>(),
}));
/** 改 store 并通知订阅者。 */
function setStore(patch: Record<string, unknown>) {
  Object.assign(store, patch);
  for (const notify of store.listeners) notify();
}
vi.mock('../store', () => ({
  // 极简的 zustand 替身：支持 selector 订阅 + 外部变更触发重渲染。
  useStore: (selector: (s: unknown) => unknown) => {
    const [, force] = React.useReducer((n: number) => n + 1, 0);
    React.useEffect(() => {
      store.listeners.add(force);
      return () => { store.listeners.delete(force); };
    }, []);
    return selector(store);
  },
}));
vi.mock('zustand/react/shallow', () => ({ useShallow: (f: unknown) => f }));

beforeEach(() => {
  vi.resetAllMocks();
  // 默认给一个「加载成功但没有工具配置」的状态：status 为 null 现在是
  // 「读取失败」分支（不再渲染推测数据），探测类用例需要正常渲染的页面。
  store.status = { database: { size: 0, profile_count: 0, path: '' } };
  store.statusError = null;
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

describe('StatusPage — 读取失败不得伪装成空数据', () => {
  it('status 为 null 时显示失败与原因，不显示「未设置」「档案 0」', async () => {
    store.status = null;
    store.statusError = '加载状态失败：连接中断';
    await renderPage();

    // 关键：不能出现任何「推测出来的正常值」
    expect(screen.queryByText('档案')).toBeNull();
    expect(screen.queryByText('未初始化')).toBeNull();
    expect(screen.queryByText('未设置')).toBeNull();
    // 应当明确告知失败
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('读取状态失败');
    expect(alert.textContent).toContain('连接中断');
  });

  it('失败态提供重试入口', async () => {
    store.status = null;
    store.statusError = 'boom';
    await renderPage();

    fireEvent.click(await screen.findByRole('button', { name: /重试/ }));
    expect(store.fetchStatus).toHaveBeenCalledWith(true);
  });
});

describe('StatusPage — 探活结果不得跨档案残留', () => {
  it('档案变化后清空旧探活结果（否则新档案会配旧 HTTP 状态）', async () => {
    setStore({
      status: {
        codex: { connected: true, profile: { name: 'A', provider: 'x', api_url: 'u', model: 'm' } },
        database: { size: 0, profile_count: 1, path: '' },
      },
    });
    api.probeActiveProfiles.mockResolvedValue([
      { target_app: 'codex', configured: true, ok: true, latency_ms: 42, probed_at: 1 },
    ]);
    await renderPage();
    fireEvent.click(screen.getByRole('button', { name: /检测连通性/ }));
    expect(await screen.findByText('可达 42ms')).toBeTruthy();

    // 档案换成 B —— 旧探活结果必须消失
    setStore({
      status: {
        codex: { connected: true, profile: { name: 'B', provider: 'x', api_url: 'u', model: 'm' } },
        database: { size: 0, profile_count: 1, path: '' },
      },
    });

    await waitFor(() => expect(screen.queryByText('可达 42ms')).toBeNull());
  });
});
