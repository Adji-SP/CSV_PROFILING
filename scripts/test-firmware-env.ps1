$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env-file.ps1')
foreach ($url in @('mqtts://192.168.1.5:8883', 'mqtts://broker.example:8883', 'wss://mqtt.example/mqtt', 'wss://mqtt.example:443/')) {
    Assert-DeviceMqttUrl $url
}
foreach ($url in @('mqtt://broker.example:1883', 'ws://broker.example/mqtt', 'mqtts://localhost:8883',
    'mqtts://0.0.0.0:8883', 'wss://127.0.0.1/mqtt', 'wss://user:password@broker.example/mqtt',
    'wss://broker.example/mqtt?token=secret', 'mqtts://broker.example:8883/invalid')) {
    $rejected = $false
    try { Assert-DeviceMqttUrl $url } catch { $rejected = $true }
    if (-not $rejected) { throw 'Invalid device MQTT URL accepted.' }
}
Write-Host 'PASS MQTT firmware URL validation: TLS, WSS, and unsafe endpoints.'
$saved = $env:CSV_TEST_MULTILINE_SENTINEL
try {
    $env:CSV_TEST_MULTILINE_SENTINEL = "first`nsecond"
    Invoke-WithSingleLineEnvironment {
        if ($env:CSV_TEST_MULTILINE_SENTINEL) { throw 'Multiline value leaked into discovery.' }
    }
    if ($env:CSV_TEST_MULTILINE_SENTINEL -ne "first`nsecond") { throw 'Value not restored.' }
    try { Invoke-WithSingleLineEnvironment { throw 'simulated discovery failure' } } catch {}
    if ($env:CSV_TEST_MULTILINE_SENTINEL -ne "first`nsecond") { throw 'Value not restored after failure.' }
} finally { $env:CSV_TEST_MULTILINE_SENTINEL = $saved }
Write-Host 'PASS multiline environment isolation and restoration.'
