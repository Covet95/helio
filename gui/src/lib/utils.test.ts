import { describe, expect, it } from 'vitest';
import { errorDetail, humanizeError, isAppError } from './utils';

describe('isAppError', () => {
  it('recognizes structured backend errors', () => {
    expect(isAppError({ kind: 'not_found', message: '没有' })).toBe(true);
    // detail 是可选字段，缺失不影响判定
    expect(isAppError({ kind: 'io', message: '读写失败' })).toBe(true);
    expect(isAppError({ kind: 'io', message: '读写失败', detail: 'os error 13' })).toBe(true);
  });

  it('rejects everything that is not a structured error', () => {
    // 未迁移的命令仍返回字符串——不能被误判
    expect(isAppError('Profile id=42 不存在')).toBe(false);
    // 普通 Error 只有 message，没有 kind
    expect(isAppError(new Error('boom'))).toBe(false);
    expect(isAppError(null)).toBe(false);
    expect(isAppError(undefined)).toBe(false);
    expect(isAppError(42)).toBe(false);
    // 形状残缺的对象
    expect(isAppError({ message: '只有 message' })).toBe(false);
    expect(isAppError({ kind: 'not_found' })).toBe(false);
  });
});

describe('humanizeError', () => {
  it('uses the backend message verbatim for structured errors', () => {
    // 回归保护：这是整个结构化错误改造要解决的问题。
    // 旧实现会用 /不存在/ 命中并改写成「未找到对应数据,可能尚未初始化」,
    // 既丢掉 id,又给出错误的排查方向(实际是 profile 被删了)。
    expect(humanizeError({ kind: 'not_found', message: 'Profile id=42 不存在' })).toBe(
      'Profile id=42 不存在',
    );
    expect(humanizeError({ kind: 'conflict', message: 'Profile id=42 已经归属明确工具' })).toBe(
      'Profile id=42 已经归属明确工具',
    );
    // 英文文案也不应被正则二次加工
    expect(humanizeError({ kind: 'io', message: 'no such table: profiles' })).toBe(
      'no such table: profiles',
    );
  });

  it('falls back when a structured error carries an empty message', () => {
    expect(humanizeError({ kind: 'internal', message: '' })).toBe('发生未知错误');
    expect(humanizeError({ kind: 'internal', message: '' }, '操作失败')).toBe('操作失败');
  });

  it('keeps the legacy string path working for unmigrated commands', () => {
    expect(humanizeError(new Error('TypeError: unavailable'))).toBe('unavailable');
    expect(humanizeError('TypeError: nothing')).toBe('nothing');
    expect(humanizeError('')).toBe('发生未知错误');
    expect(humanizeError(new Error('not found'))).toBe('未找到对应数据,可能尚未初始化');
  });

  it('still detects a missing Tauri runtime', () => {
    expect(
      humanizeError(new Error("Cannot read properties of undefined (reading 'invoke')")),
    ).toBe('无法连接到桌面后端(请在 Helio 应用内打开,而非普通浏览器)');
  });
});

describe('errorDetail', () => {
  it('exposes the technical detail separately from the user message', () => {
    expect(errorDetail({ kind: 'io', message: '数据库操作失败', detail: 'no such table: x' })).toBe(
      'no such table: x',
    );
  });

  it('returns null when there is no usable detail', () => {
    expect(errorDetail({ kind: 'io', message: '数据库操作失败' })).toBeNull();
    expect(errorDetail({ kind: 'io', message: '数据库操作失败', detail: '   ' })).toBeNull();
    expect(errorDetail('普通字符串错误')).toBeNull();
    expect(errorDetail(new Error('boom'))).toBeNull();
  });
});
