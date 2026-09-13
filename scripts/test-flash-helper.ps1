# Requires the installed toolchain. Does not compile, prompt, or access a device.
$ErrorActionPreference = 'Stop'
$helper = Join-Path $PSScriptRoot '..\flash-esp32.ps1'
$originalLocation = (Get-Location).Path
$watchedEnvironment = @{}
foreach ($name in @('PATH', 'LIB', 'INCLUDE', 'CARGO_TARGET_DIR', 'CARGO_WORKSPACE_DIR',
        'ESP_IDF_TOOLS_INSTALL_DIR', 'WIFI_SSID', 'WIFI_PASS', 'OTA_SERVER_URL')) {
    $watchedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

function Assert-Restored {
    if ((Get-Location).Path -ne $originalLocation) { throw 'Working directory was not restored.' }
    foreach ($name in $watchedEnvironment.Keys) {
        if ([Environment]::GetEnvironmentVariable($name, 'Process') -cne $watchedEnvironment[$name]) {
            throw "Environment variable $name was not restored."
        }
    }
}

& $helper -CheckOnly
Assert-Restored
Write-Output 'PASS: prerequisite-only mode and environment restoration'

$rejected = $false
try { & $helper -CheckOnly -TargetDir 'relative-cache' }
catch {
    if ($_.Exception.Message -notlike '*absolute local paths*') { throw }
    $rejected = $true
}
if (-not $rejected) { throw 'Relative target path was accepted.' }
Assert-Restored
Write-Output 'PASS: invalid target path rejected; environment restored after error'

$rejected = $false
try { & $helper -BuildOnly -WifiSsid 'TEST_ONLY' -ServerUrl 'http://localhost:7000' }
catch {
    if ($_.Exception.Message -notlike '*PC LAN base URL*') { throw }
    $rejected = $true
}
if (-not $rejected) { throw 'Localhost was accepted as a device server URL.' }
Assert-Restored
Write-Output 'PASS: localhost rejected before password prompt/build'
