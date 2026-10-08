#requires -Version 5.1
param(
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'csv-mqtt'),
    [string]$MosquittoDir = 'F:\Mosquito'
)
$ErrorActionPreference = 'Stop'
$config = Join-Path $DataDir 'mosquitto.dashboard.conf'
$broker = Join-Path $MosquittoDir 'mosquitto.exe'
if (-not (Test-Path -LiteralPath $config) -or -not (Test-Path -LiteralPath $broker)) {
    throw 'Run scripts/setup-mqtt.ps1 first, or provide the correct -DataDir and -MosquittoDir.'
}
$config = (Resolve-Path -LiteralPath $config).Path
$broker = (Resolve-Path -LiteralPath $broker).Path
$ports = @(Get-Content -LiteralPath $config | ForEach-Object { if ($_ -match '^\s*listener\s+(\d+)') { [int]$Matches[1] } })
if (-not $ports.Count) { throw 'No MQTT listeners configured.' }
$running = @()
foreach ($port in $ports) {
    $listener = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
    foreach ($entry in $listener) {
        $process = Get-CimInstance Win32_Process -Filter "ProcessId=$($entry.OwningProcess)"
        if ($process.ExecutablePath -ne $broker -or -not $process.CommandLine.Contains($config)) {
            throw "Port $port belongs to another process; nothing was stopped."
        }
        $running += $process.ProcessId
    }
}
if ($running.Count) {
    if (@($running | Select-Object -Unique).Count -ne 1) { throw 'More than one process owns these ports; check the broker configuration.' }
    foreach ($port in $ports) {
        if (-not (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)) { throw "Existing broker is missing listener $port; inspect its logs." }
    }
    Write-Host "MQTT broker already running (PID $($running[0])). Credentials unchanged."
    return
}
$logs = Join-Path $DataDir 'logs'
New-Item -ItemType Directory -Path $logs -Force | Out-Null
$stamp = [Guid]::NewGuid().ToString('N')
$process = Start-Process -FilePath $broker -ArgumentList @('-c', ('"' + $config + '"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logs "$stamp.out.log") -RedirectStandardError (Join-Path $logs "$stamp.err.log")
Start-Sleep -Milliseconds 750
if ($process.HasExited) { throw "MQTT failed to start; inspect logs in $logs." }
Write-Host "MQTT broker started (PID $($process.Id)). Logs: $logs"
