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
# 双轨制（Phase 6.0 引入，设计 §3）：
#   - 旧轨（smoke）：$SMOKE_METADATA 元数据注册表 + glob 自动展开 experiments/ 下脚本
#   - 新轨（pytest）：追加段，自动发现 tests/{parity,integration,errors}/ 下 test_*.py
#
# 每项：phase / script / target(CRS|PC) / expected(pass|known_broken|deprecated)
#       coverage：该脚本覆盖的构造器（用于覆盖度报告）
#
# 旧轨改造（设计 §3.2.1）：$MATRIX 硬编码 → $SMOKE_METADATA（只记元数据）+ glob 展开路径
$SMOKE_METADATA = @{
    "phase1_smoke.py"                    = @{ Phase="1";   Target="CRS"; Expected="pass";           Coverage="FormatField(16) / Bytes / Struct B1-B2 / ModbusRTU" }
    "phase2_smoke.py"                    = @{ Phase="2";   Target="CRS"; Expected="pass";           Coverage="Bytes(const/expr) / Computed / Tell (E1-E3)" }
    "vet_phase3_binary.py"               = @{ Phase="3";   Target="PC";  Expected="pass";           Coverage="bits2integer / integer2bits / swapbytesinbits" }
    "vet_phase3_bits_integer.py"         = @{ Phase="3";   Target="PC";  Expected="known_broken";   Coverage="Bitwise(Bit/Nibble/Octet).parse/build 基线" }
    "vet_phase32_bitstruct.py"           = @{ Phase="3.2"; Target="CRS"; Expected="pass";           Coverage="Bitwise / BitStruct / BitsInteger 基本场景" }
    "vet_phase32_deep.py"                = @{ Phase="3.2"; Target="CRS"; Expected="pass";           Coverage="Bitwise / BitStruct 跨字节 / 嵌套 / swapped" }
    "test_phase33.py"                    = @{ Phase="3.3"; Target="CRS"; Expected="pass";           Coverage="BitStruct + Padding / Bytewise / BitsSwapped" }
    "phase4_smoke_greedy_range.py"       = @{ Phase="4";   Target="CRS"; Expected="pass";           Coverage="GreedyRange (parse/build/discard/fallback/empty)" }
    "phase4_smoke_prefixed_array.py"     = @{ Phase="4";   Target="CRS"; Expected="pass";           Coverage="PrefixedArray (含嵌套/Int16ub countfield)" }
    "phase4_smoke_repeat_until.py"       = @{ Phase="4";   Target="CRS"; Expected="deprecated";     Coverage="RepeatUntil v4 PyCallable（已弃用）" }
    "phase4_repeat_until_examples.py"    = @{ Phase="4";   Target="CRS"; Expected="pass";           Coverage="RepeatUntil v5 21 样例" }
    "v4_smoke_test.py"                   = @{ Phase="1-4"; Target="CRS"; Expected="deprecated";     Coverage="RepeatUntil v4 V-1~V-5 修正" }
}

# glob 自动展开 experiments/ 下所有 .py 脚本，匹配 $SMOKE_METADATA 的 key
$smokeScripts = Get-ChildItem -Path $experimentsDir -Filter "*.py" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -in $SMOKE_METADATA.Keys }

# 构造 $MATRIX（动态）
$MATRIX = @()
foreach ($f in $smokeScripts) {
    $meta = $SMOKE_METADATA[$f.Name]
    $MATRIX += [pscustomobject]@{
        Phase    = $meta.Phase
        Script   = $f.Name
        Target   = $meta.Target
        Expected = $meta.Expected
        Coverage = $meta.Coverage
        Note     = $meta.Note
    }
}

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
# 新轨：pytest 自动发现（Phase 6.0 引入，设计 §3.2.2）
# --------------------------------------------------------------------
# 自动收集 tests/{parity,integration,errors}/ 下 test_*.py，在 CRS venv 跑。
# pytest 失败（exit != 0）阻断 L2（与旧轨 Expected="pass" 同语义）。
# pytest 不可用时 WARN skip（不阻断）。
Write-CiLog ""
Write-CiLog "==== L2 pytest track start ====" -Level STEP

$pytestRoot = Join-Path $paths.CrsDir "tests"
$pytestTargets = @(
    (Join-Path $pytestRoot "parity"),
    (Join-Path $pytestRoot "integration"),
    (Join-Path $pytestRoot "errors")
)

# 检查 pytest 是否在 CRS venv 可用
$prevPref = $ErrorActionPreference
$ErrorActionPreference = "SilentlyContinue"
$pytestCheck = & $CrsPython -m pytest --version 2>&1 | Out-String
$pytestCheckCode = $LASTEXITCODE
$ErrorActionPreference = $prevPref

if ($pytestCheckCode -ne 0) {
    Write-CiLog "pytest not available in CRS venv (exit=$pytestCheckCode), skip pytest track" -Level WARN
} else {
    Write-CiLog "pytest available: $($pytestCheck.Trim())" -Level INFO
    foreach ($target in $pytestTargets) {
        if (-not (Test-Path -LiteralPath $target)) {
            Write-CiLog "pytest target dir not exist, skip: $target" -Level WARN
            continue
        }
        $targetName = Split-Path $target -Leaf
        Write-CiLog "pytest: tests/$targetName" -Level STEP
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $prevPref2 = $ErrorActionPreference
        $ErrorActionPreference = "SilentlyContinue"
        $output = & $CrsPython -m pytest $target "-v" "--tb=short" 2>&1 | Out-String
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prevPref2
        $sw.Stop()
        $elapsedSec = [math]::Round($sw.Elapsed.TotalSeconds, 2)

        # 控制台摘要
        # exit code 约定：0=pass，5=no tests collected（空目录，WARN 不阻断），其他=FAIL 阻断
        if ($code -eq 0) {
            Write-CiLog "PASS  pytest:$targetName (exit=0, ${elapsedSec}s)" -Level OK -Indent 1
        } elseif ($code -eq 5) {
            Write-CiLog "EMPTY pytest:$targetName (exit=5, no tests collected, ${elapsedSec}s) — dir empty, not blocking" -Level WARN -Indent 1
        } else {
            Write-CiLog "FAIL  pytest:$targetName (exit=$code, ${elapsedSec}s) — blocks L2" -Level ERROR -Indent 1
            $tailPreview = ($output -split "`n" | Select-Object -Last 8) -join "`n"
            Write-Host $tailPreview -ForegroundColor DarkGray
        }

        # pytest 结果合并到 scriptResults（统一报告）
        # exit 5（空目录）视为 passed=$true 且 expected="empty"（不阻断 L2）
        $isPassed = ($code -eq 0 -or $code -eq 5)
        $effectiveExpected = if ($code -eq 5) { "empty" } else { "pass" }
        $scriptResults += [pscustomobject]@{
            phase        = "pytest"
            script       = "pytest:" + $targetName
            target_venv  = "CRS"
            expected     = $effectiveExpected
            passed       = $isPassed
            exit_code    = [int]$code
            elapsed_sec  = $elapsedSec
            coverage     = "pytest auto-discovery"
            note         = "Phase 6.0+ new track"
            output_tail  = ($output -split "`n" | Select-Object -Last 10) -join "`n"
            error        = $null
        }
        # 仅 exit != 0 且 != 5 时阻断（exit 5 = 空目录，待步骤 6 迁移后自然有测试）
        if ($code -ne 0 -and $code -ne 5) { $mustPassFailed = $true }
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
