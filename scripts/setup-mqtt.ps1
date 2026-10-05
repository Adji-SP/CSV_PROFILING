#requires -Version 5.1
<#
.SYNOPSIS
Prepare authenticated MQTT/TLS for local development. Does not change firewall or flash a board.
.EXAMPLE
.\scripts\setup-mqtt.ps1 -BrokerAddress 192.168.1.5
.EXAMPLE
.\scripts\setup-mqtt.ps1 -BrokerAddress mqtt.home.arpa -RenewCertificate -StartBroker
.NOTES
Prefer a router DNS name/DHCP reservation for BrokerAddress. Existing accounts and CA are retained.
New accounts prompt interactively via mosquitto_passwd: passwords never appear in process arguments.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $BrokerAddress,
    [string] $DeviceId = 'ESP32-S3-001',
    [string] $DataDir = (Join-Path $env:LOCALAPPDATA 'csv-mqtt'),
    [string] $MosquittoDir,
    [ValidateRange(1,65535)][int] $Port = 8883,
    [switch] $RenewCertificate,
    [switch] $StartBroker,
    [switch] $CheckOnly
)
$ErrorActionPreference = 'Stop'
if ($DeviceId -notmatch '^[A-Za-z0-9_-]{1,128}$') { throw 'DeviceId must contain only letters, numbers, hyphens or underscores.' }
if ($DeviceId -eq 'console-backend') { throw 'Use a separate device identity, not console-backend.' }
if ($BrokerAddress -notmatch '^[A-Za-z0-9][A-Za-z0-9.-]*$' -or
    $BrokerAddress -in @('localhost', '127.0.0.1', '0.0.0.0')) {
    throw 'BrokerAddress must be a LAN IPv4 address or DNS hostname reachable by the ESP32, without scheme/port.'
}
if (-not $MosquittoDir) {
    foreach ($candidate in @('F:\Mosquito', 'F:\Mosquitto', (Join-Path $env:ProgramFiles 'mosquitto'))) {
        if (Test-Path -LiteralPath (Join-Path $candidate 'mosquitto.exe')) { $MosquittoDir = $candidate; break }
    }
}
if (-not $MosquittoDir) { throw 'Mosquitto was not found. Supply -MosquittoDir pointing to its installation folder.' }
$broker = Join-Path $MosquittoDir 'mosquitto.exe'
$passwd = Join-Path $MosquittoDir 'mosquitto_passwd.exe'
foreach ($executable in @($broker, $passwd)) {
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'Mosquitto installation is incomplete.' }
}
$openssl = (Get-Command openssl.exe -ErrorAction Stop).Source
$DataDir = [IO.Path]::GetFullPath($DataDir).TrimEnd('\', '/')
if ($DataDir.Length -lt 5 -or $DataDir -eq [IO.Path]::GetPathRoot($DataDir).TrimEnd('\')) { throw 'Use a dedicated MQTT data folder, not a drive root.' }
$config = Join-Path $DataDir 'mosquitto.dashboard.conf'
Write-Host "MQTT data folder: $DataDir"
Write-Host "Backend broker: localhost:$Port"
Write-Host "Device broker: mqtts://${BrokerAddress}:$Port"
if ($CheckOnly) {
    Write-Host 'Prerequisites found. No certificates, accounts or listeners changed.'
    return
}
New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
# The private keys, hashed password file and backups must not be accessible to other ordinary users.
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetAccessRuleProtection($true, $false)
foreach ($sid in @($identity, (New-Object Security.Principal.SecurityIdentifier 'S-1-5-18'),
        (New-Object Security.Principal.SecurityIdentifier 'S-1-5-32-544'))) {
    $rule = New-Object Security.AccessControl.FileSystemAccessRule($sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
}
Set-Acl -LiteralPath $DataDir -AclObject $acl
$utf8 = New-Object Text.UTF8Encoding($false)
$backup = Join-Path $DataDir ('backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0,8))
New-Item -ItemType Directory -Path $backup | Out-Null
foreach ($name in @('ca.crt','ca.key','server.crt','server.key','passwords','acl','mosquitto.conf','mosquitto.dashboard.conf')) {
    $file = Join-Path $DataDir $name
    if (Test-Path -LiteralPath $file -PathType Leaf) { Copy-Item -LiteralPath $file -Destination (Join-Path $backup $name) }
}
function Invoke-OpenSsl([string[]] $Arguments) {
    & $openssl @Arguments 2>&1 | ForEach-Object { Write-Verbose ([string] $_) }
    if ($LASTEXITCODE -ne 0) { throw 'OpenSSL failed. Existing files are backed up; no credentials were displayed.' }
}
$ca = Join-Path $DataDir 'ca.crt'
$caKey = Join-Path $DataDir 'ca.key'
$caExists = Test-Path -LiteralPath $ca -PathType Leaf
$keyExists = Test-Path -LiteralPath $caKey -PathType Leaf
if ($caExists -ne $keyExists) { throw 'Existing CA is incomplete. Restore its matching ca.crt/ca.key; refusing to replace device trust silently.' }
if (-not $caExists) {
    Invoke-OpenSsl @('req','-x509','-newkey','rsa:2048','-nodes','-sha256','-days','3650',
        '-subj','/CN=CSV Dashboard Local MQTT CA','-addext','basicConstraints=critical,CA:TRUE',
        '-addext','keyUsage=critical,keyCertSign,cRLSign','-keyout',$caKey,'-out',$ca)
}
$serverCert = Join-Path $DataDir 'server.crt'
$serverKey = Join-Path $DataDir 'server.key'
if ($RenewCertificate -or -not (Test-Path -LiteralPath $serverCert) -or -not (Test-Path -LiteralPath $serverKey)) {
    $stage = Join-Path $DataDir ('certificate-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    $address = $null
    $san = if ([Net.IPAddress]::TryParse($BrokerAddress, [ref] $address)) { "IP:$BrokerAddress" } else { "DNS:$BrokerAddress" }
    $ext = Join-Path $stage 'server.ext'
    [IO.File]::WriteAllText($ext, "subjectAltName=DNS:localhost,IP:127.0.0.1,$san`nextendedKeyUsage=serverAuth`nbasicConstraints=critical,CA:FALSE`nkeyUsage=critical,digitalSignature,keyEncipherment`n", $utf8)
    $csr = Join-Path $stage 'server.csr'
    $newKey = Join-Path $stage 'server.key'
    $newCert = Join-Path $stage 'server.crt'
    Invoke-OpenSsl @('req','-newkey','rsa:2048','-nodes','-sha256','-subj',"/CN=$BrokerAddress",'-keyout',$newKey,'-out',$csr)
    $serial = '0x' + [guid]::NewGuid().ToString('N')
    Invoke-OpenSsl @('x509','-req','-in',$csr,'-CA',$ca,'-CAkey',$caKey,'-set_serial',$serial,'-out',$newCert,'-days','365','-sha256','-extfile',$ext)
    Invoke-OpenSsl @('verify','-CAfile',$ca,'-verify_hostname','localhost',$newCert)
    Copy-Item -LiteralPath $newKey -Destination $serverKey -Force
    Copy-Item -LiteralPath $newCert -Destination $serverCert -Force
    Write-Host 'Server certificate prepared with localhost and device-address SANs; original CA retained.'
} else {
    Write-Host 'Keeping existing server certificate. Use -RenewCertificate if its address/expiry is wrong.'
}
$passwordFile = Join-Path $DataDir 'passwords'
foreach ($user in @('console-backend', $DeviceId)) {
    $exists = (Test-Path -LiteralPath $passwordFile) -and
        (@(Get-Content -LiteralPath $passwordFile | Where-Object { $_.StartsWith($user + ':') }).Count -gt 0)
    if (-not $exists) {
        Write-Host "Create a password for MQTT account $user (save it in your ignored .env)."
        if (Test-Path -LiteralPath $passwordFile) { & $passwd $passwordFile $user }
        else { & $passwd '-c' $passwordFile $user }
        if ($LASTEXITCODE -ne 0) { throw 'MQTT account creation failed.' }
    } else { Write-Host "Keeping existing MQTT account: $user" }
}
$aclFile = Join-Path $DataDir 'acl'
$aclText = if (Test-Path -LiteralPath $aclFile) { [IO.File]::ReadAllText($aclFile) } else { '' }
foreach ($user in @('console-backend', $DeviceId)) {
    if ($aclText -notmatch ('(?m)^\s*user\s+' + [regex]::Escape($user) + '\s*$')) {
        $topics = if ($user -eq 'console-backend') { "topic read devices/+/logs`ntopic read tinyml/v1/#" }
            else { "topic write devices/$DeviceId/logs`ntopic write tinyml/v1/$DeviceId/#" }
        $aclText += "`nuser $user`n$topics`n"
    }
}
[IO.File]::WriteAllText($aclFile, $aclText, $utf8)
$configDir = $DataDir.Replace('\','/')
# A dedicated config leaves the user's original mosquitto.conf untouched.
$configText = @"
# Generated by scripts/setup-mqtt.ps1. TLS and authenticated users only.
listener $Port 0.0.0.0
allow_anonymous false
password_file $configDir/passwords
acl_file $configDir/acl
certfile $configDir/server.crt
keyfile $configDir/server.key
tls_version tlsv1.2
log_dest stdout
"@
[IO.File]::WriteAllText($config, $configText + "`n", $utf8)
Write-Host "Prepared $config"
Write-Host "Backup retained at $backup. No original configuration or accounts were deleted."
Write-Host 'Run scripts/check-mqtt.ps1 after starting the broker and updating .env.'
Write-Host 'Restrict TCP 8883 to your trusted LAN in Windows Firewall; no firewall rule was added.'
if ($StartBroker) {
    $probe = New-Object Net.Sockets.TcpClient
    try { $occupied = $probe.ConnectAsync('127.0.0.1', $Port).Wait(1000) -and $probe.Connected } catch { $occupied = $false } finally { $probe.Dispose() }
    if ($occupied) { throw "Port $Port is already in use. Stop that broker intentionally before restarting; no process was killed." }
    $logDir = Join-Path $DataDir 'logs'
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $process = Start-Process -FilePath $broker -ArgumentList @('-c', ('"' + $config + '"')) -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $logDir "$stamp.out.log") -RedirectStandardError (Join-Path $logDir "$stamp.err.log")
    Write-Host "Broker process started (PID $($process.Id)); logs: $logDir"
}
