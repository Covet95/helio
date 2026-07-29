#!/bin/bash
# Helio macOS 运行脚本 —— 只打 .app（太阳 dock 图标），绝不跑 dev。
# Windows 请用 run.ps1 / run.cmd。
# dev 跑的是裸二进制，macOS 给默认 dock 图标（用户已明确要求永久避免）。
set -e
cd "$(dirname "$0")/src-tauri"

# 关掉旧实例（dev 裸二进制 + .app）
pkill -f "Helio.app/Contents/MacOS" 2>/dev/null || true
pkill -f "target/debug/switch-api-gui" 2>/dev/null || true
pkill -f "target/release/switch-api-gui" 2>/dev/null || true

# 打包 .app（release，太阳图标自带）
echo "▶ cargo tauri build …"
cargo tauri build 2>&1 | tail -2

# 刷新 macOS 图标缓存（否则可能显示旧的缓存图标）+ 打开
killall Dock 2>/dev/null || true
killall Finder 2>/dev/null || true
sleep 1
open ../target/release/bundle/macos/Helio.app
echo "✓ Helio.app 已打开（dock = 太阳图标 + 最新代码）"
