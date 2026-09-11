//! 统一结构化错误类型（核心 crate 唯一来源）。
//!
//! # 为什么放在核心 crate
//!
//! 错误的**语义**是在这一层产生的，不是在命令层。例如
//! `Database::assign_legacy_profile` 会区分三种失败：
//!
//! - `Profile id=42 不存在` —— 资源不存在
//! - `Profile id=42 已经归属明确工具` —— 与当前状态冲突
//! - `目标工具 codex 已存在同名 Profile: foo` —— 与当前状态冲突
//!
//! 这些信息在 `db` 层是已知的，一旦退化成 `anyhow::Error` 就只剩一段文本，
//! 上层再也无法恢复。若把错误类型放在 `src-tauri`，命令层就只能靠
//! `message.contains("不存在")` 反推类别——那正是前端 `humanizeError`
//! 已经在犯的错，只是换个地方犯。因此类型定义在核心 crate，命令层只做映射。
//!
//! # 迁移是分批的
//!
//! `AppError` 序列化为 `{ kind, message, detail? }` 对象，而尚未迁移的命令
//! 仍返回字符串。前端 `toUserMessage()` 同时接受两种形状，因此新旧命令可以
//! 长期共存，迁移不必一次性完成。新增代码请直接返回 `Result<T, AppError>`。

use serde::Serialize;

/// 错误类别。
///
/// 前端应据此分支，**不要**依赖 `message` 文案（文案随时可能调整）。
/// 该枚举与 `gui/src/types/index.ts` 的 `ErrorKind` 联合类型由
/// `tests/frontend_types_sync.rs` 守卫，改动时必须同步。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 目标资源不存在（Profile / 文件 / provider / 备份）
    NotFound,
    /// 调用方传参不合法
    InvalidInput,
    /// 文件系统权限不足
    Permission,
    /// 与当前状态冲突（如编辑时试图改目标工具）
    Conflict,
    /// 磁盘 / 数据库读写失败
    Io,
    /// 操作已部分生效后失败；已完成的部分已尝试补偿回滚
    PartialFailure,
    /// 其它内部错误（含锁中毒）
    Internal,
}

/// 结构化错误。
#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub kind: ErrorKind,
    /// 给用户看的中文文案，**不含**技术细节
    pub message: String,
    /// 原始技术信息（anyhow 链 / sqlite 报错 / 路径），供「详情」折叠展示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AppError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Permission, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Io, message)
    }

    pub fn partial_failure(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::PartialFailure, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// 附加原始技术细节。空字符串会被忽略，避免 `detail: Some("")` 这种噪音。
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.trim().is_empty() {
            self.detail = Some(detail);
        }
        self
    }

    /// 类别（便于测试与调用方分支）。
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

/// `TargetApp::parse` 失败时的统一错误。
///
/// 迁移前 `format!("Unknown target app: {target_app}")` 在命令层重复了 10 次，
/// 且是英文文案。新代码请一律走这里。
pub fn unknown_target_app(raw: &str) -> AppError {
    AppError::invalid_input(format!("未知的工具标识：{raw}"))
        .with_detail(format!("TargetApp::parse({raw:?}) 返回 None"))
}

// ---------------------------------------------------------------- From 实现
//
// 有了这些实现，业务代码里的 `xxx.map_err(|e| e.to_string())?` 可以直接简化为
// `xxx?`——迁移时顺带消掉大量样板代码。

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::io("数据库操作失败").with_detail(e.to_string())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        // 用 anyhow 自己的最外层 context 当 message，而不是固定文案「操作失败」：
        // 核心层写的提示往往已经是准确的中文（如 `Profile id=42 不存在`），
        // 覆盖掉会让用户只看到一句空话、真信息被折进 detail。
        // `{e:#}` 展开整条 context 链；无额外链时与 `{e}` 相同，此时不重复填 detail。
        let full = format!("{e:#}");
        let err = AppError::internal(format!("{e}"));
        if full == err.message {
            err
        } else {
            err.with_detail(full)
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        let permission_denied = e.kind() == std::io::ErrorKind::PermissionDenied;
        let message = if permission_denied {
            "权限不足，无法访问该文件"
        } else {
            "读写文件失败"
        };
        AppError::new(
            if permission_denied {
                ErrorKind::Permission
            } else {
                ErrorKind::Io
            },
            message,
        )
        .with_detail(e.to_string())
    }
}

