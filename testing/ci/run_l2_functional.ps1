# run_l2_functional.ps1 - L2 功能门禁（META-CI-1b）
#
# 设计依据：docs/design/基础设施/CI冒烟门禁设计.md §4.2（L2 详细规范）+ §6.2（子任务 1b 范围）
#           + §1.1 B 类脚本清单（含 OBS-1 修订）
#
# 任务范围（META-CI-1b）：
#   - 调度 Phase 1-4 已实现的 smoke 脚本矩阵
#   - 收集每个脚本的 PASS/FAIL + 输出
#   - 生成 L2 报告（JSON + 控制台彩色）
#   - 退出码：0=PASS / 1=FAIL
#
# 脚本调度矩阵（设计 §4.2 + §1.1 B 类）：
#   Phase 1   phase1_smoke.py                         (CRS venv, 1b 新建)
#   Phase 2   phase2_smoke.py                         (CRS venv, 1b 新建)
#   Phase 3   vet_phase3_binary.py                    (PC  venv)
#   Phase 3   vet_phase3_bits_integer.py              (PC  venv, known_broken_python_3_14)
#   Phase 3.2 vet_phase32_bitstruct.py                (CRS venv)
#   Phase 3.2 vet_phase32_deep.py                     (CRS venv)
#   Phase 3.3 test_phase33.py                         (CRS venv, OBS-1 补入)
#   Phase 4   phase4_smoke_greedy_range.py            (CRS venv)
#   Phase 4   phase4_smoke_prefixed_array.py          (CRS venv)
#   Phase 4   phase4_smoke_repeat_until.py            (CRS venv, deprecated v4 PyCallable; v5 superseded)
#   Phase 4   phase4_repeat_until_examples.py         (CRS venv, OBS-1 补入, v5 21 样例)
#   综合      v4_smoke_test.py                        (CRS venv, deprecated v4 PyCallable + Container)
#
# venv 选择规则：
#   - target=CRS：脚本依赖 construct-rs 用户面 API（StructMixin/field/Int8ub...）
#                 → crs_venv_new 跑（含 construct-rs wheel）
#   - target=PC ：脚本依赖 Python construct 2.10.70 原版（Bitwise/Bit/BitsInteger
#                 是 Adapter 而非 Descriptor）
#                 → crs_venv_py_new 跑
#
# expected 字段（L2 判定语义）：
#   - pass           必须通过（FAIL → L2 FAIL）
#   - known_broken   已知 broken（环境/版本兼容性），FAIL 不阻断 L2；仅报告
#   - deprecated     已废弃（被新脚本取代），FAIL 不阻断 L2；仅报告
#
# 编码规范：PascalCase / try/catch / 无裸 Write-Host / 兼容 PowerShell 5.1

param(
    # CRS venv python.exe（默认从 lib_smoke 解析）
    [string]$CrsPython

    ,
    # PC venv python.exe（默认从 lib_smoke 解析）
    [string]$PcPython

    ,
    # 报告输出路径（可选；默认 testing/ci/reports/<ts>_l2_report.json）
    [string]$ReportPath
)

$ErrorActionPreference = "Stop"

# 加载共享函数库
. (Join-Path $PSScriptRoot "lib_smoke.ps1")

# --------------------------------------------------------------------
# 路径与 venv 默认解析
# --------------------------------------------------------------------
$paths = Resolve-ProjectPaths
$experimentsDir = Join-Path $paths.Root "experiments"
$reportsDir = $paths.ReportsDir
if (-not $CrsPython) {
    $CrsPython = $paths.CrsVenvPython
}
if (-not $PcPython) {
    $pcVenv = $paths.PyVenv
    $PcPython = Join-Path $pcVenv "Scripts\python.exe"
}
if (-not $ReportPath) {
    $ts = Get-CiTimestamp
    $ReportPath = Join-Path $reportsDir "${ts}_l2_report.json"
}

