use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

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

/// 校验 candidate 归一化后仍在 root 内。拒绝 `..` 越界。
pub(crate) fn is_within_root(root: &Path, candidate: &Path) -> bool {
    let norm = normalize_lexical(candidate);
    let root_norm = normalize_lexical(root);
    norm.starts_with(&root_norm)
}

/// 词法归一化：解析 `.` 与 `..`，不触碰文件系统。
fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub struct CodexSessionReader {
    pub sessions_dir: PathBuf, // ~/.codex/sessions
}

impl CodexSessionReader {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("home dir");
        Self {
            sessions_dir: home.join(".codex").join("sessions"),
        }
    }
}

impl SessionReader for CodexSessionReader {
    fn tool(&self) -> &str {
        "codex"
    }
    fn root(&self) -> PathBuf {
        self.sessions_dir.clone()
    }

    fn list_sessions(&self) -> Vec<SessionMeta> {
        let mut out = Vec::new();
        if !self.sessions_dir.exists() {
            return out;
        }

        for entry in WalkDir::new(&self.sessions_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let meta = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size_bytes = meta.len();
            let modified_at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            // 文件名即 id 兜底：rollout-<时间>-<uuid>.jsonl
            let fallback_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            // 读首行 session_meta
            let mut id = fallback_id.clone();
            let mut cwd = String::new();
            let mut started_at = modified_at;
            let mut parseable = false;
            if let Ok(file) = File::open(path) {
                let mut reader = BufReader::new(file);
                let mut first = String::new();
                if reader.read_line(&mut first).is_ok() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) {
                        if v.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                            let p = v.get("payload");
                            if let Some(p) = p {
                                if let Some(s) = p.get("id").and_then(|x| x.as_str()) {
                                    id = s.to_string();
                                }
                                if let Some(s) = p.get("cwd").and_then(|x| x.as_str()) {
                                    cwd = s.to_string();
                                }
                                if let Some(s) = p.get("timestamp").and_then(|x| x.as_str()) {
                                    started_at = parse_iso8601(s).unwrap_or(modified_at);
                                }
                            }
                            parseable = true;
                        }
                    }
                }
            }
            let message_count = count_lines(path);

            out.push(SessionMeta {
                id,
                tool: "codex".into(),
                cwd,
                title: None,
                started_at,
                modified_at,
                size_bytes,
                message_count,
                parseable,
            });
        }
        out
    }

    fn read_preview(&self, id: &str, max_chars: usize) -> anyhow::Result<Vec<PreviewMessage>> {
        let path = self
            .resolve_path(id)
            .ok_or_else(|| anyhow::anyhow!("session not found: {id}"))?;
        let file = File::open(&path)?;
        let mut out = Vec::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let payload = v.get("payload");
            let is_message =
                payload.and_then(|p| p.get("type")).and_then(|t| t.as_str()) == Some("message");
            if !is_message {
                continue;
            }
            let role = payload
                .and_then(|p| p.get("role"))
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string();
            let text = extract_text(payload, max_chars);
            if !text.is_empty() {
                out.push(PreviewMessage { role, text });
            }
        }
        Ok(out)
    }

    fn resolve_path(&self, id: &str) -> Option<PathBuf> {
        if !self.sessions_dir.exists() {
            return None;
        }
        for entry in WalkDir::new(&self.sessions_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // 精确匹配文件名（fallback id），退化时读首行 session_meta.id。
            // 禁止 contains 子串匹配：id="a" 会误命中 "abc"。
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let matched = name == id || first_session_meta_id(path).as_deref() == Some(id);
            if matched && is_within_root(&self.root(), path) {
                return Some(path.to_path_buf());
            }
        }
        None
    }
}

/// 读 jsonl 首行的 session_meta.payload.id（Codex 格式）。
fn first_session_meta_id(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).ok()?;
    let v: serde_json::Value = serde_json::from_str(&first).ok()?;
    v.pointer("/payload/id")
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

pub struct ClaudeSessionReader {
    pub projects_dir: PathBuf, // ~/.claude/projects
}

impl ClaudeSessionReader {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("home dir");
        Self {
            projects_dir: home.join(".claude").join("projects"),
        }
    }
}

