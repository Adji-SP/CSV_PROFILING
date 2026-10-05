param(
    [switch]$CheckOnly,
    [switch]$SmokeTest,
    [switch]$AllowLanDevices,
    [switch]$PublicOta,
    [ValidateRange(1, 65535)][int]$ApiPort = 18080,
    [string]$PublicFrontendHost = 'dev.iot-prototype.net',
    [string]$PublicApiUrl = 'https://api-dev.iot-prototype.net',
    [string]$PythonExe
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
if (-not $PythonExe) { $PythonExe = Join-Path $projectRoot 'python\.venv\Scripts\python.exe' }
if (-not (Test-Path -LiteralPath $PythonExe -PathType Leaf)) {
    throw 'Create python\.venv and install python\requirements.txt, or pass -PythonExe with the Python interpreter you use for CSV profiling.'
}
$PythonExe = (Resolve-Path -LiteralPath $PythonExe).Path
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo is not on PATH.' }

# Process-local overrides only. Ordinary run-dev.bat/.sh and .env are unchanged.
$publicHost = $PublicFrontendHost.Trim().ToLowerInvariant()
if ($publicHost -notmatch '^(?=.{1,253}$)[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+$') {
    throw 'PublicFrontendHost must be a DNS hostname, without a scheme or underscore.'
}
$publicApi = $null
if (-not [Uri]::TryCreate($PublicApiUrl, [UriKind]::Absolute, [ref]$publicApi) -or
    $publicApi.Scheme -ne 'https' -or $publicApi.UserInfo -or $publicApi.Query -or
    $publicApi.Fragment -or $publicApi.AbsolutePath -ne '/' -or $publicApi.IsLoopback -or
    $publicApi.HostNameType -ne [UriHostNameType]::Dns) {
    throw 'PublicApiUrl must be a public HTTPS origin, without a path or credentials.'
}
$settings = @{
    CSV_BIND_ADDRESS = '127.0.0.1'
    CSV_PORT = '5173'
    OTA_BIND_ADDRESS = $(if ($AllowLanDevices) { '0.0.0.0' } else { '127.0.0.1' })
    OTA_PORT = [string]$ApiPort
    APP_LOCAL_OTA_API_BASE_URL = "http://localhost:$ApiPort"
    APP_PUBLIC_FRONTEND_HOST = $publicHost
    APP_PUBLIC_OTA_API_BASE_URL = $publicApi.GetLeftPart([UriPartial]::Authority)
}
$origins = @('http://localhost:5173', 'http://127.0.0.1:5173', 'http://localhost:5000', 'http://127.0.0.1:5000', "https://$publicHost")
if ($env:OTA_ALLOWED_ORIGINS) { $origins += $env:OTA_ALLOWED_ORIGINS -split ',' }
$settings.OTA_ALLOWED_ORIGINS = ($origins | Select-Object -Unique) -join ','

if ($PublicOta) {
    # Do not bake a nonexistent domain into firmware. A tunnel token alone
    # neither creates DNS routes nor authorizes Internet visitors.
    foreach ($hostname in @($publicHost, $publicApi.Host)) {
        try { Resolve-DnsName $hostname -ErrorAction Stop | Out-Null }
        catch { throw "DNS does not resolve $hostname. Configure the active Cloudflare domain and published routes first; no firmware URLs were changed." }
    }
    $settings.OTA_PUBLIC_BASE_URL = $settings.APP_PUBLIC_OTA_API_BASE_URL
    Write-Warning 'Public OTA requires authenticated device access and HTTPS-capable firmware. A viewer token does not protect upload/build/deploy routes. See docs/CLOUDFLARE_DEV_SETUP.md.'
}

function Assert-PortAvailable([int]$Port) {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Any, $Port)
    try { $listener.Start() }
    catch { throw "Port $Port is in use. Stop the existing project launcher or choose another -ApiPort; no process was stopped." }
    finally { $listener.Stop() }
}
Assert-PortAvailable 5173
Assert-PortAvailable $ApiPort
$targetDir = Join-Path $projectRoot 'ota-server\target'
$rustExe = Join-Path $targetDir 'debug\csv-profiler-ota-server.exe'
# A server on a different port still locks this .exe and shares the same DB.
foreach ($process in @(Get-Process -Name 'csv-profiler-ota-server' -ErrorAction SilentlyContinue)) {
    if (-not $process.Path -or $process.Path -eq $rustExe) {
        throw "An OTA server is already running (PID $($process.Id)). Stop its launcher before switching profiles. No process was stopped."
    }
}
Write-Host 'Cloudflare development profile'
Write-Host 'Dashboard + CSV: http://localhost:5173'
Write-Host "OTA + Monitoring: http://localhost:$ApiPort"
Write-Host "Tunnel routes: $publicHost -> http://127.0.0.1:5173"
Write-Host "               $($publicApi.Host) -> http://127.0.0.1:$ApiPort"
if ($AllowLanDevices) { Write-Warning 'LAN API enabled. Restrict Windows Firewall to your trusted network.' }
if (-not $PublicOta) { Write-Host 'Device URLs still use .env. Add -PublicOta only after DNS, routing and access policies are ready.' }
if ($CheckOnly) { return }

