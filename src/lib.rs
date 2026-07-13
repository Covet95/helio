//! Helio / switch-api 核心库：适配器、数据库、模型。
//! CLI 与 Tauri GUI 共用此 crate，避免 symlink 复制模块。

pub mod adapters;
pub mod db;
pub mod models;
pub mod probe;
pub mod utils;
