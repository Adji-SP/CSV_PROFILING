param([switch]$WithoutMqtt, [switch]$CheckOnly, [switch]$SmokeTest, [string]$PythonExe)
$ErrorActionPreference = 'Stop'
$tailscale = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
$state = & $tailscale status --json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $state.BackendState -ne 'Running') { throw 'Connect Tailscale on this PC first.' }
$hostname = ([string]$state.Self.DNSName).TrimEnd('.')
if (-not $hostname) { throw 'Tailscale did not return the fixed hostname.' }
$parameters = @{
    Gateway = $true
    PublicFrontendHost = $hostname
    PublicApiUrl = "https://$hostname"
    WithMqtt = -not $WithoutMqtt
    CheckOnly = $CheckOnly
    SmokeTest = $SmokeTest
}
if ($PythonExe) { $parameters.PythonExe = $PythonExe }
& (Join-Path $PSScriptRoot 'dev.ps1') @parameters
