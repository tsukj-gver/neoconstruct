# lib_smoke.ps1 - CI 冒烟门禁共享函数库（META-CI-1a）
#
# 设计依据：docs/design/基础设施/CI冒烟门禁设计.md §4.1 + §6.2 + 附录 A
#
# 提供共享函数：
#   - Write-CiLog         颜色化日志输出（同时写控制台 + 报告流）
#   - Resolve-ProjectPaths 解析项目根/CRS 子目录/venv 路径
#   - Invoke-Step          统一执行子进程步骤，收集退出码 + stdout/stderr
#   - Write-CiSummary      控制台彩色汇总（PASS 绿 / FAIL 红 / WARN 黄）
#   - Get-CiTimestamp      生成报告文件名友好的时间戳（yyyyMMdd_HHmmss）
#
# 编码规范：
#   - 函数名 PascalCase；参数 [ValidateNotNullOrEmpty]
#   - 不使用 PowerShell 7 特性（兼容 Windows PowerShell 5.1）
#   - 错误处理 try/catch + $ErrorActionPreference='Stop'
#   - 无裸 Write-Host（封装在 Write-CiLog 中）
#
# 注意：PowerShell 5.1 param block 解析器在 ``$Param = "value",`` 形式下会把
#       尾逗号当数组操作符前缀，导致 "MissingExpressionAfterToken" 错误。
#       本文件所有 param block 均不使用参数间尾逗号（仅用换行分隔）。

# 项目根路径：通过 $PSScriptRoot 向上三级定位
#   testing/ci/lib_smoke.ps1 -> repo root
function Get-ProjectRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
}

# 生成报告文件名友好的时间戳（本地时间，避免时区字符）
function Get-CiTimestamp {
    <#
        .SYNOPSIS
        返回 yyyyMMdd_HHmmss 格式的本地时间戳，用于报告文件名。
    #>
    return (Get-Date -Format "yyyyMMdd_HHmmss")
}

# 颜色化日志：同时输出到控制台和（可选）报告流
function Write-CiLog {
    <#
        .SYNOPSIS
        统一日志输出函数，支持颜色分级。

        .PARAMETER Message
        日志内容。

        .PARAMETER Level
        INFO / OK / WARN / ERROR / STEP。默认 INFO。

        .PARAMETER Indent
        缩进空格数（用于子步骤）。默认 0。
    #>
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Message

        ,
        [ValidateSet("INFO", "OK", "WARN", "ERROR", "STEP")]
        [string]$Level = "INFO"

        ,
        [int]$Indent = 0
    )

    $prefix = ""
    $color = "Gray"
    switch ($Level) {
        "INFO"  { $prefix = "[INFO] ";  $color = "White" }
        "OK"    { $prefix = "[ OK ] ";  $color = "Green" }
        "WARN"  { $prefix = "[WARN] ";  $color = "Yellow" }
        "ERROR" { $prefix = "[FAIL] ";  $color = "Red" }
        "STEP"  { $prefix = "[STEP] ";  $color = "Cyan" }
    }

    $ts = (Get-Date -Format "HH:mm:ss")
    $pad = (" " * $Indent)
    $line = "${prefix}${ts} ${pad}${Message}"

    # 写控制台（用 Write-Host 的 -ForegroundColor，因为这是面向人工审查的彩色输出）
    Write-Host $line -ForegroundColor $color
}

