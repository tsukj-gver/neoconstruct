# ab_harness.ps1 - Controlled A/B Test 通用交替测量 harness (META-CI-1c)
#
# 通用化自 experiments/E01_O1_ab_harness.ps1：
#   - 参数化 $ToggleFile（质疑改动文件，git checkout 切换）
#   - 参数化 $BaseCommit（toggle off 状态的 commit/parent）
#   - 参数化 $BenchScript（ab_bench.py）
#   - 参数化 $BenchConfig（ab_bench_config.json）
#
# 测量序列：A1 B1 A2 B2 A3 B3（A=toggle off / B=toggle on）
# 每轮：git checkout -> cargo build -> maturin develop -> bench -> sleep 30s
# 结束后恢复 HEAD 版本。
#
# 设计依据：docs/design/基础设施/CI冒烟门禁设计.md §5.7 + L-09 对策
# 测量口径：performance-gate SKILL Checkpoint 4（min(repeat=5) × number）
#
# 编码规范：PascalCase / try/catch / 无裸 Write-Host / 兼容 PowerShell 5.1
#           全 ASCII 注释（避免 PowerShell 5.1 GBK 编码问题）

param(
    # 质疑改动文件（toggle off 时 git checkout 到 BaseCommit 版本）
    # 例如 "construct-rs/src/nodes/stop_if.rs"
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$ToggleFile
    ,
    # toggle off 状态的 commit（commit-ish，如 "c77c0ea~1" 或 "HEAD~5"）
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$BaseCommit
    ,
    # ab_bench.py 路径（默认 testing/ci/ab_test/ab_bench.py）
    [string]$BenchScript
    ,
    # ab_bench_config.json 路径（必填：场景定义 + crs/pc python 路径）
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$BenchConfig
    ,
    # 输出目录（bench JSON 写入此处，默认与 BenchConfig 同目录）
    [string]$OutputDir
    ,
    # 交替轮数（默认 3 = A/B 各 3 次）
    [int]$Rounds = 3
    ,
    # 每轮间隔 sleep 秒数（防 CPU 热节流，默认 30）
    [int]$SleepSeconds = 30
    ,
    # crs_venv_new python.exe（默认从 lib_smoke 解析）
    [string]$CrsPython
    ,
    # A/B stats 报告输出路径（可选，若指定则跑完后调 ab_stats.py）
    [string]$ReportPath
    ,
    # 跳过 cargo build + maturin develop（仅在已构建状态下使用，调试用）
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

# 加载共享函数库
. (Join-Path $PSScriptRoot "..\lib_smoke.ps1")

# --------------------------------------------------------------------
# 路径解析
# --------------------------------------------------------------------
$paths = Resolve-ProjectPaths
$repoRoot = $paths.Root
$crsDir = $paths.CrsDir

if (-not $BenchScript) {
    $BenchScript = Join-Path $PSScriptRoot "ab_bench.py"
}
if (-not $OutputDir) {
    $OutputDir = (Split-Path -Parent $BenchConfig)
}
if (-not $CrsPython) {
    $CrsPython = $paths.CrsVenvPython
}

$toggleAbs = if ([System.IO.Path]::IsPathRooted($ToggleFile)) {
    $ToggleFile
} else {
    Join-Path $repoRoot $ToggleFile
}

# --------------------------------------------------------------------
# 校验前置条件
# --------------------------------------------------------------------
function Assert-Preconditions {
    if (-not (Test-Path -LiteralPath $toggleAbs)) {
        Write-CiLog "ToggleFile not found: $toggleAbs" -Level ERROR
        exit 2
    }
    if (-not (Test-Path -LiteralPath $BenchScript)) {
        Write-CiLog "BenchScript not found: $BenchScript" -Level ERROR
        exit 2
    }
    if (-not (Test-Path -LiteralPath $BenchConfig)) {
        Write-CiLog "BenchConfig not found: $BenchConfig" -Level ERROR
        exit 2
    }
    if (-not (Test-Path -LiteralPath $CrsPython)) {
        Write-CiLog "CrsPython not found: $CrsPython" -Level ERROR
        exit 2
    }
    # 校验 $BaseCommit 能解析
    Push-Location -LiteralPath $repoRoot
    try {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $verify = git rev-parse --short $BaseCommit 2>&1
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -ne 0) {
            Write-CiLog "BaseCommit '$BaseCommit' does not resolve (exit $code): $verify" -Level ERROR
            exit 2
        }
    } finally { Pop-Location }
    Write-CiLog "BaseCommit '$BaseCommit' resolves to: $verify" -Level OK
}

# --------------------------------------------------------------------
# 切换 toggle 状态
# --------------------------------------------------------------------
function Set-ToggleState {
    param([ValidateSet("A","B")][string]$State)
    Push-Location -LiteralPath $repoRoot
    try {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        if ($State -eq "A") {
            # A = toggle off (BaseCommit 版本)
            git checkout $BaseCommit -- $ToggleFile 2>&1 | Out-Null
        } else {
            # B = toggle on (HEAD 版本)
            git checkout "HEAD" -- $ToggleFile 2>&1 | Out-Null
        }
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -ne 0) { throw "git checkout $State failed (exit $code)" }
    } finally { Pop-Location }
}

