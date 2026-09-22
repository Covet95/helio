/**
 * 导出/导入结果的提示文案。
 *
 * 从 `ExportPage` 的 6 个 handler 里抽出来。原先每个 handler 都内联拼字符串，
 * 而这些字符串里藏着真实分支逻辑（数量为 0、跳过同名、部分恢复），
 * 埋在组件里只能靠渲染测试碰运气。
 *
 * 抽成纯函数后可以逐分支单测——它们同时是给用户看的文案，出错就是误导。
 */

/** 页面底部的反馈条内容。 */
export interface Feedback {
  text: string;
  kind: 'success' | 'error' | 'info';
}

/** 取消操作时的统一提示。 */
export function cancelledMessage(action: '导出' | '导入'): string {
  return `${action}已取消`;
}

/** 数据库导出。 */
export function databaseExportMessage(): string {
  return '数据库导出成功';
}

/** 数据库导入（成功后会自动刷新应用数据）。 */
export function databaseImportMessage(): string {
  return '数据库导入成功，正在刷新…';
}

/** 便携备份导出：附带 Skills 总数。 */
export function portableExportMessage(skillsTotal: number): string {
  return `便携备份导出成功：Skills ${skillsTotal} 个`;
}

/**
 * 便携备份恢复：拼接已恢复工具数与被跳过的同名 Skills。
 *
 * 两段都是可选的——全为 0 时只报 Skills 恢复数，不留空尾巴。
 */
export function portableImportMessage(result: {
  skills: { restored: number; skipped: number };
  restored_targets: unknown[];
}): string {
  const targets =
    result.restored_targets.length > 0
      ? `，已恢复 ${result.restored_targets.length} 个工具配置`
      : '';
  const skipped = result.skills.skipped > 0 ? `，跳过同名 Skills ${result.skills.skipped} 个` : '';
  return `便携备份恢复完成：Skills ${result.skills.restored} 个${skipped}${targets}`;
}

/**
 * Skills 导出：按应用分组列出数量；一个都没有时改用 info 语气。
 *
 * 返回文案与语义一并给出——调用方不该自己判断「有没有」。
 */
export function skillsExportMessage(result: {
  total: number;
  apps: Array<{ app: string; count: number }>;
}): { text: string; kind: 'success' | 'info' } {
  if (result.total === 0) {
    return { text: '未发现任何 Skills', kind: 'info' };
  }
  const breakdown = result.apps.map((a) => `${a.app} ${a.count}`).join('、');
  return { text: `Skills 导出成功：共 ${result.total} 个（${breakdown}）`, kind: 'success' };
}

/**
 * Skills 导入：有同名跳过时列出名字，否则只报恢复数。
 *
 * 列出被跳过的名字很重要——用户需要知道哪些没进来，才能决定是否手动处理。
 */
export function skillsImportMessage(result: {
  restored: number;
  skipped: number;
  skipped_names: string[];
}): string {
  if (result.skipped > 0) {
    return `Skills 导入完成：恢复 ${result.restored} 个，跳过同名 ${result.skipped} 个（${result.skipped_names.join('、')}）`;
  }
  return `Skills 导入完成：恢复 ${result.restored} 个`;
}

/** 失败提示：统一「<动作>失败: <原因>」形状。 */
export function failureMessage(action: string, error: unknown, humanize: (e: unknown) => string) {
  return `${action}失败: ${humanize(error)}`;
}
