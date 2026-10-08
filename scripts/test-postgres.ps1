#requires -Version 5.1
param(
    [Parameter(Mandatory)][string]$PostgresBin,
    [ValidateRange(1024,65535)][int]$Port = 55439
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$PostgresBin = (Resolve-Path -LiteralPath $PostgresBin).Path
foreach ($name in @('initdb.exe','pg_ctl.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $PostgresBin $name))) { throw 'Supply a PostgreSQL bin folder containing initdb and pg_ctl.' }
}
$probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Port)
try { $probe.Start() } finally { $probe.Stop() }
$testRoot = Join-Path $root ('data\postgres-tests\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$cluster = Join-Path $testRoot 'cluster'
$passwordFile = Join-Path $testRoot 'password.tmp'
$password = [Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N')
$previousUrl = $env:TEST_DATABASE_URL
$started = $false
try {
    [IO.File]::WriteAllText($passwordFile, $password, [Text.Encoding]::ASCII)
    & (Join-Path $PostgresBin 'initdb.exe') -D $cluster --username=ota_test --auth=scram-sha-256 --encoding=UTF8 --locale=C "--pwfile=$passwordFile"
    if ($LASTEXITCODE -ne 0) { throw 'Disposable PostgreSQL initialization failed.' }
    & (Join-Path $PostgresBin 'pg_ctl.exe') -D $cluster -l (Join-Path $testRoot 'postgres.log') -o "-h 127.0.0.1 -p $Port" -w start
    if ($LASTEXITCODE -ne 0) { throw 'Disposable PostgreSQL startup failed.' }
    $started = $true
    $env:TEST_DATABASE_URL = 'postgresql://ota_test:{0}@127.0.0.1:{1}/postgres?sslmode=disable' -f $password,$Port
    & cargo test --manifest-path (Join-Path $root 'ota-server\Cargo.toml') postgres_ -- --ignored --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'PostgreSQL integration tests failed.' }
} finally {
    $env:TEST_DATABASE_URL = $previousUrl
    if ($started) {
        & (Join-Path $PostgresBin 'pg_ctl.exe') -D $cluster -m fast -w stop
    }
    if (Test-Path -LiteralPath $passwordFile) { Remove-Item -LiteralPath $passwordFile }
    Write-Host "Disposable cluster/logs retained at $testRoot. No Windows service was installed."
}
