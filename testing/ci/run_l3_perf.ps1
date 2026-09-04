# run_l3_perf.ps1 - L3 性能回归检测 PowerShell 编排（META-CI-1c）
#
# 设计依据：docs/design/基础设施/CI冒烟门禁设计.md §5 + §6.3（子任务 1c 范围）
#
# 职责：
#   - 调用 Python 子组件 run_l3_perf.py（baseline diff / 环境采集 / 报告生成）
#   - venv 管理（.venv / .venv-pc 切换）
#   - 整合到 run_smoke.ps1 的 -Level L3 支持
#
# 注意：本编排只 forward 参数到 Python 子组件；测量逻辑/判据/报告全在
#       run_l3_perf.py 中实现，便于测试与跨平台移植。
#
# 退出码：
#   0 = 全 PASS
#   1 = 有 WARN，无 FAIL
#   2 = 有 FAIL（或 A/B Test 确认回归）
#
# 编码规范：PascalCase / ValidateXxx / 无裸 Write-Host / try/catch / PowerShell 5.1 兼容

param(
    # 跑子集（仅 SCENARIO_DEFS 中 quick=true 的，~3-5min）
    [switch]$QuickMode
    ,
    # 用 mock 数据测试判据（不跑实际 bench；用于 CI 自检）
    [switch]$MockMode
    ,
    # L3 FAIL 时不触发 Controlled A/B Test（默认会触发）
    [switch]$NoAbTest
    ,
    # 报告输出路径（默认 testing/ci/reports/<ts>_L3_report.json）
    [string]$ReportPath
    ,
    # Markdown 报告输出路径（默认与 ReportPath 同名 .md）
    [string]$MarkdownPath
    ,
    # .venv python.exe（默认从 lib_smoke 解析）
    [string]$CrsPython
    ,
    # .venv-pc python.exe（默认从 lib_smoke 解析）
    [string]$PcPython
)

$ErrorActionPreference = "Stop"

# 加载共享函数库
. (Join-Path $PSScriptRoot "lib_smoke.ps1")

# --------------------------------------------------------------------
# 路径与 venv 默认解析
# --------------------------------------------------------------------
$paths = Resolve-ProjectPaths
$reportsDir = $paths.ReportsDir

if (-not $CrsPython) { $CrsPython = $paths.CrsVenvPython }
if (-not $PcPython)  { $PcPython  = Join-Path $paths.PyVenv "Scripts\python.exe" }

$ts = Get-CiTimestamp
if (-not $ReportPath) {
    $ReportPath = Join-Path $reportsDir "${ts}_L3_report.json"
}
if (-not $MarkdownPath) {
    $MarkdownPath = Join-Path $reportsDir "${ts}_L3_report.md"
}

# --------------------------------------------------------------------
# 前置检查
# --------------------------------------------------------------------
function Assert-L3Preconditions {
    if (-not (Test-Path -LiteralPath $CrsPython)) {
        Write-CiLog "CrsPython not found: $CrsPython (run L1 first to install maturin develop)" -Level ERROR
        Write-CiLog "Hint: testing\\ci\\run_smoke.ps1 -Level L1" -Level WARN
        exit 2
    }
    if (-not (Test-Path -LiteralPath $PcPython)) {
        Write-CiLog "PcPython not found: $PcPython (required for S-PERF apples-to-apples baseline)" -Level ERROR
        exit 2
    }
    $baseline = Join-Path $paths.Root "docs\perf-scenarios.csv"
    if (-not (Test-Path -LiteralPath $baseline)) {
        Write-CiLog "perf-scenarios.csv not found: $baseline" -Level ERROR
        exit 2
    }
}

# --------------------------------------------------------------------
# Main
# --------------------------------------------------------------------
Assert-L3Preconditions

Write-CiLog "==== L3 Performance Gate start ====" -Level STEP
Write-CiLog "CrsPython   : $CrsPython"
Write-CiLog "PcPython    : $PcPython"
Write-CiLog "QuickMode   : $([bool]$QuickMode)"
Write-CiLog "MockMode    : $([bool]$MockMode)"
Write-CiLog "NoAbTest    : $([bool]$NoAbTest)"
Write-CiLog "ReportPath  : $ReportPath"
Write-CiLog "MarkdownPath: $MarkdownPath"
Write-CiLog ""

# 构造参数数组
$pyArgs = @(
    (Join-Path $PSScriptRoot "run_l3_perf.py"),
    "--report=$ReportPath",
    "--markdown=$MarkdownPath",
    "--crs-python=$CrsPython",
    "--pc-python=$PcPython"
)
if ($QuickMode) { $pyArgs += "--quick" }
if ($MockMode)  { $pyArgs += "--mock" }
if ($NoAbTest)  { $pyArgs += "--no-ab-test" }

Write-CiLog "Invoking: & $CrsPython $($pyArgs -join ' ')" -Level INFO

# 子进程跑 Python 主调度；SilentlyContinue 避免 stderr 包装为异常
$prev = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    & $CrsPython @pyArgs 2>&1 | Out-Host
    $code = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $prev
}
if ($null -eq $code) { $code = 2 }

Write-CiLog ""
switch ($code) {
    0 { Write-CiSummary -LevelName "L3 Performance" -Passed $true }
    1 { Write-CiLog "L3 Performance : WARN (exit 1)" -Level WARN }
    2 { Write-CiSummary -LevelName "L3 Performance" -Passed $false }
    default { Write-CiLog "L3 Performance : UNKNOWN exit $code" -Level ERROR }
}

Write-CiLog "L3 JSON report    : $ReportPath" -Level INFO
Write-CiLog "L3 Markdown report: $MarkdownPath" -Level INFO

exit $code
