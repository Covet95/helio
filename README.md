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
| Hermes | `~/.hermes/config.yaml` (+ 可选 `auth.json`) | YAML | `model.default` / `model.provider=custom:<name>` + `custom_providers[].base_url` / `api_key` |
| OpenClaw | `~/.openclaw/openclaw.json` (+ `agents/main/agent/models.json`) | JSON | `models.providers.<id>.baseUrl` / `apiKey` + `agents.defaults.model.primary` |

工具特定说明：

- **Gemini CLI**：API key 存在 `~/.gemini/.env`（不在 settings.json）。切换时只重写 `.env` 中的 `GEMINI_API_KEY` 和 `GOOGLE_GEMINI_BASE_URL`，保留其他环境变量；settings.json（mcpServers、model、security）不动。需先设 `security.auth.selectedType` 为 `gemini-api-key`。
- **OpenCode**：profile 的 `provider` 字段（小写）作为 provider id，写入 `provider.<id>.options`。切换某个 provider 不影响其他 provider，mcp/permission/tools/agent 全部保留。
- **Codex**：API key 存在 `~/.codex/auth.json` 的 `OPENAI_API_KEY`（不在 config.toml）。切换时只重写 auth.json 的 key 与 `config.toml` 中对应 provider 的 `base_url`，保留 `wire_api` 等协议字段、MCP servers、projects 等共享配置。TOML 往返会规范化格式并丢失注释（可在 `config.backup.*.toml` 中找回）。
- **Hermes**：MVP 支持 custom OpenAI-compatible endpoint。profile 的 `provider` 为 custom 名（如 `freemodel`），写入 `model.provider: custom:freemodel`、`model.default`，并 upsert `custom_providers` 中对应项的 `base_url`/`api_key`/`api_mode`；`context_1m` 写入 `model.context_length`（1M）并镜像到当前 custom provider 条目。mcp_servers、skills、agent、platforms 等保留。若 `auth.json` credential_pool 中该 custom 已有 `access_token`，会同步更新以免 pool 影子旧 key。YAML 写回会丢失注释（可在 `config.backup.*.yaml` 找回）。已开 session 需新会话或 `hermes gateway restart` 才读新配置。
- **OpenClaw**：MVP 支持 `models.providers.<id>` custom provider。profile 的 `provider` 为 provider id（如 `cpa`），写入 `baseUrl`/`apiKey`/`api`，并设 `agents.defaults.model.primary = provider/model`；保留 fallbacks、mcp、channels、skills。若存在 `agents/main/agent/models.json` 会同步该 provider。`context_1m` 写入 `models[].contextWindow`（1M）及 `agents.defaults.contextTokens`；`max_tokens`（OpenClaw 专用）写入 `models[].maxTokens`（默认新建 128000）。已开 gateway 可能需重启才读新配置。

## 架构

```
API Profile (只存 API 信息)
    ↓
Shared Config (permissions, hooks, MCP, skills)
    ↓
适配器 (Claude Code / Codex / Gemini CLI / OpenCode / Hermes / OpenClaw)
    ↓
配置文件 (原子写入)
```

## 路线图

- [x] CLI 核心功能
- [x] Claude Code / Codex / Gemini CLI / OpenCode 适配器
- [x] GUI（Tauri）
- [x] Hermes 适配器（custom endpoint MVP）
- [x] OpenClaw 适配器（custom provider MVP）
- [ ] MCP 统一管理面板 / Proxy 模式 / Usage 统计

## 许可证

MIT
