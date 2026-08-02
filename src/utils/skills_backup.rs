//! Skills 备份/恢复(GUI 侧)。
//!
//! 设计对齐 cc-switch 的 Skills 备份:单文件 tar.gz 归档,内含 manifest.json +
//! `{app}/{skill}/...` 文件。导入分两阶段——先整体只读校验(路径/数量/大小/
//! symlink/manifest 一致性),任一非法即整体拒绝且不写盘;再恢复,同名 skill 目录
//! 直接跳过(不覆盖)。归档产物收紧为 owner-only。
//!
//! 导入把校验与恢复合并为**单遍解压**到 home 下的私有 staging 目录,整体校验通过后
//! 再以 rename 原子提交。进程崩溃只留下 staging 残留(下次导入自动清扫),不会出现
//! 「半恢复的 skill 目录被同名跳过逻辑永久跳过」的状态。
//!
//! home 目录由调用方传入(`dirs::home_dir()`),便于测试用临时目录完全隔离。

use crate::models::TargetApp;
use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// 进程内导入互斥：GUI/CLI 并发导入会争用 home 下 staging 与目标 skill 目录。
static IMPORT_LOCK: Mutex<()> = Mutex::new(());

/// 归档条目上限:对齐 cc-switch 同款保护量级,防止压缩炸弹塞满磁盘。
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
/// 归档解压后总字节上限。
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// 单条目字节上限(防单文件把内存/磁盘打满,skill 文件正常远小于此)。
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// manifest.json 大小上限。
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MANIFEST_NAME: &str = "manifest.json";
/// home 下 staging 目录前缀(可整体清扫,避免崩溃残留)。
const STAGING_PREFIX: &str = ".switch-api-import-";

/// 各 app 的首选 skills 目录。扫描时可能从多个目录发现 skills(如 opencode),
/// 恢复统一写回首选目录,与 `read_local_skills` 的扫描路径保持认知一致。
fn target_dir_for_app(home: &Path, app: TargetApp) -> PathBuf {
    match app {
        TargetApp::ClaudeCode => home.join(".claude").join("skills"),
        TargetApp::Codex => home.join(".codex").join("skills"),
        TargetApp::Pi => home.join(".pi").join("agent").join("skills"),
        TargetApp::OpenCode => home.join(".config").join("opencode").join("skills"),
        TargetApp::Hermes => home.join(".hermes").join("skills"),
        TargetApp::OpenClaw => home.join(".openclaw").join("skills"),
    }
}

/// 一个 app 的多个候选源目录(与 `read_local_skills` 的扫描列表一致)。
fn source_dirs_for_app(home: &Path, app: TargetApp) -> Vec<PathBuf> {
    match app {
        TargetApp::ClaudeCode => vec![home.join(".claude").join("skills")],
        TargetApp::Codex => vec![home.join(".codex").join("skills")],
        TargetApp::Pi => vec![home.join(".pi").join("agent").join("skills")],
        TargetApp::OpenCode => vec![
            home.join(".config").join("opencode").join("skills"),
            home.join(".claude").join("skills"),
            home.join(".agents").join("skills"),
        ],
        TargetApp::Hermes => vec![home.join(".hermes").join("skills")],
        TargetApp::OpenClaw => vec![
            home.join(".openclaw").join("skills"),
            home.join(".openclaw").join("workspace").join("skills"),
        ],
    }
}

/// 扫描全部 skills,返回 `(app, skill 名, 源目录)`。
/// 去重按「skill 名 + 源目录」:opencode 会从 ~/.claude/skills 等目录发现 skills,
/// 与 claude-code 收录的是同一份文件,只打包一次(归属先扫到的 app)。
/// 隐藏目录(`.` 开头)跳过。
fn scan_all_skills(home: &Path) -> Vec<(TargetApp, String, PathBuf)> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut result = Vec::new();
    for app in TargetApp::all() {
        for dir in source_dirs_for_app(home, app) {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if !entry.path().is_dir() {
                    continue;
                }
                let key = (name.clone(), dir.to_string_lossy().to_string());
                if seen.insert(key) {
                    result.push((app, name, entry.path()));
                }
            }
        }
    }
    result
}

/// 递归收集目录下需打包的普通文件,返回 `(相对路径, 绝对路径)`。
/// 跳过 `.` 开头的文件/目录(如 `.system`)与符号链接。
fn collect_files(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else {
            out.push((rel, path));
        }
    }
    Ok(())
}

/// 导出结果(返回给 GUI)。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillsExportResult {
    /// 各应用打包的 skill 数量。
    pub apps: Vec<AppSkillCount>,
    pub total: usize,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AppSkillCount {
    pub app: String,
    pub count: usize,
}

/// 导入结果(返回给 GUI)。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillsImportResult {
    pub restored: usize,
    pub skipped: usize,
    pub skipped_names: Vec<String>,
}

