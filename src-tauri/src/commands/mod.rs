//! Tauri command 层：按子模块拆分。
//!
//! `generate_handler!` 必须指向定义了 `#[tauri::command]` 的模块路径
//! （`pub use` 不会带上 `__cmd__*` 宏生成项）。

pub mod cc_switch;
pub(crate) mod helpers;
pub mod main_cmds;

pub use main_cmds::AppState;
pub(crate) use main_cmds::apply_profile_config;
