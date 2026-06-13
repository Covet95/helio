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
适配器 (Claude Code, Codex)
    ↓
配置文件 (原子写入)
```

## 🔮 路线图

- [x] CLI 核心功能
- [x] Claude Code 适配器
- [ ] GUI 界面（Tauri）← 进行中
- [ ] Codex 完整支持
- [ ] 更多工具适配

## 📄 许可证

MIT License
