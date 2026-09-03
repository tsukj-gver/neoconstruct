# run_l1_quality.ps1 - L1 质量门禁（META-CI-1a）
#
# 设计依据：docs/design/基础设施/CI冒烟门禁设计.md §4.1（L1 命令序列）
#
# 命令序列（任一失败则 L1 FAIL）：
#   1. cargo build --release
#   2. cargo clippy --all-targets -- -D warnings
#   3. cargo fmt --check
#   4. cargo test --lib
#   5. maturin develop --release
#     （PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1）
#
# 输出：
#   - 控制台彩色输出
#   - 返回 L1Result 对象（含每步 PASS/FAIL + 输出摘要），由 run_smoke.ps1 汇总写报告

param(
    # neoconstruct venv 路径（用于 maturin develop）
    [string]$VenvPath

    ,
    # 如果指定，把详细 JSON 报告写入此路径
    [string]$ReportPath
)

$ErrorActionPreference = "Stop"

# 加载共享函数库
. (Join-Path $PSScriptRoot "lib_smoke.ps1")

# 兜底：未传 venv 则解析默认
if (-not $VenvPath) {
    $paths = Resolve-ProjectPaths
    $VenvPath = $paths.CrsVenv
}

# --------------------------------------------------------------------
# L1 命令序列定义
# --------------------------------------------------------------------
# 每项：Label / Script（在 neoconstruct 目录执行的脚本块）
function Invoke-L1Quality {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [string]$CrsDir

        ,
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [string]$TargetVenv
    )

    # PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 必须在 cargo build 阶段就设置
    # （Python 3.14 vs PyO3 0.22.6 最大支持版本 3.13；不设会导致 cargo build 失败）
    # 设计 §4.1 仅在 maturin develop 步骤提到，但实际 cargo build / clippy / test
    # 都涉及 PyO3 编译，统一前置设置。
    $env:PYO3_USE_ABI3_FORWARD_COMPATIBILITY = "1"
    $env:VIRTUAL_ENV = $TargetVenv

    $steps = @(
        @{ Label = "cargo build --release";        Script = { cargo build --release } }
        @{ Label = "cargo clippy --all-targets -D warnings"; Script = { cargo clippy --all-targets -- -D warnings } }
        @{ Label = "cargo fmt --check";             Script = { cargo fmt --check } }
        @{ Label = "cargo test --lib";              Script = { cargo test --lib } }
        @{ Label = "maturin develop --release";     Script = {
            maturin develop --release
        } }
    )

    $results = @()
    $allPassed = $true
    $stepNumber = 0

    foreach ($step in $steps) {
        $stepNumber++
        Write-CiLog "[$stepNumber/5] $($step.Label)" -Level STEP -Indent 1
        $r = Invoke-Step -ScriptBlock $step.Script -Label $step.Label -WorkingDirectory $CrsDir

        # 截断输出摘要（避免单步日志过长）
        $tail = ($r.Output -split "`n")
        $tailPreview = ($tail | Select-Object -Last 6) -join "`n"
        if ($r.Passed) {
            Write-CiLog "PASS ($($step.Label))" -Level OK -Indent 2
        } else {
            Write-CiLog "FAIL ($($step.Label)) exit=$($r.ExitCode)" -Level ERROR -Indent 2
            Write-CiLog "stdout/stderr tail:" -Level ERROR -Indent 3
            Write-Host $tailPreview -ForegroundColor DarkGray
        }

        $results += [pscustomobject]@{
            step       = $stepNumber
            label      = $step.Label
            exit_code  = $r.ExitCode
            passed     = $r.Passed
            output     = $r.Output
        }

        if (-not $r.Passed) {
            $allPassed = $false
            # 设计 §4.1 失败处理：步骤 1-4 失败 → 阻断后续（不跑 maturin）
            # 实际：cargo 系列失败时通常 maturin develop 也会失败，提前终止节省时间
            Write-CiLog "Stopping L1 sequence after first failure (fail-fast)" -Level WARN -Indent 2
            break
        }
    }

    return [pscustomobject]@{
        level    = "L1"
        passed   = $allPassed
        steps    = $results
        duration = $null
    }
}

# --------------------------------------------------------------------
# Main
# --------------------------------------------------------------------
$paths = Resolve-ProjectPaths
$crsDir = $paths.CrsDir

if (-not (Test-Path -LiteralPath $crsDir)) {
    Write-CiLog "neoconstruct directory not found: $crsDir" -Level ERROR
    exit 2
}

if (-not (Test-Path -LiteralPath $VenvPath)) {
    Write-CiLog "venv not found: $VenvPath (maturin develop will likely fail)" -Level WARN
}

Write-CiLog "==== L1 Quality Gate start ====" -Level STEP
Write-CiLog "CrsDir : $crsDir"
Write-CiLog "Venv   : $VenvPath"

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$result = Invoke-L1Quality -CrsDir $crsDir -TargetVenv $VenvPath
$sw.Stop()
$result.duration = $sw.Elapsed.TotalSeconds

# 写报告（若指定）
if ($ReportPath) {
    Write-CiReport -Data $result -Path $ReportPath
    Write-CiLog "L1 report written: $ReportPath" -Level INFO
}

# 汇总 + 退出码
Write-CiSummary -LevelName "L1 Quality" -Passed $result.passed
if ($result.passed) { exit 0 } else { exit 1 }
