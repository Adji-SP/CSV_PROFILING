# Run from PowerShell: .\scripts\test-run-dev.ps1
# Exercise the launcher's real START/CMD quoting with finite version checks.
# /b avoids new windows, /wait captures exit status, and /c exits after the probe.
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path + '\'
$launcher = Get-Content -LiteralPath (Join-Path $projectRoot 'run-dev.bat')
$pythonExe = Join-Path $projectRoot 'python\.venv\Scripts\python.exe'

function Test-LaunchLine {
    param([string] $Title, [string] $ProbeCommand)

    $lines = @($launcher | Where-Object { $_.StartsWith('start "' + $Title + '" ') })
    if ($lines.Count -ne 1) {
        throw "Expected one launcher command for $Title"
    }
    $probe = $lines[0].Replace('%PROJECT_ROOT%', $projectRoot).Replace('%PYTHON_EXE%', $pythonExe)
    $probe = $probe.Replace('python\profiler_server.py', '--version')
    $probe = $probe.Replace('%CARGO_RUN%', $ProbeCommand)
    $probe = $probe -replace '^start ', 'start /b /wait '
    $probe = $probe -replace '(?i) /k ', ' /c '

    # Leave native stderr visible: it contains the original SyntaxError on a
    # regression. No profiling/build/server process is started by these probes.
    & $env:ComSpec /d /s /c $probe
    if ($LASTEXITCODE -ne 0) {
        throw "$Title launcher check failed with exit code $LASTEXITCODE"
    }
    Write-Output "PASS: $Title launcher command"
}

if (-not (Test-Path -LiteralPath $pythonExe -PathType Leaf)) {
    throw 'Create the project Python virtual environment before running this test.'
}
Get-Command cargo -ErrorAction Stop | Out-Null

Test-LaunchLine -Title 'CSV Profiler :5000' -ProbeCommand ''
Test-LaunchLine -Title 'OTA Server :7000' -ProbeCommand 'cargo --version'
