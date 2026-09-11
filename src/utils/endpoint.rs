//! Single source of truth for relay endpoint normalization.
//!
//! The provider compat table, the Anthropic suffix list and base-URL shaping
//! used to live in copies across probe and the OpenCode adapter, and already
//! diverged once. All URL shaping for switching and probing goes through here.

/// Anthropic-protocol compat suffixes stripped before OpenAI-protocol use.
pub const COMPAT_SUFFIXES: &[&str] = &[
    "/api/claudecode",
    "/api/anthropic",
    "/apps/anthropic",
    "/api/coding",
    "/claudecode",
    "/anthropic",
    "/step_plan",
    "/coding",
    "/claude",
];

/// (host pattern, canonical OpenAI-compatible base).
const PROVIDER_COMPAT_BASES: &[(&str, &str)] = &[
    ("bigmodel.cn", "https://open.bigmodel.cn/api/paas/v4"),
    ("z.ai", "https://api.z.ai/api/paas/v4"),
    ("deepseek.com", "https://api.deepseek.com/v1"),
    ("moonshot.cn", "https://api.moonshot.cn/v1"),
    ("openrouter.ai", "https://openrouter.ai/api/v1"),
    ("siliconflow.cn", "https://api.siliconflow.cn/v1"),
    (
        "dashscope.aliyuncs.com",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
    ),
];

/// Lowercased hostname of a URL-ish string (scheme/userinfo/port/path cut).
/// Never fails: unparseable input yields empty string and matches nothing.
pub fn host_of(api_url: &str) -> String {
    let s = api_url.trim();
    let after_scheme = match s.find("://") {
        Some(i) => &s[i + 3..],
        None => s,
    };
    let mut end = after_scheme.len();
    for delim in ["/", "?", "#"] {
        if let Some(i) = after_scheme.find(delim) {
            end = end.min(i);
        }
    }
    let hostport = &after_scheme[..end];
    let host = match hostport.rfind("@") {
        Some(i) => &hostport[i + 1..],
        None => hostport,
    };
    // Strip port. Bracketed IPv6 kept whole; bare IPv6 matches nothing downstream.
    let host = if host.starts_with("[") {
        match host.find("]") {
            Some(i) => &host[..i + 1],
            None => host,
        }
    } else {
        match host.rfind(":") {
            Some(i) => &host[..i],
            None => host,
        }
    };
    host.to_lowercase()
}

fn host_matches(host: &str, pattern: &str) -> bool {
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

/// Canonical OpenAI-compatible base for known relay hosts.
/// Hostname-based: custom domains merely containing the pattern are kept.
pub fn provider_compat_base(api_url: &str) -> Option<String> {
    let host = host_of(api_url);
    if host.is_empty() {
        return None;
    }
    PROVIDER_COMPAT_BASES
        .iter()
        .find(|(pat, _)| host_matches(&host, pat))
        .map(|(_, base)| base.to_string())
}

/// Strip Anthropic-compat suffixes (single pass each, historical behavior).
pub fn strip_compat_suffixes(base: &str) -> String {
    let mut out = base.to_string();
    for suffix in COMPAT_SUFFIXES {
        if let Some(stripped) = out.strip_suffix(suffix) {
            out = stripped.trim_end_matches("/").to_string();
        }
    }
    out
}

pub fn trim_base(api_url: &str) -> String {
    api_url.trim().trim_end_matches("/").to_string()
}

/// Normalized OpenAI-compatible baseURL for writing provider configs:
/// explicit version roots win, then provider table, then suffix strip, then /v1.
pub fn normalize_openai_compatible_base_url(api_url: &str) -> String {
    let base = trim_base(api_url);
    if base.is_empty() {
        return String::new();
    }
    if base.ends_with("/v1") || base.ends_with("/paas/v4") {
        return base;
    }
    if let Some(provider_base) = provider_compat_base(&base) {
        return provider_base;
    }
    let stripped = strip_compat_suffixes(&base);
    let base = if stripped.is_empty() { base } else { stripped };
    if base.ends_with("/v1") || base.ends_with("/paas/v4") {
        return base;
    }
    format!("{base}/v1")
}

/// Strip the first matching Anthropic-compat suffix only (probe composition).
pub fn strip_first_compat_suffix(base: &str) -> String {
    for suffix in COMPAT_SUFFIXES {
        if let Some(stripped) = base.strip_suffix(suffix) {
            return stripped.trim_end_matches("/").to_string();
        }
    }
    base.to_string()
}

/// Probe-side OpenAI-compat base (historical composition kept byte-identical:
/// provider table, else single suffix strip; version appending happens downstream).
pub fn openai_compat_base(api_url: &str) -> String {
    if let Some(url) = provider_models_url(api_url) {
        return url
            .trim_end_matches("/models")
            .trim_end_matches("/")
            .to_string();
    }
    let base = trim_base(api_url);
    strip_first_compat_suffix(&base)
}

/// Models-list URL for known relays (probe use). DeepSeek serves /models
/// without the /v1 prefix; every other table entry appends /models to base.
pub fn provider_models_url(api_url: &str) -> Option<String> {
    let host = host_of(api_url);
    if host.is_empty() {
        return None;
    }
    if host_matches(&host, "deepseek.com") {
        return Some("https://api.deepseek.com/models".to_string());
    }
    provider_compat_base(api_url).map(|base| format!("{base}/models"))
}

/// The only host serving native Anthropic and no OpenAI-compatible endpoint.
pub fn is_anthropic_official(api_url: &str) -> bool {
    host_of(api_url) == "api.anthropic.com"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_extraction() {
        assert_eq!(host_of("https://api.deepseek.com/v1"), "api.deepseek.com");
        assert_eq!(host_of("http://127.0.0.1:8317/v1"), "127.0.0.1");
        assert_eq!(host_of("https://user:pass@Example.COM/x"), "example.com");
        assert_eq!(host_of("  deepseek.com  "), "deepseek.com");
        assert_eq!(host_of(""), "");
    }

    #[test]
    fn provider_table_matches_hosts_not_substrings() {
        assert_eq!(
            provider_compat_base("https://api.deepseek.com/v1"),
            Some("https://api.deepseek.com/v1".to_string())
        );
        // Custom domains merely containing the pattern are left alone.
        assert_eq!(
            provider_compat_base("https://deepseek.com.evil.example/v1"),
            None
        );
        assert_eq!(provider_compat_base("https://notdeepseek.com/v1"), None);
        // Subdomains of known hosts still match.
        assert_eq!(
            provider_compat_base("https://open.bigmodel.cn/api/anthropic"),
            Some("https://open.bigmodel.cn/api/paas/v4".to_string())
        );
    }

    #[test]
    fn models_url_keeps_deepseek_exception() {
        assert_eq!(
            provider_models_url("https://api.deepseek.com/v1"),
            Some("https://api.deepseek.com/models".to_string())
        );
        assert_eq!(
            provider_models_url("https://api.moonshot.cn/v1"),
            Some("https://api.moonshot.cn/v1/models".to_string())
        );
        assert_eq!(provider_models_url("https://unknown.example/v1"), None);
    }

    #[test]
    fn anthropic_official_detection() {
        assert!(is_anthropic_official("https://api.anthropic.com"));
        assert!(is_anthropic_official("https://api.anthropic.com/v1"));
        assert!(!is_anthropic_official("https://api.deepseek.com/anthropic"));
        assert!(!is_anthropic_official(
            "https://api.anthropic.com.evil.example"
        ));
    }
}
