// @vitest-environment jsdom
/**
 * `Modal` / `ConfirmDialog` 的手写交互逻辑。
 *
 * 审查 §5.3：这两个组件全局复用（187 行）却无测试，而焦点返还、Esc 关闭、
 * pending 防重入都是手写实现——写错的表现是「对话框关了但焦点丢了」
 * 或「确认按钮点两下执行两次」，都属于不好复现、又确实会伤到用户的问题。
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';
import { ConfirmDialog, Modal } from './Modal';

afterEach(cleanup);

/**
 * `React.createElement` 的重载推断不了「props 里含必填 children」的组件
 * （会把 children 当成第三个参数而不是 props 的一部分），这里显式声明类型。
 */
type ModalProps = React.ComponentProps<typeof Modal>;

/** 渲染一个 Modal，children 默认给个占位段落。 */
function modal(props: Omit<ModalProps, 'children'>, children?: React.ReactNode) {
  return React.createElement(
    Modal,
    props as ModalProps,
    children ?? React.createElement('p', null, 'x'),
  );
}

describe('Modal', () => {
  it('渲染标题、内容与页脚，并带无障碍属性', () => {
    render(
      modal(
        { title: '标题', onClose: () => {}, footer: React.createElement('button', null, '确定') },
        React.createElement('p', null, '正文'),
      ),
    );

    const dialog = screen.getByRole('dialog');
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(screen.getByText('标题')).toBeTruthy();
    expect(screen.getByText('正文')).toBeTruthy();
    expect(screen.getByText('确定')).toBeTruthy();
    // 标题 id 必须被 aria-labelledby 引用，否则屏幕阅读器读不出对话框名。
    const labelledBy = dialog.getAttribute('aria-labelledby');
    expect(labelledBy).toBeTruthy();
    expect(document.getElementById(labelledBy!)?.textContent).toBe('标题');
  });

  it('Esc 触发 onClose', () => {
    const onClose = vi.fn();
    render(modal({ title: 't', onClose }));

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('busy 时 Esc 不关闭（提交中不能中途退出）', () => {
    const onClose = vi.fn();
    render(modal({ title: 't', onClose, busy: true }));

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog').getAttribute('aria-busy')).toBe('true');
  });

  it('busy 时关闭按钮禁用', () => {
    render(modal({ title: 't', onClose: () => {}, busy: true }));
    expect((screen.getByLabelText('关闭') as HTMLButtonElement).disabled).toBe(true);
  });

  it('非 Esc 按键不关闭', () => {
    const onClose = vi.fn();
    render(modal({ title: 't', onClose }));

    fireEvent.keyDown(document, { key: 'a' });
    fireEvent.keyDown(document, { key: 'Enter' });
    expect(onClose).not.toHaveBeenCalled();
  });

  it('卸载后不再响应 Esc（否则监听器泄漏）', () => {
    const onClose = vi.fn();
    render(modal({ title: 't', onClose }));
    cleanup();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
  });

  it('关闭后焦点回到打开它的元素', () => {
    const trigger = document.createElement('button');
    trigger.textContent = '打开';
    document.body.appendChild(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    const { unmount } = render(modal({ title: 't', onClose: () => {} }));
    // 真实浏览器里焦点会进到对话框内的首个可聚焦元素；jsdom 不会自动移动，
    // 这里手动模拟，否则「焦点返还」无从验证（不移动就没有可返还的焦点）。
    (screen.getByLabelText('关闭') as HTMLButtonElement).focus();
    expect(document.activeElement).not.toBe(trigger);

    unmount();

    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });

  it('触发元素已被移除时卸载不抛错', () => {
    // 注意：`isConnected` 那道守卫在 jsdom 里**无法验证**——jsdom 对游离
    // 节点的 `focus()` 本就是 no-op（activeElement 留在 body），所以去掉
    // 守卫这个测试照样通过。它只能在真实浏览器里生效（那里对游离元素
    // focus 会把焦点丢到 body，用户按 Tab 就从页首重新开始）。
    // 保留此用例仅作「卸载不炸」的冒烟，别把它当成守卫的证明。
    const trigger = document.createElement('button');
    document.body.appendChild(trigger);
    trigger.focus();

    const { unmount } = render(modal({ title: 't', onClose: () => {} }));
    trigger.remove();
    expect(() => unmount()).not.toThrow();
  });

  it('alertdialog 角色用于需要用户立即处理的确认框', () => {
    render(modal({ title: 't', onClose: () => {}, role: 'alertdialog' }));
    expect(screen.getByRole('alertdialog')).toBeTruthy();
  });
});

describe('ConfirmDialog', () => {
  const base = {
    title: '删除',
    message: '确定删除？',
    onCancel: vi.fn(),
    onConfirm: vi.fn().mockResolvedValue(undefined),
  };

  it('默认角色是 alertdialog，并用 aria-describedby 关联说明文字', () => {
    render(React.createElement(ConfirmDialog, base));

    const dialog = screen.getByRole('alertdialog');
    const describedBy = dialog.getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy!)?.textContent).toBe('确定删除？');
  });

  it('自定义确认/取消文案', () => {
    render(
      React.createElement(ConfirmDialog, { ...base, confirmText: '删除 3 个', cancelText: '再想想' }),
    );
    expect(screen.getByRole('button', { name: '删除 3 个' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '再想想' })).toBeTruthy();
  });

  it('确认时先显示处理中，再调 onConfirm', async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    render(React.createElement(ConfirmDialog, { ...base, onConfirm }));

    fireEvent.click(screen.getByRole('button', { name: '确定' }));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
  });

  it('pending 期间重复点击不重复执行（防重入）', async () => {
    let release: () => void = () => {};
    const onConfirm = vi.fn(() => new Promise<void>((r) => { release = r; }));
    render(React.createElement(ConfirmDialog, { ...base, onConfirm }));

    // 注意：不能用 testing-library 的 click——它在 act() 里包了 await，
    // 每次点击都会把 pending 状态冲刷干净，于是 disabled 就足以挡住第二次，
    // `pendingRef` 那道守卫根本走不到（实测：删掉守卫测试仍通过）。
    // 这里直接派发原生事件，模拟「同一 tick 内连点两下」。
    const confirmBtn = screen.getByRole('button', { name: '确定' }) as HTMLButtonElement;
    confirmBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    confirmBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    confirmBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));

    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
    release();
    await waitFor(() => expect(screen.queryByRole('button', { name: '处理中…' })).toBeNull());
  });

  it('onConfirm 抛错时在框内显示原因，且不关闭', async () => {
    const onConfirm = vi.fn().mockRejectedValue(new Error('目标文件被占用'));
    render(React.createElement(ConfirmDialog, { ...base, onConfirm }));

    fireEvent.click(screen.getByRole('button', { name: '确定' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('目标文件被占用');
    // 对话框还在——用户需要看到失败原因，而不是一闪而过。
    expect(screen.getByRole('alertdialog')).toBeTruthy();
  });

  it('失败后可以重试（pending 复位）', async () => {
    const onConfirm = vi.fn()
      .mockRejectedValueOnce(new Error('第一次失败'))
      .mockResolvedValueOnce(undefined);
    render(React.createElement(ConfirmDialog, { ...base, onConfirm }));

    fireEvent.click(screen.getByRole('button', { name: '确定' }));
    await screen.findByRole('alert');
    fireEvent.click(screen.getByRole('button', { name: '确定' }));

    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(2));
  });

  it('取消调用 onCancel，不调 onConfirm', () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(React.createElement(ConfirmDialog, { ...base, onCancel, onConfirm }));

    fireEvent.click(screen.getByRole('button', { name: '取消' }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
