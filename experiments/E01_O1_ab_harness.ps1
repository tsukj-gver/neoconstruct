# E01 O1 regression investigation: Controlled A/B Test harness
# Alternate measurement A(O1 off) -> B(O1 on) -> A -> B -> A -> B (3 rounds, 6 runs)
# Each run: checkout stop_if.rs -> cargo build --release -> maturin develop --release -> bench
# After each run: sleep 30s (avoid CPU thermal throttling)
# At end: restore HEAD version of stop_if.rs
# Pure ASCII to avoid Windows PowerShell 5.1 encoding issues.

$ErrorActionPreference = "Stop"

$REPO     = "<legacy-repo>"
$CRS_DIR  = Join-Path $REPO "construct-rs"
$STOP_IF  = "construct-rs/src/nodes/stop_if.rs"
$BENCH    = Join-Path $REPO "experiments\E01_O1_ab_bench.py"
$CRS_PY   = "<opencode-temp>\crs_venv_new\Scripts\python.exe"
$LOG      = Join-Path $REPO "experiments\E01_O1_ab_harness.log"

$O1_PARENT = "c77c0ea~1"   # parent of O1 commit (O1 off state)

function Write-Log {
    param([string]$Msg)
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $Msg"
    Write-Host $line
    Add-Content -LiteralPath $LOG -Value $line -Encoding UTF8
}

function Set-State {
    param([string]$State)
    Push-Location $REPO
    try {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        if ($State -eq "A") {
            git checkout $O1_PARENT -- $STOP_IF 2>&1 | Out-Null
        } else {
            git checkout "HEAD" -- $STOP_IF 2>&1 | Out-Null
        }
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -ne 0) { throw "git checkout $State failed (exit $code)" }
    } finally { Pop-Location }
}

function Build-Install {
    $env:PYO3_USE_ABI3_FORWARD_COMPATIBILITY = "1"
    $env:VIRTUAL_ENV = "<opencode-temp>\crs_venv_new"
    Push-Location $CRS_DIR
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
        if ($code1 -ne 0) { throw "cargo build failed (exit $code1):`n$buildOut" }
        if ($code2 -ne 0) { throw "maturin develop failed (exit $code2):`n$matOut" }
    } finally { Pop-Location }
}

function Run-Bench {
    param([string]$Label)
    $env:PYO3_USE_ABI3_FORWARD_COMPATIBILITY = "1"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $out = & $CRS_PY $BENCH $Label 2>&1
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) {
        Write-Log "  BENCH $Label FAILED (exit $code)"
        $out | Add-Content -LiteralPath $LOG -Encoding UTF8
        throw "bench failed (exit $code)"
    }
    $summaryLine = ($out | Select-String -Pattern '^\[SUMMARY\]' | Select-Object -Last 1).Line
    if (-not $summaryLine) { $summaryLine = ($out | Select-Object -Last 3) -join ' | ' }
    Write-Log "  BENCH $Label : $summaryLine"
    $out | Add-Content -LiteralPath $LOG -Encoding UTF8
}

# ============================================================
# Main
# ============================================================
if (Test-Path $LOG) { Remove-Item $LOG }
Write-Log "==== E01 O1 Controlled A/B Test start ===="
Write-Log "O1 commit parent = $O1_PARENT (O1 off) ; B = HEAD (O1 on)"
Write-Log "Sequence: A1 B1 A2 B2 A3 B3"

Push-Location $REPO
$statusStopIf = (git status --porcelain -- $STOP_IF) -join ','
Pop-Location
Write-Log "Initial stop_if.rs git status: '$statusStopIf' (empty = clean)"

# Verify O1_PARENT resolves
Push-Location $REPO
$verify = git rev-parse --short $O1_PARENT 2>&1
Pop-Location
Write-Log "Verify O1_PARENT ($O1_PARENT) resolves to: $verify"

$sequence = @(
    @{Round=1; State="A"; Label="A1_O1off"},
    @{Round=1; State="B"; Label="B1_O1on"},
    @{Round=2; State="A"; Label="A2_O1off"},
    @{Round=2; State="B"; Label="B2_O1on"},
    @{Round=3; State="A"; Label="A3_O1off"},
    @{Round=3; State="B"; Label="B3_O1on"}
)

foreach ($step in $sequence) {
    $round = $step.Round
    $state = $step.State
    $label = $step.Label
    Write-Log ""
    Write-Log "----- Round $round / Condition $state ($label) -----"
    Write-Log "  Set-State $state ..."
    Set-State -State $state
    Write-Log "  Build + maturin develop ..."
    Build-Install
    Write-Log "  Bench ..."
    Run-Bench -Label $label
    Write-Log "  Sleep 30s ..."
    Start-Sleep -Seconds 30
}

Write-Log ""
Write-Log "----- Restore HEAD stop_if.rs -----"
Set-State -State "B"
Write-Log "==== A/B Test end ===="
Write-Log "Log: $LOG"
