// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import React from 'react';
import { MemoryRouter } from 'react-router-dom';

vi.mock('../lib/tauri', () => ({
  tauriApi: { scanLocalApi: vi.fn(), scanCcSwitch: vi.fn(), addProfile: vi.fn() },
}));


describe('ImportPage tool selector', () => {
  it('reuses the shared AppSelector (single source of tool tabs)', async () => {
    const { useStore } = await import('../store');
    useStore.setState({ selectedTool: 'codex' as any });
    const { default: ImportPage } = await import('./ImportPage');
    render(React.createElement(MemoryRouter, null, React.createElement(ImportPage)));
    // shared AppSelector renders role=group with aria-label; the old inline copy did not
    const group = await screen.findByRole('group', { name: '目标工具' });
    expect(group).toBeTruthy();
    cleanup();
  });
});
