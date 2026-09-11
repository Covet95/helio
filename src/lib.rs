//! Helio 核心库：适配器、数据库、模型和探活能力。
//! 由 Tauri GUI 使用，避免重复实现配置处理逻辑。

pub mod adapters;
pub mod db;
pub mod error;
pub mod models;
pub mod probe;
pub mod utils;