$previousSettings = @{}
$children = @()
try {
    foreach ($key in $settings.Keys) {
        $previousSettings[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $settings[$key], 'Process')
    }
    & $PythonExe -c 'import flask, pandas, ydata_profiling'
    if ($LASTEXITCODE -ne 0) { throw 'Install python/requirements.txt using the selected Python interpreter.' }
    # Compile before starting either service; do not leave Flask half-running if Cargo fails.
    & cargo build --manifest-path (Join-Path $projectRoot 'ota-server\Cargo.toml') --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'Rust compilation failed. Neither service was started.' }
    $logDir = Join-Path $projectRoot ('data\dev-profile\' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    $children += Start-Process -FilePath $PythonExe -ArgumentList @('-u', ('"' + (Join-Path $projectRoot 'python\profiler_server.py') + '"')) -WorkingDirectory $projectRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'flask.out.log') -RedirectStandardError (Join-Path $logDir 'flask.err.log')
    $children += Start-Process -FilePath $rustExe -WorkingDirectory $projectRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'ota.out.log') -RedirectStandardError (Join-Path $logDir 'ota.err.log')
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    $ready = $false
    Write-Host "Starting services. Logs: $logDir"
    while ([DateTime]::UtcNow -lt $deadline) {
        foreach ($child in $children) {
            if ($child.HasExited) { throw "Project service PID $($child.Id) exited with code $($child.ExitCode). See logs in $logDir." }
        }
        try {
            $api = Invoke-RestMethod "http://127.0.0.1:$ApiPort/health" -TimeoutSec 2
            $browser = Invoke-WebRequest 'http://127.0.0.1:5173/app-config.js' -UseBasicParsing -TimeoutSec 2
            $ready = $api.service -eq 'csv-profiling-api' -and $browser.Content.Contains("http://localhost:$ApiPort")
        } catch { $ready = $false }
        if ($ready) { break }
        Start-Sleep -Seconds 1
    }
    if (-not $ready) { throw "Startup timed out. See logs in $logDir." }
    Write-Host 'Both services are ready. Press Ctrl+C here to stop these two services.' -ForegroundColor Green
    Write-Host 'The installed cloudflared Windows service is managed separately.'
    if ($SmokeTest) { Write-Host 'Smoke test passed; stopping the test services.'; return }
    while ($true) {
        foreach ($child in $children) {
            if ($child.HasExited) { throw "Project service PID $($child.Id) exited. See logs in $logDir." }
        }
        Start-Sleep -Seconds 1
    }
} finally {
    # Only stop exact processes created by this invocation, never arbitrary port owners.
    foreach ($child in $children) {
        if (-not $child.HasExited) { $child.Kill(); $child.WaitForExit(5000) | Out-Null }
        $child.Dispose()
    }
    foreach ($key in $previousSettings.Keys) {
        [Environment]::SetEnvironmentVariable($key, $previousSettings[$key], 'Process')
    }
}
