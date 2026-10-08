# Select an assigned LAN address; never guess between multiple active networks.
function Resolve-DevLanAddress {
    param([string]$RequestedAddress, [string[]]$AssignedAddresses, [string[]]$GatewayAddresses)
    if ($RequestedAddress) {
        $parsed = $null
        if (-not [Net.IPAddress]::TryParse($RequestedAddress, [ref]$parsed) -or
            $parsed.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork -or
            [Net.IPAddress]::IsLoopback($parsed) -or
            $RequestedAddress -notin $AssignedAddresses) {
            throw 'LanAddress must be an IPv4 address currently assigned to this PC, not localhost.'
        }
        return $RequestedAddress
    }
    $candidates = @($GatewayAddresses | Where-Object { $_ -and $_ -notlike '127.*' -and $_ -notlike '169.254.*' } | Select-Object -Unique)
    if ($candidates.Count -ne 1) {
        throw 'Cannot choose one LAN network. Pass -LanAddress with the PC address reachable by your ESP32.'
    }
    return $candidates[0]
}

function Get-DevLanAddress([string]$RequestedAddress) {
    $assigned = @(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction Stop | Select-Object -ExpandProperty IPAddress)
    $gateway = @(Get-NetIPConfiguration -ErrorAction Stop | Where-Object {
        $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq 'Up'
    } | ForEach-Object { $_.IPv4Address.IPAddress })
    return Resolve-DevLanAddress -RequestedAddress $RequestedAddress -AssignedAddresses $assigned -GatewayAddresses $gateway
}
