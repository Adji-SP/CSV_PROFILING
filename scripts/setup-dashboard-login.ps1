param(
    [string]$Username = 'admin',
    [switch]$ResetPassword,
    [switch]$ResetTotp,
    [string]$DeviceId,
    [string]$PublicUrl,
    [string]$PythonExe
)
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $PythonExe) { $PythonExe = Join-Path $projectRoot 'python\.venv\Scripts\python.exe' }
if (-not (Test-Path -LiteralPath $PythonExe -PathType Leaf)) { throw 'Python environment is missing.' }
& $PythonExe -m pip install -r (Join-Path $projectRoot 'python\gateway\requirements.txt')
if ($LASTEXITCODE -ne 0) { throw 'Could not install login dependencies.' }
$arguments = @('-m', 'gateway.provision', '--username', $Username)
if ($ResetPassword) { $arguments += '--reset-password' }
if ($ResetTotp) { $arguments += '--reset-totp' }
if ($DeviceId) { $arguments += @('--device-id', $DeviceId) }
if ($PublicUrl) { $arguments += @('--public-url', $PublicUrl) }
Push-Location (Join-Path $projectRoot 'python')
try {
    & $PythonExe @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Could not provision dashboard login.' }
} finally { Pop-Location }