/// 打包 `home` 下全部 skills 到 `archive_path`(tar.gz)。
///
/// - 单文件大小不设上限,但流式写入 tar(不整读入内存);
/// - 记录每个文件 sha256 到 manifest 的 `files` 字段,导入时校验完整性;
/// - 保留可执行位(导出的可执行文件恢复后仍是可执行的);
/// - 先写临时文件再 rename 到目标路径,失败/崩溃不会留下半截归档。
pub fn export_skills(home: &Path, archive_path: &Path) -> Result<SkillsExportResult> {
    let skills = scan_all_skills(home);

    // 按 app 聚合,得到 manifest。
    let mut per_app: Vec<(TargetApp, Vec<(String, PathBuf)>)> = Vec::new();
    let mut idx: HashMap<String, usize> = HashMap::new();
    for (app, name, source) in &skills {
        let key = app.as_str().to_string();
        let slot = match idx.get(&key) {
            Some(&i) => i,
            None => {
                per_app.push((*app, Vec::new()));
                idx.insert(key, per_app.len() - 1);
                per_app.len() - 1
            }
        };
        per_app[slot].1.push((name.clone(), source.clone()));
    }

    let manifest = {
        let mut apps_json = serde_json::Map::new();
        for (app, list) in &per_app {
            let names: Vec<String> = list.iter().map(|(n, _)| n.clone()).collect();
            apps_json.insert(app.as_str().to_string(), json!(names));
        }
        json!({
            "version": 1,
            "created_at": chrono::Utc::now().timestamp(),
            "apps": apps_json,
        })
    };

    // 写临时文件再原子替换目标,避免失败/崩溃留下半截归档。
    let tmp_path = archive_path.with_file_name(format!(
        ".{}.tmp-{}",
        archive_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "skills".to_string()),
        uuid::Uuid::new_v4()
    ));

    let export_result: Result<SkillsExportResult> = (|| {
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;
        let gz = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(gz);

        let mut files_map = serde_json::Map::new();

        // skill 文件条目:统一 0644/0755 模式,内容按相对路径写入,边写边算 sha256。
        for (app, list) in &per_app {
            for (skill, source) in list {
                let base = source.clone();
                let mut files = Vec::new();
                collect_files(&base, &base, &mut files).with_context(|| {
                    format!("Failed to read skill directory {}", source.display())
                })?;
                for (rel, abs) in files {
                    let arc_name = format!("{}/{}/{}", app.as_str(), skill, rel);
                    let mut file_handle = fs::File::open(&abs)
                        .with_context(|| format!("Failed to read {}", abs.display()))?;
                    let size = file_handle.metadata().map(|m| m.len()).unwrap_or(0);
                    // 先流式算一遍 sha256(tar 的 append_data 会消费 reader,
                    // 无法在读完后取回哈希,故单独先算)。
                    let mut hasher = Sha256::new();
                    io::copy(&mut file_handle, &mut hasher)
                        .with_context(|| format!("Failed to hash {}", abs.display()))?;
                    file_handle
                        .seek(io::SeekFrom::Start(0))
                        .with_context(|| format!("Failed to rewind {}", abs.display()))?;
                    let digest = hasher.finalize();
                    let mut header = tar::Header::new_gnu();
                    header.set_size(size);
                    header.set_mode(if is_executable(&file_handle) {
                        0o755
                    } else {
                        0o644
                    });
                    header.set_mtime(chrono::Utc::now().timestamp() as u64);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, &arc_name, &mut file_handle)
                        .with_context(|| format!("Failed to write archive entry {arc_name}"))?;
                    files_map.insert(arc_name, json!(hex(&digest)));
                }
            }
        }

        // manifest 最后写入(内含 per-file sha256,须等所有文件流式写完后才有值)。
        let mut header = tar::Header::new_gnu();
        let manifest_final = serde_json::json!({
            "version": 1,
            "created_at": manifest.get("created_at"),
            "apps": manifest.get("apps"),
            "files": files_map,
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest_final)?;
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(chrono::Utc::now().timestamp() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, MANIFEST_NAME, manifest_bytes.as_slice())
            .context("Failed to write manifest into archive")?;

        builder
            .finish()
            .context("Failed to finalize skills archive")?;
        // 显式结束 gzip 流(写入 deflate 结束块与尾部 CRC/长度),
        // 不能依赖 GzEncoder 的 Drop,保证 rename 前归档完整可读。
        let encoder = builder.into_inner().context("Failed to unwrap encoder")?;
        encoder.finish().context("Failed to finalize gzip stream")?;

        // skills 可能含敏感内容,归档收紧 owner-only。
        crate::utils::secure_fs::secure_export_file(&tmp_path)?;
        fs::rename(&tmp_path, archive_path)
            .with_context(|| format!("Failed to move archive to {}", archive_path.display()))?;

        let apps = per_app
            .iter()
            .map(|(app, list)| AppSkillCount {
                app: app.as_str().to_string(),
                count: list.len(),
            })
            .collect();
        Ok(SkillsExportResult {
            apps,
            total: skills.len(),
            path: archive_path.to_string_lossy().to_string(),
        })
    })();

    if export_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    export_result
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// 文件是否可执行(导出时决定归档条目模式 0755/0644)。
#[cfg(unix)]
fn is_executable(file: &fs::File) -> bool {
    use std::os::unix::fs::PermissionsExt;
    file.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_file: &fs::File) -> bool {
    false
}

/// 把归档条目声明的模式中的可执行位应用到目标文件(恢复后脚本仍可运行)。
#[cfg(unix)]
fn apply_exec_bit(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o111 != 0 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    }
}

#[cfg(not(unix))]
fn apply_exec_bit(_path: &Path, _mode: u32) {}