/// `Mutex::lock()` 失败 = 之前有线程持锁时 panic（锁中毒）。这是内部错误，
/// 但必须给用户一句能行动的话，而不是 `poisoned lock: another task failed inside`。
impl<T> From<std::sync::PoisonError<T>> for AppError {
    fn from(e: std::sync::PoisonError<T>) -> Self {
        AppError::internal("内部状态锁不可用，请重启应用").with_detail(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::invalid_input("配置内容不是合法的 JSON").with_detail(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_json(err: &AppError) -> serde_json::Value {
        serde_json::to_value(err).expect("AppError 必须可序列化")
    }

    #[test]
    fn serializes_kind_as_snake_case_with_message() {
        let v = to_json(&AppError::not_found("Profile 不存在"));
        assert_eq!(v["kind"], "not_found");
        assert_eq!(v["message"], "Profile 不存在");
        // 无 detail 时该键必须整个消失，而不是变成 null
        assert!(v.get("detail").is_none());
    }

    #[test]
    fn every_kind_has_a_stable_wire_value() {
        // 这些取值与 gui/src/types/index.ts 的 ErrorKind 联合类型一一对应，
        // 由 tests/frontend_types_sync.rs 守卫；此处再钉一遍以防 rename_all 被改。
        let cases = [
            (ErrorKind::NotFound, "not_found"),
            (ErrorKind::InvalidInput, "invalid_input"),
            (ErrorKind::Permission, "permission"),
            (ErrorKind::Conflict, "conflict"),
            (ErrorKind::Io, "io"),
            (ErrorKind::PartialFailure, "partial_failure"),
            (ErrorKind::Internal, "internal"),
        ];
        for (kind, wire) in cases {
            let v = to_json(&AppError::new(kind, "x"));
            assert_eq!(v["kind"], wire, "ErrorKind::{kind:?} 的线上取值变了");
        }
    }

    #[test]
    fn with_detail_keeps_the_technical_text_separate() {
        let v = to_json(&AppError::io("数据库操作失败").with_detail("no such table: profiles"));
        assert_eq!(v["message"], "数据库操作失败");
        assert_eq!(v["detail"], "no such table: profiles");
    }

    #[test]
    fn with_detail_ignores_blank_detail() {
        for blank in ["", "   ", "\n"] {
            let err = AppError::internal("x").with_detail(blank);
            assert_eq!(err.detail, None, "空白 detail 不应被保留");
        }
    }

    #[test]
    fn display_shows_the_user_facing_message_not_the_detail() {
        let err = AppError::io("读写文件失败").with_detail("Permission denied (os error 13)");
        assert_eq!(err.to_string(), "读写文件失败");
    }

    #[test]
    fn sqlite_errors_become_io_with_detail() {
        let sqlite_err = rusqlite::Error::QueryReturnedNoRows;
        let err = AppError::from(sqlite_err);
        assert_eq!(err.kind, ErrorKind::Io);
        assert!(err.detail.is_some(), "原始 sqlite 报错必须进 detail");
    }

    #[test]
    fn permission_denied_is_distinguished_from_other_io() {
        let denied = AppError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(denied.kind, ErrorKind::Permission);

        let missing = AppError::from(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(missing.kind, ErrorKind::Io);
    }

    #[test]
    fn poisoned_lock_becomes_internal_with_actionable_message() {
        let err = AppError::from(std::sync::PoisonError::new(()));
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(
            err.message.contains("重启"),
            "锁中毒要给用户可行动的提示，实际为：{}",
            err.message
        );
    }

    #[test]
    fn unknown_target_app_is_invalid_input_and_keeps_the_raw_value() {
        let err = unknown_target_app("gemini");
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(err.message.contains("gemini"), "文案要带上原始输入");
        assert!(
            err.detail.as_deref().unwrap_or("").contains("gemini"),
            "detail 也要带上原始输入"
        );
    }

    #[test]
    fn anyhow_keeps_the_original_message_visible_instead_of_hiding_it() {
        // 回归保护：曾经这里写死 "操作失败"，导致核心层写好的
        // `Profile id=42 不存在` 被折进 detail，用户在界面上只看到一句空话。
        let err = AppError::from(anyhow::anyhow!("Profile id=42 不存在"));
        assert_eq!(err.message, "Profile id=42 不存在");
        assert_eq!(
            err.kind,
            ErrorKind::Internal,
            "未分类的 anyhow 只能是 Internal"
        );
        assert_eq!(err.detail, None, "无 context 链时不应重复填 detail");
    }

    #[test]
    fn anyhow_context_chain_goes_to_detail() {
        use anyhow::Context;
        let err = AppError::from(
            Err::<(), _>(anyhow::anyhow!("no such table: profiles"))
                .context("加载 Profile 列表失败")
                .unwrap_err(),
        );
        assert_eq!(
            err.message, "加载 Profile 列表失败",
            "最外层 context 作 message"
        );
        let detail = err.detail.expect("context 链必须进 detail");
        assert!(
            detail.contains("no such table: profiles"),
            "detail 要保留根因，实际为：{detail}"
        );
    }
}
