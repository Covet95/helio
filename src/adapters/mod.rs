use crate::models::{ApiProfile, TargetApp};
use anyhow::Result;
use std::path::PathBuf;

/// 配置适配器 trait
pub trait ConfigAdapter {
    /// 配置文件路径
    fn config_path(&self) -> PathBuf;

    /// 读取当前配置
    fn read_config(&self) -> Result<serde_json::Value>;

    /// 读取 MCP servers 的原始 JSON（mcpServers / mcp_servers / mcp）。
    #[cfg(feature = "tauri-gui")]
    fn read_mcp_servers_raw(&self) -> Result<Option<serde_json::Value>> {
        let config = self.read_config()?;
        Ok(config
            .get("mcpServers")
            .or_else(|| config.get("mcp_servers"))
            .or_else(|| config.get("mcp"))
            .cloned())
    }

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

    /// 应用 API 凭据到工具特定的位置（默认无操作）。
    /// 大多数工具的 API 凭据通过 merge_config 写入主配置文件即可。
    /// Gemini 等工具的 API key 存储在独立的 .env 文件，需要重写此方法。
    fn apply_api_credentials(&self, _api_profile: &ApiProfile) -> Result<()> {
        Ok(())
    }
}

pub mod claude_code;
pub mod codex;
pub mod gemini;
pub mod opencode;

/// 获取适配器
pub fn get_adapter(target_app: TargetApp) -> Box<dyn ConfigAdapter> {
    match target_app {
        TargetApp::ClaudeCode => Box::new(claude_code::ClaudeCodeAdapter::new()),
        TargetApp::Codex => Box::new(codex::CodexAdapter::new()),
        TargetApp::Gemini => Box::new(gemini::GeminiAdapter::new()),
        TargetApp::OpenCode => Box::new(opencode::OpenCodeAdapter::new()),
    }
}
