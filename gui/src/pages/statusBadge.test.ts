/**
 * 徽标推导的分支测试。
 *
 * 六条分支且**顺序即优先级**——顺序错了会把「不可达」显示成「已配置」，
 * 用户据此以为一切正常。这里逐条钉住，并显式验证优先级。
 */
import { describe, expect, it } from 'vitest';
import type { ToolProbeResult } from '@/types';
import { toolBadge } from './statusBadge';

/** 造一个探测结果，默认「可达」。 */
function probe(overrides: Partial<ToolProbeResult> = {}): ToolProbeResult {
  return { target_app: 'codex', configured: true, ok: true, probed_at: 0, ...overrides };
}

describe('toolBadge — 无探测结果', () => {
  it('已配置 / 未设置', () => {
    expect(toolBadge(true).text).toBe('已配置');
    expect(toolBadge(false).text).toBe('未设置');
    expect(toolBadge(false).dotClass).toContain('ink-faint');
  });
});

describe('toolBadge — 探测结果优先', () => {
  it('可达时带延迟', () => {
    const b = toolBadge(true, probe({ latency_ms: 120 }));
    expect(b.text).toBe('可达 120ms');
    expect(b.textClass).toBe('text-ok');
  });

  it('可达但无延迟时不留空格', () => {
    expect(toolBadge(true, probe()).text).toBe('可达');
  });

  it('latency_ms 为 0 也算有值（0ms 是合法的快，不是「没有」）', () => {
    expect(toolBadge(true, probe({ latency_ms: 0 })).text).toBe('可达 0ms');
  });

  it('degraded 显示较慢并带 warn 色', () => {
    const b = toolBadge(true, probe({ status: 'degraded', latency_ms: 900 }));
    expect(b.text).toBe('较慢 900ms');
    expect(b.textClass).toBe('text-warn');
  });

  it('不可达时用 danger 色', () => {
    const b = toolBadge(true, probe({ ok: false, configured: true }));
    expect(b.text).toBe('不可达');
    expect(b.textClass).toBe('text-danger');
  });

  it('未配置且探测失败时保持「未设置」，不谎报不可达', () => {
    const b = toolBadge(false, probe({ ok: false, configured: false }));
    expect(b.text).toBe('未设置');
    expect(b.textClass).toBe('text-ink-faint');
  });
});

describe('toolBadge — 优先级', () => {
  it('managed 压过一切：即便 ok=false 也算正常', () => {
    const b = toolBadge(true, probe({ managed: true, ok: false, status: 'failed' }));
    expect(b.text).toBe('工具托管');
    expect(b.textClass).toBe('text-ok');
  });

  it('managed 压过 degraded（否则会误报较慢）', () => {
    expect(toolBadge(true, probe({ managed: true, status: 'degraded' })).text).toBe('工具托管');
  });

  it('degraded 压过普通可达', () => {
    expect(toolBadge(true, probe({ status: 'degraded' })).text).toContain('较慢');
  });

  it('探测结果压过本地档案状态', () => {
    // 档案说「已配置」，但刚测出来不可达——以探测为准。
    expect(toolBadge(true, probe({ ok: false })).text).toBe('不可达');
  });
});
