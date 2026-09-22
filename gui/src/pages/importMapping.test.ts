/**
 * 本机导入字段映射的表驱动测试。
 *
 * 这是全库**第二个**「按 tool 门控字段」的地方（另一个是
 * `profiles/submitNormalize.ts`）。两处规则不一致，且此前 ImportPage 只有
 * 一个「选择器存在」断言——30 个字段的映射完全没有测试。
 *
 * 这里逐字段钉住 ImportPage 的**现有**行为；与 submitNormalize 的分歧
 * 单独成组，标注出来等产品决策，不在重构里顺手改。
 */
import { describe, expect, it } from 'vitest';
import type { TargetApp } from '@/types';
import { buildImportPayload, friendlyImportError, type Scanned } from './importMapping';

/** 造一份「每个字段都有值」的扫描结果。 */
function scanned(overrides: Partial<Scanned> = {}): Scanned {
  return {
    found: true,
    api_url: 'https://x.example/v1',
    api_key: 'sk-x',
    provider: 'custom',
    model: 'gpt-5',
    model_mapping: { a: 'b' },
    reasoning_effort: 'medium',
    reasoning_summary: 'auto',
    verbosity: 'low',
    context_1m: true,
    wire_api: 'responses',
    env_key: 'OPENAI_API_KEY',
    requires_openai_auth: true,
    experimental_bearer_token: 'tok',
    service_tier: 'flex',
    supports_standalone_web_search: true,
    aws_profile: 'default',
    aws_region: 'us-east-1',
    auth_command: 'get-token',
    auth_args: ['--x'],
    auth_timeout_ms: 5000,
    auth_refresh_interval_ms: 1000,
    auth_cwd: '/tmp',
    api_mode: 'chat',
    opencode_api_mode: 'chat_completions',
    opencode_models: ['m1'],
    opencode_model_configs: { m1: { max_tokens: 100 } },
    max_tokens: 4096,
    source: 'codex config',
    ...overrides,
  };
}

const ALL_TOOLS: TargetApp[] = [
  'claude-code', 'codex', 'pi', 'opencode', 'hermes', 'openclaw', 'zcode',
];

describe('buildImportPayload — 全工具透传字段', () => {
  const SHARED = [
    'provider', 'api_url', 'api_key', 'model', 'model_mapping',
    'reasoning_effort', 'context_1m', 'service_tier',
    'supports_standalone_web_search', 'aws_profile', 'aws_region',
  ] as const;

  it('每个工具都保留这些字段', () => {
    for (const tool of ALL_TOOLS) {
      const p = buildImportPayload(scanned(), tool, 'n');
      for (const field of SHARED) {
        expect(p[field], `${tool}.${field}`).toEqual(scanned()[field]);
      }
    }
  });

  it('name 与 target_app 始终写入', () => {
    for (const tool of ALL_TOOLS) {
      const p = buildImportPayload(scanned(), tool, '我的档案');
      expect(p.name).toBe('我的档案');
      expect(p.target_app).toBe(tool);
    }
  });
});

describe('buildImportPayload — Codex 专属字段', () => {
  const CODEX_ONLY = [
    'reasoning_summary', 'verbosity', 'env_key', 'wire_api',
    'requires_openai_auth', 'experimental_bearer_token',
    'auth_command', 'auth_args', 'auth_timeout_ms',
    'auth_refresh_interval_ms', 'auth_cwd',
  ] as const;

  it('codex 全部保留', () => {
    const p = buildImportPayload(scanned(), 'codex', 'n');
    for (const field of CODEX_ONLY) {
      expect(p[field], field).toEqual(scanned()[field]);
    }
  });

  it('其余 6 个工具一律清空', () => {
    for (const tool of ALL_TOOLS.filter((t) => t !== 'codex')) {
      const p = buildImportPayload(scanned(), tool, 'n');
      for (const field of CODEX_ONLY) {
        expect(p[field], `${tool}.${field} 应为 undefined`).toBeUndefined();
      }
    }
  });
});

