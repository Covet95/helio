import { describe, expect, it } from 'vitest';
import { errorDetail, formatBackupTime, humanizeError, isAppError, maskApiKey } from './utils';

describe('isAppError', () => {
  it('recognizes structured backend errors', () => {
    expect(isAppError({ kind: 'not_found', message: '没有' })).toBe(true);
    // detail 是可选字段，缺失不影响判定
    expect(isAppError({ kind: 'io', message: '读写失败' })).toBe(true);
    expect(isAppError({ kind: 'io', message: '读写失败', detail: 'os error 13' })).toBe(true);
  });

  it('rejects everything that is not a structured error', () => {
    // 字符串不是结构化错误——即便现在所有命令都返回对象，判据也不能放宽，
    // 否则 Tauri 自身抛的字符串（如命令 panic）会被误判成 AppError。
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
    // 英文文案也不应被正则二次加工（这三条正则已删除，见下一条用例）
    expect(humanizeError({ kind: 'io', message: 'no such table: profiles' })).toBe(
      'no such table: profiles',
    );
  });

  it('appends the technical detail so the root cause is never hidden', () => {
    // detail 只含 anyhow 链上 message 之外的部分，因此拼接后不重复。
    // 这条断言的意义：后端把根因放进 detail 时，前端必须真的显示它，
    // 否则等于把失败原因藏起来。
    expect(
      humanizeError({
        kind: 'io',
        message: '加载 Profile 列表失败',
        detail: 'no such table: profiles',
      }),
    ).toBe('加载 Profile 列表失败：no such table: profiles');
  });

  it('falls back when a structured error carries an empty message', () => {
    expect(humanizeError({ kind: 'internal', message: '' })).toBe('发生未知错误');
    expect(humanizeError({ kind: 'internal', message: '' }, '操作失败')).toBe('操作失败');
  });

  it('no longer rewrites messages by matching their text', () => {
    // 回归保护：被删掉的三条正则会把「not found」改写成
    // 「未找到对应数据,可能尚未初始化」、把「permission denied」改写成
    // 「权限不足,无法访问该资源」。现在所有 #[tauri::command] 都返回 AppError，
    // 后端文案本身就是中文且准确，这类改写只会把准确信息改成错误信息。
    expect(humanizeError(new Error('not found'))).toBe('not found');
    expect(humanizeError(new Error('permission denied'))).toBe('permission denied');
    expect(humanizeError('EACCES')).toBe('EACCES');
  });

  it('keeps the plain-string path for non-backend errors', () => {
    expect(humanizeError(new Error('TypeError: unavailable'))).toBe('unavailable');
    expect(humanizeError('TypeError: nothing')).toBe('nothing');
    expect(humanizeError('')).toBe('发生未知错误');
  });

  it('stringifies non-Error objects instead of showing [object Object]', () => {
    expect(humanizeError({ foo: 1 } as unknown as Error)).toBe('{"foo":1}');
    expect(humanizeError(null as unknown as Error)).toBe('null');
    expect(humanizeError(undefined as unknown as Error)).toBe('发生未知错误');
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

describe('formatBackupTime', () => {
  it('把后端时间戳转成可读时间', () => {
    expect(formatBackupTime('20260101_120000_000000')).toBe('2026-01-01 12:00:00');
    expect(formatBackupTime('20261231_235959_123456')).toBe('2026-12-31 23:59:59');
  });

  it('格式意外时原样返回（展示层不该因它整页报错）', () => {
    expect(formatBackupTime('weird-name')).toBe('weird-name');
    expect(formatBackupTime('')).toBe('');
  });
});

describe('maskApiKey', () => {
  it('短 key 整个遮掉（不留任何字符）', () => {
    expect(maskApiKey('sk-short')).toBe('***');
    expect(maskApiKey('')).toBe('***');
  });

  it('长 key 保留前后片段', () => {
    expect(maskApiKey('sk-1234567890abcdefghij')).toBe('sk-1234567...fghij');
  });

  it('多字节字符不从中间切开（否则会留下孤立代理项显示成 �）', () => {
    // emoji 占两个 UTF-16 码元，按 slice 切会切出半个
    const key = 'sk-' + '🔑'.repeat(8) + '-abcdefghij';
    const masked = maskApiKey(key);
    expect(masked).not.toContain('\uFFFD');
    expect(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(masked)).toBe(false);
  });

  it('按码点计数：15 个 emoji 不算「短」', () => {
    // 15 个 emoji = 30 个 UTF-16 码元，按码点算正好是阈值
    expect(maskApiKey('🔑'.repeat(15))).toBe('***');
    expect(maskApiKey('🔑'.repeat(16))).not.toBe('***');
  });
});