# --------------------------------------------------------------------
# cargo build + maturin develop
# ---------------------------------------------------------------------
function Invoke-BuildInstall {
    $env:PYO3_USE_ABI3_FORWARD_COMPATIBILITY = "1"
    $env:VIRTUAL_ENV = Split-Path -Parent (Split-Path -Parent $CrsPython)
    Push-Location -LiteralPath $crsDir
    try {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $buildOut = & cargo build --release 2>&1
        $code1 = $LASTEXITCODE
        $matOut = ""
        if ($code1 -eq 0) {
            $matOut = & maturin develop --release 2>&1
            $code2 = $LASTEXITCODE
        } else { $code2 = 1 }
        $ErrorActionPreference = $prev
        if ($code1 -ne 0) { throw "cargo build failed (exit $code1)" }
        if ($code2 -ne 0) { throw "maturin develop failed (exit $code2)" }
    } finally { Pop-Location }
}

# --------------------------------------------------------------------
# 跑 bench（A 或 B 状态下的 label）
# --------------------------------------------------------------------
function Invoke-Bench {
    param([string]$Label)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $outPath = Join-Path $OutputDir "ab_bench_$Label.json"
    $out = & $CrsPython $BenchScript --label $Label --config $BenchConfig --output $outPath 2>&1
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) {
        Write-CiLog "BENCH $Label FAILED (exit $code)" -Level ERROR
        $out | Out-Host
        throw "bench failed (exit $code)"
    }
    $summaryLine = ($out | Select-String -Pattern '^\[SUMMARY\]' | Select-Object -Last 1).Line
    if (-not $summaryLine) { $summaryLine = ($out | Select-Object -Last 3) -join ' | ' }
    Write-CiLog "BENCH $Label : $summaryLine" -Level OK
}

# --------------------------------------------------------------------
# Main
# --------------------------------------------------------------------
Assert-Preconditions

Write-CiLog "==== Controlled A/B Test harness start ====" -Level STEP
Write-CiLog "ToggleFile  : $ToggleFile"
Write-CiLog "BaseCommit  : $BaseCommit (A=off)"
Write-CiLog "BenchScript : $BenchScript"
Write-CiLog "BenchConfig : $BenchConfig"
Write-CiLog "OutputDir   : $OutputDir"
Write-CiLog "CrsPython   : $CrsPython"
Write-CiLog "Rounds      : $Rounds (A/B x $Rounds = $($Rounds * 2) runs)"
Write-CiLog "SleepSeconds: $SleepSeconds"
Write-CiLog ""

# 检查初始 git status（应为 clean，否则可能丢失改动）
Push-Location -LiteralPath $repoRoot
$initialStatus = (git status --porcelain -- $ToggleFile) -join ','
Pop-Location
Write-CiLog "Initial ToggleFile git status: '$initialStatus' (empty = clean)"
if ($initialStatus) {
    Write-CiLog "WARNING: ToggleFile has uncommitted changes; harness may overwrite" -Level WARN
}

# 构造交替序列 A1 B1 A2 B2 A3 B3
$sequence = @()
for ($r = 1; $r -le $Rounds; $r++) {
    $sequence += @{Round=$r; State="A"; Label="A${r}_off"}
    $sequence += @{Round=$r; State="B"; Label="B${r}_on"}
}

foreach ($step in $sequence) {
    Write-CiLog ""
    Write-CiLog "----- Round $($step.Round) / Condition $($step.State) ($($step.Label)) -----" -Level STEP
    Write-CiLog "  Set-ToggleState $($step.State) ..."
    Set-ToggleState -State $step.State
    if (-not $SkipBuild) {
        Write-CiLog "  cargo build + maturin develop ..."
        Invoke-BuildInstall
    } else {
        Write-CiLog "  SkipBuild=true, skipping build" -Level WARN
    }
    Write-CiLog "  Bench $($step.Label) ..."
    Invoke-Bench -Label $step.Label
    Write-CiLog "  Sleep $SleepSeconds s ..."
    Start-Sleep -Seconds $SleepSeconds
}

# 恢复 HEAD 版本
Write-CiLog ""
Write-CiLog "----- Restore HEAD $ToggleFile -----" -Level STEP
Set-ToggleState -State "B"
Write-CiLog "==== A/B Test harness complete ====" -Level OK

# 可选：调用 ab_stats.py 生成统计报告
if ($ReportPath) {
    $statsScript = Join-Path $PSScriptRoot "ab_stats.py"
    if (Test-Path -LiteralPath $statsScript) {
        $aLabels = @(for ($r = 1; $r -le $Rounds; $r++) { "A${r}_off" }) -join ","
        $bLabels = @(for ($r = 1; $r -le $Rounds; $r++) { "B${r}_on" }) -join ","
        # 从 config 读 scenarios 列表
        $scenarios = (& $CrsPython -c "import json; cfg=json.load(open(r'$BenchConfig', encoding='utf-8')); print(','.join(s['key'] for s in cfg['scenarios']))")
        Write-CiLog "Running ab_stats.py (A=$aLabels, B=$bLabels, scenarios=$scenarios) ..."
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & $CrsPython $statsScript --a-labels $aLabels --b-labels $bLabels `
            --scenarios $scenarios --bench-dir $OutputDir `
            --bench-pattern "ab_bench_{label}.json" --output $ReportPath 2>&1 | Out-Host
        $statsCode = $LASTEXITCODE
        $ErrorActionPreference = $prev
        Write-CiLog "ab_stats.py exit=$statsCode, report=$ReportPath" -Level INFO
    } else {
        Write-CiLog "ab_stats.py not found, skip stats: $statsScript" -Level WARN
    }
}

Write-CiLog "Overall: PASS (harness completed)" -Level OK
exit 0
