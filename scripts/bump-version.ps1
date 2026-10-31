#Requires -Version 5.1
<#
.SYNOPSIS
  将墨笺 (md-work) 工作区版本号升到指定 semver，并同步所有需手工对齐的引用。

.DESCRIPTION
  唯一权威来源是根目录 Cargo.toml 的 [workspace.package].version。
  本脚本会同步更新：
    - Cargo.toml
    - packaging/nsis/md-work.nsi（注释示例与 !define VERSION 回退值）
    - README.md（「当前版本」与示例产物路径）
    - tests/e2e/windows-checklist.md
    - Cargo.lock（通过 cargo check 刷新工作区成员版本）

  运行时自动派生、无需改文件的引用（env!("CARGO_PKG_VERSION")、xtask、
  VERSIONINFO、设置页、CLI --version、标签校验等）在下次构建后自然对齐。

.PARAMETER Version
  目标版本，形如 1.2.3（不含 v 前缀）。与 -Major/-Minor/-Patch 互斥。

.PARAMETER Major
  主版本 +1，次/修订归零（1.1.1 → 2.0.0）。

.PARAMETER Minor
  次版本 +1，修订归零（1.1.1 → 1.2.0）。

.PARAMETER Patch
  修订号 +1（1.1.1 → 1.1.2）。

.PARAMETER DryRun
  只打印将要改动的内容，不写盘、不跑 cargo。

.PARAMETER SkipCheck
  改完文件后跳过 cargo check（不刷新 Cargo.lock；一般不要加）。

.EXAMPLE
  .\scripts\bump-version.ps1 1.1.2

.EXAMPLE
  .\scripts\bump-version.ps1 -Patch

.EXAMPLE
  .\scripts\bump-version.ps1 -Minor -DryRun
