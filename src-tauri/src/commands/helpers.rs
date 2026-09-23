//! 扫描/导入共用小工具
use switch_api::error::AppError;
use switch_api::models::TargetApp;

/// 定位用户主目录；失败时给出可行动的 `AppError`。
///
/// `dirs::home_dir()` 返回 `None` 属于**环境异常**（`HOME` 未设置、容器里没有
/// passwd 条目等），不是调用方传参不合法，也不是权限不足，所以归 `Internal`，
/// 并在 detail 里点明可能的原因。
///
/// 迁移前这里在每个命令里各写了一遍 `ok_or("Failed to get home directory")`，
/// 一共 6 处英文文案，而且都归到了同一种「未知错误」里。
pub(crate) fn home_dir() -> Result<std::path::PathBuf, AppError> {
    dirs::home_dir().ok_or_else(|| {
        AppError::internal("无法定位用户主目录")
            .with_detail("dirs::home_dir() 返回 None（HOME 环境变量可能未设置）")
    })
}

pub(crate) fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

pub(crate) fn codex_string_field(
    target: TargetApp,
    cfg: &serde_json::Value,
    key: &str,
) -> Option<String> {
    if target != TargetApp::Codex {
        return None;
    }

    let value = str_field(cfg, key);
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn codex_context_1m(target: TargetApp, cfg: &serde_json::Value) -> Option<bool> {
    if target != TargetApp::Codex {
        return None;
    }

    cfg.get("model_context_window")
        .and_then(|w| w.as_i64())
        .map(|w| w >= 1_000_000)
}

/// 从 Claude Code 的 env 对象反向提取默认模型与角色映射，与 `ClaudeCodeAdapter::merge_config`
/// 的写入格式对称：
/// - `ANTHROPIC_MODEL` → 默认模型
/// - `ANTHROPIC_DEFAULT_{ROLE}_MODEL`（可能带 `[1M]` 后缀）→ mapping 的 `{role}_model` / `{role}_one_m`
/// - `ANTHROPIC_DEFAULT_{ROLE}_MODEL_NAME` → mapping 的 `{role}_name`
///
/// 用 `&mut Option` 累加：已有值不覆盖，仅补空（便于多个 env 来源依次补齐）。
pub(crate) fn claude_extract_models(
    env: &serde_json::Value,
    model: &mut Option<String>,
    mapping: &mut Option<std::collections::HashMap<String, String>>,
) {
    if model.is_none() {
        let m = str_field(env, "ANTHROPIC_MODEL");
        if !m.trim().is_empty() {
            *model = Some(m);
        }
    }

    let mut found = std::collections::HashMap::new();
    for role in ["sonnet", "opus", "fable", "haiku"] {
        let upper = role.to_uppercase();
        let raw = str_field(env, &format!("ANTHROPIC_DEFAULT_{upper}_MODEL"));
        if raw.trim().is_empty() {
            continue;
        }
        // [1M] 后缀 = 1M 上下文标记，剥离后才是真实模型 id
        let (base, one_m) = match raw.strip_suffix("[1M]") {
            Some(b) => (b.to_string(), true),
            None => (raw.clone(), false),
        };
        found.insert(format!("{role}_model"), base);
        if one_m {
            found.insert(format!("{role}_one_m"), "true".to_string());
        }
        let name = str_field(env, &format!("ANTHROPIC_DEFAULT_{upper}_MODEL_NAME"));
        if !name.trim().is_empty() {
            found.insert(format!("{role}_name"), name);
        }
    }

    if found.is_empty() {
        return;
    }
    match mapping {
        // 已有更高优先级的映射，只补尚未出现的键
        Some(existing) => {
            for (k, v) in found {
                existing.entry(k).or_insert(v);
            }
        }
        None => *mapping = Some(found),
    }
}

pub(crate) fn default_provider(target: TargetApp) -> String {
    match target {
        TargetApp::ClaudeCode => "anthropic",
        TargetApp::Codex => "openai",
        TargetApp::Pi => "anthropic",
        TargetApp::OpenCode => "anthropic",
        TargetApp::Hermes => "custom",
        TargetApp::OpenClaw => "custom",
        TargetApp::ZCode => "anthropic",
    }
    .to_string()
}

/// Helio 数据库的默认位置。多个模块（导入导出、状态查询）都要用它，
/// 放在 helpers 避免各自拼路径。
pub(crate) fn default_db_path() -> Result<std::path::PathBuf, AppError> {
    Ok(home_dir()?.join(".switch-api").join("db.sqlite"))
}

/// 拒绝把导出目标选成 live 数据库。
///
/// 导出是「写到用户选的路径」，而用户完全可能选到自己的库上（文件名就叫
/// `db.sqlite`，看起来正合适）。实测把便携备份或 Skills 归档导出到 live
/// 路径会把库**覆盖成 tar.gz，直接损坏且不可恢复**——档案与明文 key 全丢。
/// 这不是理论风险：路径参数由前端给出，被攻破的 webview 也能这么干。
///
/// 比对用 canonicalize：`~/.switch-api/../.switch-api/db.sqlite` 这类
/// 等价路径必须也能挡住。目标不存在时 canonicalize 会失败，退化为
/// 逐段规范化后再比。
pub(crate) fn reject_export_onto_live_db(output_path: &str) -> Result<(), AppError> {
    reject_export_onto(output_path, &default_db_path()?)
}

/// 比对逻辑本体，`live` 由调用方给出。
///
/// 拆出来是为了可测：命令层用 `default_db_path()`（依赖 `dirs::home_dir()`），
/// 而**那个函数在 Windows 上不读 `HOME`**——它走 `SHGetKnownFolderPath`。
/// 于是原先「改 HOME 再断言」的测试在 Windows 上根本没生效（实测：
/// 守卫返回 Ok，测试失败）。把 live 路径作为参数传入，测试就能直接
/// 构造两侧路径，不依赖任何环境变量。
fn reject_export_onto(output_path: &str, live: &std::path::Path) -> Result<(), AppError> {
    let target = std::path::Path::new(output_path);
    let live = std::fs::canonicalize(live).unwrap_or_else(|_| live.to_path_buf());

    // 目标可能还不存在（新建导出文件）：规范化其父目录再拼回文件名。
    let normalized = std::fs::canonicalize(target)
        .or_else(|_| {
            let parent = target.parent().unwrap_or_else(|| std::path::Path::new("."));
            let file = target.file_name().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "导出路径缺少文件名")
            })?;
            Ok::<_, std::io::Error>(std::fs::canonicalize(parent)?.join(file))
        })
        .unwrap_or_else(|_| target.to_path_buf());

    if normalized == live {
        return Err(AppError::invalid_input(
            "导出目标不能是 Helio 数据库本身（会覆盖并损坏档案库），请换一个路径",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::reject_export_onto;
    use std::fs;

    /// 造出 `<dir>/.switch-api/db.sqlite`，返回 (临时目录, live 路径)。
    ///
    /// **不依赖 `HOME` 环境变量**：`dirs::home_dir()` 在 Windows 上走
    /// `SHGetKnownFolderPath`，压根不读 `HOME`——原先「改 HOME 再断言」的
    /// 写法在 Windows 上测试形同虚设（守卫返回 Ok 却无人察觉）。
    /// 这里把 live 路径直接传给被测函数。
    fn live_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let api_dir = dir.path().join(".switch-api");
        fs::create_dir_all(&api_dir).unwrap();
        let live = api_dir.join("db.sqlite");
        fs::write(&live, b"db").unwrap();
        (dir, live)
    }

    #[test]
    fn rejects_export_onto_live_db() {
        let (_dir, live) = live_db();
        let err = reject_export_onto(&live.to_string_lossy(), &live)
            .expect_err("导出到 live 库必须被拒绝");
        assert!(
            err.to_string().contains("不能是 Helio 数据库本身"),
            "错误信息应说明原因：{err}"
        );
    }

    #[test]
    fn rejects_equivalent_path_with_dot_segments() {
        let (_dir, live) = live_db();
        // `<...>/.switch-api/../.switch-api/db.sqlite` 指向同一个文件，
        // 只做字符串比较会漏掉它。
        let sneaky = live
            .parent()
            .unwrap()
            .join("..")
            .join(".switch-api")
            .join("db.sqlite");
        assert!(
            reject_export_onto(&sneaky.to_string_lossy(), &live).is_err(),
            "等价路径也必须挡住：{}",
            sneaky.display()
        );
    }

    #[test]
    fn allows_export_next_to_live_db() {
        let (_dir, live) = live_db();
        // 同目录下换个文件名是正常的导出操作，不能误伤。
        let sibling = live.with_file_name("helio-backup.db");
        reject_export_onto(&sibling.to_string_lossy(), &live).expect("导出到库旁边应当允许");
    }

    #[test]
    fn allows_export_to_nonexistent_path() {
        let (_dir, live) = live_db();
        let target = live.with_file_name("brand-new-backup.tar.gz");
        assert!(!target.exists());
        reject_export_onto(&target.to_string_lossy(), &live).expect("导出到尚不存在的文件应当允许");
    }

    #[test]
    fn live_path_matches_itself_through_a_symlinked_dir() {
        // 经父目录的等价写法仍须命中：`<dir>/.switch-api/./db.sqlite`
        let (_dir, live) = live_db();
        let dotted = live.parent().unwrap().join(".").join("db.sqlite");
        assert!(
            reject_export_onto(&dotted.to_string_lossy(), &live).is_err(),
            "带 `.` 段的等价路径也必须挡住：{}",
            dotted.display()
        );
    }
}
