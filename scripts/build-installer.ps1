#Requires -Version 5.1
<#
.SYNOPSIS
  构建墨笺 (md-work) 的 release 便携包与 NSIS 安装包，用于本机验证「发布 → 安装 → 使用」。

.DESCRIPTION
  封装已有 xtask 流程：
    1) （可选）cargo test --workspace
    2) cargo run -p xtask -- icon     # 若 assets/icon.ico 缺失
    3) cargo run -p xtask -- package  # release 构建 → dist\md-work-<ver>-win64\ + .zip
    4) cargo run -p xtask -- installer # makensis → dist\md-work-<ver>-setup.exe

  本机 NSIS 默认：E:\apps\nsis。也可用 -NsisHome / 环境变量 NSIS_HOME / PATH。

.PARAMETER NsisHome
  NSIS 安装根目录（内含 makensis.exe）。默认 E:\apps\nsis。

.PARAMETER SkipTests
  跳过 cargo test（开发阶段已测过时可加）。

.PARAMETER SkipIcon
  不重新生成 icon.ico（已有且未改 SVG 时可加）。

.PARAMETER SkipPackage
  跳过 release 构建与便携包，仅重打安装包（需 dist 里已有 md-work.exe）。

.EXAMPLE
  .\scripts\build-installer.ps1

.EXAMPLE
  .\scripts\build-installer.ps1 -SkipTests

.EXAMPLE
  .\scripts\build-installer.ps1 -NsisHome 'D:\tools\nsis' -SkipPackage
#>
[CmdletBinding()]
param(
    [string] $NsisHome = $(if ($env:NSIS_HOME) { $env:NSIS_HOME } else { 'E:\apps\nsis' }),
    [switch] $SkipTests,
    [switch] $SkipIcon,
    [switch] $SkipPackage
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step([string] $Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Resolve-RepoRoot {
    $here = $PSScriptRoot
    if (-not $here) {
        $here = Split-Path -Parent $MyInvocation.MyCommand.Path
    }
    $root = (Resolve-Path (Join-Path $here '..')).Path
    if (-not (Test-Path (Join-Path $root 'Cargo.toml'))) {
        throw "无法定位仓库根目录（相对脚本上级应有 Cargo.toml）：$root"
    }
    return $root
}

function Find-Makensis([string] $PreferredHome) {
    $candidates = New-Object System.Collections.Generic.List[string]
    if ($PreferredHome) {
        [void]$candidates.Add((Join-Path $PreferredHome 'makensis.exe'))
    }
    if ($env:ProgramFiles) {
        [void]$candidates.Add((Join-Path $env:ProgramFiles 'NSIS\makensis.exe'))
    }
    $pf86 = ${env:ProgramFiles(x86)}
    if ($pf86) {
        [void]$candidates.Add((Join-Path $pf86 'NSIS\makensis.exe'))
    }

    foreach ($path in $candidates) {
        if ($path -and (Test-Path -LiteralPath $path)) {
            return (Resolve-Path -LiteralPath $path).Path
        }
    }

    $cmd = Get-Command makensis -ErrorAction SilentlyContinue
    if ($cmd -and $cmd.Source) {
        return $cmd.Source
    }

    return $null
}

function Get-WorkspaceVersion([string] $Root) {
    $cargoToml = Join-Path $Root 'Cargo.toml'
    $text = Get-Content -LiteralPath $cargoToml -Raw
    if ($text -match '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }
    throw "无法从 Cargo.toml 的 [workspace.package] 读取 version"
}

function Invoke-Cargo {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Root,
        [Parameter(Mandatory = $true)]
        [string[]] $CargoArgs
    )
    Push-Location $Root
    try {
        & cargo @CargoArgs
        if ($LASTEXITCODE -ne 0) {
            throw ("cargo " + ($CargoArgs -join ' ') + " 失败 (exit $LASTEXITCODE)")
        }
    }
    finally {
        Pop-Location
    }
}

$root = Resolve-RepoRoot
$version = Get-WorkspaceVersion $root
$makensis = Find-Makensis $NsisHome

Write-Host "仓库:     $root"
Write-Host "版本:     $version"
Write-Host "NSIS 根:  $NsisHome"

if (-not $makensis) {
    $hint = Join-Path $NsisHome 'makensis.exe'
    throw "未找到 makensis.exe。请确认 NSIS 3 已安装，或指定 -NsisHome / 设置环境变量 NSIS_HOME。已尝试: $hint、Program Files\NSIS、PATH。"
}
Write-Host "makensis: $makensis"

# 让本进程及子进程（xtask -> Command::new("makensis")）都能找到 makensis
$nsisDir = Split-Path -Parent $makensis
$env:Path = "$nsisDir;$env:Path"
$env:NSIS_HOME = $nsisDir

if (-not $SkipTests) {
    Write-Step "运行 cargo test --workspace"
    Invoke-Cargo -Root $root -CargoArgs @('test', '--workspace')
}
else {
    Write-Host "已跳过测试 (-SkipTests)"
}

$iconPath = Join-Path $root 'assets\icon.ico'
if ((-not $SkipIcon) -or (-not (Test-Path -LiteralPath $iconPath))) {
    Write-Step "生成图标 (xtask icon)"
    Invoke-Cargo -Root $root -CargoArgs @('run', '-p', 'xtask', '--', 'icon')
}
else {
    Write-Host "已跳过图标 (-SkipIcon)，沿用 $iconPath"
}

if (-not $SkipPackage) {
    Write-Step "Release 构建 + 便携包 (xtask package)"
    Invoke-Cargo -Root $root -CargoArgs @('run', '-p', 'xtask', '--', 'package')
}
else {
    $bundleExe = Join-Path $root ("dist\md-work-$version-win64\md-work.exe")
    if (-not (Test-Path -LiteralPath $bundleExe)) {
        throw "SkipPackage 需要已有便携包，但缺少: $bundleExe；请去掉 -SkipPackage 重跑"
    }
    Write-Host "已跳过 package (-SkipPackage)，沿用现有 dist 产物"
}

Write-Step "编译 NSIS 安装包 (xtask installer)"
Invoke-Cargo -Root $root -CargoArgs @('run', '-p', 'xtask', '--', 'installer')

$setup = Join-Path $root ("dist\md-work-$version-setup.exe")
$zip = Join-Path $root ("dist\md-work-$version-win64.zip")
$bundle = Join-Path $root ("dist\md-work-$version-win64")

if (-not (Test-Path -LiteralPath $setup)) {
    throw "安装包未生成: $setup"
}

Write-Host ""
Write-Host "完成。" -ForegroundColor Green
Write-Host "  安装包:  $setup"
if (Test-Path -LiteralPath $zip) {
    Write-Host "  便携 zip: $zip"
}
Write-Host "  便携目录: $bundle"
Write-Host ""
Write-Host "本机验证发布安装模式建议：" -ForegroundColor Yellow
Write-Host "  1. 关掉正在跑的开发版 md-work.exe"
Write-Host "  2. 运行安装包（或静默：& `"$setup`" /S）"
Write-Host "  3. 从开始菜单 / `$env:LOCALAPPDATA\Programs\md-work\md-work.exe 启动"
Write-Host "  4. 确认工作区、笔记、设置与开发模式行为一致"
Write-Host "  5. 卸载走「应用和功能」或 uninstall.exe（笔记与设置默认保留）"