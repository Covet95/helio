// @vitest-environment jsdom
/**
 * `command()` 包装的 fallback 行为。
 *
 * 审查 §5.3：「fallback 分支行为分歧没有测试钉住」。
 *
 * 这里的规则是：
 * - 给了 fallback → 非 Tauri 环境（浏览器里预览）返回它，页面照常渲染；
 * - 没给 fallback → 拒绝，因为这类命令没有合理的「空值」，静默返回
 *   undefined 会让调用方拿到假数据（例如把「没有档案」当成「查询成功返回空」）。
 *
 * 区分这两种是刻意的，值得钉住——写反了要么预览崩、要么预览时假装成功。
 */
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

/** 重新加载模块，让 `canUseTauri()` 重新求值。 */
async function loadApi() {
  vi.resetModules();
  return (await import('./tauri')).tauriApi;
}

/** 模拟在 Tauri 里运行 / 在浏览器里运行。 */
function setTauri(yes: boolean) {
  if (yes) {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  } else {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  }
}

beforeEach(() => {
  vi.resetAllMocks();
  invoke.mockResolvedValue(undefined);
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe('非 Tauri 环境（浏览器预览）', () => {
  beforeEach(() => setTauri(false));

  it('带 fallback 的命令返回 fallback，不抛错', async () => {
    const api = await loadApi();
    await expect(api.listProfiles()).resolves.toEqual([]);
    await expect(api.getStatus()).resolves.toEqual({
      database: { size: 0, profile_count: 0, path: '' },
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('不带 fallback 的命令拒绝，并给出可照做的提示', async () => {
    const api = await loadApi();
    await expect(api.exportDatabase('/tmp/x.db')).rejects.toThrow('请在 Helio 桌面应用内执行此操作');
  });

  it('拒绝时也不调用 invoke', async () => {
    const api = await loadApi();
    await api.addProfile({} as never).catch(() => {});
    expect(invoke).not.toHaveBeenCalled();
  });

  it('listConfigBackups 等列表型命令返回空数组而非 undefined', async () => {
    const api = await loadApi();
    // 返回 undefined 会让 `.map()` 崩，这是 fallback 存在的意义。
    const backups = await api.listConfigBackups('codex');
    expect(Array.isArray(backups)).toBe(true);
    const sessions = await api.listSessions();
    expect(Array.isArray(sessions)).toBe(true);
  });

  it('readCodexConfigRaw 返回空串（调用方直接渲染它）', async () => {
    const api = await loadApi();
    await expect(api.readCodexConfigRaw()).resolves.toBe('');
  });
});

describe('Tauri 环境', () => {
  beforeEach(() => setTauri(true));

  it('走 invoke，忽略 fallback', async () => {
    invoke.mockResolvedValue([{ id: 1 }]);
    const api = await loadApi();
    await expect(api.listProfiles()).resolves.toEqual([{ id: 1 }]);
    expect(invoke).toHaveBeenCalledWith('list_profiles', undefined);
  });

  it('invoke 拒绝时原样抛出（不吞成 fallback）', async () => {
    invoke.mockRejectedValue(new Error('db locked'));
    const api = await loadApi();
    await expect(api.listProfiles()).rejects.toThrow('db locked');
  });

  it('参数按后端期望的形状传递', async () => {
    const api = await loadApi();
    await api.switchProfile('codex', 'my-profile', true);
    expect(invoke).toHaveBeenCalledWith('switch_profile', {
      targetApp: 'codex',
      profileName: 'my-profile',
      probe: true,
    });
  });

  it('probe 省略时传 false（而非 undefined）', async () => {
    const api = await loadApi();
    await api.switchProfile('codex', 'p');
    expect(invoke).toHaveBeenCalledWith('switch_profile', {
      targetApp: 'codex',
      profileName: 'p',
      probe: false,
    });
  });

  it('deleteProfile 用 name + targetApp 定位（同名档案可跨工具共存）', async () => {
    const api = await loadApi();
    await api.deleteProfile('codex', 'proxy');
    expect(invoke).toHaveBeenCalledWith('delete_profile', {
      name: 'proxy',
      targetApp: 'codex',
    });
  });
});