/// 编码目录名反解 cwd：-Users-u-Desktop-power -> /Users/u/Desktop/power
/// Windows 盘符目录名（C-Users-u-Desktop-power，编码自 C:\Users\...）→ C:\Users\u\Desktop\power
/// 区分依据：Unix 编码以 - 开头（根 / 被替换成前导 -），Windows 编码以盘符字母开头。
fn decode_project_dir(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = name.split('-').filter(|s| !s.is_empty()).collect();
    if name.starts_with('-') {
        // macOS / Linux：-Users-u-Desktop-power -> /Users/u/Desktop/power
        return format!("/{}", parts.join("/"));
    }
    // Windows：C-Users-u-Desktop-power -> C:\Users\u\Desktop\power
    if parts.is_empty() {
        return String::new();
    }
    let mut out = format!("{}:", parts[0]);
    for part in &parts[1..] {
        out.push('\\');
        out.push_str(part);
    }
    out
}

impl SessionReader for ClaudeSessionReader {
    fn tool(&self) -> &str {
        "claude-code"
    }
    fn root(&self) -> PathBuf {
        self.projects_dir.clone()
    }

    fn list_sessions(&self) -> Vec<SessionMeta> {
        let mut out = Vec::new();
        if !self.projects_dir.exists() {
            return out;
        }

        for entry in WalkDir::new(&self.projects_dir)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let meta = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size_bytes = meta.len();
            let modified_at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let dir_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let mut cwd = decode_project_dir(dir_name);
            let mut title: Option<String> = None;
            let mut started_at = modified_at;
            let mut parseable = false;

            // 扫前 50 行找 cwd / ai-title / 起始时间
            if let Ok(file) = File::open(path) {
                for (i, line) in BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .enumerate()
                {
                    if i >= 50 {
                        break;
                    }
                    let v: serde_json::Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    parseable = true;
                    if title.is_none() {
                        if let Some(t) = v.get("title").and_then(|x| x.as_str()) {
                            title = Some(t.to_string());
                        }
                    }
                    if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
                        cwd = c.to_string();
                    }
                    if started_at == modified_at {
                        if let Some(ts) = v.get("timestamp").and_then(|x| x.as_str()) {
                            if let Some(t) = parse_iso8601(ts) {
                                started_at = t;
                            }
                        }
                    }
                }
            }
            let message_count = count_lines(path);

            out.push(SessionMeta {
                id,
                tool: "claude-code".into(),
                cwd,
                title,
                started_at,
                modified_at,
                size_bytes,
                message_count,
                parseable,
            });
        }
        out
    }

    fn read_preview(&self, id: &str, max_chars: usize) -> anyhow::Result<Vec<PreviewMessage>> {
        let path = self
            .resolve_path(id)
            .ok_or_else(|| anyhow::anyhow!("session not found: {id}"))?;
        let file = File::open(&path)?;
        let mut out = Vec::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if ty != "user" && ty != "assistant" {
                continue;
            }
            let msg = v.get("message");
            let role = msg
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                .unwrap_or(ty)
                .to_string();
            let text = claude_message_text(msg, max_chars);
            if !text.is_empty() {
                out.push(PreviewMessage { role, text });
            }
        }
        Ok(out)
    }

    fn resolve_path(&self, id: &str) -> Option<PathBuf> {
        if !self.projects_dir.exists() {
            return None;
        }
        for entry in WalkDir::new(&self.projects_dir)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem == id && is_within_root(&self.root(), path) {
                return Some(path.to_path_buf());
            }
        }
        None
    }
}

/// Claude message.content 可能是 string 或 [{type,text}] 数组
fn claude_message_text(msg: Option<&serde_json::Value>, max_chars: usize) -> String {
    let mut buf = String::new();
    if let Some(content) = msg.and_then(|m| m.get("content")) {
        if let Some(s) = content.as_str() {
            buf.push_str(s);
        } else if let Some(arr) = content.as_array() {
            for seg in arr {
                if let Some(t) = seg.get("text").and_then(|x| x.as_str()) {
                    buf.push_str(t);
                }
            }
        }
    }
    if buf.chars().count() > max_chars {
        buf = buf.chars().take(max_chars).collect::<String>() + "…";
    }
    buf
}

/// 删除单个会话：移系统垃圾桶。目标不存在视为成功；越界拒绝。
pub fn delete_one(reader: &dyn SessionReader, id: &str) -> DeleteResult {
    delete_one_with(reader, id, &trash_delete)
}

/// trash 删除的默认实现
fn trash_delete(path: &Path) -> anyhow::Result<()> {
    trash::delete(path).map_err(|e| anyhow::anyhow!("移入垃圾桶失败: {e}"))
}