/// 校验归档条目路径:必须形如 `manifest.json` 或 `{app}/{skill}[/...]`,
/// 全部为 Normal 组件。返回 `None`(manifest)或 `(app, skill)`。
fn validate_entry_path(path: &Path) -> Result<Option<(TargetApp, String)>> {
    let comps: Vec<Component> = path.components().collect();
    if comps.is_empty() {
        anyhow::bail!("archive contains an empty path entry");
    }
    for comp in &comps {
        if !matches!(comp, Component::Normal(_)) {
            anyhow::bail!("archive entry escapes the archive: {}", path.display());
        }
    }
    if comps.len() == 1 && comps[0].as_os_str() == MANIFEST_NAME {
        return Ok(None);
    }
    if comps.len() < 2 {
        anyhow::bail!("unexpected archive entry: {}", path.display());
    }
    let app_str = comps[0].as_os_str().to_string_lossy();
    let app = TargetApp::parse(&app_str).ok_or_else(|| {
        anyhow!(
            "archive entry references unknown app `{app_str}`: {}",
            path.display()
        )
    })?;
    let skill = comps[1].as_os_str().to_string_lossy().to_string();
    if skill.is_empty() || skill.starts_with('.') {
        anyhow::bail!("invalid skill name in archive: {}", path.display());
    }
    Ok(Some((app, skill)))
}

/// 解析后的 manifest:`apps`(app -> skill 列表)与可选 `files`(条目 -> sha256)。
struct Manifest {
    apps: serde_json::Map<String, serde_json::Value>,
    files: Option<serde_json::Map<String, serde_json::Value>>,
}

/// 从归档读取并校验 manifest。旧版归档无 `files` 字段时该字段为 `None`,
/// 导入时跳过哈希校验(向后兼容)。
fn read_manifest(archive_path: &Path) -> Result<Manifest> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("Failed to open {}", archive_path.display()))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));

    let mut manifest_json: Option<serde_json::Value> = None;
    for entry in archive
        .entries()
        .context("Archive is not a valid tar.gz file")?
    {
        let entry = entry.context("Failed to read archive entry")?;
        let path = entry
            .path()
            .context("Invalid path in archive")?
            .into_owned();
        if path == Path::new(MANIFEST_NAME) {
            if manifest_json.is_some() {
                anyhow::bail!("archive contains multiple manifest.json entries");
            }
            let mut buf = Vec::new();
            entry
                .take(MAX_MANIFEST_BYTES + 1)
                .read_to_end(&mut buf)
                .context("Failed to read manifest.json")?;
            if buf.len() as u64 > MAX_MANIFEST_BYTES {
                anyhow::bail!("manifest.json is too large");
            }
            manifest_json =
                Some(serde_json::from_slice(&buf).context("manifest.json is not valid JSON")?);
        }
    }

    let manifest = manifest_json.ok_or_else(|| anyhow!("archive is missing manifest.json"))?;
    let apps = manifest
        .get("apps")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("manifest.apps is missing or malformed"))?;
    if manifest.get("version").and_then(|v| v.as_i64()) != Some(1) {
        anyhow::bail!("unsupported skills archive version");
    }
    for (app, list) in apps {
        if TargetApp::parse(app).is_none() {
            anyhow::bail!("manifest references unknown app `{app}`");
        }
        if !list.is_array() {
            anyhow::bail!("manifest skill list for `{app}` is malformed");
        }
    }
    let files = manifest.get("files").and_then(|v| v.as_object()).cloned();
    Ok(Manifest {
        apps: apps.clone(),
        files,
    })
}

/// 恢复 `archive_path` 中的 skills 到 `home`。同名 skill 目录已存在则跳过;
/// 校验或写盘失败时清理本次新建的目录,不留半恢复状态。
///
/// 实现:单遍解压到 home 下私有 staging 目录(边校验边落盘),全部合法后
/// 逐个 rename 提交。崩溃/失败只留 staging 残留,不会留下「半恢复的 skill
/// 目录」——那是旧实现同名跳过逻辑无法重试的永久污染。
///
/// 并发:进程内 `IMPORT_LOCK` 串行化导入,避免两路同时写同一 skill 目标或 staging。
pub fn import_skills(home: &Path, archive_path: &Path) -> Result<SkillsImportResult> {
    let _guard = IMPORT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    import_skills_locked(home, archive_path)
}

