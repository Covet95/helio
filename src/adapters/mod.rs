use crate::models::{ApiProfile, TargetApp};
use anyhow::Result;
use std::path::PathBuf;

/// 配置适配器 trait
pub trait ConfigAdapter {
    /// 目标应用名称
    fn target_app(&self) -> TargetApp;

    /// 配置文件路径
    fn config_path(&self) -> PathBuf;

    /// 读取当前配置
    fn read_config(&self) -> Result<serde_json::Value>;

    /// 提取共享配置（排除 API 信息）
    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value;

    /// 合并 API Profile 和共享配置
    fn merge_config(&self, api_profile: &ApiProfile, shared_config: &serde_json::Value) -> serde_json::Value;

    /// 原子写入配置
    fn write_config(&self, config: &serde_json::Value) -> Result<()>;

    /// 备份配置
    fn backup_config(&self) -> Result<PathBuf>;

    /// 清理旧备份
    fn cleanup_old_backups(&self, keep: usize) -> Result<()>;
}

pub mod claude_code;
pub mod codex;

/// 获取适配器
pub fn get_adapter(target_app: TargetApp) -> Box<dyn ConfigAdapter> {
    match target_app {
        TargetApp::ClaudeCode => Box::new(claude_code::ClaudeCodeAdapter::new()),
        TargetApp::Codex => Box::new(codex::CodexAdapter::new()),
    }
}