# --------------------------------------------------------------------
# L2 脚本调度矩阵（顺序执行）
# --------------------------------------------------------------------
# 每项：phase / script / target(CRS|PC) / expected(pass|known_broken|deprecated)
#       coverage：该脚本覆盖的构造器（用于覆盖度报告）
$MATRIX = @(
    @{ Phase="1";   Script="phase1_smoke.py";                    Target="CRS"; Expected="pass"
       Coverage="FormatField(16 Int 单例) / Bytes(count) / GreedyBytes / Struct B1-B2 / ModbusRTU" }
    @{ Phase="2";   Script="phase2_smoke.py";                    Target="CRS"; Expected="pass"
       Coverage="Bytes(const) / Bytes(this.field) / Bytes(a+b) / Computed(a+b) / Tell + Computed (E1-E3)" }

    @{ Phase="3";   Script="vet_phase3_binary.py";               Target="PC";  Expected="pass"
       Coverage="bits2integer / integer2bits / swapbytesinbits (BitsInteger 底层)" }
    @{ Phase="3";   Script="vet_phase3_bits_integer.py";         Target="PC";  Expected="known_broken"
       Coverage="Bitwise(Bit/Nibble/Octet/BitsInteger).parse/build 原版行为基线"
       Note="Python construct 2.10.70 在 Python 3.14 上 Bitwise RestreamedBytesIO 兼容性问题；脚本本身设计为 PC venv 跑" }

    @{ Phase="3.2"; Script="vet_phase32_bitstruct.py";           Target="CRS"; Expected="pass"
       Coverage="Bitwise / BitStruct / BitsInteger / Bit / Nibble 基本场景" }
    @{ Phase="3.2"; Script="vet_phase32_deep.py";                Target="CRS"; Expected="pass"
       Coverage="Bitwise / BitStruct 跨字节 / 嵌套 / swapped / 64bit 深度场景" }
    @{ Phase="3.3"; Script="test_phase33.py";                    Target="CRS"; Expected="pass"
       Coverage="BitStruct + Padding / Bytewise / BitsSwapped / ByteSwapped / 错误路径"
       Note="OBS-1 补入" }

    @{ Phase="4";   Script="phase4_smoke_greedy_range.py";       Target="CRS"; Expected="pass"
       Coverage="GreedyRange (parse/build/discard/fallback/empty)" }
    @{ Phase="4";   Script="phase4_smoke_prefixed_array.py";     Target="CRS"; Expected="pass"
       Coverage="PrefixedArray (含嵌套/Int16ub countfield/signed negative/overflow 错误路径)" }
    @{ Phase="4";   Script="phase4_smoke_repeat_until.py";       Target="CRS"; Expected="deprecated"
       Coverage="RepeatUntil v4 PyCallable 路径（已弃用）"
       Note="v5 删除 PyCallable (ADR-014)；由 phase4_repeat_until_examples.py 取代；FAIL 不阻断" }
    @{ Phase="4";   Script="phase4_repeat_until_examples.py";    Target="CRS"; Expected="pass"
       Coverage="RepeatUntil v5 21 用户面样例（Element/Index/Expr/parse-build 对称/discard/嵌套）"
       Note="OBS-1 补入" }

    @{ Phase="1-4"; Script="v4_smoke_test.py";                   Target="CRS"; Expected="deprecated"
       Coverage="RepeatUntil v4 V-1~V-5 修正（PyCallable + Container）"
       Note="v5 删除 PyCallable + Container lib 已下线；FAIL 不阻断；与 phase4_repeat_until_examples.py 部分覆盖重叠" }
)