describe('buildImportPayload — 工具专属字段', () => {
  it('api_mode 只给 hermes / openclaw', () => {
    for (const tool of ALL_TOOLS) {
      const p = buildImportPayload(scanned(), tool, 'n');
      const expected = tool === 'hermes' || tool === 'openclaw' ? 'chat' : undefined;
      expect(p.api_mode, tool).toBe(expected);
    }
  });

  it('opencode 专属字段只给 opencode', () => {
    const forOpenCode = buildImportPayload(scanned(), 'opencode', 'n');
    expect(forOpenCode.opencode_api_mode).toBe('chat_completions');
    expect(forOpenCode.opencode_model_configs).toEqual({ m1: { max_tokens: 100 } });
    // 档案字段名是 models，扫描结果叫 opencode_models
    expect(forOpenCode.models).toEqual(['m1']);

    for (const tool of ALL_TOOLS.filter((t) => t !== 'opencode')) {
      const p = buildImportPayload(scanned(), tool, 'n');
      expect(p.opencode_api_mode, tool).toBeUndefined();
      expect(p.opencode_model_configs, tool).toBeUndefined();
      expect(p.models, tool).toBeUndefined();
    }
  });

  it('max_tokens 只给 openclaw', () => {
    for (const tool of ALL_TOOLS) {
      const p = buildImportPayload(scanned(), tool, 'n');
      expect(p.max_tokens, tool).toBe(tool === 'openclaw' ? 4096 : undefined);
    }
  });

  it('字段全缺失时输出全是 undefined，不崩', () => {
    for (const tool of ALL_TOOLS) {
      const p = buildImportPayload({ found: false, api_url: '', api_key: '', provider: '', source: 's' }, tool, 'n');
      expect(p.name).toBe('n');
      expect(p.model).toBeUndefined();
      expect(p.api_mode).toBeUndefined();
      expect(p.auth_command).toBeUndefined();
    }
  });
});

describe('与 submitNormalize 的分歧（待产品决策，此处只记录现状）', () => {
  it('分歧 1：supports_standalone_web_search 在导入路径不做 Bedrock 门控', () => {
    // submitNormalize 对 amazon-bedrock 会清掉它（Bedrock 走 AWS 凭据链）；
    // ImportPage 直接透传。两条路径写同一列，结果不一致。
    const p = buildImportPayload(
      scanned({ provider: 'amazon-bedrock' }),
      'codex',
      'n',
    );
    expect(p.supports_standalone_web_search).toBe(true);
  });

  it('分歧 2：导入路径不校验凭据模式互斥', () => {
    // submitNormalize 拒绝 env_key 与 Bearer 并存；导入路径原样写入。
    // 数据来自本机配置文件，来源可信度高于手填，故不视为漏洞——
    // 但两条路径的约束不同，需要明确谁是对的。
    const p = buildImportPayload(
      scanned({ env_key: 'ENV', experimental_bearer_token: 'tok' }),
      'codex',
      'n',
    );
    expect(p.env_key).toBe('ENV');
    expect(p.experimental_bearer_token).toBe('tok');
  });

  it('分歧 3：导入路径不做 wire_api 归一', () => {
    // submitNormalize 把 chat 系历史值归一为 responses；导入原样保留。
    const p = buildImportPayload(scanned({ wire_api: 'chat' }), 'codex', 'n');
    expect(p.wire_api).toBe('chat');
  });
});

describe('friendlyImportError', () => {
  it('唯一约束冲突时给出可照做的提示', () => {
    const text = friendlyImportError(
      'error: UNIQUE constraint failed: profiles.name',
      '我的档案',
      '原始错误',
    );
    expect(text).toBe('已存在同名档案「我的档案」，请改个名字再导入');
  });

  it('其它错误走 humanize 结果', () => {
    expect(friendlyImportError('disk full', 'n', '磁盘已满')).toBe('导入失败: 磁盘已满');
  });

  it('大小写不敏感（后端措辞可能变）', () => {
    expect(friendlyImportError('unique constraint failed', 'n', 'x')).toContain('已存在同名档案');
  });
});
