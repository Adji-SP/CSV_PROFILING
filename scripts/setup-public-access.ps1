# Publish ONLY the authenticated gateway. Never publish Flask or Axum directly.
param([switch]$PrivateOnly)
$ErrorActionPreference = 'Stop'
$tailscale = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
$state = & $tailscale status --json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $state.BackendState -ne 'Running') { throw 'Tailscale must be connected.' }
$hostname = ([string]$state.Self.DNSName).TrimEnd('.')
$gateway = Invoke-RestMethod 'http://127.0.0.1:8088/auth/health' -TimeoutSec 5
if ($gateway.service -ne 'dashboard-gateway' -or $gateway.authentication -ne 'required') {
    throw 'Start scripts/dev-public.ps1 first; the authentication gateway is not ready.'
}
foreach ($port in @(5173,18080,8088)) {
    $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction Stop)
    if (-not $listeners -or @($listeners | Where-Object { $_.LocalAddress -ne '127.0.0.1' }).Count) {
        throw "Port $port must be bound ONLY to 127.0.0.1 before publishing."
    }
}
foreach ($route in @('/api/ota/projects', '/api/ota/devices', '/sessions', '/api/tinyml/status')) {
    $code = 0
    try { $result = Invoke-WebRequest ("http://127.0.0.1:8088" + $route) -UseBasicParsing -TimeoutSec 5; $code = [int]$result.StatusCode }
    catch { if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode } else { throw } }
    if ($code -ne 401) { throw "Gateway authentication check failed for $route (HTTP $code)." }
}
$configText = (& $tailscale serve status --json | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'Cannot read Tailscale Serve configuration.' }
$config = $configText | ConvertFrom-Json
$site = @($config.Web.PSObject.Properties | Where-Object { $_.Name -eq "${hostname}:443" })
if ($site.Count) {
    foreach ($handler in $site[0].Value.Handlers.PSObject.Properties) {
        $target = [string]$handler.Value.Proxy
        $known = switch ($handler.Name.TrimEnd('/')) {
            '' { $target -in @('http://127.0.0.1:5173','http://127.0.0.1:8088') }
            '/api/ota' { $target -eq 'http://127.0.0.1:18080/api/ota' }
            '/api/tinyml' { $target -eq 'http://127.0.0.1:18080/api/tinyml' }
            '/health' { $target -eq 'http://127.0.0.1:18080/health' }
            default { $false }
        }
        if (-not $known) { throw "Unrecognized existing Tailscale route $($handler.Name). No routes changed." }
    }
}
function Invoke-Tailscale([string[]]$Arguments) {
    & $tailscale @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'Tailscale route update failed. Follow any account enable link shown above.' }
}
Invoke-Tailscale @('serve', '--bg', '--yes', '--https=443', 'http://127.0.0.1:8088')
if ($site.Count) {
    foreach ($handler in $site[0].Value.Handlers.PSObject.Properties) {
        if ($handler.Name -ne '/') {
            Invoke-Tailscale @('serve', '--https=443', "--set-path=$($handler.Name)", 'off')
        }
    }
}
$saved = (& $tailscale serve status --json | Out-String | ConvertFrom-Json)
$savedSite = @($saved.Web.PSObject.Properties | Where-Object { $_.Name -eq "${hostname}:443" })
$handlers = @($savedSite[0].Value.Handlers.PSObject.Properties)
if ($handlers.Count -ne 1 -or $handlers[0].Name -ne '/' -or $handlers[0].Value.Proxy -ne 'http://127.0.0.1:8088') {
    throw 'Unexpected routes remain. Public access was not enabled.'
}
if (-not $PrivateOnly) {
    Invoke-Tailscale @('funnel', '--bg', '--yes', '--https=443', 'http://127.0.0.1:8088')
    $saved = (& $tailscale serve status --json | Out-String | ConvertFrom-Json)
    $enabled = @($saved.AllowFunnel.PSObject.Properties | Where-Object { $_.Name -eq "${hostname}:443" -and $_.Value -eq $true })
    if (-not $enabled.Count) { throw 'Funnel is not enabled yet. Follow the Tailscale account approval link, then rerun this script.' }
}
Write-Host "Dashboard login: https://$hostname/login" -ForegroundColor Green
& $tailscale serve status
