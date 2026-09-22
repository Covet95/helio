/**
 * 导出/导入提示文案的分支测试。
 *
 * 这些文案里藏着真实逻辑（数量为 0、跳过同名、部分恢复），此前埋在
 * `ExportPage` 的 6 个 handler 里、零覆盖。
 */
import { describe, expect, it } from 'vitest';
import {
  cancelledMessage,
  databaseExportMessage,
  databaseImportMessage,
  failureMessage,
  portableExportMessage,
  portableImportMessage,
  skillsExportMessage,
  skillsImportMessage,
} from './exportMessages';

describe('取消与简单结果', () => {
  it('取消提示区分导出/导入', () => {
    expect(cancelledMessage('导出')).toBe('导出已取消');
    expect(cancelledMessage('导入')).toBe('导入已取消');
  });

  it('数据库导入提示包含刷新说明', () => {
    expect(databaseImportMessage()).toContain('刷新');
    expect(databaseExportMessage()).toBe('数据库导出成功');
  });
});

describe('便携备份', () => {
  it('导出提示带上 Skills 总数', () => {
    expect(portableExportMessage(0)).toBe('便携备份导出成功：Skills 0 个');
    expect(portableExportMessage(7)).toContain('7 个');
  });

  it('恢复提示在两段都为 0 时不留空尾巴', () => {
    const text = portableImportMessage({
      skills: { restored: 3, skipped: 0 },
      restored_targets: [],
    });
    expect(text).toBe('便携备份恢复完成：Skills 3 个');
  });

  it('恢复提示在有跳过时列出数量', () => {
    const text = portableImportMessage({
      skills: { restored: 3, skipped: 2 },
      restored_targets: [],
    });
    expect(text).toContain('跳过同名 Skills 2 个');
  });

  it('恢复提示在有工具配置时列出数量', () => {
    const text = portableImportMessage({
      skills: { restored: 1, skipped: 0 },
      restored_targets: [{}, {}, {}],
    });
    expect(text).toContain('已恢复 3 个工具配置');
  });

  it('两段都有时都出现，且顺序为 Skills → 跳过 → 工具', () => {
    const text = portableImportMessage({
      skills: { restored: 5, skipped: 1 },
      restored_targets: [{}, {}],
    });
    expect(text).toBe('便携备份恢复完成：Skills 5 个，跳过同名 Skills 1 个，已恢复 2 个工具配置');
  });
});

describe('Skills 导出', () => {
  it('一个都没有时用 info 语气，不谎报成功', () => {
    const result = skillsExportMessage({ total: 0, apps: [] });
    expect(result.kind).toBe('info');
    expect(result.text).toBe('未发现任何 Skills');
  });

  it('有内容时按应用分组列出', () => {
    const result = skillsExportMessage({
      total: 5,
      apps: [
        { app: 'claude-code', count: 3 },
        { app: 'codex', count: 2 },
      ],
    });
    expect(result.kind).toBe('success');
    expect(result.text).toBe('Skills 导出成功：共 5 个（claude-code 3、codex 2）');
  });

  it('单个应用时不出现多余分隔符', () => {
    const result = skillsExportMessage({ total: 2, apps: [{ app: 'pi', count: 2 }] });
    expect(result.text).toBe('Skills 导出成功：共 2 个（pi 2）');
  });
});

describe('Skills 导入', () => {
  it('无跳过时只报恢复数', () => {
    expect(skillsImportMessage({ restored: 4, skipped: 0, skipped_names: [] })).toBe(
      'Skills 导入完成：恢复 4 个',
    );
  });

  it('有跳过时列出被跳过的名字（用户需要知道哪些没进来）', () => {
    const text = skillsImportMessage({
      restored: 2,
      skipped: 2,
      skipped_names: ['alpha', 'beta'],
    });
    expect(text).toContain('跳过同名 2 个');
    expect(text).toContain('alpha、beta');
  });
});

describe('失败提示', () => {
  it('统一为「<动作>失败: <原因>」', () => {
    const humanize = (e: unknown) => `原因(${String(e)})`;
    expect(failureMessage('导出', 'boom', humanize)).toBe('导出失败: 原因(boom)');
    expect(failureMessage('便携备份恢复', new Error('x'), humanize)).toBe(
      '便携备份恢复失败: 原因(Error: x)',
    );
  });
});
