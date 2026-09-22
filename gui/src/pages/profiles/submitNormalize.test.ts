/**
 * `submitNormalize` 的表驱动测试。
 *
 * 这段逻辑此前埋在 `ProfileFormModal.submit()` 里、零覆盖——而它是全应用
 * 最容易出错的地方：7 个工具共享一套表单，字段按 tool 条件保留/清空，
 * 凭据模式之间还有互斥规则。
 */
import { describe, expect, it } from 'vitest';
import type { ApiProfile, TargetApp } from '@/types';
import { ensureKeyPool, normalizeSubmit, normalizeWireApi, withActiveKey } from './submitNormalize';

/** 造一个各字段齐全的基础表单，测试只覆盖关心的部分。 */
function form(overrides: Partial<ApiProfile> = {}): ApiProfile {
  return {
    name: 'p',
    provider: 'custom',
    api_url: 'https://x.example/v1',
    api_key: 'sk-x',
    ...overrides,
  } as ApiProfile;
}

/** 断言归一化成功并返回结果，失败时带上错误信息。 */
function ok(result: ReturnType<typeof normalizeSubmit>) {
  if (!result.ok) throw new Error(`预期成功，实际失败：${result.error}`);
  return result.value;
}

describe('normalizeSubmit — 必填校验', () => {
  it('缺少名称 / Provider / URL / 凭据时报错', () => {
    expect(normalizeSubmit(form({ name: '  ' }), 'codex').ok).toBe(false);
    expect(normalizeSubmit(form({ provider: '' }), 'codex').ok).toBe(false);
    expect(normalizeSubmit(form({ api_url: '' }), 'codex').ok).toBe(false);
    expect(normalizeSubmit(form({ api_key: '' }), 'codex').ok).toBe(false);
  });

  it('Bedrock 不需要 URL 与 Key（走 AWS 凭据链）', () => {
    const value = ok(
      normalizeSubmit(form({ provider: 'amazon-bedrock', api_url: '', api_key: '' }), 'codex'),
    );
    expect(value.api_url).toBe('');
    expect(value.api_key).toBe('');
  });

  it('env_key / Bearer / auth_command 任一存在即可免填 api_key', () => {
    for (const extra of [
      { env_key: 'OPENAI_API_KEY' },
      { experimental_bearer_token: 'tok' },
      { auth_command: 'get-token' },
    ]) {
      expect(normalizeSubmit(form({ api_key: '', ...extra }), 'codex').ok).toBe(true);
    }
  });
});

describe('normalizeSubmit — 凭据模式互斥', () => {
  it('env_key 与 Bearer 同时存在时拒绝', () => {
    const result = normalizeSubmit(
      form({ env_key: 'ENV', experimental_bearer_token: 'tok' }),
      'codex',
    );
    expect(result.ok).toBe(false);
  });

  it('auth_command 优先：清掉 env_key / Bearer / requires_openai_auth', () => {
    const value = ok(
      normalizeSubmit(
        form({
          auth_command: '  get-token  ',
          requires_openai_auth: true,
        }),
        'codex',
      ),
    );
    expect(value.auth_command).toBe('get-token');
    expect(value.env_key).toBeUndefined();
    expect(value.experimental_bearer_token).toBeUndefined();
    expect(value.requires_openai_auth).toBeUndefined();
  });

  it('env_key 与 Bearer 并存时报错，即使同时填了 auth_command', () => {
    // 互斥检查在 auth 清理**之前**执行，所以填了 auth_command 也不能绕过。
    // 这是原有行为，此处钉住以免重构改变它。
    const result = normalizeSubmit(
      form({ auth_command: 'cmd', env_key: 'ENV', experimental_bearer_token: 'tok' }),
      'codex',
    );
    expect(result.ok).toBe(false);
  });

  it('auth_args 去空白并丢空项；全空则为 undefined', () => {
    const value = ok(
      normalizeSubmit(form({ auth_command: 'cmd', auth_args: [' a ', '', '  ', 'b'] }), 'codex'),
    );
    expect(value.auth_args).toEqual(['a', 'b']);

    const empty = ok(normalizeSubmit(form({ auth_command: 'cmd', auth_args: ['', ' '] }), 'codex'));
    expect(empty.auth_args).toBeUndefined();
  });

  it('auth 超时/间隔只接受正数', () => {
    const value = ok(
      normalizeSubmit(
        form({
          auth_command: 'cmd',
          auth_timeout_ms: 5000.9,
          auth_refresh_interval_ms: -1,
        }),
        'codex',
      ),
    );
    expect(value.auth_timeout_ms).toBe(5000);
    expect(value.auth_refresh_interval_ms).toBeUndefined();
  });

  it('Bedrock 清掉 key 池与 web search', () => {
    const value = ok(
      normalizeSubmit(
        form({
          provider: 'amazon-bedrock',
          api_keys: [{ id: 'k1', label: 'a', key: 'sk-1', is_active: true }],
          supports_standalone_web_search: true,
        }),
        'codex',
      ),
    );
    expect(value.api_keys).toBeUndefined();
    expect(value.supports_standalone_web_search).toBeUndefined();
  });
});

