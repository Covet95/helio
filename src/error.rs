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
    /// 磁盘 / 数据库 / 网络等 I/O 失败
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

    /// 在 message 前加一层中文上下文，保留原有 message 与 detail。
    ///
    /// 用于「上层知道自己在做什么、底层只知道自己失败了」的场景：
    /// 例如底层给出「读写文件失败」，上层补上「读取 config.toml 失败」，
    /// 用户才知道是哪个文件。
    ///
    /// 之所以用**前缀拼接**而不是替换：替换会把底层已有的信息挤进 detail，
    /// 一旦某处只显示 message 就会丢信息。宁可稍微啰嗦，也不要静默丢东西。
    pub fn with_context(mut self, context: impl AsRef<str>) -> Self {
        let context = context.as_ref();
        if !context.is_empty() {
            self.message = format!("{context}：{}", self.message);
        }
        self
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
        // message 取 anyhow 的**最外层** context，而不是固定文案「操作失败」：
        // 核心层写的提示往往已经是准确的中文（如 `Profile id=42 不存在`），
        // 覆盖掉会让用户只看到一句空话。
        //
        // detail 只放**链上剩余部分**（根因 + 中间层），不重复 message 那句。
        // 这样 message + detail 合起来正好是完整信息、且没有冗余——前端把两者
        // 一起显示时（见 gui/src/lib/utils.ts 的 humanizeError）用户能看到全貌，
        // 而不是像以前那样只知道「失败了」却不知道为什么。
        let mut chain = e.chain();
        let message = chain
            .next()
            .map(ToString::to_string)
            .unwrap_or_else(|| e.to_string());
        let rest: Vec<String> = chain.map(ToString::to_string).collect();

        let err = AppError::new(classify_anyhow_chain(&e), message);
        if rest.is_empty() {
            err
        } else {
            err.with_detail(rest.join(": "))
        }
    }
}

/// 沿 anyhow 的 context 链下钻，按**错误类型**判定类别。
///
/// 这是让 `?` 自动带上正确类别的关键：db / adapter 层大量返回
/// `anyhow::Result`，如果一律归为 `Internal`，前端就仍然无法区分
/// 「磁盘坏了」和「逻辑错了」。
///
/// 注意这里判的是**类型**而不是文案——按类型不会因为改措辞而失效，
/// 这正是 `humanizeError` 用正则猜文案那套做法要解决的问题。
fn classify_anyhow_chain(e: &anyhow::Error) -> ErrorKind {
    for cause in e.chain() {
        // 必须放在最前：回滚失败才是「部分生效」，比底层是 io 还是 sqlite 更重要。
        if cause.downcast_ref::<RollbackFailed>().is_some() {
            return ErrorKind::PartialFailure;
        }
        // reqwest 要排在 io 之前：reqwest::Error 内部常常再包一层 io::Error
        // （超时、连接被拒都是），按 io 判会把「网络不通」误报成「磁盘读写失败」。
        if cause.downcast_ref::<reqwest::Error>().is_some() {
            return ErrorKind::Io;
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return if io.kind() == std::io::ErrorKind::PermissionDenied {
                ErrorKind::Permission
            } else {
                ErrorKind::Io
            };
        }
        if cause.downcast_ref::<rusqlite::Error>().is_some() {
            return ErrorKind::Io;
        }
        if cause.downcast_ref::<serde_json::Error>().is_some() {
            return ErrorKind::InvalidInput;
        }
    }
    // 链上没有可识别的具体错误类型：诚实地标为未分类，而不是猜。
    ErrorKind::Internal
}

/// 「切换事务已部分生效，且补偿回滚也失败了」的标记错误。
///
/// 单独做成一个类型，是为了让 `classify_anyhow_chain` 能**按类型**识别出
/// `PartialFailure`，而不是去匹配 "rollback failed" 这段文案——文案会改，
/// 类型不会。这也是本模块反复强调的原则：类别来自类型，不来自措辞。
#[derive(Debug)]
pub struct RollbackFailed {
    message: String,
}

impl RollbackFailed {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RollbackFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RollbackFailed {}

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

/// 网络请求失败（超时 / 连接被拒 / TLS 握手失败等）。
///
/// 归入 `Io` 而不是新开一个 `Network` 变体：Rust 的 `std::io::Error` 本来就覆盖
/// socket 层，reqwest 的错误链底下也常常就是 io::Error；单开变体只会让前端多一个
/// 分支，却没有对应的不同处置方式。
///
/// 文案按 `reqwest::Error` 自带的分类给出——超时和连不上对用户是两件事：
/// 前者要调超时或换网络，后者要检查地址/代理。
impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        let message = if e.is_timeout() {
            "请求超时"
        } else if e.is_connect() {
            "无法连接到服务地址"
        } else {
            "网络请求失败"
        };
        AppError::io(message).with_detail(e.to_string())
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

