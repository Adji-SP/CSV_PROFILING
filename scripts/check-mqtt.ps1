#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $EnvFile = (Join-Path $PSScriptRoot '..\.env'),
    [string] $PythonPath,
    [switch] $ProbeDevice,
    [switch] $ProbeWebSocket,
    [string] $MosquittoPub = 'F:\Mosquito\mosquitto_pub.exe',
    [ValidateRange(1,65535)][int] $WebSocketPort = 9001
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env-file.ps1')
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$values = Read-ProjectEnv $EnvFile
$config = @{}
foreach ($name in @('MQTT_HOST','MQTT_PORT','MQTT_USERNAME','MQTT_PASSWORD','MQTT_CA_FILE',
        'MQTT_CERT_FILE','MQTT_KEY_FILE','DEVICE_ID','DEVICE_MQTT_URL','DEVICE_MQTT_USERNAME',
        'DEVICE_MQTT_PASSWORD','DEVICE_MQTT_CA_FILE','DEVICE_MQTT_CA_PEM',
        'DEVICE_MQTT_CERT_FILE','DEVICE_MQTT_KEY_FILE')) {
    if ($values.ContainsKey($name)) { $config[$name] = $values[$name] }
    else { $config[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
}
if (-not $PythonPath) {
    if ($values['OTA_PYTHON_PATH']) { $PythonPath = $values['OTA_PYTHON_PATH'] }
    elseif (Test-Path -LiteralPath 'D:\Python\python.exe') { $PythonPath = 'D:\Python\python.exe' }
    else { $PythonPath = (Get-Command python.exe -ErrorAction Stop).Source }
}
# Secrets travel through stdin, not command arguments or a diagnostic file.
$payload = @{ root = $root; env = $config; probe_device = [bool] $ProbeDevice; probe_websocket = [bool] $ProbeWebSocket; mosquitto_pub = $MosquittoPub; websocket_port = $WebSocketPort }
$payload | ConvertTo-Json -Depth 4 -Compress | & $PythonPath (Join-Path $PSScriptRoot 'check-mqtt.py')
if ($LASTEXITCODE -ne 0) { throw 'MQTT check failed. See the setting-specific message above; no credentials were printed.' }
