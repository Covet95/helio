use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 会话元数据（列表展示用）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    pub tool: String,
    pub cwd: String,
    pub title: Option<String>,
    pub started_at: i64,
    pub modified_at: i64,
    pub size_bytes: u64,
    pub message_count: usize,
    pub parseable: bool,
}

/// 对话预览消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreviewMessage {
    pub role: String,
    pub text: String,
}

/// 删除结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    pub id: String,
    pub tool: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// 会话读取器：每个工具一个实现，隔离文件结构差异
pub trait SessionReader {
    /// 工具标识："codex" | "claude-code"
    fn tool(&self) -> &str;

    /// 会话根目录（用于路径安全校验）
    fn root(&self) -> PathBuf;

    /// 扫描并返回全部会话元数据
    fn list_sessions(&self) -> Vec<SessionMeta>;

    /// 解析指定会话，返回对话预览（text 按 max_chars 截断）
    fn read_preview(&self, id: &str, max_chars: usize) -> anyhow::Result<Vec<PreviewMessage>>;

    /// id -> 文件路径；返回的路径 MUST 仍在 root() 内，否则返回 None（路径安全）
    fn resolve_path(&self, id: &str) -> Option<PathBuf>;
}
