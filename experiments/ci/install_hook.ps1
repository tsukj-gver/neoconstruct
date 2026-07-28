# install_hook.ps1 - 安装 pre-commit hook 到 .git/hooks/（META-CI-1d）
#
# 设计依据：docs/design/CI冒烟门禁设计.md §3.2（pre-commit 默认不安装）
#
# 行为：
#   - 复制 pre-commit.template 到 .git/hooks/pre-commit
#   - 不安装 pre-push hook（本项目禁 push，永不触发；详见设计 §3.3）
#   - 不设置夜间定时（无 CI 服务器；详见设计 §3.4）
#
# 安装后行为（详见 pre-commit.template）：
#   - 每次 git commit 触发 run_smoke.ps1 -Level "L1,L4"（~30s-2min）
#   - 失败时阻断 commit (exit 1)
#   - 紧急情况可用 git commit --no-verify 绕过
#
# 卸载：直接删除 .git/hooks/pre-commit
#
# 编码规范：PascalCase / ValidateXxx / 无裸 Write-Host / try/catch / PowerShell 5.1 兼容

param(
    # 强制覆盖已存在的 pre-commit hook
    [switch]$Force
)

$ErrorActionPreference = "Stop"

# --------------------------------------------------------------------
# 辅助函数（内联轻量日志，避免引入 lib_smoke.ps1 依赖）
# --------------------------------------------------------------------
function Write-HookLog {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Message
        ,
        [ValidateSet("INFO", "OK", "WARN", "ERROR", "STEP")]
        [string]$Level = "INFO"
    )
    $color = "Gray"
    $prefix = "[INFO] "
    switch ($Level) {
        "INFO"  { $prefix = "[INFO] "; $color = "White" }
        "OK"    { $prefix = "[ OK ] "; $color = "Green" }
        "WARN"  { $prefix = "[WARN] "; $color = "Yellow" }
        "ERROR" { $prefix = "[FAIL] "; $color = "Red" }
        "STEP"  { $prefix = "[STEP] "; $color = "Cyan" }
    }
    $ts = (Get-Date -Format "HH:mm:ss")
    Write-Host "${prefix}${ts} ${Message}" -ForegroundColor $color
}

# --------------------------------------------------------------------
# 路径解析
# --------------------------------------------------------------------
$scriptDir = $PSScriptRoot
$templatePath = Join-Path $scriptDir "pre-commit.template"
if (-not (Test-Path -LiteralPath $templatePath)) {
    Write-HookLog "pre-commit.template not found: $templatePath" -Level ERROR
    Write-HookLog "META-CI-1d should have created this file; check repo integrity" -Level WARN
    exit 1
}

# 项目根 = experiments/ci 的上两级
$projectRoot = (Resolve-Path (Join-Path $scriptDir "..\..")).Path
$gitDir = Join-Path $projectRoot ".git"
$hooksDir = Join-Path $gitDir "hooks"
$targetPath = Join-Path $hooksDir "pre-commit"

# --------------------------------------------------------------------
# 前置检查
# --------------------------------------------------------------------
Write-HookLog "==== pre-commit hook installer ====" -Level STEP
Write-HookLog "Project root : $projectRoot"
Write-HookLog "Template     : $templatePath"
Write-HookLog "Target       : $targetPath"

if (-not (Test-Path -LiteralPath $gitDir)) {
    Write-HookLog ".git directory not found at: $gitDir" -Level ERROR
    Write-HookLog "This script must be run from a git repository" -Level WARN
    exit 1
}

if (-not (Test-Path -LiteralPath $hooksDir)) {
    Write-HookLog "Creating hooks directory: $hooksDir"
    New-Item -ItemType Directory -Path $hooksDir -Force | Out-Null
}

# 检查已存在的 pre-commit hook
if (Test-Path -LiteralPath $targetPath) {
    if (-not $Force) {
        # 检查是否已是本仓库的 hook（通过文件头标识识别）
        $existingContent = Get-Content -LiteralPath $targetPath -Raw -ErrorAction SilentlyContinue
        if ($existingContent -and ($existingContent -match "construct-rs CI smoke gate \(META-CI-1d\)")) {
            Write-HookLog "pre-commit hook already installed (matches this project's template)" -Level OK
            Write-HookLog "To reinstall: install_hook.ps1 -Force" -Level INFO
            exit 0
        }
        Write-HookLog "A pre-commit hook already exists at: $targetPath" -Level ERROR
        Write-HookLog "Refusing to overwrite without -Force (existing hook may be user-configured)" -Level WARN
        Write-HookLog "To overwrite: install_hook.ps1 -Force" -Level INFO
        Write-HookLog "To inspect existing: Get-Content $targetPath" -Level INFO
        exit 1
    }
    Write-HookLog "Overwriting existing hook (-Force specified)" -Level WARN
}

# --------------------------------------------------------------------
# 复制 hook
# --------------------------------------------------------------------
try {
    Copy-Item -LiteralPath $templatePath -Destination $targetPath -Force
    # Git for Windows 的 hook 不需要执行位（用 sh 解释器运行），
    # 但 chmod +x 在 WSL/cygwin 环境下推荐做。本仓库主开发环境是 Windows + Git Bash，
    # Git for Windows 自带 sh，hook 文件无需执行位。
    Write-HookLog "Hook installed: $targetPath" -Level OK
} catch {
    Write-HookLog "Failed to install hook: $_" -Level ERROR
    exit 1
}

# --------------------------------------------------------------------
# 安装后验证（不实际触发 hook，仅检查文件可读）
# --------------------------------------------------------------------
if (-not (Test-Path -LiteralPath $targetPath)) {
    Write-HookLog "Post-install verification failed: file not found" -Level ERROR
    exit 1
}

$size = (Get-Item -LiteralPath $targetPath).Length
if ($size -lt 200) {
    Write-HookLog "Post-install verification warning: file size $size bytes (suspiciously small)" -Level WARN
}

# 验证 hook 文件头部标识（确保复制正确）
$headContent = Get-Content -LiteralPath $targetPath -TotalCount 3 -ErrorAction SilentlyContinue
$foundMarker = $false
foreach ($line in $headContent) {
    if ($line -match "construct-rs CI smoke gate") {
        $foundMarker = $true
        break
    }
}
if (-not $foundMarker) {
    Write-HookLog "Post-install verification failed: marker not found in first 3 lines" -Level ERROR
    exit 1
}

# --------------------------------------------------------------------
# 完成提示
# --------------------------------------------------------------------
Write-HookLog ""
Write-HookLog "==== Installation complete ====" -Level STEP
Write-HookLog "Hook behavior:" -Level INFO
Write-HookLog "  - Triggers on: git commit"
Write-HookLog "  - Runs       : run_smoke.ps1 -Level 'L1,L4' (~30s-2min)"
Write-HookLog "  - On failure : blocks commit (exit 1)"
Write-HookLog "  - On bypass  : git commit --no-verify (emergency only)"
Write-HookLog ""
Write-HookLog "Uninstall: Remove-Item -LiteralPath '$targetPath'" -Level INFO
Write-HookLog "Reinstall: install_hook.ps1 -Force" -Level INFO

exit 0
