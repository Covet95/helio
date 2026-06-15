# Helio

AI CLI 工具的 API 配置切换器。把 API 凭据与共享配置（权限 / Hooks / MCP / Skills）分层存储，切换时只换 API，其余不动。

## 特性

- 配置分层：API 凭据与共享配置分离，避免重复维护
- 切换零丢失：只改 API URL/Key，permissions / hooks / MCP / skills 完整保留
- Rust 实现，启动快、占用低
- 原子写入 + 自动备份 + 轮转清理
- 单文件数据库，便于备份与团队共享

## 安装

### 桌面应用（Helio GUI，推荐）

```bash
./run.sh   # 打包 .app 并打开（构建产物在 target/release/bundle/）
```

或手动打包 `.dmg` / `.app`：

```bash
cd src-tauri && cargo tauri build
# 安装包：target/release/bundle/dmg/Helio_<版本>_aarch64.dmg
```

### 命令行（CLI）

```bash
cargo build --release
sudo cp target/release/switch-api /usr/local/bin/
```

## 使用

```bash
switch-api init claude-code
switch-api profile add official --url https://api.anthropic.com --key YOUR_KEY
switch-api switch claude-code official
switch-api status
switch-api export --output backup.db
```

## 支持的工具

| 工具 | 配置文件 | 格式 | API 凭据位置 |
|------|---------|------|-------------|
| Claude Code | `~/.claude/settings.local.json` | JSON | `env.ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` |
| Codex | `~/.codex/config.toml` + `auth.json` | TOML + JSON | `auth.json` 的 `OPENAI_API_KEY` + `model_providers.<id>.base_url` |
| Gemini CLI | `~/.gemini/settings.json` + `.env` | JSON + env | `.env` 的 `GEMINI_API_KEY` / `GOOGLE_GEMINI_BASE_URL` |
| OpenCode | `~/.config/opencode/opencode.json` | JSON | `provider.<id>.options.apiKey` / `baseURL` |

工具特定说明：

- **Gemini CLI**：API key 存在 `~/.gemini/.env`（不在 settings.json）。切换时只重写 `.env` 中的 `GEMINI_API_KEY` 和 `GOOGLE_GEMINI_BASE_URL`，保留其他环境变量；settings.json（mcpServers、model、security）不动。需先设 `security.auth.selectedType` 为 `gemini-api-key`。
- **OpenCode**：profile 的 `provider` 字段（小写）作为 provider id，写入 `provider.<id>.options`。切换某个 provider 不影响其他 provider，mcp/permission/tools/agent 全部保留。
- **Codex**：API key 存在 `~/.codex/auth.json` 的 `OPENAI_API_KEY`（不在 config.toml）。切换时只重写 auth.json 的 key 与 `config.toml` 中对应 provider 的 `base_url`，保留 `wire_api` 等协议字段、MCP servers、projects 等共享配置。TOML 往返会规范化格式并丢失注释（可在 `config.backup.*.toml` 中找回）。

## 架构

```
API Profile (只存 API 信息)
    ↓
Shared Config (permissions, hooks, MCP, skills)
    ↓
适配器 (Claude Code / Codex / Gemini CLI / OpenCode)
    ↓
配置文件 (原子写入)
```

## 路线图

- [x] CLI 核心功能
- [x] Claude Code / Codex / Gemini CLI / OpenCode 适配器
- [x] GUI（Tauri）
- [ ] OpenClaw / Hermes 适配器
- [ ] MCP 统一管理面板 / Proxy 模式 / Usage 统计

## 许可证

MIT
