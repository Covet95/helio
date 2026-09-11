// @vitest-environment jsdom
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import React from 'react';
import type { ApiProfile, TargetApp } from '../../types';
import { SUPPORTED_TOOLS } from '../../types';
import { ProfileModal } from './ProfileFormModal';

vi.mock('../../lib/tauri', () => ({
  tauriApi: {
    fetchModels: vi.fn().mockResolvedValue([]),
    testModel: vi.fn(),
    failoverProfileKeys: vi.fn(),
  },
}));

beforeAll(() => {
  // jsdom does not implement <dialog>.showModal().
  if (!HTMLDialogElement.prototype.showModal) {
    HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    };
    HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
      this.removeAttribute('open');
    };
  }
});

function sampleProfile(tool: TargetApp, extra?: Partial<ApiProfile>): ApiProfile {
  return {
    id: 1,
    name: 'mks',
    provider: 'openai',
    api_url: 'https://cn2.picpi.top/v1',
    api_key: 'sk-test-123',
    model: 'gpt-5.6-sol',
    target_app: tool,
    ...extra,
  };
}

describe('ProfileModal edit rendering', () => {
  for (const tool of SUPPORTED_TOOLS) {
    it(`renders api fields for ${tool.id}`, () => {
      render(
        React.createElement(ProfileModal, {
          profile: sampleProfile(tool.id as TargetApp),
          initialTool: tool.id as TargetApp,
          onClose: () => {},
          onSave: async () => {},
        }),
      );
      expect(screen.getByLabelText('名称')).toBeTruthy();
      expect(screen.getByLabelText('API URL')).toBeTruthy();
      cleanup();
    });
  }

  it('renders exact production row shape (mks/codex)', () => {
    render(
      React.createElement(ProfileModal, {
        profile: {
          id: 20,
          name: 'mks',
          provider: 'openai',
          api_url: 'https://cn2.picpi.top/v1',
          api_key: 'sk-REDACTED',
          model_mapping: undefined,
          model: 'gpt-5.6-sol',
          reasoning_effort: 'medium',
          context_1m: true,
          target_app: 'codex',
          models: undefined,
          wire_api: 'responses',
          env_key: undefined,
          requires_openai_auth: undefined,
          service_tier: undefined,
          experimental_bearer_token: undefined,
          supports_standalone_web_search: undefined,
          aws_profile: undefined,
          aws_region: undefined,
          api_mode: undefined,
          max_tokens: undefined,
          api_keys: [
            { id: 'k1784188509510099000', label: 'default', key: 'sk-REDACTED', is_active: true },
          ],
          catalog_models: undefined,
          created_at: 1781451144,
          updated_at: 1785810841,
          opencode_api_mode: undefined,
          opencode_model_configs: undefined,
          reasoning_summary: undefined,
          verbosity: undefined,
          auth_command: undefined,
          auth_args: undefined,
          auth_timeout_ms: undefined,
          auth_refresh_interval_ms: undefined,
          auth_cwd: undefined,
        } as ApiProfile,
        initialTool: 'codex' as TargetApp,
        onClose: () => {},
        onSave: async () => {},
      }),
    );
    expect(screen.getByLabelText('名称')).toBeTruthy();
    expect(screen.getByLabelText('API URL')).toBeTruthy();
    expect(screen.getByLabelText('API Key')).toBeTruthy();
    cleanup();
  });

  it('renders multi-key branch', () => {
    render(
      React.createElement(ProfileModal, {
        profile: sampleProfile('claude-code', {
          api_keys: [
            { id: 'a', label: 'one', key: 'sk-1', is_active: true },
            { id: 'b', label: 'two', key: 'sk-2', is_active: false },
          ],
        }),
        initialTool: 'claude-code' as TargetApp,
        onClose: () => {},
        onSave: async () => {},
      }),
    );
    expect(screen.getByLabelText('名称')).toBeTruthy();
    cleanup();
  });

  it('renders opencode model cards without fetching (union of model/models/configs)', () => {
    render(
      React.createElement(ProfileModal, {
        profile: sampleProfile('opencode', {
          model: 'gpt-5',
          models: ['gpt-5'],
          model_configs: { 'ghost-model': { limit: { context: 1000, output: 100 } } },
          opencode_api_mode: 'chat_completions',
        }),
        initialTool: 'opencode' as TargetApp,
        onClose: () => {},
        onSave: async () => {},
      }),
    );
    // fetchModels is mocked to [] — the custom config-only model must still be manageable.
    expect(screen.getByLabelText('删除模型 ghost-model')).toBeTruthy();
    expect(screen.getByLabelText('挂载模型 ghost-model')).toBeTruthy();
    cleanup();
  });

  it('opencode variant rows carry exactly one effort select', () => {
    const { container } = render(
      React.createElement(ProfileModal, {
        profile: sampleProfile('opencode', {
          model: 'gpt-5',
          models: ['gpt-5'],
          model_configs: { 'gpt-5': { variants: { low: {} } } },
          opencode_api_mode: 'chat_completions',
        }),
        initialTool: 'opencode' as TargetApp,
        onClose: () => {},
        onSave: async () => {},
      }),
    );
    // variant row: id editor, one effort select, disabled toggle, delete.
    expect(screen.getByLabelText('Variant low 名称')).toBeTruthy();
    expect(screen.getByLabelText('删除模型 gpt-5')).toBeTruthy();
    const effort = screen.getByLabelText('Variant low 推理强度') as HTMLSelectElement;
    expect(Array.from(effort.options).map((o) => o.value)).toEqual(
      ['', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
    );
    expect(container.querySelectorAll('select').length).toBe(1);
    // variant quick-add covers the official OpenAI set names.
    expect(screen.getByRole('button', { name: '+ none' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '+ minimal' })).toBeTruthy();
    cleanup();
  });

  it('deleting the default model promotes the next remaining model', () => {
    render(
      React.createElement(ProfileModal, {
        profile: sampleProfile('opencode', {
          model: 'model-a',
          models: ['model-a', 'model-b'],
          opencode_api_mode: 'chat_completions',
        }),
        initialTool: 'opencode' as TargetApp,
        onClose: () => {},
        onSave: async () => {},
      }),
    );
    fireEvent.click(screen.getByLabelText('删除模型 model-a'));
    expect(screen.queryByLabelText('删除模型 model-a')).toBeNull();
    expect(screen.getByLabelText('删除模型 model-b')).toBeTruthy();
    expect((screen.getByLabelText('默认模型') as HTMLInputElement).value).toBe('model-b');
    cleanup();
  });

  it('default model checkbox is disabled, unchecking keeps the config card', () => {
    render(
      React.createElement(ProfileModal, {
        profile: sampleProfile('opencode', {
          model: 'model-a',
          models: ['model-a', 'model-b'],
          model_configs: { 'model-b': { name: 'B' } },
          opencode_api_mode: 'chat_completions',
        }),
        initialTool: 'opencode' as TargetApp,
        onClose: () => {},
        onSave: async () => {},
      }),
    );
    expect((screen.getByLabelText('挂载模型 model-a') as HTMLInputElement).disabled).toBe(true);
    fireEvent.click(screen.getByLabelText('挂载模型 model-b'));
    // Unmount only: the configured card (and its delete path) must remain.
    expect(screen.getByLabelText('删除模型 model-b')).toBeTruthy();
    expect((screen.getByLabelText('挂载模型 model-b') as HTMLInputElement).checked).toBe(false);
    cleanup();
  });
});