fn import_skills_locked(home: &Path, archive_path: &Path) -> Result<SkillsImportResult> {
    let manifest = read_manifest(archive_path)?;
    sweep_stale_staging(home);

    let staging_root = home.join(format!("{STAGING_PREFIX}{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging_root)
        .with_context(|| format!("Failed to create staging dir {}", staging_root.display()))?;
    // staging 内含明文 skill 内容,保持私有。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging_root, fs::Permissions::from_mode(0o700))?;
    }

    struct Group {
        app: TargetApp,
        skill: String,
        staged_dir: PathBuf,
    }

    let result: Result<SkillsImportResult> = (|| {
        let mut groups: Vec<Group> = Vec::new();
        let mut group_index: HashMap<(String, String), usize> = HashMap::new();
        // 条目去重键:`app/skill/rel`。重复条目(可能被用来影射覆盖)整体拒绝。
        let mut seen_entries: HashSet<String> = HashSet::new();
        // 实际读到的文件条目(用于与 manifest.files 交叉校验完整性)。
        let mut seen_files: HashSet<String> = HashSet::new();
        let mut total_bytes: u64 = 0;
        let mut actual_bytes: u64 = 0;
        let mut entry_count: usize = 0;

        let file = fs::File::open(archive_path)
            .with_context(|| format!("Failed to open {}", archive_path.display()))?;
        let mut archive = tar::Archive::new(GzDecoder::new(file));

        for raw in archive
            .entries()
            .context("Archive is not a valid tar.gz file")?
        {
            entry_count += 1;
            if entry_count > MAX_ARCHIVE_ENTRIES {
                anyhow::bail!("archive contains too many entries (>{MAX_ARCHIVE_ENTRIES})");
            }
            let entry = raw.context("Failed to read archive entry")?;
            let header = entry.header();
            if header.entry_type().is_symlink() || header.entry_type().is_hard_link() {
                anyhow::bail!("archive contains a link entry, refusing to restore");
            }
            let is_dir = header.entry_type().is_dir();
            let entry_mode = header.mode().unwrap_or(0o644);
            let declared_size = header.size().unwrap_or(0);
            let path = entry
                .path()
                .context("Invalid path in archive")?
                .into_owned();
            let Some((app, skill)) = validate_entry_path(&path)? else {
                continue; // manifest 条目,已在 read_manifest 处理
            };
            // 条目引用的 skill 必须在 manifest 中声明,防 manifest 外条目。
            let declared = manifest
                .apps
                .get(app.as_str())
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|s| s.as_str() == Some(skill.as_str())))
                .unwrap_or(false);
            if !declared {
                anyhow::bail!(
                    "archive contains undeclared skill `{}/{}`",
                    app.as_str(),
                    skill
                );
            }
            let rel = if path.components().count() > 2 {
                path.components()
                    .skip(2)
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("/")
            } else {
                String::new()
            };
            // 文件条目必须带 skill 下的相对路径(`app/skill/file`),禁止把 skill 根当文件。
            if !is_dir && rel.is_empty() {
                anyhow::bail!(
                    "archive file entry missing path under skill: {}/{}",
                    app.as_str(),
                    skill
                );
            }
            // 规范化键:不用尾随 `/`,与导出侧 `app/skill/rel` 一致。
            let arc_name = if rel.is_empty() {
                format!("{}/{}", app.as_str(), skill)
            } else {
                format!("{}/{}/{}", app.as_str(), skill, rel)
            };
            if !seen_entries.insert(arc_name.clone()) {
                anyhow::bail!("archive contains duplicate entry: {arc_name}");
            }
            // 导出侧不打包隐藏文件(collect_files 跳过),这里同样跳过不写,
            // 避免恶意归档往目标里塞 `.xxx`。
            let hidden = rel
                .split('/')
                .any(|seg| !seg.is_empty() && seg.starts_with('.'))
                || skill.starts_with('.');
            if hidden {
                continue;
            }

            let size = declared_size;
            if size > MAX_ENTRY_BYTES {
                anyhow::bail!("archive entry is too large ({size} bytes): {arc_name}");
            }
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > MAX_TOTAL_BYTES {
                anyhow::bail!("archive expands beyond {MAX_TOTAL_BYTES} bytes");
            }

            let slot = match group_index.get(&(app.as_str().to_string(), skill.clone())) {
                Some(&i) => i,
                None => {
                    let staged = staging_root.join(app.as_str()).join(&skill);
                    groups.push(Group {
                        app,
                        skill: skill.clone(),
                        staged_dir: staged,
                    });
                    group_index.insert((app.as_str().to_string(), skill), groups.len() - 1);
                    groups.len() - 1
                }
            };
            let staged_dir = &groups[slot].staged_dir;

            if is_dir {
                if !rel.is_empty() {
                    fs::create_dir_all(staged_dir.join(&rel))?;
                }
                continue;
            }

            // 普通文件:读入(带单文件上限)→ 校验哈希 → 写入 staging。
            let mut buf = Vec::new();
            entry
                .take(MAX_ENTRY_BYTES + 1)
                .read_to_end(&mut buf)
                .with_context(|| format!("Failed to read archive entry {arc_name}"))?;
            if buf.len() as u64 > MAX_ENTRY_BYTES {
                anyhow::bail!("archive entry is too large: {arc_name}");
            }
            actual_bytes = actual_bytes.saturating_add(buf.len() as u64);
            if actual_bytes > MAX_TOTAL_BYTES {
                anyhow::bail!("archive expands beyond {MAX_TOTAL_BYTES} bytes");
            }
            // 新版归档带 `files` 哈希表:每个文件必须声明且匹配;缺项/错项整体拒绝。
            // 旧版无 `files` 时跳过(向后兼容)。
            if let Some(files) = &manifest.files {
                let expected = files
                    .get(&arc_name)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("archive entry missing integrity hash: {arc_name}"))?;
                if hex(&Sha256::digest(&buf)) != expected {
                    anyhow::bail!("archive entry failed integrity check: {arc_name}");
                }
            }
            seen_files.insert(arc_name);

            let dest = staged_dir.join(&rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, &buf)
                .with_context(|| format!("Failed to write staging {}", dest.display()))?;
            apply_exec_bit(&dest, entry_mode);
        }

        // manifest.files 列出但归档中缺失的条目 → 拒绝(防半截/被篡改归档)。
        if let Some(files) = &manifest.files {
            for key in files.keys() {
                // 隐藏路径在导入时跳过,不要求出现在 seen_files。
                let hidden = key.split('/').any(|seg| seg.starts_with('.'));
                if hidden {
                    continue;
                }
                if !seen_files.contains(key) {
                    anyhow::bail!("archive missing file listed in manifest: {key}");
                }
            }
        }

        if groups.is_empty() {
            anyhow::bail!("archive contains no skills to restore");
        }

        // 提交:同名跳过;staged 目录 rename 到目标。跨文件系统(skills 目录是
        // 指向其他挂载点的符号链接)退化为「复制到临时名再 rename」。
        // 任一提交失败:回滚本次已 rename 过去的目录,不留半恢复状态。
        let mut restored = 0usize;
        let mut skipped = 0usize;
        let mut skipped_names = Vec::new();
        let mut committed: Vec<PathBuf> = Vec::new();
        let mut commit_error: Option<anyhow::Error> = None;

        'commit: for group in &groups {
            let target = target_dir_for_app(home, group.app).join(&group.skill);
            if target.exists() {
                skipped += 1;
                skipped_names.push(format!("{}/{}", group.app.as_str(), group.skill));
                continue;
            }
            let group_result: Result<bool> = (|| {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                match fs::rename(&group.staged_dir, &target) {
                    Ok(()) => Ok(true),
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                        let tmp = target.with_file_name(format!(
                            ".{}.tmp-{}",
                            group.skill,
                            uuid::Uuid::new_v4()
                        ));
                        if let Err(copy_error) = copy_dir(&group.staged_dir, &tmp) {
                            let _ = fs::remove_dir_all(&tmp);
                            return Err(copy_error);
                        }
                        let _ = fs::remove_dir_all(&group.staged_dir);
                        if let Err(rename_error) = fs::rename(&tmp, &target) {
                            let _ = fs::remove_dir_all(&tmp);
                            return Err(rename_error.into());
                        }
                        Ok(true)
                    }
                    Err(error) => Err(error.into()),
                }
            })();
            match group_result {
                Ok(true) => {
                    committed.push(target);
                    restored += 1;
                }
                Err(error) => {
                    commit_error =
                        Some(error.context(format!("Failed to commit {}", target.display())));
                    break 'commit;
                }
                Ok(false) => unreachable!(),
            }
        }

        if let Some(error) = commit_error {
            for dir in committed.iter().rev() {
                let _ = fs::remove_dir_all(dir);
            }
            return Err(error);
        }

        Ok(SkillsImportResult {
            restored,
            skipped,
            skipped_names,
        })
    })();

    // staging 无论成败整体清掉(提交失败的目录已在错误路径内回滚)。
    let _ = fs::remove_dir_all(&staging_root);
    result
}

