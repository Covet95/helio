/**
 * 会话删除/清理文案的分支测试。
 *
 * 删除是不可逆的破坏性操作；失败汇总写得含糊，用户就不知道漏了哪几个。
 */
import { describe, expect, it } from 'vitest';
import { singleDeleteFailure, summarizeFailures, validateCleanupDays } from './historyMessages';

describe('validateCleanupDays', () => {
  it('正数通过', () => {
    expect(validateCleanupDays('30')).toBeNull();
    expect(validateCleanupDays('1')).toBeNull();
  });

  it('空串 / 0 / 负数 / 非数字都拒绝', () => {
    // Number('') === 0，容易被 `if (!days)` 之外的写法漏掉。
    expect(validateCleanupDays('')).toBe('请输入有效的天数');
    expect(validateCleanupDays('0')).toBe('请输入有效的天数');
    expect(validateCleanupDays('-5')).toBe('请输入有效的天数');
    expect(validateCleanupDays('abc')).toBe('请输入有效的天数');
  });

  it('Infinity 拒绝：否则 JSON 序列化成 null，用户看到的是后端反序列化报错', () => {
    // 后端 `cleanup_cutoff` 会挡住它（不构成数据风险），但报错信息是
    // 「invalid type: null, expected i64」——不如前端直接说清楚。
    expect(validateCleanupDays('Infinity')).toBe('请输入有效的天数');
  });
});

describe('summarizeFailures', () => {
  it('全部成功时返回 null（调用方据此不显示错误条）', () => {
    expect(summarizeFailures([{ ok: true }, { ok: true }], '批量删除')).toBeNull();
  });

  it('空结果不算失败', () => {
    expect(summarizeFailures([], '清理')).toBeNull();
  });

  it('部分失败时给出 N/M 与逐条原因', () => {
    const text = summarizeFailures(
      [{ ok: true }, { ok: false, error: '权限不足' }, { ok: false, error: '文件占用' }],
      '批量删除',
    );
    expect(text).toBe('批量删除失败 2/3 个：权限不足；文件占用');
  });

  it('原因为空时填「未知错误」，不留空洞', () => {
    expect(summarizeFailures([{ ok: false }], '清理')).toBe('清理失败 1/1 个：未知错误');
  });

  it('全部失败也如实报告', () => {
    expect(summarizeFailures([{ ok: false, error: 'x' }], '清理')).toContain('1/1');
  });
});

describe('singleDeleteFailure', () => {
  it('带上后端原因', () => {
    expect(singleDeleteFailure('文件被占用')).toBe('删除失败：文件被占用');
  });

  it('无原因时兜底', () => {
    expect(singleDeleteFailure(undefined)).toBe('删除失败：未知错误');
    expect(singleDeleteFailure('')).toBe('删除失败：未知错误');
  });
});
