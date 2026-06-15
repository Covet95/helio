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

    fn read_preview(&self, _id: &str, _max_chars: usize) -> anyhow::Result<Vec<PreviewMessage>> {
        // Task 6 实现
        Ok(Vec::new())
    }

    fn resolve_path(&self, id: &str) -> Option<PathBuf> {
        // Task 7 实现
        let _ = id;
        None
    }
}

/// 数 jsonl 行数（消息数估算）
fn count_lines(path: &Path) -> usize {
    File::open(path)
        .map(|f| BufReader::new(f).lines().count())
        .unwrap_or(0)
}

/// 解析 ISO8601 时间为 unix 秒
fn parse_iso8601(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
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
    fn test_is_within_root_rejects_escape() {
        let root = Path::new("/home/u/.codex/sessions");
        // 正常子路径
        assert!(is_within_root(root, Path::new("/home/u/.codex/sessions/2026/a.jsonl")));
        // 越界
        assert!(!is_within_root(root, Path::new("/home/u/.codex/../evil.jsonl")));
        assert!(!is_within_root(root, Path::new("/etc/passwd")));
    }
}
