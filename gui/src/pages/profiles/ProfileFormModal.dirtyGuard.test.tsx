// @vitest-environment jsdom
/**
 * 关闭表单时的未保存修改守卫。
 *
 * 原先改完字段直接点「取消」或按 Esc 会**静默丢弃**全部修改，没有任何提示。
 * `ProfileFormModal` 有 30+ 个字段、还带 OpenCode 的模型/变体配置，
 * 误关一次损失很大。
 *
 * 这里同时钉住一个容易踩的坑：确认框叠在表单之上时，Esc 必须只作用于
 * 最上层——否则按 Esc 会「关掉确认框 + 关掉整个表单」，等于没防。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';
import type { ApiProfile } from '../../types';

const api = vi.hoisted(() => ({
  listProfiles: vi.fn(), getStatus: vi.fn(), addProfile: vi.fn(),
  updateProfile: vi.fn(), deleteProfile: vi.fn(), switchProfile: vi.fn(),
  scanLocalApi: vi.fn(), fetchModels: vi.fn(), testModel: vi.fn(), copyText: vi.fn(),
}));
vi.mock('../../lib/tauri', () => ({ tauriApi: api }));

const saved = {
  id: 1, name: '原名', provider: 'openai', api_url: 'https://x/v1',
  api_key: 'sk-1', target_app: 'codex', created_at: 1, updated_at: 1,
} as unknown as ApiProfile;

beforeEach(() => vi.resetAllMocks());
afterEach(cleanup);

/** 渲染编辑既有档案的表单。 */
function renderEditing(onClose = vi.fn()) {
  return import('./ProfileFormModal').then(({ ProfileModal }) => {
    render(
      React.createElement(ProfileModal, {
        profile: saved,
        initialTool: 'codex' as never,
        onClose,
        onSave: vi.fn(),
      }),
    );
    return onClose;
  });
}

/** 把名称改掉，制造未保存修改。 */
function makeDirty() {
  fireEvent.change(screen.getByDisplayValue('原名'), { target: { value: '改过的名字' } });
}

describe('未保存修改守卫', () => {
  it('没有改动时点取消直接关闭，不打扰用户', async () => {
    const onClose = await renderEditing();
    fireEvent.click(screen.getByRole('button', { name: '取消' }));

    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('有改动时点取消先问一句，不直接关闭', async () => {
    const onClose = await renderEditing();
    makeDirty();
    fireEvent.click(screen.getByRole('button', { name: '取消' }));

    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('尚未保存');
    expect(onClose).not.toHaveBeenCalled();
  });

  it('确认放弃后才真正关闭', async () => {
    const onClose = await renderEditing();
    makeDirty();
    fireEvent.click(screen.getByRole('button', { name: '取消' }));

    const dialog = await screen.findByRole('alertdialog');
    const discard = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('放弃修改'),
    )!;
    fireEvent.click(discard);

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('选择继续编辑则留在表单，修改还在', async () => {
    const onClose = await renderEditing();
    makeDirty();
    fireEvent.click(screen.getByRole('button', { name: '取消' }));

    const dialog = await screen.findByRole('alertdialog');
    const keep = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('继续编辑'),
    )!;
    fireEvent.click(keep);

    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect((screen.getByDisplayValue('改过的名字') as HTMLInputElement).value).toBe('改过的名字');
  });

  it('提示里带上档案名，用户知道在放弃哪个', async () => {
    await renderEditing();
    makeDirty();
    fireEvent.click(screen.getByRole('button', { name: '取消' }));

    expect((await screen.findByRole('alertdialog')).textContent).toContain('原名');
  });

  it('非名称字段的改动同样触发守卫（不能只盯 name）', async () => {
    await renderEditing();
    fireEvent.change(screen.getByLabelText('API URL'), {
      target: { value: 'https://changed.example/v1' },
    });
    fireEvent.click(screen.getByRole('button', { name: '取消' }));

    expect(await screen.findByRole('alertdialog')).toBeTruthy();
  });
});

describe('切换目标工具前的守卫（仅新建时）', () => {
  /** 渲染新建表单（工具可选）。 */
  async function renderNew(onClose = vi.fn()) {
    const { ProfileModal } = await import('./ProfileFormModal');
    render(
      React.createElement(ProfileModal, {
        profile: null,
        initialTool: 'codex' as never,
        onClose,
        onSave: vi.fn(),
      }),
    );
    return onClose;
  }

  /** 新建表单里切到另一个工具。 */
  function switchTo(label: string) {
    fireEvent.click(screen.getByRole('button', { name: label }));
  }

  it('没填过东西时直接切换，不打扰', async () => {
    await renderNew();
    switchTo('Claude Code');

    expect(screen.queryByRole('alertdialog')).toBeNull();
    // 切过去了：Claude Code 的表单字段出现
    expect(screen.getByText('目标工具')).toBeTruthy();
  });

  it('已填内容时切换先问一句（否则输入无声消失）', async () => {
    await renderNew();
    fireEvent.change(screen.getByLabelText('API URL'), {
      target: { value: 'https://typed.example/v1' },
    });
    switchTo('Claude Code');

    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('清空');
    // 还在原表单，输入还在
    expect((screen.getByLabelText('API URL') as HTMLInputElement).value).toBe('https://typed.example/v1');
  });

  it('确认切换后表单重置为目标工具的字段', async () => {
    await renderNew();
    fireEvent.change(screen.getByLabelText('API URL'), {
      target: { value: 'https://typed.example/v1' },
    });
    switchTo('Claude Code');
    const dialog = await screen.findByRole('alertdialog');
    const confirm = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('切换并清空'),
    )!;
    fireEvent.click(confirm);

    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    // 原输入被清掉，换成目标工具的默认值（Claude Code 预置官方 endpoint）
    expect((screen.getByLabelText('API URL') as HTMLInputElement).value).not.toBe(
      'https://typed.example/v1',
    );
  });

  it('选择留在当前工具则不切换、不清空', async () => {
    await renderNew();
    fireEvent.change(screen.getByLabelText('API URL'), {
      target: { value: 'https://typed.example/v1' },
    });
    switchTo('Claude Code');
    const dialog = await screen.findByRole('alertdialog');
    const keep = Array.from(dialog.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('留在当前工具'),
    )!;
    fireEvent.click(keep);

    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect((screen.getByLabelText('API URL') as HTMLInputElement).value).toBe('https://typed.example/v1');
  });

  it('点当前已选工具不弹确认（不是切换）', async () => {
    await renderNew();
    fireEvent.change(screen.getByLabelText('API URL'), {
      target: { value: 'https://typed.example/v1' },
    });
    switchTo('Codex');

    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect((screen.getByLabelText('API URL') as HTMLInputElement).value).toBe('https://typed.example/v1');
  });
});

describe('嵌套对话框的 Esc 只作用于最上层', () => {
  it('Esc 打开确认框，再 Esc 只关确认框、表单留着', async () => {
    const onClose = await renderEditing();
    makeDirty();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(await screen.findByRole('alertdialog')).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('alertdialog')).toBeNull();
    // 关键：表单不能被一起关掉，否则等于没防
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByDisplayValue('改过的名字')).toBeTruthy();
  });

  it('没有改动时 Esc 直接关闭表单', async () => {
    const onClose = await renderEditing();
    fireEvent.keyDown(document, { key: 'Escape' });

    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