describe('normalizeSubmit — 按 tool 清理字段', () => {
  const TOOLS: TargetApp[] = ['claude-code', 'codex', 'pi', 'opencode', 'hermes', 'openclaw', 'zcode'];

  it('非 Codex 工具不保存 Codex 专属字段', () => {
    for (const tool of TOOLS.filter((t) => t !== 'codex')) {
      const value = ok(
        normalizeSubmit(form({ wire_api: 'responses', auth_command: 'cmd' }), tool),
      );
      expect(value.wire_api, tool).toBeUndefined();
      expect(value.auth_command, tool).toBeUndefined();
      expect(value.catalog_models, tool).toBeUndefined();
    }
  });

  it('非 Codex 工具会透传 env_key（**已知不一致**，行为已被钉住）', () => {
    // 这里记录一处**预先存在**的行为不一致，重构只是如实保留、未改变它：
    //
    //   - 本表单：非 codex 工具的 env_key 原样提交；
    //   - ImportPage：`env_key: tool === 'codex' ? ... : undefined`，做了门控；
    //   - 后端：只有 codex 适配器读 env_key，但 DB 里所有工具共用同一列。
    //
    // 后果：从表单保存一个 Claude 档案时，填过的 env_key 会存进 codex 专属列，
    // 而它对该工具毫无作用——属于脏数据。
    //
    // 修它需要改行为（前端门控或后端忽略），超出本次「纯提取」的范围，
    // 因此先钉住现状。若将来决定统一，把下面的断言改成 toBeUndefined 即可。
    for (const tool of TOOLS.filter((t) => t !== 'codex')) {
      const value = ok(normalizeSubmit(form({ env_key: 'ENV' }), tool));
      expect(value.env_key, `${tool} 当前会透传 env_key`).toBe('ENV');
    }
  });

  it('opencode 专属字段只对 opencode 保留', () => {
    const forOpenCode = ok(
      normalizeSubmit(
        form({ opencode_api_mode: '  chat_completions  ', model_configs: {} }),
        'opencode',
      ),
    );
    expect(forOpenCode.opencode_api_mode).toBe('chat_completions');

    const forOthers = ok(normalizeSubmit(form({ opencode_api_mode: 'chat_completions' }), 'pi'));
    expect(forOthers.opencode_api_mode).toBeUndefined();
    expect(forOthers.model_configs).toBeUndefined();
  });

  it('target_app 始终写成当前 tool', () => {
    for (const tool of TOOLS) {
      expect(ok(normalizeSubmit(form(), tool)).target_app, tool).toBe(tool);
    }
  });
});

describe('normalizeWireApi', () => {
  it('responses 系别名与 chat 系历史值都归一为 responses', () => {
    for (const input of [
      'responses',
      'openai-responses',
      'openai_responses',
      'codex_responses',
      'chat',
      'chat_completions',
      'openai-chat',
      '  RESPONSES  ',
    ]) {
      expect(normalizeWireApi(input, 'codex'), input).toBe('responses');
    }
  });

  it('未知值返回 undefined（不保存，后端默认 responses）', () => {
    expect(normalizeWireApi('bogus', 'codex')).toBeUndefined();
    expect(normalizeWireApi('', 'codex')).toBeUndefined();
    expect(normalizeWireApi(undefined, 'codex')).toBeUndefined();
  });

  it('非 Codex 工具一律 undefined', () => {
    expect(normalizeWireApi('responses', 'pi')).toBeUndefined();
  });
});

describe('key 池', () => {
  it('已有 key 池时原样复制（不共享引用）', () => {
    const original = [{ id: 'k1', label: 'a', key: 'sk-1', is_active: true }];
    const pool = ensureKeyPool(form({ api_keys: original }));
    expect(pool).toEqual(original);
    expect(pool[0]).not.toBe(original[0]);
  });

  it('无池但有单 key 时由它造一条', () => {
    const pool = ensureKeyPool(form({ api_key: 'sk-solo' }));
    expect(pool).toHaveLength(1);
    expect(pool[0].key).toBe('sk-solo');
    expect(pool[0].is_active).toBe(true);
  });

  it('都没有时给一条空 key 供填写', () => {
    const pool = ensureKeyPool(form({ api_key: '', api_keys: undefined }));
    expect(pool).toHaveLength(1);
    expect(pool[0].key).toBe('');
  });

  it('withActiveKey 把活跃 key 回填到 api_key', () => {
    const pool = [
      { id: 'k1', label: 'a', key: 'sk-a', is_active: false },
      { id: 'k2', label: 'b', key: 'sk-b', is_active: true },
    ];
    expect(withActiveKey(form(), pool).api_key).toBe('sk-b');
  });

  it('无活跃标记时取第一把', () => {
    const pool = [
      { id: 'k1', label: 'a', key: 'sk-a', is_active: false },
      { id: 'k2', label: 'b', key: 'sk-b', is_active: false },
    ];
    expect(withActiveKey(form(), pool).api_key).toBe('sk-a');
  });

  it('提交时 api_key 始终等于活跃 key（后端只读单值字段）', () => {
    const value = ok(
      normalizeSubmit(
        form({
          api_key: 'stale',
          api_keys: [{ id: 'k2', label: 'b', key: 'sk-active', is_active: true }],
        }),
        'codex',
      ),
    );
    expect(value.api_key).toBe('sk-active');
  });
});
