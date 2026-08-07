# Helio

AI CLI 工具的 API 配置切换器。把 API 凭据与共享配置（权限 / Hooks / MCP / Skills）分层存储，切换时只换 API，其余不动。

## 特性

- 配置分层：API 凭据与共享配置分离，避免重复维护
- 切换零丢失：只改 API URL/Key，permissions / hooks / MCP / skills 完整保留
- Rust 实现，启动快、占用低
- 原子写入 + 自动备份 + 轮转清理
- 单文件数据库，便于备份与团队共享

## 安装

### GitHub 发版包（推荐）

正式多端安装包由 **GitHub Actions** 构建，见 [Releases](https://github.com/Covet95/helio/releases)：

| 平台 | 下载 |
|------|------|
| Windows | NSIS `Helio_*_x64-setup.exe` |
| macOS | `.dmg`（Apple Silicon / Intel） |
| Linux | `.deb` / AppImage |

维护者发版：同步更新 `Cargo.toml`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`gui/package.json` 与 `gui/package-lock.json` 的版本号 → 推 `main` → `git tag vX.Y.Z && git push origin vX.Y.Z`（工作流 [release.yml](.github/workflows/release.yml)）。

> **macOS 提示「已损坏」？** 当前发版包未做 Apple 公证（省钱路线）。装好后执行 `xattr -cr /Applications/Helio.app`，或右键 → 打开。

### 前置依赖

| 平台 | 依赖 |
|------|------|
| 通用 | Rust（stable）、Node.js 18+ |
| macOS | Xcode CLT |
| Windows | [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)、MSVC 构建工具（Visual Studio Build Tools）、`cargo install tauri-cli --version "^2"` |
| Linux | `webkit2gtk` / 发行版对应的 Tauri 系统依赖 |

配置目录均基于用户主目录（Windows 上即 `%USERPROFILE%\.claude` 等），与各 AI CLI 在 Windows 上的路径约定一致。

### 桌面应用（Helio GUI，推荐）

**macOS**

从 [Releases](https://github.com/Covet95/helio/releases) 下对应芯片的 `.dmg`，拖到「应用程序」。若系统报「已损坏」或无法验证开发者：

```bash
xattr -cr /Applications/Helio.app
```

本机构建：

```bash
./run.sh   # 打包 .app 并打开（构建产物在 target/release/bundle/）
```

或手动打包 `.dmg` / `.app`：

```bash
cargo tauri build
# 安装包：target/release/bundle/dmg/Helio_<版本>_aarch64.dmg
```

**Windows（只打一份 NSIS 安装包，不要再装 MSI）**

```powershell
.\run.ps1
# 产物：target\release\bundle\nsis\Helio_<版本>_x64-setup.exe
```

手动构建：

```powershell
cd gui; npm install; cd ..
cargo install tauri-cli --version "^2"   # 首次
cargo tauri build --bundles nsis
# 安装包（唯一）：target\release\bundle\nsis\Helio_*_x64-setup.exe
# 绿色版：      target\release\Helio.exe
```

> 注意：不要对同一台机器同时安装 NSIS 与 MSI，否则桌面会出现**两个 Helio**。

开发模式（热重载前端）：

```powershell
# 仓库根目录
cargo tauri dev
```

Windows 行为说明：

- **唯一官方安装包**：NSIS `*-setup.exe`（当前用户，安装到 `%LOCALAPPDATA%\Helio`）
- 关闭主窗口会**缩到系统托盘**，不会退出；托盘左键打开，右键切换 profile
- 托盘图标为金橙色太阳（macOS 仍为模板黑图标）
- 需要 **WebView2**（Win10/11 通常自带）

### 命令行（CLI）

**macOS / Linux**

```bash
cargo build --release
sudo cp target/release/switch-api /usr/local/bin/
```

**Windows**

```powershell
cargo build --release
# 可执行文件：target\release\switch-api.exe
# 可选：复制到已在 PATH 中的目录，例如
Copy-Item target\release\switch-api.exe $env:USERPROFILE\bin\
```

## 使用

```bash
switch-api init claude-code
switch-api profile add official --url https://api.anthropic.com --key YOUR_KEY
switch-api switch claude-code official
switch-api status
switch-api export --output backup.db
switch-api import backup.db
```

### 备份与恢复

「备份 / 恢复」页面的**便携备份**是迁移 Helio 管理的 API 档案、共享配置（含 MCP）与 Skills 的推荐方式。导出前会从全部已支持工具读取当前共享配置并写回数据库，然后把数据库一致性快照与 Skills 归档封装进一个私有 `tar.gz` 文件。恢复先校验归档和组件哈希，再导入数据库、恢复 Skills，并把导入档案中已激活的工具配置写回本机。

同名 Skills 在恢复时仍会跳过而不是覆盖；不在 Helio profile 中的原始工具凭据不会被自动收集。单独的「导出数据库」与「导出 Skills」保留给高级恢复和排查场景。

导出是**一致性快照**（`VACUUM INTO`），单文件、含尚未落盘到主文件的已提交写入，权限固定为仅当前用户可读写（0600），且不会改动你选择的导出目录本身的权限。

导入只接受 Helio 自己的数据库：先只读校验（完整性 + `api_profiles` schema 特征），再在私有 staging 副本上跑一遍 schema 迁移，全部通过才原子替换现有库。任一步失败都中止，现有数据不受影响。旧版本导出的备份可以直接导入，替换时自动升级 schema；导入完成后会把其中已激活的档案写回对应工具配置。

替换前会把现有库快照成 `db.backup.<时间戳>.sqlite`（与数据库同目录），最多保留 10 份，超出自动清理；schema 迁移前的 `*.premigrate.*` 备份同样保留 10 份。**这些备份含明文 API Key**，与数据库一样是 0600，转存前请注意。

恢复：关闭 Helio，把想要的 `db.backup.*.sqlite` 通过 `switch-api import` 导入即可（不要手工改名覆盖，那样会漏掉 `-wal` 边车文件的清理）。

### 模型探活（GUI「测试模型」）

探活按 **目标工具 + 协议字段** 发最小请求，与 switch 后的接入语义对齐：

| 工具 | 探活协议 |
|------|----------|
| Claude Code | Anthropic Messages（`x-api-key`，**不**剥 `/anthropic` 后缀改打 OpenAI） |
| Codex | 固定使用 Responses；旧 `wire_api=chat` 会迁移为 Responses |
| Pi | 官方 Google host → generateContent；自定义默认 chat；可按 api_mode/wire_api 走 anthropic/responses |
| OpenCode | Chat Completions（`@ai-sdk/openai-compatible`）或 Responses（`@ai-sdk/openai`），按 Profile 配置 |
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
| Codex | `~/.codex/config.toml` + 可选 `auth.json` | TOML + JSON | `env_key` 指向环境变量，或由 Helio 写入 `auth.json` 的 `OPENAI_API_KEY` |
| Pi | `~/.pi/agent/settings.json` + `auth.json` + 可选 `models.json` | JSON | `auth.json` 按 provider 的 api_key；自定义 endpoint 写 `models.json.providers.<id>` |
| OpenCode | `~/.config/opencode/opencode.json` | JSON | `provider.<id>.options.apiKey` / `baseURL` |
| Hermes | `~/.hermes/config.yaml` (+ 可选 `auth.json`) | YAML | `model.default` / `model.provider=custom:<name>` + `custom_providers[].base_url` / `api_key` |
| OpenClaw | `~/.openclaw/openclaw.json` (+ `agents/main/agent/models.json`) | JSON | `models.providers.<id>.baseUrl` / `apiKey` + `agents.defaults.model.primary` |

工具特定说明：

- **Pi**：凭据写 `~/.pi/agent/auth.json`（按 provider id merge api_key，保留其它 OAuth/key）。官方 base 只动 auth + `settings.json` 的 `defaultProvider`/`defaultModel`；自定义 `api_url` 会 upsert `models.json.providers.<id>`（`baseUrl`/`api`/`apiKey`/model）。不改 skills/extensions/themes/trust。
- **OpenCode**：profile 的 `provider` 字段（小写）作为 provider id，写入 `provider.<id>.options`。`opencode_api_mode` 可选 `chat_completions` 或 `responses`，分别使用 `@ai-sdk/openai-compatible` / `@ai-sdk/openai`。模型列表写入 `provider.<id>.models`；`model_configs` 可配置每个模型的 `name`、`limit`、`options` 与 `variants`（例如 `low`、`high`、`max` 或自定义名称），思考强度/预算分别位于 `options.reasoningEffort` / `options.thinking` 和 Variant 对象中。切换时只清理 Helio 上次管理、但已从 Profile 移除的模型，手动添加的模型与字段保留；切换某个 provider 不影响其他 provider，mcp/permission/tools/agent 全部保留。
- **Codex**：自定义 provider 固定生成 Responses 配置。档案设置 `env_key` 时，`model_providers.<id>.env_key` 指向该环境变量、`requires_openai_auth=false`，且 Helio 不修改 `auth.json`；未设置时，Helio 使用文件凭据模式，将活跃 key 写入 `~/.codex/auth.json`，并设置 `auth_mode=apikey`、`cli_auth_credentials_store=file`。`openai`、`ollama`、`lmstudio` 等保留 ID 自动加 `-custom`。**Amazon Bedrock 是内置 provider**：Helio 写入 `model_provider = "amazon-bedrock"`，可选 AWS 覆盖项写入 `[model_providers.amazon-bedrock.aws]` 的 `profile` / `region`，不要求 API URL/Key，也不修改 `auth.json`。模型目录的推理能力使用 `reasoning_levels`（`minimal` / `low` / `medium` / `high` / `xhigh`）；历史 `supports_reasoning=true` 仅用于导入兼容。自定义 provider 的联网搜索需要同时声明 provider 的 `supports_standalone_web_search=true` 与模型目录的联网搜索能力。切换 config/auth/catalog 任一步失败都会回滚全部受管文件。TOML 往返会规范化格式并丢失注释，时间戳备份仍可用于人工恢复。

> 迁移提示：历史 `wire_api=chat`、`requires_openai_auth=false`（但没有 `env_key`）和 Bearer Token 配置在下次读取/切换时会归一化为 Responses + 文件 API Key 鉴权。数据库、导出、备份及 Codex 凭据文件在 Unix 上会修复为仅当前用户可读写。
- **Hermes**：MVP 支持 custom OpenAI-compatible endpoint。profile 的 `provider` 为 custom 名（如 `freemodel`），写入 `model.provider: custom:freemodel`、`model.default`，并 upsert `custom_providers` 中对应项的 `base_url`/`api_key`/`api_mode`；`context_1m` 决定 `model.context_length`（并镜像到当前 custom provider）：开启 = **1M**；关闭 = 模型感知默认（**Grok 500k**，其它 **200k**）。新建 Hermes profile 默认关闭 1M。mcp_servers、skills、agent、platforms 等保留。switch 时把 Helio 多 key **整表镜像**到 `auth.json` 的 `credential_pool[custom:<name>]`（活跃 key 在前；无文件则创建）。YAML 写回会丢失注释（可在 `config.backup.*.yaml` 找回）。已开 session 需新会话或 `hermes gateway restart` 才读新配置。
- **OpenClaw**：MVP 支持 `models.providers.<id>` custom provider。profile 的 `provider` 为 provider id（如 `cpa`），写入 `baseUrl`/`apiKey`/`api`，并设 `agents.defaults.model.primary = provider/model`；保留 fallbacks、mcp、channels、skills。若存在 `agents/main/agent/models.json` 会同步该 provider。`context_1m` 写入 `models[].contextWindow` 与 `agents.defaults.contextTokens`（开 1M / 关则 Grok 500k、其它 200k）；`max_tokens`（OpenClaw 专用）写入 `models[].maxTokens`（默认新建 128000）。已开 gateway 可能需重启才读新配置。

## 架构

```
API Profile (只存 API 信息)
    ↓
Shared Config (permissions, hooks, MCP, skills)
    ↓
适配器 (Claude Code / Codex / Pi / OpenCode / Hermes / OpenClaw)
    ↓
配置文件 (原子写入)
```

## 路线图

- [x] CLI 核心功能
- [x] Claude Code / Codex / OpenCode 适配器
- [x] GUI（Tauri）
- [x] Hermes 适配器（custom endpoint MVP）
- [x] OpenClaw 适配器（custom provider MVP）
- [x] Pi 适配器（auth.json / models.json merge；移除 Gemini CLI 目标）
- [x] 探活按工具协议对齐 + 同 API 多 Key 池
- [x] Codex 模型目录（model_catalog_json / `/model` 第三方模型名）
- [x] Windows 支持（托盘、NSIS 打包、`run.ps1`）
- [ ] MCP 统一管理面板 / Proxy 模式 / Usage 统计

## 许可证

MIT
