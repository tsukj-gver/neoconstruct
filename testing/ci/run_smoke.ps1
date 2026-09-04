# run_smoke.ps1 - CI 冒烟门禁主入口（META-CI-1a + META-CI-1b + META-CI-1c）
#
# 设计依据：docs/design/基础设施/CI冒烟门禁设计.md §3 + §6.1（子任务 1a/1b/1c 范围）
#
# 任务范围：
#   - 1a：支持 -Level L1 / L4 / L1,L4 / All
#   - 1b：扩展支持 -Level L2 / L1,L2,L4 / All
#   - 1c：扩展支持 -Level L3 / L1,L2,L3,L4 / All
#   - 编排 L1 (run_l1_quality.ps1) + L2 (run_l2_functional.ps1)
#         + L3 (run_l3_perf.ps1) + L4 (run_l4_consistency.py)
#   - 报告输出：testing/ci/reports/{timestamp}_{level}_report.json
#   - 退出码：0=PASS / 1=FAIL
#
# 设计文档 §A.1 使用 -Mode 参数；本子任务按 PM/REV 共识采用更直观的 -Level 参数。
# 1c 阶段 All = L1+L2+L3+L4（全部 4 层）。

param(
    # 层次选择：L1 / L2 / L3 / L4 / 任意逗号分隔组合 / All（1c 阶段 All = L1+L2+L3+L4）
    [ValidateNotNullOrEmpty()]
    [string]
    $Level = 'All'
    ,
    # 跳过 maturin develop（L1 内部步骤 5），用于加速重复跑（不推荐常规使用）
    [switch]
    $SkipMaturin
    ,
    # L3 QuickMode（只跑 SCENARIO_DEFS 中 quick=true 的子集）
    [switch]
    $L3QuickMode
    ,
    # L3 MockMode（用 mock 数据测试判据，不跑实际 bench；用于 CI 自检）
    [switch]
    $L3MockMode
    ,
    # L3 FAIL 时不触发 Controlled A/B Test
    [switch]
    $L3NoAbTest
    ,
    # 报告目录（默认 testing/ci/reports）
    [string]
    $ReportDir
    ,
    # venv 路径（默认 .venv）
    [string]
    $VenvPath
)

$ErrorActionPreference = "Stop"

# 加载共享函数库
. (Join-Path $PSScriptRoot "lib_smoke.ps1")

# --------------------------------------------------------------------
# 参数标准化：解析 -Level 为层次列表
# --------------------------------------------------------------------
function Resolve-Levels {
    param([string]$LevelStr)

    $LevelStr = $LevelStr.Trim()
    if ($LevelStr -ieq "All") {
        # 1c 阶段 All = L1,L2,L3,L4（全部 4 层）
        return @("L1", "L2", "L3", "L4")
    }

    $levels = $LevelStr -split "," | ForEach-Object {
        $_.Trim().ToUpper()
    } | Where-Object { $_ -ne "" }

    foreach ($lv in $levels) {
        if ($lv -notin @("L1", "L2", "L3", "L4")) {
            Write-CiLog "Unsupported level: $lv (supports L1 / L2 / L3 / L4 / All)" -Level ERROR
            exit 2
        }
    }
    return $levels
}

# --------------------------------------------------------------------
# L1 调用：转发参数到 run_l1_quality.ps1
# --------------------------------------------------------------------
function Invoke-L1Gate {
    param(
        [string]$TargetVenv
        ,
        [string]$L1ReportPath
    )

    $l1Script = Join-Path $PSScriptRoot "run_l1_quality.ps1"
    # hashtable splatting 才能按命名参数匹配（array splatting 是位置参数）
    $l1Args = @{ ReportPath = $L1ReportPath }
    if ($TargetVenv) { $l1Args["VenvPath"] = $TargetVenv }

    # 子脚本直接通过 Out-Host 输出，避免输出被当作返回值
    & $l1Script @l1Args 2>&1 | Out-Host
    return $LASTEXITCODE
}

# --------------------------------------------------------------------
# L4 调用：转发参数到 run_l4_consistency.py
# --------------------------------------------------------------------
function Invoke-L4Gate {
    param(
        [string]$PythonExe
        ,
        [string]$L4ReportPath
    )

    $l4Script = Join-Path $PSScriptRoot "run_l4_consistency.py"

    # 选择 Python：优先 .venv，回退系统 python
    $py = $PythonExe
    if (-not $py -or -not (Test-Path -LiteralPath $py)) {
        $py = "python"  # 依赖 PATH
    }

    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        # Python 的 stdout 直接通过 Out-Host 发往控制台，不进入 PowerShell 管道
        # （否则会被当作函数返回值捕获，污染 Invoke-L4Gate 的 int 返回）
        & $py $l4Script --report=$L4ReportPath 2>&1 | Out-Host
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    if ($null -eq $code) { $code = 1 }
    return [int]$code
}

