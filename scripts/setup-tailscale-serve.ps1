param(
    [ValidateRange(1, 65535)][int]$ApiPort = 18080,
    [ValidateRange(1, 65535)][int]$DashboardPort = 5173,
    [switch]$NoElevation
)

$ErrorActionPreference = 'Stop'

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not $NoElevation -and -not (Test-Administrator)) {
    $arguments = @(
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-File', ('"' + $PSCommandPath + '"'),
        '-ApiPort', [string]$ApiPort,
        '-DashboardPort', [string]$DashboardPort,
        '-NoElevation'
    )
    $process = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -Verb RunAs -Wait -PassThru
    exit $process.ExitCode
}

$tailscale = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
if (-not (Test-Path -LiteralPath $tailscale -PathType Leaf)) {
    throw 'Tailscale is not installed. Install it and sign in before running this script.'
}

$status = (& $tailscale status --json | ConvertFrom-Json)
if ($status.BackendState -ne 'Running') {
    throw "Tailscale is not connected (state: $($status.BackendState)). Open Tailscale and sign in first."
}

$hostname = ([string]$status.Self.DNSName).Trim().TrimEnd('.').ToLowerInvariant()
if (-not $hostname) {
    throw 'Tailscale did not provide a MagicDNS hostname for this computer.'
}

$existingRoutes = (& $tailscale serve status --json | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'Could not inspect existing Tailscale routes.' }
if ($existingRoutes -match 'http://127\.0\.0\.1:8088' -or $existingRoutes -match '"AllowFunnel"\s*:\s*\{[^}]*true') {
    throw 'The authenticated/public gateway is already configured. Use setup-public-access.ps1 to preserve login protection.'
}

function Set-ServeRoute([string]$Path, [string]$Target) {
    $arguments = @('serve', '--bg', '--yes', '--https=443')
    if ($Path -ne '/') { $arguments += "--set-path=$Path" }
    $arguments += $Target
    $output = (& $tailscale @arguments 2>&1 | Out-String).Trim()
    if ($output) { Write-Host $output }
    if ($output -match 'Serve is not enabled') {
        throw 'Enable Tailscale Serve using the URL above, then run this script again.'
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Could not configure Tailscale Serve route $Path."
    }
}

# These commands update only this node's HTTPS routes. They do not reset or
# delete unrelated Tailscale settings.
Set-ServeRoute '/' "http://127.0.0.1:$DashboardPort"
Set-ServeRoute '/api/ota' "http://127.0.0.1:$ApiPort/api/ota"
Set-ServeRoute '/api/tinyml' "http://127.0.0.1:$ApiPort/api/tinyml"
Set-ServeRoute '/health' "http://127.0.0.1:$ApiPort/health"

$configured = (& $tailscale serve status 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0 -or $configured -match 'No serve config') {
    throw 'Tailscale did not save the Serve configuration.'
}

Write-Host ''
Write-Host 'Tailscale Serve is configured.' -ForegroundColor Green
Write-Host "Dashboard: https://$hostname"
Write-Host "OTA API:   https://$hostname/api/ota/status"
Write-Host ''
Write-Host 'This address is private to devices signed in to your Tailscale network.'
Write-Host 'Now start the project with: .\scripts\dev-tailscale.ps1'
