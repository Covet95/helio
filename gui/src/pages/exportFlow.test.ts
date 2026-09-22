/**
 * 传输流程的分支测试。
 *
 * 三条出口各有各的坑：取消时**不能**调用命令（否则会用空路径去写文件）、
 * 对话框抛错要算失败而不是崩溃、命令抛错要报错而不是静默。
 * 这些分支此前在 `ExportPage` 里重复了 6 遍且零覆盖。
 */
import { describe, expect, it, vi } from 'vitest';
import { runTransfer, type TransferDeps, type TransferSpec } from './exportFlow';

/** 造一组依赖桩，默认都成功。 */
function deps(overrides: Partial<TransferDeps> = {}): TransferDeps {
  return {
    save: vi.fn().mockResolvedValue('/tmp/out.db'),
    open: vi.fn().mockResolvedValue('/tmp/in.db'),
    humanize: (e: unknown) => `原因(${String(e)})`,
    ...overrides,
  };
}

/** 造一个导出 spec，默认执行成功。 */
function spec(overrides: Partial<TransferSpec> = {}): TransferSpec {
  return {
    mode: 'export',
    request: { defaultPath: 'a.db', filterName: 'Database', extensions: ['db'] },
    action: '导出',
    execute: vi.fn().mockResolvedValue({ text: '成功', kind: 'success' as const }),
    ...overrides,
  };
}

describe('runTransfer — 成功路径', () => {
  it('导出用 save 并把路径交给 execute', async () => {
    const d = deps();
    const execute = vi.fn().mockResolvedValue({ text: '成功', kind: 'success' as const });
    const result = await runTransfer(d, spec({ execute }));

    expect(d.save).toHaveBeenCalledOnce();
    expect(d.open).not.toHaveBeenCalled();
    expect(execute).toHaveBeenCalledWith('/tmp/out.db');
    expect(result).toEqual({ text: '成功', kind: 'success' });
  });

  it('导入用 open，不用 save', async () => {
    const d = deps();
    const execute = vi.fn().mockResolvedValue({ text: '恢复完成', kind: 'success' as const });
    const result = await runTransfer(d, spec({ mode: 'import', execute }));

    expect(d.open).toHaveBeenCalledOnce();
    expect(d.save).not.toHaveBeenCalled();
    expect(execute).toHaveBeenCalledWith('/tmp/in.db');
    expect(result.text).toBe('恢复完成');
  });

  it('请求参数原样透传给对话框', async () => {
    const d = deps();
    const request = {
      defaultPath: 'helio-portable-1.tar.gz',
      filterName: 'Helio 便携备份',
      extensions: ['tar.gz', 'tgz'],
    };
    await runTransfer(d, spec({ request }));
    expect(d.save).toHaveBeenCalledWith(request);
  });
});

describe('runTransfer — 取消', () => {
  it('用户取消时不执行命令（否则会拿空路径去写文件）', async () => {
    const d = deps({ save: vi.fn().mockResolvedValue(null) });
    const execute = vi.fn();
    const result = await runTransfer(d, spec({ execute }));

    expect(execute).not.toHaveBeenCalled();
    expect(result).toEqual({ text: '导出已取消', kind: 'info' });
  });

  it('导入取消时提示「导入已取消」', async () => {
    const d = deps({ open: vi.fn().mockResolvedValue(null) });
    const execute = vi.fn();
    const result = await runTransfer(d, spec({ mode: 'import', execute }));

    expect(execute).not.toHaveBeenCalled();
    expect(result).toEqual({ text: '导入已取消', kind: 'info' });
  });

  it('空字符串也算取消（对话框可能返回 ""）', async () => {
    const d = deps({ save: vi.fn().mockResolvedValue('') });
    const execute = vi.fn();
    const result = await runTransfer(d, spec({ execute }));

    expect(execute).not.toHaveBeenCalled();
    expect(result.kind).toBe('info');
  });
});

describe('runTransfer — 失败', () => {
  it('对话框本身抛错（插件加载失败）算失败，不崩', async () => {
    const d = deps({ save: vi.fn().mockRejectedValue(new Error('plugin missing')) });
    const execute = vi.fn();
    const result = await runTransfer(d, spec({ execute }));

    expect(execute).not.toHaveBeenCalled();
    expect(result.kind).toBe('error');
    expect(result.text).toBe('导出失败: 原因(Error: plugin missing)');
  });

  it('命令抛错时带上动作名前缀', async () => {
    const d = deps();
    const execute = vi.fn().mockRejectedValue(new Error('disk full'));
    const result = await runTransfer(d, spec({ action: '便携备份恢复', execute }));

    expect(result.kind).toBe('error');
    expect(result.text).toBe('便携备份恢复失败: 原因(Error: disk full)');
  });

  it('失败时 humanize 收到原始错误对象', async () => {
    const humanize = vi.fn((e: unknown) => `原因(${String(e)})`);
    const d = deps({ humanize });
    const boom = new Error('boom');
    await runTransfer(d, spec({ execute: vi.fn().mockRejectedValue(boom) }));
    expect(humanize).toHaveBeenCalledWith(boom);
  });

  it('execute 返回 error 反馈时原样传出（不算异常）', async () => {
    const d = deps();
    const execute = vi.fn().mockResolvedValue({ text: '未发现任何 Skills', kind: 'info' as const });
    const result = await runTransfer(d, spec({ execute }));
    expect(result).toEqual({ text: '未发现任何 Skills', kind: 'info' });
  });
});