# --------------------------------------------------------------------
# L2 调用：转发参数到 run_l2_functional.ps1
# --------------------------------------------------------------------
function Invoke-L2Gate {
    param(
        [string]$TargetVenv
        ,
        [string]$L2ReportPath
    )

    $l2Script = Join-Path $PSScriptRoot "run_l2_functional.ps1"
    $l2Args = @{ ReportPath = $L2ReportPath }
    if ($TargetVenv) { $l2Args["CrsPython"] = $TargetVenv }

    & $l2Script @l2Args 2>&1 | Out-Host
    return $LASTEXITCODE
}

# --------------------------------------------------------------------
# L3 调用：转发参数到 run_l3_perf.ps1
# --------------------------------------------------------------------
function Invoke-L3Gate {
    param(
        [string]$TargetVenv
        ,
        [string]$L3ReportPath
        ,
        [switch]$QuickMode
        ,
        [switch]$MockMode
        ,
        [switch]$NoAbTest
    )

    $l3Script = Join-Path $PSScriptRoot "run_l3_perf.ps1"
    $l3Args = @{ ReportPath = $L3ReportPath }
    if ($TargetVenv) { $l3Args["CrsPython"] = $TargetVenv }
    if ($QuickMode)  { $l3Args["QuickMode"] = $true }
    if ($MockMode)   { $l3Args["MockMode"] = $true }
    if ($NoAbTest)   { $l3Args["NoAbTest"] = $true }

    & $l3Script @l3Args 2>&1 | Out-Host
    $code = $LASTEXITCODE
    # L3 退出码：0=PASS / 1=WARN / 2=FAIL
    # 主 runner 把 1（WARN）和 0（PASS）都视为通过；2 才视为 FAIL
    if ($null -eq $code) { $code = 2 }
    return [int]$code
}

# --------------------------------------------------------------------
# Main
# --------------------------------------------------------------------
$paths = Resolve-ProjectPaths
if (-not $ReportDir)  { $ReportDir  = $paths.ReportsDir }
if (-not $VenvPath)   { $VenvPath   = $paths.CrsVenv }
$crsVenvPython = $paths.CrsVenvPython

# 确保报告目录存在
if (-not (Test-Path -LiteralPath $ReportDir)) {
    New-Item -ItemType Directory -Path $ReportDir -Force | Out-Null
}

$levels = Resolve-Levels -LevelStr $Level
$ts = Get-CiTimestamp

Write-CiLog "==== Smoke Gate start (levels: $($levels -join ',')) ====" -Level STEP
Write-CiLog "ReportDir : $ReportDir"
Write-CiLog "Venv      : $VenvPath"

$overallPassed = $true
$perLevel = @{}

foreach ($lv in $levels) {
    Write-CiLog ""
    Write-CiLog "-------- Running $lv --------" -Level STEP

    $reportFile = Join-Path $ReportDir "${ts}_${lv}_report.json"
    $subCode = 1

    if ($lv -eq "L1") {
        $subCode = Invoke-L1Gate -TargetVenv $VenvPath -L1ReportPath $reportFile
    }
    elseif ($lv -eq "L2") {
        $subCode = Invoke-L2Gate -TargetVenv $crsVenvPython -L2ReportPath $reportFile
    }
    elseif ($lv -eq "L3") {
        $subCode = Invoke-L3Gate -TargetVenv $crsVenvPython -L3ReportPath $reportFile `
            -QuickMode:$L3QuickMode -MockMode:$L3MockMode -NoAbTest:$L3NoAbTest
    }
    elseif ($lv -eq "L4") {
        $subCode = Invoke-L4Gate -PythonExe $crsVenvPython -L4ReportPath $reportFile
    }

    # L3 退出码语义：0=PASS / 1=WARN / 2=FAIL
    # 其他层次：0=PASS / 非0=FAIL
    if ($lv -eq "L3") {
        $lvPassed = ($subCode -le 1)  # WARN(1) 不阻断整体
    } else {
        $lvPassed = ($subCode -eq 0)
    }
    $perLevel[$lv] = [pscustomobject]@{
        Level      = $lv
        Passed     = $lvPassed
        ExitCode   = $subCode
        ReportPath = $reportFile
    }

    Write-CiSummary -LevelName $lv -Passed $lvPassed
    if (-not $lvPassed) { $overallPassed = $false }
}

# 汇总报告（run_smoke 主报告，列出各层次退出码）
$summaryReport = [pscustomobject]@{
    timestamp   = $ts
    levels      = $levels
    overall     = $overallPassed
    details     = $perLevel.Values
}
$summaryPath = Join-Path $ReportDir "${ts}_smoke_summary.json"
Write-CiReport -Data $summaryReport -Path $summaryPath

# 控制台最终汇总
Write-CiLog ""
Write-CiLog "==== Smoke Gate summary ====" -Level STEP
foreach ($r in $perLevel.Values) {
    $tag = if ($r.Passed) { "PASS" } else { "FAIL" }
    Write-CiLog ("  {0,-4} {1,-5}  exit={2}  report={3}" -f $r.Level, $tag, $r.ExitCode, $r.ReportPath) -Level INFO
}
Write-CiLog "Summary report: $summaryPath" -Level INFO

if ($overallPassed) {
    Write-CiLog "Overall: PASS" -Level OK
    exit 0
} else {
    Write-CiLog "Overall: FAIL" -Level ERROR
    exit 1
}