/// 可注入删除函数（测试用）
fn delete_one_with(
    reader: &dyn SessionReader,
    id: &str,
    do_delete: &dyn Fn(&Path) -> anyhow::Result<()>,
) -> DeleteResult {
    let tool = reader.tool().to_string();
    let path = match reader.resolve_path(id) {
        Some(p) => p,
        None => {
            return DeleteResult {
                id: id.into(),
                tool,
                ok: true,
                error: None,
            }
        } // 不存在=成功
    };
    if !is_within_root(&reader.root(), &path) {
        return DeleteResult {
            id: id.into(),
            tool,
            ok: false,
            error: Some("路径越界，拒绝删除".into()),
        };
    }
    if !path.exists() {
        return DeleteResult {
            id: id.into(),
            tool,
            ok: true,
            error: None,
        };
    }
    match do_delete(&path) {
        Ok(_) => DeleteResult {
            id: id.into(),
            tool,
            ok: true,
            error: None,
        },
        Err(e) => DeleteResult {
            id: id.into(),
            tool,
            ok: false,
            error: Some(e.to_string()),
        },
    }
}

/// 估算 jsonl 行数（消息数）。采样前 SAMPLE_ROWS 行后按文件大小比例外推，
/// 避免大文件全量逐行读取。
fn count_lines(path: &Path) -> usize {
    const SAMPLE_ROWS: usize = 500;
    let Ok(file) = File::open(path) else {
        return 0;
    };
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut reader = BufReader::new(file);
    let mut rows = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => rows += 1,
            Err(_) => break,
        }
        if rows >= SAMPLE_ROWS {
            break;
        }
    }
    // 只读到采样上限（文件未读完）：按字节比例外推
    let consumed = reader.stream_position().unwrap_or(0);
    if consumed > 0 && consumed < total {
        rows = (rows as u128 * total as u128 / consumed as u128) as usize;
    }
    rows
}

/// 从 message.payload 的 content 数组提取文本，按 max_chars 截断
fn extract_text(payload: Option<&serde_json::Value>, max_chars: usize) -> String {
    let mut buf = String::new();
    if let Some(content) = payload
        .and_then(|p| p.get("content"))
        .and_then(|c| c.as_array())
    {
        for seg in content {
            if let Some(t) = seg.get("text").and_then(|x| x.as_str()) {
                buf.push_str(t);
            }
        }
    }
    if buf.chars().count() > max_chars {
        buf = buf.chars().take(max_chars).collect::<String>() + "…";
    }
    buf
}