# 解析项目路径与 venv，返回哈希表
function Resolve-ProjectPaths {
    <#
        .SYNOPSIS
        返回项目根、neoconstruct 子目录、venv python.exe 等路径。

        .DESCRIPTION
        venv 路径来自 docs/design/基础设施/CI冒烟门禁设计.md §4.1 / §5.3 约定：
          crs_venv_new   -> neoconstruct maturin develop 目标 venv
          crs_venv_py_new -> Python construct 对比 venv（L3 用，L1 不需要）
        位于 %TEMP%/opencode/ 下。
    #>
    $root = Get-ProjectRoot
    $crsDir = Join-Path $root "neoconstruct"
    $tempBase = Join-Path $env:LOCALAPPDATA "Temp\opencode"

    # 兼容回退：如果 LOCALAPPDATA\Temp\opencode 不存在，尝试 C:\Users\<u>\AppData\Local\Temp\opencode
    if (-not (Test-Path -LiteralPath $tempBase)) {
        $tempBase = Join-Path $env:TEMP "opencode"
    }

    $crsVenv = Join-Path $tempBase "crs_venv_new"
    $pyVenv = Join-Path $tempBase "crs_venv_py_new"
    $crsVenvPython = Join-Path $crsVenv "Scripts\python.exe"

    return @{
        Root            = $root
        CrsDir          = $crsDir
        CrsVenv         = $crsVenv
        PyVenv          = $pyVenv
        CrsVenvPython   = $crsVenvPython
        ReportsDir      = Join-Path $root "testing\\ci\\reports"
    }
}

# 统一执行子进程，捕获退出码与合并输出
function Invoke-Step {
    <#
        .SYNOPSIS
        执行一条命令，捕获退出码与输出。

        .PARAMETER ScriptBlock
        要执行的脚本块。脚本块内部应包含单条主命令。

        .PARAMETER Label
        步骤标签（用于报告与日志）。

        .PARAMETER WorkingDirectory
        工作目录（默认当前）。
    #>
    param(
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [scriptblock]$ScriptBlock

        ,
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [string]$Label

        ,
        [string]$WorkingDirectory
    )

    # 用 SilentlyContinue：避免 PowerShell 5.1 把 native command 的 stderr 输出
    # （如 cargo 的编译日志）包装成 RemoteException ErrorRecord，污染 output 文本。
    # exit code 仍正确捕获（LASTEXITCODE）。
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    try {
        $output = if ($WorkingDirectory) {
            Push-Location -LiteralPath $WorkingDirectory
            try {
                & $ScriptBlock 2>&1
            } finally {
                Pop-Location
            }
        } else {
            & $ScriptBlock 2>&1
        }
        $code = $LASTEXITCODE
        # 没有显式退出码（如 cmdlet）时默认 0
        if ($null -eq $code) { $code = 0 }
    } finally {
        $ErrorActionPreference = $prev
    }

    $outStr = ($output | Out-String).TrimEnd()

    return [pscustomobject]@{
        Label    = $Label
        ExitCode = [int]$code
        Output   = $outStr
        Passed   = ($code -eq 0)
    }
}

# 控制台汇总（一个 level 一行）
function Write-CiSummary {
    <#
        .SYNOPSIS
        根据 PASS/FAIL 输出彩色汇总行。

        .PARAMETER LevelName
        层次名（如 "L1"/"L4"/"Overall"）。

        .PARAMETER Passed
        是否通过。
    #>
    param(
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [string]$LevelName

        ,
        [Parameter(Mandatory = $true)]
        [bool]$Passed
    )

    if ($Passed) {
        Write-CiLog "$LevelName : PASS" -Level OK
    } else {
        Write-CiLog "$LevelName : FAIL" -Level ERROR
    }
}

# 写 JSON 报告（避免依赖 PowerShell 5.1 中 ConvertTo-Json 的深度限制）
function Write-CiReport {
    <#
        .SYNOPSIS
        把结果对象序列化为 JSON 写入文件。

        .PARAMETER Data
        要序列化的对象（Hashtable / PSObject / 数组）。

        .PARAMETER Path
        目标 JSON 文件绝对路径。
    #>
    param(
        [Parameter(Mandatory = $true)]
        $Data

        ,
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [string]$Path
    )

    $json = $Data | ConvertTo-Json -Depth 10 -Compress
    [System.IO.File]::WriteAllText($Path, $json, [System.Text.UTF8Encoding]::new($false))
}