#>
[CmdletBinding(DefaultParameterSetName = 'Explicit')]
param(
    [Parameter(ParameterSetName = 'Explicit', Position = 0, Mandatory = $true)]
    [string] $Version,

    [Parameter(ParameterSetName = 'Major')]
    [switch] $Major,

    [Parameter(ParameterSetName = 'Minor')]
    [switch] $Minor,

    [Parameter(ParameterSetName = 'Patch')]
    [switch] $Patch,

    [switch] $DryRun,
    [switch] $SkipCheck
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

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

function Get-WorkspaceVersion([string] $Root) {
    $cargoToml = Join-Path $Root 'Cargo.toml'
    $text = Get-Content -LiteralPath $cargoToml -Raw
    if ($text -match '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }
    throw "无法从 Cargo.toml 的 [workspace.package] 读取 version"
}

function Test-SemVer([string] $Value) {
    return $Value -match '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$'
}

function Split-SemVer([string] $Value) {
    if (-not (Test-SemVer $Value)) {
        throw "非法 semver（仅支持 X.Y.Z，不含预发布后缀）：$Value"
    }
    $parts = $Value.Split('.')
    return @{
        Major = [int]$parts[0]
        Minor = [int]$parts[1]
        Patch = [int]$parts[2]
    }
}

function Format-SemVer($Parts) {
    return "{0}.{1}.{2}" -f $Parts.Major, $Parts.Minor, $Parts.Patch
}

function Get-NextVersion {
    param(
        [string] $Current,
        [string] $Explicit,
        [switch] $BumpMajor,
        [switch] $BumpMinor,
        [switch] $BumpPatch
    )

    if ($Explicit) {
        $trimmed = $Explicit.Trim()
        if ($trimmed.StartsWith('v') -or $trimmed.StartsWith('V')) {
            $trimmed = $trimmed.Substring(1)
        }
        if (-not (Test-SemVer $trimmed)) {
            throw "目标版本非法（仅支持 X.Y.Z）：$Explicit"
        }
        return $trimmed
    }

    $cur = Split-SemVer $Current
    if ($BumpMajor) {
        return Format-SemVer @{ Major = $cur.Major + 1; Minor = 0; Patch = 0 }
    }
    if ($BumpMinor) {
        return Format-SemVer @{ Major = $cur.Major; Minor = $cur.Minor + 1; Patch = 0 }
    }
    if ($BumpPatch) {
        return Format-SemVer @{ Major = $cur.Major; Minor = $cur.Minor; Patch = $cur.Patch + 1 }
    }
    throw "必须指定 Version，或使用 -Major / -Minor / -Patch"
}

function Set-Utf8NoBomFile([string] $Path, [string] $Content) {
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Content, $utf8)
}

function Update-CargoToml([string] $Path, [string] $Old, [string] $New) {
    $text = Get-Content -LiteralPath $Path -Raw
    $pattern = '(?ms)(\[workspace\.package\].*?^\s*version\s*=\s*")' + [regex]::Escape($Old) + '(")'
    $replacement = '${1}' + $New + '${2}'
    $updated = [regex]::Replace($text, $pattern, $replacement, 1)
    if ($updated -eq $text) {
        throw "Cargo.toml 中未找到 [workspace.package] version = `"$Old`""
    }
    return $updated
}

function Update-Nsis([string] $Path, [string] $Old, [string] $New) {
    $text = Get-Content -LiteralPath $Path -Raw
    $updated = $text.Replace($Old, $New)
    if ($updated -eq $text) {
        throw "NSIS 脚本中未出现旧版本 $Old：$Path"
    }
    return $updated
}

function Update-Readme([string] $Path, [string] $Old, [string] $New) {
    $text = Get-Content -LiteralPath $Path -Raw
    $marker = [char]0x5F53 + [char]0x524D + [char]0x7248 + [char]0x672C + [char]0xFF1A  # 当前版本：
    $updated = $text
    $updated = $updated.Replace("**$marker$Old**", "**$marker$New**")
    $updated = $updated.Replace("md-work-$Old-", "md-work-$New-")
    if ($updated -eq $text) {
        throw "README.md 中未找到需同步的版本引用（当前版本或示例产物路径）"
    }
    return $updated
}

function Update-Checklist([string] $Path, [string] $Old, [string] $New) {
    $text = Get-Content -LiteralPath $Path -Raw
    $updated = $text.Replace($Old, $New)
    if ($updated -eq $text) {
        throw "验收清单中未出现旧版本 $Old：$Path"
    }
    return $updated
}

$root = Resolve-RepoRoot
$oldVersion = Get-WorkspaceVersion $root
$explicitVersion = $null
if ($PSCmdlet.ParameterSetName -eq 'Explicit') {
    $explicitVersion = $Version
}
$newVersion = Get-NextVersion `
    -Current $oldVersion `
    -Explicit $explicitVersion `
    -BumpMajor:$Major `
    -BumpMinor:$Minor `
    -BumpPatch:$Patch

if ($oldVersion -eq $newVersion) {
    throw "目标版本与当前相同：$oldVersion"
}

$targets = @(
    'Cargo.toml',
    'packaging\nsis\md-work.nsi',
    'README.md',
    'tests\e2e\windows-checklist.md'
)

Write-Host "仓库:   $root"
Write-Host "当前:   $oldVersion"
Write-Host "目标:   $newVersion"
if ($DryRun) {
    Write-Host "模式:   DryRun（不写盘）" -ForegroundColor Yellow
}

$pending = @()
foreach ($rel in $targets) {
    $full = Join-Path $root $rel
    if (-not (Test-Path -LiteralPath $full)) {
        throw "缺少文件: $full"
    }

    switch -Wildcard ($rel) {
        'Cargo.toml' {
            $newText = Update-CargoToml $full $oldVersion $newVersion
        }
        'packaging\nsis\md-work.nsi' {
            $newText = Update-Nsis $full $oldVersion $newVersion
        }
        'README.md' {
            $newText = Update-Readme $full $oldVersion $newVersion
        }
        'tests\e2e\windows-checklist.md' {
            $newText = Update-Checklist $full $oldVersion $newVersion
        }
        default {
            throw "未注册的同步文件: $rel"
        }
    }

    $pending += @{ Path = $full; Rel = $rel; Text = $newText }
    Write-Host "将更新: $rel"
}

if ($DryRun) {
    Write-Host ""
    Write-Host "DryRun 完成，未修改任何文件。" -ForegroundColor Green
    Write-Host "正式升版示例："
    Write-Host "  .\scripts\bump-version.ps1 $newVersion"
    exit 0
}

foreach ($entry in $pending) {
    Set-Utf8NoBomFile -Path $entry.Path -Content $entry.Text
    Write-Host "已写入: $($entry.Rel)" -ForegroundColor Cyan
}

$verified = Get-WorkspaceVersion $root
if ($verified -ne $newVersion) {
    throw "写入后校验失败：Cargo.toml 读到 $verified，期望 $newVersion"
}

if (-not $SkipCheck) {
    Write-Host ""
    Write-Host "==> cargo check --workspace（刷新 Cargo.lock 中的工作区成员版本）" -ForegroundColor Cyan
    Push-Location $root
    try {
        & cargo check --workspace
        if ($LASTEXITCODE -ne 0) {
            throw "cargo check --workspace 失败 (exit $LASTEXITCODE)"
        }
    }
    finally {
        Pop-Location
    }
}
else {
    Write-Host "已跳过 cargo check (-SkipCheck)；请稍后自行运行以刷新 Cargo.lock" -ForegroundColor Yellow
}

Write-Host ""
Write-Host "版本已升至 $newVersion。" -ForegroundColor Green
Write-Host "后续（按需）："
Write-Host "  1. 检查 git diff，确认只改了预期文件"
Write-Host "  2. 提交变更（脚本本身不提交）"
Write-Host "  3. 发版时打标签 v$newVersion 并推送（见 doc/ci.md）"
Write-Host "  4. 需要安装包时再运行 .\scripts\build-installer.ps1"