/// 解析 ISO8601 时间为 unix 秒
fn parse_iso8601(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// 全部 reader
fn all_readers() -> Vec<Box<dyn SessionReader>> {
    vec![
        Box::new(CodexSessionReader::new()),
        Box::new(ClaudeSessionReader::new()),
    ]
}

/// 按 tool / search 过滤（search 命中 cwd 或 title）
pub(crate) fn apply_filters(
    metas: Vec<SessionMeta>,
    tool: Option<&str>,
    search: Option<&str>,
) -> Vec<SessionMeta> {
    metas
        .into_iter()
        .filter(|m| {
            if let Some(t) = tool {
                if m.tool != t {
                    return false;
                }
            }
            if let Some(q) = search {
                if q.is_empty() {
                    return true;
                }
                let hit =
                    m.cwd.contains(q) || m.title.as_deref().map(|t| t.contains(q)).unwrap_or(false);
                if !hit {
                    return false;
                }
            }
            true
        })
        .collect()
}

fn reader_for(tool: &str) -> Option<Box<dyn SessionReader>> {
    match tool {
        "codex" => Some(Box::new(CodexSessionReader::new())),
        "claude-code" => Some(Box::new(ClaudeSessionReader::new())),
        _ => None,
    }
}

#[tauri::command]
pub async fn list_sessions(
    tool: Option<String>,
    search: Option<String>,
) -> Result<Vec<SessionMeta>, String> {
    let mut all = Vec::new();
    for r in all_readers() {
        all.extend(r.list_sessions());
    }
    // 默认按修改时间倒序
    all.sort_by_key(|b| std::cmp::Reverse(b.modified_at));
    Ok(apply_filters(all, tool.as_deref(), search.as_deref()))
}

#[tauri::command]
pub async fn read_session_preview(tool: String, id: String) -> Result<Vec<PreviewMessage>, String> {
    let reader = reader_for(&tool).ok_or_else(|| format!("未知工具: {tool}"))?;
    reader.read_preview(&id, 4000).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_session(tool: String, id: String) -> Result<DeleteResult, String> {
    let reader = reader_for(&tool).ok_or_else(|| format!("未知工具: {tool}"))?;
    Ok(delete_one(reader.as_ref(), &id))
}

#[derive(serde::Deserialize)]
pub struct DeleteItem {
    pub tool: String,
    pub id: String,
}

#[tauri::command]
pub async fn delete_sessions(items: Vec<DeleteItem>) -> Result<Vec<DeleteResult>, String> {
    let mut out = Vec::new();
    for it in items {
        match reader_for(&it.tool) {
            Some(r) => out.push(delete_one(r.as_ref(), &it.id)),
            None => out.push(DeleteResult {
                id: it.id,
                tool: it.tool,
                ok: false,
                error: Some("未知工具".into()),
            }),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn cleanup_sessions(
    tool: Option<String>,
    older_than_days: i64,
) -> Result<Vec<DeleteResult>, String> {
    if older_than_days <= 0 {
        return Ok(Vec::new());
    }
    let now = chrono::Utc::now().timestamp();
    let cutoff = now - older_than_days * 86400;
    let mut out = Vec::new();
    for r in all_readers() {
        if let Some(t) = &tool {
            if r.tool() != t {
                continue;
            }
        }
        for m in r.list_sessions() {
            if m.modified_at < cutoff {
                out.push(delete_one(r.as_ref(), &m.id));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("helio-sh-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_codex_list_extracts_meta() {
        let root = temp_dir("codex-list");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        let f = day.join("rollout-2026-06-03T09-01-24-abc.jsonl");
        fs::write(&f,
            "{\"timestamp\":\"2026-06-03T01:01:29.422Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"abc\",\"timestamp\":\"2026-06-03T01:01:24.892Z\",\"cwd\":\"/Users/u/proj\"}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\"}}\n").unwrap();

        let reader = CodexSessionReader {
            sessions_dir: root.clone(),
        };
        let list = reader.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "abc");
        assert_eq!(list[0].tool, "codex");
        assert_eq!(list[0].cwd, "/Users/u/proj");
        assert!(list[0].parseable);
        assert!(list[0].message_count >= 1);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_codex_read_preview_extracts_messages() {
        let root = temp_dir("codex-prev");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("rollout-p-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"p1\",\"cwd\":\"/p\"}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello world\"}]}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi there\"}]}}\n").unwrap();

        let reader = CodexSessionReader {
            sessions_dir: root.clone(),
        };
        let msgs = reader.read_preview("p1", 1000).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert!(msgs[0].text.contains("hello world"));
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].text.contains("hi there"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_is_within_root_rejects_escape() {
        let root = Path::new("/home/u/.codex/sessions");
        // 正常子路径
        assert!(is_within_root(
            root,
            Path::new("/home/u/.codex/sessions/2026/a.jsonl")
        ));
        // 越界
        assert!(!is_within_root(
            root,
            Path::new("/home/u/.codex/../evil.jsonl")
        ));
        assert!(!is_within_root(root, Path::new("/etc/passwd")));
    }

    #[test]
    fn test_codex_list_corrupt_file_marked_unparseable() {
        let root = temp_dir("codex-corrupt");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        // 合法
        fs::write(
            day.join("rollout-good-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"g1\",\"cwd\":\"/p\"}}\n",
        )
        .unwrap();
        // 损坏
        fs::write(day.join("rollout-bad-2.jsonl"), "this is not json\n").unwrap();

        let reader = CodexSessionReader {
            sessions_dir: root.clone(),
        };
        let list = reader.list_sessions();
        assert_eq!(list.len(), 2, "损坏文件仍应被列出");
        let bad = list.iter().find(|m| !m.parseable).expect("应有不可解析项");
        assert!(!bad.parseable);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_delete_moves_file_away() {
        let root = temp_dir("codex-del");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        let f = day.join("rollout-del-1.jsonl");
        fs::write(
            &f,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"d1\"}}\n",
        )
        .unwrap();
        assert!(f.exists());

        let reader = CodexSessionReader {
            sessions_dir: root.clone(),
        };
        let trashed = temp_dir("trashbin");
        let fake_delete = |p: &Path| -> anyhow::Result<()> {
            let dest = trashed.join(p.file_name().unwrap());
            fs::rename(p, dest)?;
            Ok(())
        };
        let res = delete_one_with(&reader, "d1", &fake_delete);
        assert!(res.ok, "删除应成功: {:?}", res.error);
        assert!(!f.exists(), "文件应已被移走");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_claude_list_extracts_cwd_and_title() {
        let root = temp_dir("claude-list");
        let proj = root.join("-Users-u-Desktop-power");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("sess-1.jsonl"),
            "{\"type\":\"queue-operation\",\"sessionId\":\"sess-1\",\"timestamp\":\"2026-05-28T02:10:56.578Z\"}\n\
             {\"type\":\"ai-title\",\"title\":\"修复登录bug\"}\n\
             {\"type\":\"user\",\"cwd\":\"/Users/u/Desktop/power\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n").unwrap();

        let reader = ClaudeSessionReader {
            projects_dir: root.clone(),
        };
        let list = reader.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "sess-1");
        assert_eq!(list[0].tool, "claude-code");
        assert_eq!(list[0].cwd, "/Users/u/Desktop/power");
        assert_eq!(list[0].title.as_deref(), Some("修复登录bug"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_filter_by_search() {
        let metas = vec![
            SessionMeta {
                id: "a".into(),
                tool: "codex".into(),
                cwd: "/x/proj".into(),
                title: None,
                started_at: 0,
                modified_at: 0,
                size_bytes: 0,
                message_count: 0,
                parseable: true,
            },
            SessionMeta {
                id: "b".into(),
                tool: "codex".into(),
                cwd: "/y/other".into(),
                title: Some("登录".into()),
                started_at: 0,
                modified_at: 0,
                size_bytes: 0,
                message_count: 0,
                parseable: true,
            },
        ];
        let r = apply_filters(metas.clone(), None, Some("proj"));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "a");
        let r2 = apply_filters(metas, None, Some("登录"));
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].id, "b");
    }

    #[test]
    fn test_delete_missing_file_is_ok() {
        let root = temp_dir("codex-del-missing");
        fs::create_dir_all(&root).unwrap();
        let reader = CodexSessionReader {
            sessions_dir: root.clone(),
        };
        let res = delete_one(&reader, "does-not-exist");
        assert!(res.ok, "目标不存在应视为成功");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_decode_project_dir_windows_drive() {
        assert_eq!(
            decode_project_dir("C-Users-u-Desktop-power"),
            "C:\\Users\\u\\Desktop\\power"
        );
        assert_eq!(decode_project_dir("c-users-x"), "c:\\users\\x");
        assert_eq!(decode_project_dir("C-"), "C:");
    }

    #[test]
    fn test_decode_project_dir_unix() {
        assert_eq!(
            decode_project_dir("-Users-u-Desktop-power"),
            "/Users/u/Desktop/power"
        );
        // 单字母 Unix 目录名不能被误判为 Windows 盘符
        assert_eq!(decode_project_dir("-u"), "/u");
        assert_eq!(decode_project_dir(""), "");
        assert_eq!(decode_project_dir("-"), "/");
    }

    #[test]
    fn test_resolve_path_no_substring_mismatch() {
        let root = temp_dir("resolve-exact");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        // id "ab" 与 "abc" 前缀相同：必须精确命中，禁止 contains 子串匹配
        fs::write(
            day.join("rollout-abc-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"abc\",\"cwd\":\"/p\"}}\n",
        )
        .unwrap();
        fs::write(
            day.join("rollout-ab-2.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"ab\",\"cwd\":\"/p\"}}\n",
        )
        .unwrap();

        let reader = CodexSessionReader {
            sessions_dir: root.clone(),
        };
        let p = reader.resolve_path("abc").unwrap();
        assert!(p.to_string_lossy().ends_with("rollout-abc-1.jsonl"));
        // id "ab" 应精确命中 ab 文件，而不是误命中 abc 文件
        let p2 = reader.resolve_path("ab").unwrap();
        assert!(p2.to_string_lossy().ends_with("rollout-ab-2.jsonl"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_count_lines_sampling_large_file() {
        let root = temp_dir("count-sampling");
        fs::create_dir_all(&root).unwrap();
        let f = root.join("big.jsonl");
        // 2500 行 * ~1KB 每行 > 采样上限
        let mut content = String::new();
        for _ in 0..2500 {
            content.push_str(&"x".repeat(1024));
            content.push('\n');
        }
        fs::write(&f, &content).unwrap();
        let n = count_lines(&f);
        assert!((2000..=3200).contains(&n), "外推估算应在合理范围，实际 {n}");
        fs::remove_dir_all(&root).ok();
    }
}