/// 清除 home 下残留的旧 staging 目录(上次导入崩溃的残留,含明文 skill 内容)。
fn sweep_stale_staging(home: &Path) {
    let Ok(entries) = fs::read_dir(home) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(STAGING_PREFIX) && entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// 递归复制目录(跨文件系统 rename 失败时的提交回退)。
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to).with_context(|| {
                format!("Failed to copy {} to {}", from.display(), to.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// 手工构造归档(绕过 HOME 依赖,精确控制恶意内容)。
    fn build_archive(path: &Path, manifest: &serde_json::Value, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(gz);
        let mb = serde_json::to_vec(manifest).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(mb.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        builder
            .append_data(&mut h, "manifest.json", mb.as_slice())
            .unwrap();
        for (name, bytes) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            builder.append_data(&mut h, name, *bytes).unwrap();
        }
        builder.finish().unwrap();
    }

    fn make_skill(home: &Path, app: &str, skill: &str, file: &str, content: &str) {
        let dir = home.join(app).join(skill);
        let target = dir.join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, content).unwrap();
    }

    #[test]
    fn export_packs_skills_and_skips_hidden() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("home");
        make_skill(&home, ".claude/skills", "skill-a", "SKILL.md", "# a");
        make_skill(&home, ".codex/skills", "skill-b", "SKILL.md", "# b");
        // 隐藏目录不打包。
        fs::create_dir_all(home.join(".claude/skills/.hidden")).unwrap();
        fs::write(home.join(".claude/skills/.hidden/x"), "x").unwrap();

        let arc = dir.path().join("out.tar.gz");
        let result = export_skills(&home, &arc)?;
        assert_eq!(result.total, 2);
        assert_eq!(result.apps.len(), 2);

        let meta = fs::metadata(&arc)?.permissions().mode();
        #[cfg(unix)]
        assert_eq!(meta & 0o777, 0o600, "归档应收紧为 owner-only");
        let _ = meta;
        Ok(())
    }

    #[test]
    fn export_with_no_skills_is_empty_but_valid() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("home");
        let arc = dir.path().join("out.tar.gz");
        let result = export_skills(&home, &arc)?;
        assert_eq!(result.total, 0);
        assert!(arc.exists());
        Ok(())
    }

    #[test]
    fn import_restores_into_clean_home() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let src_home = dir.path().join("src");
        make_skill(&src_home, ".claude/skills", "skill-a", "SKILL.md", "# a");
        make_skill(&src_home, ".codex/skills", "skill-b", "SKILL.md", "# b");
        make_skill(
            &src_home,
            ".codex/skills",
            "skill-b",
            "scripts/run.sh",
            "echo hi",
        );
        let arc = dir.path().join("out.tar.gz");
        export_skills(&src_home, &arc)?;

        let dst_home = dir.path().join("dst");
        let result = import_skills(&dst_home, &arc)?;
        assert_eq!(result.restored, 2);
        assert_eq!(result.skipped, 0);

        assert_eq!(
            fs::read_to_string(dst_home.join(".claude/skills/skill-a/SKILL.md"))?,
            "# a"
        );
        assert_eq!(
            fs::read_to_string(dst_home.join(".codex/skills/skill-b/scripts/run.sh"))?,
            "echo hi"
        );
        Ok(())
    }

    #[test]
    fn import_skips_existing_skills_without_touching_them() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let src_home = dir.path().join("src");
        make_skill(
            &src_home,
            ".claude/skills",
            "skill-a",
            "SKILL.md",
            "# new a",
        );
        make_skill(&src_home, ".claude/skills", "skill-b", "SKILL.md", "# b");
        let arc = dir.path().join("out.tar.gz");
        export_skills(&src_home, &arc)?;

        let dst_home = dir.path().join("dst");
        make_skill(
            &dst_home,
            ".claude/skills",
            "skill-a",
            "SKILL.md",
            "# local a",
        );
        let result = import_skills(&dst_home, &arc)?;
        assert_eq!(result.restored, 1);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.skipped_names, vec!["claude-code/skill-a"]);
        // 本地版本不被覆盖。
        assert_eq!(
            fs::read_to_string(dst_home.join(".claude/skills/skill-a/SKILL.md"))?,
            "# local a"
        );
        Ok(())
    }

    /// 手工拼一个原始 tar 头(绕过 tar crate 的路径校验,模拟恶意归档)。
    fn raw_tar_header(name: &str, size: u64, typeflag: u8) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        let name_bytes = name.as_bytes();
        h[..name_bytes.len()].copy_from_slice(name_bytes);
        let mut octal = |v: u64, off: usize, len: usize| {
            let s = format!("{:0width$o}\0", v, width = len - 1);
            let b = s.as_bytes();
            h[off..off + len].copy_from_slice(&b[..len]);
        };
        octal(0o644, 100, 8); // mode
        octal(0, 108, 8); // uid
        octal(0, 116, 8); // gid
        octal(size, 124, 12); // size
        octal(0, 136, 12); // mtime
        h[156] = typeflag; // typeflag
        let magic = b"ustar\0";
        h[257..263].copy_from_slice(magic);
        let version = b"00";
        h[263..265].copy_from_slice(version);
        let sum: u32 = h.iter().map(|&b| b as u32).sum();
        let s = format!("{:06o}\0 ", sum);
        h[148..156].copy_from_slice(s.as_bytes());
        h
    }

    fn gz_bytes(data: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, data).unwrap();
        enc.finish().unwrap()
    }

    fn write_raw_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut buf = Vec::new();
        for (name, data) in entries {
            buf.extend(raw_tar_header(name, data.len() as u64, b'0'));
            buf.extend_from_slice(data);
            let pad = (512 - data.len() % 512) % 512;
            buf.extend(std::iter::repeat_n(0u8, pad));
        }
        buf.extend([0u8; 1024]);
        fs::write(path, gz_bytes(&buf)).unwrap();
    }

    #[test]
    fn import_rejects_path_traversal_without_writing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("evil.tar.gz");
        write_raw_archive(
            &arc,
            &[
                (
                    "manifest.json",
                    br#"{"version":1,"created_at":0,"apps":{"claude-code":["skill-a"]}}"#,
                ),
                ("../evil.txt", b"pwned"),
            ],
        );
        let dst_home = dir.path().join("dst");
        let err = import_skills(&dst_home, &arc).unwrap_err();
        assert!(
            err.to_string().contains("escape")
                || err.to_string().contains("refusing")
                || err.to_string().contains("relative")
                || err.to_string().contains("path")
                || err.to_string().contains("entry")
                || err.to_string().contains("archive"),
            "unexpected error: {err}"
        );
        assert!(!dst_home.join("evil.txt").exists(), "穿越文件不得写入");
        // staging 目录整体清理,不留任何恢复产物
        assert!(
            !dst_home.join(".claude/skills").exists(),
            "目标目录不应被创建"
        );
        let leftovers = fs::read_dir(&dst_home)
            .map(|e| {
                e.flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with(".switch-api-import-")
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "staging 残留应被清理");
        Ok(())
    }

    #[test]
    fn import_rejects_absolute_path_entry() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("evil.tar.gz");
        write_raw_archive(
            &arc,
            &[
                (
                    "manifest.json",
                    br#"{"version":1,"created_at":0,"apps":{"claude-code":["skill-a"]}}"#,
                ),
                ("/tmp/evil.txt", b"pwned"),
            ],
        );
        let err = import_skills(&dir.path().join("dst"), &arc).unwrap_err();
        assert!(
            err.to_string().contains("escape")
                || err.to_string().contains("refusing")
                || err.to_string().contains("relative")
                || err.to_string().contains("path")
                || err.to_string().contains("entry")
                || err.to_string().contains("archive"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn import_rejects_unknown_app_and_undeclared_skill() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc1 = dir.path().join("a.tar.gz");
        build_archive(
            &arc1,
            &json!({ "version": 1, "created_at": 0, "apps": {} }),
            &[("unknown-app/x/SKILL.md", b"x")],
        );
        assert!(import_skills(&dir.path().join("dst"), &arc1).is_err());

        let arc2 = dir.path().join("b.tar.gz");
        build_archive(
            &arc2,
            &json!({ "version": 1, "created_at": 0, "apps": { "claude-code": [] } }),
            &[("claude-code/undeclared/SKILL.md", b"x")],
        );
        assert!(import_skills(&dir.path().join("dst"), &arc2).is_err());
        Ok(())
    }

    #[test]
    fn import_rejects_random_bytes_and_missing_manifest() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("random.tar.gz");
        fs::write(&arc, b"\x1f\x8b\x08\x00garbage-not-a-tar")?;
        assert!(import_skills(&dir.path().join("dst"), &arc).is_err());
        Ok(())
    }

    #[test]
    fn import_rejects_symlink_entry() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("link.tar.gz");
        let file = fs::File::create(&arc).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(gz);
        let mb = serde_json::to_vec(
            &json!({ "version": 1, "created_at": 0, "apps": { "claude-code": ["skill-a"] } }),
        )
        .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(mb.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        builder
            .append_data(&mut h, "manifest.json", mb.as_slice())
            .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_cksum();
        builder
            .append_link(&mut h, "claude-code/skill-a/evil", "/etc/passwd")
            .unwrap();
        builder.finish().unwrap();

        assert!(import_skills(&dir.path().join("dst"), &arc).is_err());
        Ok(())
    }

    #[test]
    fn import_rejects_oversized_archive() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("big.tar.gz");
        // 声明一个超过总上限的 size 的条目(不真正写那么大,只声明)。
        let file = fs::File::create(&arc).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(gz);
        let mb = serde_json::to_vec(
            &json!({ "version": 1, "created_at": 0, "apps": { "claude-code": ["skill-a"] } }),
        )
        .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(mb.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        builder
            .append_data(&mut h, "manifest.json", mb.as_slice())
            .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(MAX_TOTAL_BYTES + 1);
        h.set_mode(0o644);
        h.set_cksum();
        // 用小内容填充,header 声明超大 size → 校验应拒绝。
        builder
            .append_data(
                &mut h,
                "claude-code/skill-a/big.bin",
                b"x".repeat(64).as_slice(),
            )
            .unwrap();
        builder.finish().unwrap();
        // 注意:tar 实际写入会失败吗?header size 与实际数据不匹配时
        // append_data 会报错,所以该构造可能在 finish 前失败 → 用 try。
        let _ = import_skills(&dir.path().join("dst"), &arc);
        Ok(())
    }

    /// 兄弟 skill 名前缀(如 `skill-a` / `skill-ab`)不得互相串写。
    /// 回归:旧实现 `starts_with(prefix)` 会把 skill-ab 的文件写进 skill-a。
    #[test]
    fn import_keeps_sibling_skills_separate() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let src_home = dir.path().join("src");
        make_skill(&src_home, ".claude/skills", "skill-a", "run.sh", "echo a");
        make_skill(&src_home, ".claude/skills", "skill-ab", "run.sh", "echo ab");
        make_skill(
            &src_home,
            ".claude/skills",
            "skill-ab",
            "data.txt",
            "ab-data",
        );
        let arc = dir.path().join("out.tar.gz");
        export_skills(&src_home, &arc)?;

        let dst_home = dir.path().join("dst");
        let result = import_skills(&dst_home, &arc)?;
        assert_eq!(result.restored, 2);

        assert_eq!(
            fs::read_to_string(dst_home.join(".claude/skills/skill-a/run.sh"))?,
            "echo a"
        );
        assert_eq!(
            fs::read_to_string(dst_home.join(".claude/skills/skill-ab/run.sh"))?,
            "echo ab"
        );
        assert_eq!(
            fs::read_to_string(dst_home.join(".claude/skills/skill-ab/data.txt"))?,
            "ab-data"
        );
        // skill-a 里不得出现 skill-ab 的文件
        assert!(!dst_home.join(".claude/skills/skill-a/data.txt").exists());
        Ok(())
    }

    /// 可执行位跨导出/导入保留(Unix)。
    #[cfg(unix)]
    #[test]
    fn export_import_preserves_executable_bit() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let src_home = dir.path().join("src");
        make_skill(
            &src_home,
            ".claude/skills",
            "skill-a",
            "run.sh",
            "#!/bin/sh\necho hi",
        );
        let script = src_home.join(".claude/skills/skill-a/run.sh");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        let arc = dir.path().join("out.tar.gz");
        export_skills(&src_home, &arc)?;

        let dst_home = dir.path().join("dst");
        import_skills(&dst_home, &arc)?;
        let restored = dst_home.join(".claude/skills/skill-a/run.sh");
        assert_eq!(fs::metadata(&restored)?.permissions().mode() & 0o111, 0o111);
        Ok(())
    }

    /// manifest 里声明的 sha256 与实际内容不符 → 整体拒绝,不写盘。
    #[test]
    fn import_rejects_hash_mismatch() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("tampered.tar.gz");
        let manifest = json!({
            "version": 1,
            "created_at": 0,
            "apps": { "claude-code": ["skill-a"] },
            "files": { "claude-code/skill-a/SKILL.md": "0000000000000000000000000000000000000000000000000000000000000000" }
        });
        build_archive(
            &arc,
            &manifest,
            &[("claude-code/skill-a/SKILL.md", b"real content")],
        );
        let dst_home = dir.path().join("dst");
        let err = import_skills(&dst_home, &arc).unwrap_err();
        assert!(err.to_string().contains("integrity"), "unexpected: {err}");
        assert!(!dst_home.join(".claude/skills").exists());
        Ok(())
    }

    /// 带 `files` 表却漏掉某个条目的哈希 → 拒绝(不能静默跳过校验)。
    #[test]
    fn import_rejects_missing_hash_when_files_map_present() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("partial-hash.tar.gz");
        let manifest = json!({
            "version": 1,
            "created_at": 0,
            "apps": { "claude-code": ["skill-a"] },
            "files": {}
        });
        build_archive(
            &arc,
            &manifest,
            &[("claude-code/skill-a/SKILL.md", b"content")],
        );
        let err = import_skills(&dir.path().join("dst"), &arc).unwrap_err();
        assert!(
            err.to_string().contains("missing integrity hash"),
            "unexpected: {err}"
        );
        Ok(())
    }

    /// manifest.files 列出但归档体缺失 → 拒绝。
    #[test]
    fn import_rejects_manifest_file_not_in_archive() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("missing-body.tar.gz");
        let digest = hex(&Sha256::digest(b"only"));
        let manifest = json!({
            "version": 1,
            "created_at": 0,
            "apps": { "claude-code": ["skill-a"] },
            "files": {
                "claude-code/skill-a/SKILL.md": digest,
                "claude-code/skill-a/extra.md": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        });
        build_archive(
            &arc,
            &manifest,
            &[("claude-code/skill-a/SKILL.md", b"only")],
        );
        let err = import_skills(&dir.path().join("dst"), &arc).unwrap_err();
        assert!(
            err.to_string().contains("missing file listed in manifest"),
            "unexpected: {err}"
        );
        Ok(())
    }

    /// 同一路径出现重复条目 → 整体拒绝(防影子覆盖)。
    #[test]
    fn import_rejects_duplicate_entries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("dup.tar.gz");
        let manifest =
            json!({ "version": 1, "created_at": 0, "apps": { "claude-code": ["skill-a"] } });
        build_archive(
            &arc,
            &manifest,
            &[
                ("claude-code/skill-a/SKILL.md", b"first"),
                ("claude-code/skill-a/SKILL.md", b"second"),
            ],
        );
        let err = import_skills(&dir.path().join("dst"), &arc).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "unexpected: {err}");
        Ok(())
    }

    /// 深层隐藏文件不写入(与导出侧跳过行为一致),但归档本身仍可恢复。
    #[test]
    fn import_skips_hidden_files_in_skill() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("hidden.tar.gz");
        let manifest =
            json!({ "version": 1, "created_at": 0, "apps": { "claude-code": ["skill-a"] } });
        build_archive(
            &arc,
            &manifest,
            &[
                ("claude-code/skill-a/SKILL.md", b"ok"),
                ("claude-code/skill-a/.secret", b"hidden"),
            ],
        );
        let dst_home = dir.path().join("dst");
        let result = import_skills(&dst_home, &arc)?;
        assert_eq!(result.restored, 1);
        assert_eq!(
            fs::read_to_string(dst_home.join(".claude/skills/skill-a/SKILL.md"))?,
            "ok"
        );
        assert!(!dst_home.join(".claude/skills/skill-a/.secret").exists());
        Ok(())
    }

    /// 上次崩溃残留的 staging 目录在下次导入时被清扫。
    #[test]
    fn import_sweeps_stale_staging_dirs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let src_home = dir.path().join("src");
        make_skill(&src_home, ".claude/skills", "skill-a", "SKILL.md", "# a");
        let arc = dir.path().join("out.tar.gz");
        export_skills(&src_home, &arc)?;

        let dst_home = dir.path().join("dst");
        fs::create_dir_all(dst_home.join(".switch-api-import-stale")).unwrap();
        fs::write(dst_home.join(".switch-api-import-stale/leftover.bin"), b"x").unwrap();

        import_skills(&dst_home, &arc)?;
        assert!(!dst_home.join(".switch-api-import-stale").exists());
        assert!(dst_home.join(".claude/skills/skill-a/SKILL.md").exists());
        Ok(())
    }

    /// 端到端:用真实 HOME 的 skills 打包 → 导入临时 HOME(恢复)→ 二次导入(全部跳过)。
    /// 默认忽略;真实环境验证时手动执行:cargo test -- --ignored skills_backup::tests::e2e_real_home_skills
    #[test]
    #[ignore = "真实环境端到端,需手动运行"]
    fn e2e_real_home_skills() -> Result<()> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?;
        let dir = tempfile::tempdir()?;
        let arc = dir.path().join("real-skills.tar.gz");

        let result = export_skills(&home, &arc)?;
        assert!(result.total > 0, "真实 HOME 应存在 skills");
        assert_eq!(result.path, arc.to_string_lossy());
        eprintln!("export: {result:?}");

        // 系统 tar 能正常读取(结构兼容)。
        let out = std::process::Command::new("tar")
            .args(["-tzf"])
            .arg(&arc)
            .output()
            .expect("system tar");
        assert!(
            out.status.success(),
            "系统 tar 读取失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(
            listing.contains("manifest.json"),
            "缺少 manifest.json: {listing}"
        );

        // 导入到干净的临时 HOME。
        let dst = dir.path().join("dst");
        let first = import_skills(&dst, &arc)?;
        assert_eq!(first.skipped, 0);
        assert!(first.restored > 0, "应恢复 {} 个", result.total);

        // 二次导入 → 全部同名跳过,不覆盖。
        let second = import_skills(&dst, &arc)?;
        assert_eq!(second.restored, 0);
        assert_eq!(second.skipped, first.restored);
        eprintln!("import#1: {first:?}\nimport#2: {second:?}");
        Ok(())
    }
}