# --------------------------------------------------------------------
# 执行单个 smoke 脚本
# --------------------------------------------------------------------
function Invoke-SmokeScript {
    param(
        [Parameter(Mandatory=$true)] [ValidateNotNullOrEmpty()] [string]$ScriptPath
        ,
        [Parameter(Mandatory=$true)] [ValidateNotNullOrEmpty()] [string]$PythonExe
        ,
        [Parameter(Mandatory=$true)] [ValidateNotNullOrEmpty()] [string]$Label
    )

    if (-not (Test-Path -LiteralPath $ScriptPath)) {
        return [pscustomobject]@{
            Passed=$false; ExitCode=-1; Output=""
            Error="script file not found: $ScriptPath"
        }
    }
    if (-not (Test-Path -LiteralPath $PythonExe)) {
        return [pscustomobject]@{
            Passed=$false; ExitCode=-1; Output=""
            Error="python.exe not found: $PythonExe"
        }
    }

    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    try {
        # L2 脚本可能跑 5-30s（含子进程 PC 对照），给 90s 上限
        $output = & $PythonExe $ScriptPath 2>&1 | Out-String
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    if ($null -eq $code) { $code = 0 }

    return [pscustomobject]@{
        Passed    = ($code -eq 0)
        ExitCode  = [int]$code
        Output    = $output.TrimEnd()
        Error     = $null
    }
}

# --------------------------------------------------------------------
# Main
# --------------------------------------------------------------------
Write-CiLog "==== L2 Functional Gate start ====" -Level STEP
Write-CiLog "CrsPython : $CrsPython"
Write-CiLog "PcPython  : $PcPython"
Write-CiLog "ReportsDir: $reportsDir"
Write-CiLog ""

# venv 前置检查（warn 不阻断）
if (-not (Test-Path -LiteralPath $CrsPython)) {
    Write-CiLog "CrsPython not found: $CrsPython (CRS-venv scripts will fail)" -Level WARN
}
if (-not (Test-Path -LiteralPath $PcPython)) {
    Write-CiLog "PcPython not found: $PcPython (PC-venv scripts will fail)" -Level WARN
}

# 确保报告目录存在
if (-not (Test-Path -LiteralPath $reportsDir)) {
    New-Item -ItemType Directory -Path $reportsDir -Force | Out-Null
}

# 调度执行
$scriptResults = @()
$mustPassFailed = $false

for ($i = 0; $i -lt $MATRIX.Count; $i++) {
    $entry = $MATRIX[$i]
    $phase = $entry.Phase
    $script = $entry.Script
    $target = $entry.Target
    $expected = $entry.Expected
    $coverage = $entry.Coverage

    $scriptPath = Join-Path $experimentsDir $script
    $py = if ($target -eq "CRS") { $CrsPython } else { $PcPython }

    $stepIdx = "[{0}/{1}]" -f ($i + 1), $MATRIX.Count
    Write-CiLog "$stepIdx Phase $phase  $script  (target=$target, expected=$expected)" -Level STEP

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $r = Invoke-SmokeScript -ScriptPath $scriptPath -PythonExe $py -Label $script
    $sw.Stop()
    $elapsedSec = [math]::Round($sw.Elapsed.TotalSeconds, 2)

    # 决定本项是否阻断
    $blocksL2 = (-not $r.Passed) -and ($expected -eq "pass")
    if ($blocksL2) { $mustPassFailed = $true }

    # 控制台输出
    if ($r.Passed) {
        Write-CiLog "PASS  $script (exit=$($r.ExitCode), ${elapsedSec}s)" -Level OK -Indent 1
    } else {
        if ($expected -eq "pass") {
            Write-CiLog "FAIL  $script (exit=$($r.ExitCode), ${elapsedSec}s) — blocks L2" -Level ERROR -Indent 1
        } elseif ($expected -eq "known_broken") {
            Write-CiLog "KNOWN_BROKEN  $script (exit=$($r.ExitCode), ${elapsedSec}s) — does not block L2" -Level WARN -Indent 1
        } elseif ($expected -eq "deprecated") {
            Write-CiLog "DEPRECATED    $script (exit=$($r.ExitCode), ${elapsedSec}s) — does not block L2" -Level WARN -Indent 1
        }
        # 输出摘要（尾部 5 行）
        $tail = ($r.Output -split "`n")
        $tailPreview = ($tail | Select-Object -Last 5) -join "`n"
        Write-Host $tailPreview -ForegroundColor DarkGray
    }

    $scriptResults += [pscustomobject]@{
        phase        = $phase
        script       = $script
        target_venv  = $target
        expected     = $expected
        passed       = $r.Passed
        exit_code    = $r.ExitCode
        elapsed_sec  = $elapsedSec
        coverage     = $coverage
        note         = $entry.Note
        output_tail  = ($r.Output -split "`n" | Select-Object -Last 10) -join "`n"
        error        = $r.Error
    }
}

# --------------------------------------------------------------------
# 覆盖度报告：构造器 × phase × 脚本 矩阵
# --------------------------------------------------------------------
Write-CiLog ""
Write-CiLog "==== L2 Coverage Matrix ====" -Level STEP
Write-CiLog ("{0,-8} {1,-38} {2,-4} {3,-12} {4}" -f "Phase", "Script", "Pass", "Expected", "Coverage")
foreach ($r in $scriptResults) {
    $passTag = if ($r.passed) { "OK" } else { "NO" }
    Write-CiLog ("{0,-8} {1,-38} {2,-4} {3,-12} {4}" -f $r.phase, $r.script, $passTag, $r.expected, $r.coverage) -Level INFO
}

# --------------------------------------------------------------------
# 总体判定
# --------------------------------------------------------------------
$totalCount = $scriptResults.Count
$passedCount = @($scriptResults | Where-Object { $_.passed }).Count
$failedMustPass = @($scriptResults | Where-Object { -not $_.passed -and $_.expected -eq "pass" }).Count
$knownBrokenCount = @($scriptResults | Where-Object { -not $_.passed -and $_.expected -eq "known_broken" }).Count
$deprecatedCount = @($scriptResults | Where-Object { -not $_.passed -and $_.expected -eq "deprecated" }).Count

$l2Passed = -not $mustPassFailed

# JSON 报告
$report = [pscustomobject]@{
    level         = "L2"
    timestamp     = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
    crs_python    = $CrsPython
    pc_python     = $PcPython
    passed        = $l2Passed
    summary       = [pscustomobject]@{
        total_scripts      = $totalCount
        passed             = $passedCount
        failed_must_pass   = $failedMustPass
        known_broken       = $knownBrokenCount
        deprecated         = $deprecatedCount
    }
    scripts       = $scriptResults
}
Write-CiReport -Data $report -Path $ReportPath

# 控制台汇总
Write-CiLog ""
Write-CiLog "==== L2 Summary ====" -Level STEP
Write-CiLog ("Total scripts   : {0}" -f $totalCount)
Write-CiLog ("Passed          : {0}" -f $passedCount)
Write-CiLog ("Failed must-pass: {0}" -f $failedMustPass)
Write-CiLog ("Known broken    : {0}" -f $knownBrokenCount)
Write-CiLog ("Deprecated      : {0}" -f $deprecatedCount)
Write-CiLog "L2 report: $ReportPath" -Level INFO
Write-CiSummary -LevelName "L2 Functional" -Passed $l2Passed

if ($l2Passed) {
    Write-CiLog "Overall: PASS" -Level OK
    exit 0
} else {
    Write-CiLog "Overall: FAIL" -Level ERROR
    exit 1
}
