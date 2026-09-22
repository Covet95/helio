/**
 * 会话删除/清理的结果文案与输入校验。
 *
 * 从 `HistoryPage` 的三个 handler 里抽出来。删除是**不可逆的破坏性操作**
 * （虽然走系统垃圾桶），失败却只报了个「N/M 个」的话，用户不知道该去捞哪几个。
 * 这些分支此前零覆盖。
 */

/**
 * 后端返回的单条删除结果。
 *
 * `error` 用 `string | null`（Rust 的 `Option<String>` 序列化成 null），
 * 不是 `undefined`——写成可选属性会让类型和实际 JSON 对不上。
 */
export interface DeleteOutcome {
  ok: boolean;
  error?: string | null;
}

/** 清理天数校验：必须是正数。返回错误文案或 null。 */
export function validateCleanupDays(input: string): string | null {
  const days = Number(input);
  // Number('') === 0，Number('abc') === NaN，两者都该拒。
  if (!Number.isFinite(days) || days <= 0) return '请输入有效的天数';
  return null;
}

/**
 * 批量操作失败汇总。
 *
 * 全成功返回 null（调用方据此不显示错误条）；部分失败时逐条列出原因，
 * 让用户知道哪些没删掉。
 */
export function summarizeFailures(results: DeleteOutcome[], verb: string): string | null {
  const failed = results.filter((r) => !r.ok);
  if (failed.length === 0) return null;
  const reasons = failed.map((f) => f.error || '未知错误').join('；');
  return `${verb}失败 ${failed.length}/${results.length} 个：${reasons}`;
}

/** 单条删除失败文案。 */
export function singleDeleteFailure(error?: string | null): string {
  return `删除失败：${error || '未知错误'}`;
}
