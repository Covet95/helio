use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
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
            let is_message = payload
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                == Some("message");
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
            // 文件名包含 id，或首行 session_meta.id == id
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let mut matched = name.contains(id);
            if !matched {
                if let Ok(file) = File::open(path) {
                    let mut first = String::new();
                    if BufReader::new(file).read_line(&mut first).is_ok() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) {
                            if v.pointer("/payload/id").and_then(|x| x.as_str()) == Some(id) {
                                matched = true;
                            }
                        }
                    }
                }
            }
            if matched && is_within_root(&self.root(), path) {
                return Some(path.to_path_buf());
            }
        }
        None
    }
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
fn decode_project_dir(name: &str) -> String {
    name.replacen('-', "/", 1).replace('-', "/")
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
        None => return DeleteResult { id: id.into(), tool, ok: true, error: None }, // 不存在=成功
    };
    if !is_within_root(&reader.root(), &path) {
        return DeleteResult { id: id.into(), tool, ok: false, error: Some("路径越界，拒绝删除".into()) };
    }
    if !path.exists() {
        return DeleteResult { id: id.into(), tool, ok: true, error: None };
    }
    match do_delete(&path) {
        Ok(_) => DeleteResult { id: id.into(), tool, ok: true, error: None },
        Err(e) => DeleteResult { id: id.into(), tool, ok: false, error: Some(e.to_string()) },
    }
}

/// 数 jsonl 行数（消息数估算）
fn count_lines(path: &Path) -> usize {
    File::open(path)
        .map(|f| BufReader::new(f).lines().count())
        .unwrap_or(0)
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
pub(crate) fn apply_filters(metas: Vec<SessionMeta>, tool: Option<&str>, search: Option<&str>) -> Vec<SessionMeta> {
    metas.into_iter().filter(|m| {
        if let Some(t) = tool { if m.tool != t { return false; } }
        if let Some(q) = search {
            if q.is_empty() { return true; }
            let hit = m.cwd.contains(q) || m.title.as_deref().map(|t| t.contains(q)).unwrap_or(false);
            if !hit { return false; }
        }
        true
    }).collect()
}

fn reader_for(tool: &str) -> Option<Box<dyn SessionReader>> {
    match tool {
        "codex" => Some(Box::new(CodexSessionReader::new())),
        "claude-code" => Some(Box::new(ClaudeSessionReader::new())),
        _ => None,
    }
}

#[tauri::command]
pub async fn list_sessions(tool: Option<String>, search: Option<String>) -> Result<Vec<SessionMeta>, String> {
    let mut all = Vec::new();
    for r in all_readers() { all.extend(r.list_sessions()); }
    // 默认按修改时间倒序
    all.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
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
pub struct DeleteItem { pub tool: String, pub id: String }

#[tauri::command]
pub async fn delete_sessions(items: Vec<DeleteItem>) -> Result<Vec<DeleteResult>, String> {
    let mut out = Vec::new();
    for it in items {
        match reader_for(&it.tool) {
            Some(r) => out.push(delete_one(r.as_ref(), &it.id)),
            None => out.push(DeleteResult { id: it.id, tool: it.tool, ok: false, error: Some("未知工具".into()) }),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn cleanup_sessions(tool: Option<String>, older_than_days: i64) -> Result<Vec<DeleteResult>, String> {
    let now = chrono::Utc::now().timestamp();
    let cutoff = now - older_than_days * 86400;
    let mut out = Vec::new();
    for r in all_readers() {
        if let Some(t) = &tool { if r.tool() != t { continue; } }
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

        let reader = CodexSessionReader { sessions_dir: root.clone() };
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

        let reader = CodexSessionReader { sessions_dir: root.clone() };
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
        assert!(is_within_root(root, Path::new("/home/u/.codex/sessions/2026/a.jsonl")));
        // 越界
        assert!(!is_within_root(root, Path::new("/home/u/.codex/../evil.jsonl")));
        assert!(!is_within_root(root, Path::new("/etc/passwd")));
    }

    #[test]
    fn test_codex_list_corrupt_file_marked_unparseable() {
        let root = temp_dir("codex-corrupt");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        // 合法
        fs::write(day.join("rollout-good-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"g1\",\"cwd\":\"/p\"}}\n").unwrap();
        // 损坏
        fs::write(day.join("rollout-bad-2.jsonl"), "this is not json\n").unwrap();

        let reader = CodexSessionReader { sessions_dir: root.clone() };
        let list = reader.list_sessions();
        assert_eq!(list.len(), 2, "损坏文件仍应被列出");
        let bad = list.iter().find(|m| !m.parseable).expect("应有不可解析项");
        assert_eq!(bad.parseable, false);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_delete_moves_file_away() {
        let root = temp_dir("codex-del");
        let day = root.join("2026/06/03");
        fs::create_dir_all(&day).unwrap();
        let f = day.join("rollout-del-1.jsonl");
        fs::write(&f, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"d1\"}}\n").unwrap();
        assert!(f.exists());

        let reader = CodexSessionReader { sessions_dir: root.clone() };
        let trashed = temp_dir("trashbin");
        let fake_delete = |p: &Path| -> anyhow::Result<()> {
            let dest = trashed.join(p.file_name().unwrap());
            fs::rename(p, dest)?; Ok(())
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

        let reader = ClaudeSessionReader { projects_dir: root.clone() };
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
            SessionMeta { id:"a".into(), tool:"codex".into(), cwd:"/x/proj".into(), title:None,
                started_at:0, modified_at:0, size_bytes:0, message_count:0, parseable:true },
            SessionMeta { id:"b".into(), tool:"codex".into(), cwd:"/y/other".into(), title:Some("登录".into()),
                started_at:0, modified_at:0, size_bytes:0, message_count:0, parseable:true },
        ];
        let r = apply_filters(metas.clone(), None, Some("proj"));
        assert_eq!(r.len(), 1); assert_eq!(r[0].id, "a");
        let r2 = apply_filters(metas, None, Some("登录"));
        assert_eq!(r2.len(), 1); assert_eq!(r2[0].id, "b");
    }

    #[test]
    fn test_delete_missing_file_is_ok() {
        let root = temp_dir("codex-del-missing");
        fs::create_dir_all(&root).unwrap();
        let reader = CodexSessionReader { sessions_dir: root.clone() };
        let res = delete_one(&reader, "does-not-exist");
        assert!(res.ok, "目标不存在应视为成功");
        fs::remove_dir_all(&root).ok();
    }
}
