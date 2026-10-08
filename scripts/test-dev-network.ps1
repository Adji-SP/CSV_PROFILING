$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'dev-network.ps1')
$assigned = @('127.0.0.1', '192.168.1.23', '10.0.0.23')
if ((Resolve-DevLanAddress -AssignedAddresses $assigned -GatewayAddresses @('192.168.1.23')) -ne '192.168.1.23') { throw 'Automatic selection failed.' }
if ((Resolve-DevLanAddress -RequestedAddress '10.0.0.23' -AssignedAddresses $assigned -GatewayAddresses $assigned) -ne '10.0.0.23') { throw 'Explicit selection failed.' }
foreach ($request in @('127.0.0.1', '192.168.1.99', 'example.com')) {
    $rejected = $false
    try { Resolve-DevLanAddress -RequestedAddress $request -AssignedAddresses $assigned | Out-Null }
    catch { $rejected = $true }
    if (-not $rejected) { throw 'Unsafe or unassigned address accepted.' }
}
foreach ($gateways in @(@(), @('192.168.1.23', '10.0.0.23'))) {
    $rejected = $false
    try { Resolve-DevLanAddress -AssignedAddresses $assigned -GatewayAddresses $gateways | Out-Null }
    catch { $rejected = $true }
    if (-not $rejected) { throw 'Ambiguous or unavailable network accepted.' }
}
Write-Host 'PASS LAN URL selection, explicit override, and invalid/ambiguous network checks.'
