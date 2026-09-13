#requires -Version 5.1
<#
.SYNOPSIS
Build the managed ESP32-S3 firmware and optionally flash it over USB.
.EXAMPLE
.\flash-esp32.ps1
.EXAMPLE
.\flash-esp32.ps1 -BuildOnly
.EXAMPLE
.\flash-esp32.ps1 -CheckOnly
.NOTES
Requires ESP Rust, Visual Studio C++ Build Tools, base Python, ldproxy and espflash.
Passwords are prompted securely, but necessarily embedded in the compiled firmware.
No tools are installed automatically. No flash operation runs without confirmation.
#>
[CmdletBinding()]
param(
    [switch] $BuildOnly,
    [switch] $CheckOnly,
    [string] $WifiSsid,
    [Security.SecureString] $WifiPassword,
    [string] $ServerUrl,
    [string] $DeviceName = 'My ESP32-S3',
    [string] $Version = '1.0.0',
    [string] $Port,
    [string] $TargetDir,
    [string] $ToolsDir,
    [string] $PythonPath
)

$ErrorActionPreference = 'Stop'
$projectDir = Join-Path $PSScriptRoot 'examples\esp32-rust-ota-client'
$savedEnvironment = @{}
Get-ChildItem Env: | ForEach-Object { $savedEnvironment[$_.Name] = $_.Value }
$locationPushed = $false

function Invoke-Tool {
    param([string] $Executable, [string[]] $Arguments)
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable failed with exit code $LASTEXITCODE. Stopping; no further flashing will be attempted."
    }
}

