# Helio

Helio 是一个用于管理 AI CLI API 配置的桌面应用和命令行工具。

它把 API 凭据与共享配置分开管理。切换 API 时，只更新受管的 API 配置，尽量保留权限、Hooks、MCP、Skills 等现有设置。

## 功能

- 管理多个 API Profile，并在不同工具之间快速切换
- 支持多 Key、活跃 Key、探活和故障转移
- 保留权限、Hooks、MCP、Skills 等共享配置
- 配置切换使用原子写入、事务回滚和自动备份
- 支持数据库备份、便携备份与恢复
- 支持桌面 GUI 和 CLI

## 安装

从 [GitHub Releases](https://github.com/Covet95/helio/releases) 下载对应平台的安装包：

| 平台 | 安装包 |
| --- | --- |
| Windows | `Helio_*_x64-setup.exe` |
| macOS | `.dmg`，支持 Apple Silicon 和 Intel |
| Linux | `.deb` 或 AppImage |

macOS 安装包当前未进行 Apple 公证。如果系统提示无法验证或应用已损坏，安装后执行：

```bash
xattr -cr /Applications/Helio.app
```

## 快速开始

### GUI

1. 安装并打开 Helio。
2. 创建一个 Profile，填写工具、API 地址、模型和 API Key。
3. 保存后使用「切换」应用配置。
4. 使用「测试模型」确认当前 API 可用。

### CLI

```bash
switch-api init claude-code
switch-api profile add official --url https://api.anthropic.com --key YOUR_KEY
switch-api switch claude-code official
switch-api status
```

常用备份命令：

```bash
switch-api export --output backup.db
switch-api import backup.db
```

## 支持的工具

- Claude Code
- Codex
- Pi
- OpenCode
- Hermes
- OpenClaw

不同工具的 API 协议和模型配置由 Helio 按工具类型处理。模型探活失败时，先检查 API 地址、协议类型、模型名称和 API Key。

## 备份与恢复

优先使用 GUI 中的「便携备份」迁移 Helio 配置。便携备份可以包含：

- API Profile 和多 Key 配置
- 共享配置，包括 MCP
- Helio 管理的 Skills

导入前会校验备份内容，校验通过后才替换当前数据库和受管配置。恢复过程中会保留必要的备份，便于回退。

备份文件可能包含明文 API Key。请将数据库、导出文件和备份文件保存在私有目录中，不要上传到公开仓库或分享给他人。

## 数据位置

Helio 使用用户主目录下的工具配置目录，并在本机保存自己的配置数据库。卸载应用不会自动替用户删除这些配置；如需迁移或清理，请先使用备份和导出功能。

## 许可证

MIT
