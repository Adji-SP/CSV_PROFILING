# Shared non-executing dotenv reader. Values are literal: no PowerShell or variable expansion.
function Invoke-WithSingleLineEnvironment {
    param([Parameter(Mandatory)][scriptblock]$Action)
    # VS DevShell parses cmd.exe SET output. PEM continuation lines can otherwise
    # be echoed or misparsed. Remove multiline values only during tool discovery.
    $multiline = @{}
    Get-ChildItem Env: | Where-Object { $_.Value.Contains("`n") -or $_.Value.Contains("`r") } | ForEach-Object {
        $multiline[$_.Name] = $_.Value
    }
    try {
        foreach ($name in $multiline.Keys) { [Environment]::SetEnvironmentVariable($name, $null, 'Process') }
        & $Action
    } finally {
        foreach ($name in $multiline.Keys) { [Environment]::SetEnvironmentVariable($name, $multiline[$name], 'Process') }
    }
}

function Assert-DeviceMqttUrl {
    param([Parameter(Mandatory)][string]$Value)
    $uri = $null
    if (-not [Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$uri) -or
        $uri.Scheme -notin @('mqtts', 'wss') -or -not $uri.Host -or
        $uri.IsLoopback -or $uri.Host -in @('0.0.0.0', '[::]', '::') -or
        $uri.UserInfo -or $uri.Query -or $uri.Fragment -or
        ($uri.Scheme -eq 'mqtts' -and $uri.AbsolutePath -ne '/')) {
        throw 'DEVICE_MQTT_URL must be mqtts://broker:8883 or wss://broker/mqtt, without credentials, query, or fragment; never localhost.'
    }
}

function Read-ProjectEnv {
    param([Parameter(Mandatory)][string] $Path)
    $values = @{}
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $values }
    $lines = [IO.File]::ReadAllLines((Resolve-Path -LiteralPath $Path).Path)
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $line = $lines[$i].Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        if ($line -notmatch '^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') {
            throw "Invalid environment setting on line $($i + 1). Expected NAME=value."
        }
        $name = $Matches[1]
        $value = $Matches[2]
        if ($value.StartsWith('"') -or $value.StartsWith("'")) {
            $double = $value.StartsWith('"')
            $pattern = if ($double) { '(?s)^"((?:\\.|[^"\\])*)"\s*(?:#.*)?$' } else { "(?s)^'([^']*)'\s*(?:#.*)?$" }
            while ($value -notmatch $pattern) {
                $i++
                if ($i -ge $lines.Length) { throw "Unclosed quoted value for $name. No values were displayed." }
                $value += "`n" + $lines[$i]
            }
            $value = $Matches[1]
            if ($double) {
                $value = [regex]::Replace($value, '\\([nr"\\])', {
                    param($match)
                    switch ($match.Groups[1].Value) { 'n' { "`n" } 'r' { "`r" } default { $match.Groups[1].Value } }
                })
            }
        } else {
            $value = ($value -replace '\s+#.*$', '').Trim()
            if ($value -match '\s') { throw "Quote the value for $name because it contains spaces." }
        }
        $values[$name] = $value
    }
    return $values
}

function Import-FirmwareEnvironment {
    param([Parameter(Mandatory)][string] $Path, [Parameter(Mandatory)][string] $Root)
    $values = Read-ProjectEnv $Path
    # Explicit CLI arguments are applied later. File settings override stale caller values.
    foreach ($name in @('WIFI_SSID', 'WIFI_PASS', 'DEVICE_NAME', 'DEVICE_ID', 'FIRMWARE_VERSION',
            'OTA_SERVER_URL', 'OTA_PUBLIC_BASE_URL', 'DEVICE_OTA_TOKEN', 'OTA_PYTHON_PATH', 'OTA_CARGO_TARGET_DIR',
            'OTA_ESP_IDF_TOOLS_DIR', 'DEVICE_MQTT_URL', 'DEVICE_MQTT_USERNAME', 'DEVICE_MQTT_PASSWORD',
            'DEVICE_MQTT_CA_PEM', 'DEVICE_MQTT_CERT_PEM', 'DEVICE_MQTT_KEY_PEM',
            'DEVICE_MQTT_CA_FILE', 'DEVICE_MQTT_CERT_FILE', 'DEVICE_MQTT_KEY_FILE')) {
        if ($values.ContainsKey($name)) { [Environment]::SetEnvironmentVariable($name, $values[$name], 'Process') }
    }
    foreach ($kind in @('CA', 'CERT', 'KEY')) {
        $fileName = "DEVICE_MQTT_${kind}_FILE"
        $pemName = "DEVICE_MQTT_${kind}_PEM"
        $file = [Environment]::GetEnvironmentVariable($fileName, 'Process')
        if ([string]::IsNullOrWhiteSpace($file)) { continue }
        if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($pemName, 'Process'))) {
            throw "Set either $fileName or $pemName, not both."
        }
        if (-not [IO.Path]::IsPathRooted($file)) { $file = Join-Path $Root $file }
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "$fileName points to a missing file." }
        $pem = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $file).Path)
        if ($pem.Length -gt 65536 -or $pem -notmatch '-----BEGIN (?:CERTIFICATE|(?:RSA |EC )?PRIVATE KEY)-----') {
            throw "$fileName must contain a PEM certificate/key, at most 64 KiB."
        }
        [Environment]::SetEnvironmentVariable($pemName, $pem, 'Process')
    }
}
