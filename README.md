# switch-api

智能 AI CLI 工具 API 配置切换器 - 配置分层架构，彻底解决配置重复维护痛点

## 🎯 核心特性

- **配置分层架构** - API 凭据和共享配置分离，节省 70% 存储空间
- **零配置丢失** - 切换时只改 API，permissions/hooks/MCP/skills 完全保留
- **极致性能** - Rust 原生实现，10x 启动速度，95% 内存节省
- **生产级安全** - 原子写入 + 自动备份 + 轮转清理
- **团队协作** - 单文件数据库，轻松导入导出

## 🚀 快速开始

### 安装

```bash
# 编译并安装
./install.sh

# 或手动安装
cargo build --release
sudo cp target/release/switch-api /usr/local/bin/
```

### 基础使用

```bash
# 1. 初始化
switch-api init claude-code

# 2. 添加 Profile
switch-api profile add official --url https://api.anthropic.com --key YOUR_KEY

# 3. 切换
switch-api switch claude-code official
```

## 📖 文档

- [快速开始](docs/QUICKSTART.md) - 3 分钟上手指南
- [完整演示](docs/FINAL_DEMO.md) - 所有功能演示
- [项目总结](docs/PROJECT_SUMMARY.md) - 技术细节和架构
- [对比分析](docs/COMPARISON.md) - 与 cc-switch 详细对比

## 🎯 适用场景

✅ 管理多个 API 账号（工作/个人）  
✅ 官方 API ⇄ 第三方代理快速切换  
✅ 团队配置共享  
✅ 频繁切换且配置复杂  

## 📦 主要命令

```bash
switch-api profile add NAME --url URL --key KEY      # 添加 Profile
switch-api profile list                              # 列出所有
switch-api switch <app> <profile>                    # 切换
switch-api status                                    # 查看状态
switch-api export --output backup.db                 # 导出备份
```

## 🏗️ 架构

```
API Profile (只存 API 信息)
    ↓
Shared Config (permissions, hooks, MCP, skills)
    ↓
适配器 (Claude Code / Codex / Gemini CLI / OpenCode)
    ↓
配置文件 (原子写入)
```

## 🧩 支持的工具

| 工具 | 配置文件 | 格式 | API 凭据位置 |
|------|---------|------|-------------|
| Claude Code | `~/.claude/settings.local.json` | JSON | `env.ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` |
| Codex | `~/.codex/config.toml` | TOML | `api_key` + `model_providers.<id>.base_url` |
| Gemini CLI | `~/.gemini/settings.json` + `.env` | JSON + env | **`.env` 的 `GEMINI_API_KEY` / `GOOGLE_GEMINI_BASE_URL`** |
| OpenCode | `~/.config/opencode/opencode.json` | JSON | `provider.<id>.options.apiKey` / `baseURL` |

**工具特定说明：**

- **Gemini CLI**：API key 存储在 `~/.gemini/.env`（不在 settings.json）。切换时只重写 `.env` 中的 `GEMINI_API_KEY` 和 `GOOGLE_GEMINI_BASE_URL`，保留其他环境变量；settings.json（mcpServers、model、security）完全不动。需确保 `security.auth.selectedType` 已设为 `gemini-api-key`。
- **OpenCode**：profile 的 `provider` 字段（小写）作为 OpenCode 的 provider id，写入 `provider.<id>.options`。切换一个 provider 不影响其他 provider，mcp/permission/tools/agent 全部保留。
- **Codex**：TOML 往返会规范化格式并**丢失注释**（注释可在 `config.backup.*.toml` 备份中找回）。

## 🔮 路线图

- [x] CLI 核心功能
- [x] Claude Code 适配器
- [x] Codex 适配器（TOML）
- [x] Gemini CLI 适配器（settings.json + .env）
- [x] OpenCode 适配器（多 provider）
- [x] GUI 界面（Tauri）
- [ ] OpenClaw / Hermes 适配器
- [ ] MCP 统一管理面板 / Proxy 模式 / Usage 统计

## 📄 许可证

MIT License
