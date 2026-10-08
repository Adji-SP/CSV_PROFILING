param(
    [switch]$CheckOnly,
    [switch]$SmokeTest,
    [switch]$WithMqtt,
    [switch]$AllowLanDevices,
    [string]$LanAddress,
    [switch]$PublicOta,
    [switch]$Gateway,
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

if ($Gateway) {
    if ($AllowLanDevices) { throw 'The login gateway requires loopback-only Flask and Rust services. Omit -AllowLanDevices.' }
    $settings.GATEWAY_PUBLIC_URL = $settings.APP_PUBLIC_OTA_API_BASE_URL
    $settings.GATEWAY_OTA_ORIGIN = "http://127.0.0.1:$ApiPort"
    $settings.GATEWAY_CSV_ORIGIN = 'http://127.0.0.1:5173'
    $settings.OTA_PUBLIC_BASE_URL = $settings.APP_PUBLIC_OTA_API_BASE_URL
}

if ($PublicOta -and -not $Gateway) {
    # Do not bake a nonexistent domain into firmware. A tunnel token alone
    # neither creates DNS routes nor authorizes Internet visitors.
    foreach ($hostname in @($publicHost, $publicApi.Host)) {
        try { Resolve-DnsName $hostname -ErrorAction Stop | Out-Null }
        catch { throw "DNS does not resolve $hostname. Configure the active Cloudflare domain and published routes first; no firmware URLs were changed." }
    }
    $settings.OTA_PUBLIC_BASE_URL = $settings.APP_PUBLIC_OTA_API_BASE_URL
    Write-Warning 'Public OTA requires authenticated device access and HTTPS-capable firmware. A viewer token does not protect upload/build/deploy routes. See docs/CLOUDFLARE_DEV_SETUP.md.'
} elseif ($AllowLanDevices) {
    . (Join-Path $PSScriptRoot 'dev-network.ps1')
    $deviceAddress = Get-DevLanAddress $LanAddress
    # Match both the active adapter and this launcher's port, not a stale .env URL.
    $settings.OTA_PUBLIC_BASE_URL = "http://${deviceAddress}:$ApiPort"
} elseif ($LanAddress) {
    throw 'Use -LanAddress with -AllowLanDevices.'
}

function Assert-PortAvailable([int]$Port) {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Any, $Port)
    try { $listener.Start() }
    catch { throw "Port $Port is in use. Stop the existing project launcher or choose another -ApiPort; no process was stopped." }
    finally { $listener.Stop() }
}
Assert-PortAvailable 5173
Assert-PortAvailable $ApiPort
if ($Gateway) { Assert-PortAvailable 8088 }
$targetDir = Join-Path $projectRoot 'ota-server\target'
$rustExe = Join-Path $targetDir 'debug\csv-profiler-ota-server.exe'
# A server on a different port still locks this .exe and shares the same DB.
foreach ($process in @(Get-Process -Name 'csv-profiler-ota-server' -ErrorAction SilentlyContinue)) {
    if (-not $process.Path -or $process.Path -eq $rustExe) {
        throw "An OTA server is already running (PID $($process.Id)). Stop its launcher before switching profiles. No process was stopped."
    }
}
Write-Host 'Secure development profile'
Write-Host 'Dashboard + CSV: http://localhost:5173'
Write-Host "OTA + Monitoring: http://localhost:$ApiPort"
if ($Gateway) {
    Write-Host "Tunnel route: $publicHost -> http://127.0.0.1:8088 (login gateway)"
} else {
    Write-Host "Tunnel routes: $publicHost -> http://127.0.0.1:5173"
    Write-Host "               $($publicApi.Host) -> http://127.0.0.1:$ApiPort"
}
if ($AllowLanDevices) { Write-Warning 'LAN API enabled. Restrict Windows Firewall to your trusted network.' }
if ($settings.OTA_PUBLIC_BASE_URL) {
    Write-Host "Firmware download / new build URL: $($settings.OTA_PUBLIC_BASE_URL)"
    Write-Host 'Already-flashed devices keep their embedded server URL until rebuilt.'
} else {
    Write-Host 'Browser-only profile: device URLs still use .env. Use -AllowLanDevices for LAN OTA.'
}
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
    if ($Gateway) {
        Push-Location (Join-Path $projectRoot 'python')
        try {
            & $PythonExe -c 'from gateway.config import load_config; from gateway.server import create_app; create_app(load_config())'
            if ($LASTEXITCODE -ne 0) { throw 'Login gateway setup is incomplete. Run scripts/setup-dashboard-login.ps1.' }
        } finally { Pop-Location }
    }
    if ($WithMqtt) { & (Join-Path $PSScriptRoot 'start-mqtt.ps1') }
    # Compile before starting either service; do not leave Flask half-running if Cargo fails.
    & cargo build --manifest-path (Join-Path $projectRoot 'ota-server\Cargo.toml') --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'Rust compilation failed. Neither service was started.' }
    $logDir = Join-Path $projectRoot ('data\dev-profile\' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    $children += Start-Process -FilePath $PythonExe -ArgumentList @('-u', ('"' + (Join-Path $projectRoot 'python\profiler_server.py') + '"')) -WorkingDirectory $projectRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'flask.out.log') -RedirectStandardError (Join-Path $logDir 'flask.err.log')
    $children += Start-Process -FilePath $rustExe -WorkingDirectory $projectRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'ota.out.log') -RedirectStandardError (Join-Path $logDir 'ota.err.log')
    if ($Gateway) {
        $children += Start-Process -FilePath $PythonExe -ArgumentList @('-u', '-m', 'gateway') -WorkingDirectory (Join-Path $projectRoot 'python') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logDir 'gateway.out.log') -RedirectStandardError (Join-Path $logDir 'gateway.err.log')
    }
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
            if ($Gateway) {
                $gatewayHealth = Invoke-RestMethod 'http://127.0.0.1:8088/auth/health' -TimeoutSec 2
                $ready = $ready -and $gatewayHealth.authentication -eq 'required'
            }
        } catch { $ready = $false }
        if ($ready) { break }
        Start-Sleep -Seconds 1
    }
    if (-not $ready) { throw "Startup timed out. See logs in $logDir." }
    Write-Host 'Services are ready. Press Ctrl+C here to stop this stack.' -ForegroundColor Green
    if ($Gateway) { Write-Host "Log in: $($settings.GATEWAY_PUBLIC_URL)/login" -ForegroundColor Cyan }
    Write-Host 'Any Tailscale or Cloudflare tunnel is managed separately.'
    if ($SmokeTest) {
        $ota = Invoke-RestMethod "http://127.0.0.1:$ApiPort/api/ota/status" -TimeoutSec 5
        if ($settings.OTA_PUBLIC_BASE_URL -and $ota.public_base_url -ne $settings.OTA_PUBLIC_BASE_URL) {
            throw 'Smoke test: API advertises an unexpected firmware download address.'
        }
        if ($WithMqtt) {
            . (Join-Path $PSScriptRoot 'env-file.ps1')
            $fileSettings = Read-ProjectEnv (Join-Path $projectRoot '.env')
            $viewerToken = if ($env:CONSOLE_VIEWER_TOKEN) { $env:CONSOLE_VIEWER_TOKEN } else { $fileSettings['CONSOLE_VIEWER_TOKEN'] }
            $headers = @{ Authorization = 'Bearer ' + $viewerToken }
            $connected = $false
            for ($attempt = 0; $attempt -lt 10; $attempt++) {
                $monitor = Invoke-RestMethod "http://127.0.0.1:$ApiPort/api/tinyml/status" -Headers $headers -TimeoutSec 3
                if ($monitor.mqtt_configured -and $monitor.mqtt_connected) { $connected = $true; break }
                Start-Sleep -Seconds 1
            }
            if (-not $connected) { throw 'Smoke test: Rust reporting has not established its MQTT subscription.' }
            Write-Host 'PASS authenticated Reporting API and Rust MQTT subscription.'
        }
        Write-Host 'Smoke test passed; stopping the test services.'
        return
    }
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