    #[test]
    fn anyhow_chain_is_classified_by_error_type_not_by_text() {
        // 关键：类别来自错误的**类型**，不是文案。改措辞不会让它失效——
        // 这正是 humanizeError 用正则猜文案那套做法的问题所在。
        let missing = AppError::from(anyhow::Error::from(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        )));
        assert_eq!(missing.kind, ErrorKind::Io);

        let denied = AppError::from(anyhow::Error::from(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert_eq!(denied.kind, ErrorKind::Permission);

        let sqlite = AppError::from(anyhow::Error::from(rusqlite::Error::QueryReturnedNoRows));
        assert_eq!(sqlite.kind, ErrorKind::Io);
    }

    #[test]
    fn classification_looks_through_context_layers() {
        use anyhow::Context;
        // db / adapter 层的典型形态：具体错误外面包了好几层 context。
        // 如果不穿透，db 层的失败会全部退化成 Internal，类别就白加了。
        let err = AppError::from(
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                .context("写入配置失败")
                .context("切换 Profile 失败")
                .unwrap_err(),
        );
        assert_eq!(
            err.kind,
            ErrorKind::Permission,
            "必须穿透 context 链看到根因"
        );
        assert_eq!(
            err.message, "切换 Profile 失败",
            "最外层 context 仍然是给用户看的 message"
        );
    }

    #[test]
    fn unclassifiable_anyhow_stays_internal_rather_than_guessing() {
        // 链上没有可识别的具体类型时，诚实标为未分类，而不是硬塞进 Io。
        let err = AppError::from(anyhow::anyhow!("纯粹的业务逻辑错误"));
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "纯粹的业务逻辑错误");
    }

    #[tokio::test]
    async fn reqwest_errors_become_io_with_a_chinese_message() {
        // 用「空 host」的 URL 触发 reqwest::Error：不依赖网络，也不会超时挂住。
        let raw = reqwest::Client::new()
            .get("http://")
            .send()
            .await
            .expect_err("空 host 的 URL 必须构造失败");

        let err = AppError::from(raw);
        assert_eq!(
            err.kind,
            ErrorKind::Io,
            "网络失败归 Io，不能退化成 Internal"
        );
        assert!(
            ["请求超时", "无法连接到服务地址", "网络请求失败"].contains(&err.message.as_str()),
            "文案要区分超时 / 连不上 / 其它网络失败，实际为：{}",
            err.message
        );
        assert!(err.detail.is_some(), "reqwest 的原始报错必须进 detail");
    }

    #[tokio::test]
    async fn reqwest_error_inside_a_context_chain_still_classifies_as_io() {
        // 关键：reqwest::Error 底下常常还包着一层 io::Error。如果不把 reqwest 的
        // 判断排在 io 之前，链上先被命中的会是那个 io::Error——语义就从「网络不通」
        // 漂移成了「磁盘读写失败」。
        let raw = reqwest::Client::new()
            .get("http://")
            .send()
            .await
            .expect_err("空 host 的 URL 必须构造失败");
        let err = AppError::from(anyhow::Error::from(raw).context("加载模型列表失败"));

        assert_eq!(err.kind, ErrorKind::Io);
        assert_eq!(err.message, "加载模型列表失败");
        assert!(
            err.detail.as_deref().is_some_and(|d| !d.trim().is_empty()),
            "detail 要保留 reqwest 的原始报错，实际为：{:?}",
            err.detail
        );
    }

    #[test]
    fn rollback_failure_is_partial_failure_not_io() {
        // 回滚失败意味着「部分生效」，这个判断比「底层是 io 还是 sqlite」更重要，
        // 所以分类必须优先命中 PartialFailure。注意它内部把底层错误格式化成了
        // 文本，链上不会再出现 io::Error——这正是用标记类型而不是匹配文案的原因。
        let err = AppError::from(anyhow::Error::new(RollbackFailed::new(
            "写入配置失败；回滚失败：权限不足",
        )));
        assert_eq!(err.kind, ErrorKind::PartialFailure);
    }

    #[test]
    fn a_switch_failure_that_rolled_back_cleanly_is_not_partial() {
        // 回滚成功的失败是干净的 Io，不能被误标成部分失败——否则前端会去提示
        // 「请核对工具实际状态」，而实际上什么都没落地。
        let err = AppError::from(anyhow::Error::from(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert_eq!(err.kind, ErrorKind::Permission);
    }
}
