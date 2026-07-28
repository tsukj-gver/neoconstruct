# 4.x O1-O4 Controlled A/B Test harness (VET stage)
# Alternates A (O1-O4 off) -> B (O1-O4 on) -> A -> B -> A -> B (3 rounds, 6 runs)
# Each run: Copy-Item snapshot -> cargo build --release -> maturin develop --release -> bench
# After each run: sleep 30s (avoid CPU thermal throttling)
# At end: restore the original working-tree error.rs (O1-O4 on)
#
# Snapshot strategy (avoids denied git stash/checkout):
#   1. Before harness: copy working-tree error.rs to _snapshot\error_rs_on.rs (B state)
#   2. Before harness: git show HEAD:construct-rs/src/error.rs > _snapshot\error_rs_off.rs (A state)
#   3. A/B switch: Copy-Item _snapshot\error_rs_<state>.rs construct-rs\src\error.rs -Force
#
# Pure ASCII to avoid Windows PowerShell 5.1 encoding issues.

$ErrorActionPreference = "Stop"

$REPO      = "<legacy-repo>"
$CRS_DIR   = Join-Path $REPO "construct-rs"
$ERROR_RS  = "construct-rs\src\error.rs"
$BENCH     = Join-Path $REPO "experiments\phase4_4x_ab_bench.py"
$CRS_PY    = "<opencode-temp>\crs_venv_new\Scripts\python.exe"
$LOG       = Join-Path $REPO "experiments\phase4_4x_ab_harness.log"
$SNAP_DIR  = Join-Path $REPO "experiments\_snapshot_4x"
$SNAP_ON   = Join-Path $SNAP_DIR "error_rs_on.rs"
$SNAP_OFF  = Join-Path $SNAP_DIR "error_rs_off.rs"
$DLL_PATH  = Join-Path $CRS_DIR "target\release\construct_rs.dll"
$DLL_HASH_ON  = Join-Path $SNAP_DIR "dll_hash_on.txt"
$DLL_HASH_OFF = Join-Path $SNAP_DIR "dll_hash_off.txt"

function Write-Log {
    param([string]$Msg)
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $Msg"
    Write-Host $line
    Add-Content -LiteralPath $LOG -Value $line -Encoding UTF8
}

function Get-File-MD5 {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return "N/A" }
    return (Get-FileHash -LiteralPath $Path -Algorithm MD5).Hash
}

function Set-State {
    param([string]$State)
    $snap = if ($State -eq "A") { $SNAP_OFF } else { $SNAP_ON }
    Copy-Item -LiteralPath $snap -Destination (Join-Path $REPO $ERROR_RS) -Force
    Write-Log "  Set-State $State : copied $snap -> $ERROR_RS"
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
# Pre-flight: snapshot error.rs (on / off) + DLL hashes
# ============================================================
if (-not (Test-Path $SNAP_DIR)) {
    New-Item -ItemType Directory -Path $SNAP_DIR | Out-Null
}

if (Test-Path $LOG) { Remove-Item -LiteralPath $LOG }

Write-Log "==== 4.x O1-O4 Controlled A/B Test start ===="

# Snapshot ON (current working tree, O1-O4 applied)
Copy-Item -LiteralPath (Join-Path $REPO $ERROR_RS) -Destination $SNAP_ON -Force
Write-Log "Snapshot ON  (working tree) -> $SNAP_ON"

# Snapshot OFF (HEAD, O1-O4 not yet committed)
# Use cmd /c redirect to avoid PowerShell Out-File UTF8 BOM + line-ending corruption
$offSnapRel = "experiments\_snapshot_4x\error_rs_off.rs"
Push-Location $REPO
cmd /c "git show HEAD:construct-rs/src/error.rs > $offSnapRel" 2>&1 | Out-Null
$gitCode = $LASTEXITCODE
Pop-Location
if ($gitCode -ne 0) {
    Write-Log "FATAL: git show HEAD:construct-rs/src/error.rs failed (exit $gitCode)"
    throw "git show failed"
}
Write-Log "Snapshot OFF (HEAD)        -> $SNAP_OFF (via cmd /c redirect, no BOM)"

# DLL hash snapshot after first build of each state
Write-Log ""
Write-Log "---- Pre-flight: build both states once + capture DLL hash ----"

Set-State -State "A"
Write-Log "  Build A (off) ..."
Build-Install
$hashA = Get-File-MD5 $DLL_PATH
$sizeA = if (Test-Path -LiteralPath $DLL_PATH) { (Get-Item -LiteralPath $DLL_PATH).Length } else { 0 }
Write-Log "  DLL A (off): md5=$hashA  size=$sizeA bytes"
$hashA | Out-File -LiteralPath $DLL_HASH_OFF -Encoding UTF8

Set-State -State "B"
Write-Log "  Build B (on) ..."
Build-Install
$hashB = Get-File-MD5 $DLL_PATH
$sizeB = if (Test-Path -LiteralPath $DLL_PATH) { (Get-Item -LiteralPath $DLL_PATH).Length } else { 0 }
Write-Log "  DLL B (on):  md5=$hashB  size=$sizeB bytes"
$hashB | Out-File -LiteralPath $DLL_HASH_ON -Encoding UTF8

if ($hashA -eq $hashB) {
    Write-Log "WARN: DLL hash identical for A and B -- cargo may have skipped re-compile"
} else {
    Write-Log "OK: DLL hashes differ ($hashA vs $hashB) -- cargo did rebuild"
}

# ============================================================
# Main: A1 B1 A2 B2 A3 B3
# ============================================================
$sequence = @(
    @{Round=1; State="A"; Label="A1_Ooff"},
    @{Round=1; State="B"; Label="B1_Oon"},
    @{Round=2; State="A"; Label="A2_Ooff"},
    @{Round=2; State="B"; Label="B2_Oon"},
    @{Round=3; State="A"; Label="A3_Ooff"},
    @{Round=3; State="B"; Label="B3_Oon"}
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

# ============================================================
# Cleanup: restore working tree (O1-O4 on)
# ============================================================
Write-Log ""
Write-Log "----- Restore working tree error.rs (O1-O4 on) -----"
Set-State -State "B"
Build-Install
Write-Log "==== A/B Test end ===="
Write-Log "Log:     $LOG"
Write-Log "Snap:    $SNAP_DIR"
Write-Log "DLL md5: OFF=$hashA  ON=$hashB"
