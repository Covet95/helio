//! Tauri command 层：按子模块拆分。
//!
//! `generate_handler!` 必须指向定义了 `#[tauri::command]` 的模块路径
//! （`pub use` 不会带上 `__cmd__*` 宏生成项）。

pub mod cc_switch;
pub mod clipboard;
pub(crate) mod helpers;
pub mod main_cmds;

pub use main_cmds::AppState;
/// 错误类型定义在核心 crate（`switch_api::error`），因为错误语义是在那一层
/// 产生的；这里只做转出，方便命令层 `use crate::commands::{AppError, ...}`。
/// 需要按类别分支时直接从 `switch_api::error::ErrorKind` 引入。
pub use switch_api::error::{unknown_target_app, AppError};
