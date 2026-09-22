// @vitest-environment jsdom
/**
 * `ApiModeSelector` 的行为。
 *
 * 这段按钮组原先在 `ProfileFormModal` 里逐字重复三遍，零覆盖。抽出来后
 * 值得钉住的关键规则是「空值按 chat_completions 处理」——写错了表现为
 * 三个按钮都不高亮，用户看不出当前是什么模式。
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import React from 'react';
import { API_MODE_OPTIONS, ApiModeSelector } from './helpers';

afterEach(cleanup);

/** 取当前高亮的按钮文案。 */
function highlighted(): string[] {
  return API_MODE_OPTIONS.filter((o) =>
    screen.getByRole('button', { name: o.label }).className.includes('border-accent'),
  ).map((o) => o.label);
}

describe('ApiModeSelector', () => {
  it('渲染三个协议选项', () => {
    render(React.createElement(ApiModeSelector, { onChange: () => {} }));
    for (const option of API_MODE_OPTIONS) {
      expect(screen.getByRole('button', { name: option.label })).toBeTruthy();
    }
  });

  it('点击回调传出对应的取值', () => {
    const onChange = vi.fn();
    render(React.createElement(ApiModeSelector, { onChange }));

    fireEvent.click(screen.getByRole('button', { name: 'Anthropic' }));
    expect(onChange).toHaveBeenCalledWith('anthropic_messages');

    fireEvent.click(screen.getByRole('button', { name: 'Responses' }));
    expect(onChange).toHaveBeenCalledWith('codex_responses');
  });

  it('有值时只高亮一项', () => {
    render(React.createElement(ApiModeSelector, { value: 'anthropic_messages', onChange: () => {} }));
    expect(highlighted()).toEqual(['Anthropic']);
  });

  it('空值按 chat_completions 处理（否则三个按钮都不高亮）', () => {
    for (const value of [undefined, '']) {
      render(React.createElement(ApiModeSelector, { value, onChange: () => {} }));
      expect(highlighted(), String(value)).toEqual(['Chat']);
      cleanup();
    }
  });

  it('未知取值时也不留空——回落到 Chat 高亮', () => {
    render(React.createElement(ApiModeSelector, { value: 'bogus', onChange: () => {} }));
    expect(highlighted()).toEqual(['Chat']);
  });
});
