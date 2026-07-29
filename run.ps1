# Helio Windows —— 只打「一份」NSIS 安装包（不打 MSI，避免双桌面图标）
# 需要：Rust、Node.js、WebView2 Runtime、cargo-tauri（cargo install tauri-cli --version "^2"）
# tauri-cli 2.x 执行 beforeBuildCommand 时 cwd 为 gui/（frontend 包目录），
# 因此 tauri.conf 使用 `npm run build`；从仓库根目录调用 cargo tauri 即可。
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

function Stop-HelioProcesses {
    Get-Process -Name "Helio" -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

Write-Host "▶ 结束旧的 Helio 进程 …"
Stop-HelioProcesses

if (-not (Test-Path "gui/node_modules")) {
    Write-Host "▶ npm install (gui) …"
    Push-Location gui
    npm install
    if ($LASTEXITCODE -ne 0) { throw "npm install failed" }
    Pop-Location
}

$tauriOk = $false
try {
    cargo tauri --version | Out-Null
    $tauriOk = $true
} catch {}
if (-not $tauriOk) {
    Write-Host "▶ 安装 cargo-tauri …"
    cargo install tauri-cli --version "^2" --locked
}

Write-Host "▶ cargo tauri build --bundles nsis …"
cargo tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) { throw "tauri build failed" }

$setup = Get-ChildItem -Path "target/release/bundle/nsis" -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

$exe = "target/release/Helio.exe"

if ($setup) {
    Write-Host ""
    Write-Host "✓ Windows 安装包已生成（请只安装这一份）："
    Write-Host "  $($setup.FullName)"
    Write-Host ""
    Write-Host "  安装：双击运行，或静默安装 →  & '$($setup.FullName)' /S"
    Write-Host "  安装位置（当前用户）：%LOCALAPPDATA%\Helio"
    Write-Host "  桌面只会有 1 个「Helio」快捷方式"
} else {
    Write-Host "✗ 未找到 NSIS 安装包"
    exit 1
}

if (Test-Path $exe) {
    Write-Host "  绿色版（免安装调试）：$(Resolve-Path $exe)"
}