try {
    if ($env:OS -ne 'Windows_NT') { throw 'This helper is for Windows PowerShell.' }
    Push-Location -LiteralPath $projectDir
    $locationPushed = $true

    # Reuse this PC's existing cache where available; never move/delete a cache.
    $cacheDrive = if (Test-Path -LiteralPath 'F:\') { 'F:\' } else { [IO.Path]::GetPathRoot($projectDir) }
    if (-not $TargetDir) { $TargetDir = Join-Path $cacheDrive 'csv-esp' }
    if (-not $ToolsDir) { $ToolsDir = Join-Path $cacheDrive 'csv-idf' }
    foreach ($path in @($TargetDir, $ToolsDir)) {
        if ($path -notmatch '^[A-Za-z]:[\\/]' -or $path -match '\s') {
            throw 'TargetDir and ToolsDir must be absolute local paths without spaces, e.g. F:\csv-esp.'
        }
    }
    $TargetDir = [IO.Path]::GetFullPath($TargetDir).TrimEnd('\')
    $ToolsDir = [IO.Path]::GetFullPath($ToolsDir).TrimEnd('\')
    if (Test-Path -LiteralPath $TargetDir) { $TargetDir = (Resolve-Path -LiteralPath $TargetDir).Path }
    if ($TargetDir.Length -gt 14) { throw 'TargetDir is too long for the ESP-IDF Windows build. Use a short path such as F:\csv-esp.' }
    if ($TargetDir.Length -lt 4 -or $ToolsDir.Length -lt 4 -or $TargetDir -eq $ToolsDir) {
        throw 'Use distinct dedicated cache folders, not a drive root.'
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Install Visual Studio 2022 Build Tools with the C++ workload and Windows SDK first.' }
    $vsPath = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vsPath) { throw 'No complete Visual Studio C++ toolchain found.' }
    $env:PATH = (Split-Path $vswhere) + ';' + $env:PATH
    & (Join-Path $vsPath 'Common7\Tools\Launch-VsDevShell.ps1') -Arch amd64 -HostArch amd64 -SkipAutomaticLocation

    $espExport = Join-Path $env:USERPROFILE 'export-esp.ps1'
    if (-not (Test-Path -LiteralPath $espExport)) { throw 'ESP Rust environment missing. Install it with espup first (export-esp.ps1 is required).' }
    . $espExport
    $env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH
    if (-not $PythonPath) {
        if (Test-Path -LiteralPath 'D:\Python\python.exe') { $PythonPath = 'D:\Python\python.exe' }
        else { $PythonPath = (Get-Command python.exe -ErrorAction Stop).Source }
    }
    $PythonPath = (Resolve-Path -LiteralPath $PythonPath).Path
    if ($PythonPath -match 'WindowsApps') { throw 'Specify -PythonPath pointing to a real base Python installation, not the Windows Store shortcut.' }
    Invoke-Tool $PythonPath @('-c', 'import sys; sys.exit(sys.prefix != sys.base_prefix)')
    $env:PATH = (Split-Path $PythonPath) + ';' + $env:PATH
    Remove-Item Env:VIRTUAL_ENV -ErrorAction SilentlyContinue
    $env:CARGO_TARGET_DIR = $TargetDir
    $env:CARGO_WORKSPACE_DIR = $projectDir
    $env:ESP_IDF_TOOLS_INSTALL_DIR = "custom:$ToolsDir"
    foreach ($tool in @('cargo', 'rustc', 'ldproxy', 'espflash', 'link.exe', 'git')) {
        Get-Command $tool -ErrorAction Stop | Out-Null
    }
    Invoke-Tool cargo @('+esp', '--version')
    Invoke-Tool rustc @('+esp', '--version')
    Invoke-Tool espflash @('--version')
    Invoke-Tool $PythonPath @('--version')
    Write-Host "Build folder: $TargetDir"
    Write-Host "ESP-IDF cache: $ToolsDir"
    if ($CheckOnly) {
        Write-Host 'Prerequisite checks passed. No build or device operation was performed.'
        return
    }

    Write-Host 'This example requires an ESP32-S3 with at least 4 MiB flash.'
    Write-Host 'Network credentials are embedded in the firmware. Do not share that firmware publicly.'
    if (-not $WifiSsid) { $WifiSsid = Read-Host 'Wi-Fi name (SSID)' }
    if ([string]::IsNullOrWhiteSpace($WifiSsid) -or [Text.Encoding]::UTF8.GetByteCount($WifiSsid) -gt 32) {
        throw 'Wi-Fi name must contain 1-32 UTF-8 bytes.'
    }
    if (-not $ServerUrl) { $ServerUrl = Read-Host 'OTA server LAN URL (for example http://192.168.1.5:7000)' }
    $server = $null
    if (-not [Uri]::TryCreate($ServerUrl, [UriKind]::Absolute, [ref] $server) -or
        $server.Scheme -notin @('http', 'https') -or $server.IsLoopback -or
        $server.Host -in @('0.0.0.0', '[::]', '::') -or $server.UserInfo -or
        $server.Query -or $server.Fragment -or $server.AbsolutePath -ne '/') {
        throw 'Use the PC LAN base URL, with no credentials, path, query, or fragment; not localhost or 0.0.0.0.'
    }
    if (-not $WifiPassword) { $WifiPassword = Read-Host 'Wi-Fi password' -AsSecureString }
    $passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($WifiPassword)
    try { $env:WIFI_PASS = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer) }
    if ([Text.Encoding]::UTF8.GetByteCount([string] $env:WIFI_PASS) -gt 64) { throw 'Wi-Fi password exceeds 64 bytes.' }
    $env:WIFI_SSID = $WifiSsid
    $env:OTA_SERVER_URL = $ServerUrl.TrimEnd('/')
    $env:DEVICE_NAME = $DeviceName
    $env:FIRMWARE_VERSION = $Version

    Write-Host 'Building firmware. Compiler errors will remain visible.'
    Invoke-Tool cargo @('+esp', 'build', '--release', '--target', 'xtensa-esp32s3-espidf', '--locked')
    $releaseDir = Join-Path $TargetDir 'xtensa-esp32s3-espidf\release'
    $elf = Join-Path $releaseDir 'esp32-rust-ota-client'
    $bootloader = Join-Path $releaseDir 'bootloader.bin'
    $partitionTable = Join-Path $projectDir 'partitions.csv'
    foreach ($file in @($elf, $bootloader, $partitionTable)) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Required output missing: $file" }
    }
    $image = Join-Path $releaseDir 'firmware-ota.bin'
    Invoke-Tool espflash @('save-image', '-S', '--chip', 'esp32s3', '--format', 'esp-idf', '--flash-size', '4mb',
        '--partition-table', $partitionTable, '--target-app-partition', 'ota_0', $elf, $image)
    $imageSize = (Get-Item -LiteralPath $image).Length
    if ($imageSize -le 0 -or $imageSize -gt 0x1e0000) { throw 'Application image does not fit the example OTA slot.' }
    Write-Host "OTA application image: $image ($imageSize bytes)"
    Write-Host ('SHA-256: ' + (Get-FileHash -LiteralPath $image -Algorithm SHA256).Hash)
    if ($BuildOnly) { Write-Host 'Build complete. Nothing was flashed.'; return }

    Invoke-Tool espflash @('-S', 'list-ports')
    if (-not $Port) { $Port = Read-Host 'ESP32 serial port (for example COM5)' }
    if ($Port -notmatch '^COM[1-9][0-9]*$') { throw 'Specify a Windows COM port, such as COM5.' }
    Write-Host "WARNING: Flashing $Port replaces its bootloader, partition table and application."
    Write-Host 'Back up important firmware/data first. Close other serial monitors.'
    Write-Host 'Confirm this is your ESP32-S3, with at least 4 MiB flash, and that this partition layout is appropriate.'
    $confirmation = Read-Host "Type FLASH $Port to proceed, or press Enter to cancel"
    if ($confirmation -cne "FLASH $Port") { Write-Host 'Cancelled. Firmware was built, but nothing was flashed.'; return }
    Invoke-Tool espflash @('flash', '-S', '--chip', 'esp32s3', '--port', $Port, '--monitor',
        '--bootloader', $bootloader, '--partition-table', $partitionTable,
        '--target-app-partition', 'ota_0', $elf)
}
finally {
    # No credentials or developer-shell environment are left in the caller's shell.
    foreach ($entry in @(Get-ChildItem Env:)) {
        if (-not $savedEnvironment.ContainsKey($entry.Name)) {
            [Environment]::SetEnvironmentVariable($entry.Name, $null, 'Process')
        }
    }
    foreach ($key in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($key, $savedEnvironment[$key], 'Process')
    }
    if ($locationPushed) { Pop-Location }
}
