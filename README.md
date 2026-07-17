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

### 模型探活（GUI「测试模型」）

探活按 **目标工具 + 协议字段** 发最小请求，与 switch 后的接入语义对齐：

| 工具 | 探活协议 |
|------|----------|
| Claude Code | Anthropic Messages（`x-api-key`，**不**剥 `/anthropic` 后缀改打 OpenAI） |
| Codex | `wire_api`；**空 = Responses**（与接入默认一致）；`experimental_bearer_token` 参与鉴权 |
| Gemini | 官方 host → generateContent；自定义 URL → OpenAI chat |
| OpenCode | OpenAI-compatible chat |
| Hermes / OpenClaw | `api_mode`：chat / anthropic_messages / responses |

「加载模型列表」仍偏 OpenAI `/models`，列表失败不代表对话不可用。

### 同一 API 多 Key

一个 profile 可挂多把 key，**手动选活跃 key**；`switch` 只写入活跃那一把（目标 CLI 仍是单 key）。

```bash
switch-api profile key add official --key sk-backup --label 备用 --activate
switch-api profile key list official
switch-api profile key use official 备用
switch-api profile key remove official 备用
switch-api profile key failover official          # 按序探活，成功则设活跃；已是 active 则 re-switch
switch-api switch claude-code official --probe   # 写入前探活/failover，全失败不写
```

GUI：档案表单可「测试全部 Key」与 **Failover**（协议级探活，会发最小模型请求）。

### 状态页「检测连通性」（对齐 CC Switch stream_check）

对每个已配置工具的 `api_url` 做 **GET 可达性**探测（默认不自动打外网，需手动点按钮）：

- 任意 HTTP 响应（含 401/403/404）= **可达**；仅 DNS/连接/TLS/超时 = **不可达**
- 不校验 API Key、不发对话请求（可达 ≠ 配置正确）
- 超时 8s、超时重试 1 次；TTFB > 6000ms 标为「较慢」
- 与档案表单「测试模型」职责分离：前者「能不能到」，后者「能不能用」

Hermes switch 时会把 profile 多 key **镜像**到 `~/.hermes/auth.json` 的 `credential_pool[custom:<name>]`（活跃 key 在前）。

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
- **Hermes**：MVP 支持 custom OpenAI-compatible endpoint。profile 的 `provider` 为 custom 名（如 `freemodel`），写入 `model.provider: custom:freemodel`、`model.default`，并 upsert `custom_providers` 中对应项的 `base_url`/`api_key`/`api_mode`；`context_1m` 决定 `model.context_length`（并镜像到当前 custom provider）：开启 = **1M**；关闭 = 模型感知默认（**Grok 500k**，其它 **200k**）。新建 Hermes profile 默认关闭 1M。mcp_servers、skills、agent、platforms 等保留。switch 时把 Helio 多 key **整表镜像**到 `auth.json` 的 `credential_pool[custom:<name>]`（活跃 key 在前；无文件则创建）。YAML 写回会丢失注释（可在 `config.backup.*.yaml` 找回）。已开 session 需新会话或 `hermes gateway restart` 才读新配置。
- **OpenClaw**：MVP 支持 `models.providers.<id>` custom provider。profile 的 `provider` 为 provider id（如 `cpa`），写入 `baseUrl`/`apiKey`/`api`，并设 `agents.defaults.model.primary = provider/model`；保留 fallbacks、mcp、channels、skills。若存在 `agents/main/agent/models.json` 会同步该 provider。`context_1m` 写入 `models[].contextWindow` 与 `agents.defaults.contextTokens`（开 1M / 关则 Grok 500k、其它 200k）；`max_tokens`（OpenClaw 专用）写入 `models[].maxTokens`（默认新建 128000）。已开 gateway 可能需重启才读新配置。

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
- [x] 探活按工具协议对齐 + 同 API 多 Key 池
- [ ] MCP 统一管理面板 / Proxy 模式 / Usage 统计

## 许可证

MIT
