param(
    [ValidateRange(1, 65535)][int]$ApiPort = 18080,
    [switch]$CheckOnly,
    [switch]$SmokeTest,
    [switch]$AllowLanDevices,
    [switch]$WithoutMqtt,
    [string]$LanAddress,
    [string]$PythonExe
)

$ErrorActionPreference = 'Stop'
$tailscale = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
if (-not (Test-Path -LiteralPath $tailscale -PathType Leaf)) {
    throw 'Tailscale is not installed. Install it and sign in first.'
}

$status = (& $tailscale status --json | ConvertFrom-Json)
if ($status.BackendState -ne 'Running') {
    throw "Tailscale is not connected (state: $($status.BackendState))."
}

$hostname = ([string]$status.Self.DNSName).Trim().TrimEnd('.').ToLowerInvariant()
if (-not $hostname) { throw 'Tailscale did not provide a MagicDNS hostname.' }

$arguments = @{
    ApiPort = $ApiPort
    PublicFrontendHost = $hostname
    PublicApiUrl = "https://$hostname"
}
if (-not $WithoutMqtt) { $arguments.WithMqtt = $true }
if ($CheckOnly) { $arguments.CheckOnly = $true }
if ($SmokeTest) { $arguments.SmokeTest = $true }
if ($AllowLanDevices) { $arguments.AllowLanDevices = $true }
if ($LanAddress) { $arguments.LanAddress = $LanAddress }
if ($PythonExe) { $arguments.PythonExe = $PythonExe }

Write-Host "Fixed private URL: https://$hostname" -ForegroundColor Cyan
Write-Host 'Tailscale proxies the dashboard and both Rust API groups through HTTPS port 443.'
Write-Host "Local services still listen separately on ports 5173 and $ApiPort."
Write-Host ''

& (Join-Path $PSScriptRoot 'dev.ps1') @arguments
