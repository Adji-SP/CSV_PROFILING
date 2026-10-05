param(
    [switch]$LocalOnly,
    [switch]$TunnelOnly,
    [ValidateRange(1, 65535)][int]$ApiPort = 18080,
    [ValidateRange(1, 65535)][int]$ConnectorMetricsPort = 20241,
    [string]$PublicFrontendUrl = 'https://dev.iot-prototype.net',
    [string]$PublicApiUrl = 'https://api-dev.iot-prototype.net'
)

$failed = 0
function Check-Url([string]$Label, [string]$Url, [string]$ExpectedText) {
    try {
        $response = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 8
        if ($response.StatusCode -ne 200 -or $response.Content -notlike "*$ExpectedText*") {
            throw "unexpected response ($($response.StatusCode))"
        }
        Write-Host "PASS $Label - $Url" -ForegroundColor Green
    } catch {
        Write-Host "FAIL $Label - $Url ($($_.Exception.Message))" -ForegroundColor Red
        $script:failed++
    }
}

if ($LocalOnly -and $TunnelOnly) { throw 'Use either -LocalOnly or -TunnelOnly, not both.' }
if (-not $TunnelOnly) {
    Check-Url 'Frontend local' 'http://localhost:5173/' 'CSV'
    Check-Url 'Browser API config local' 'http://localhost:5173/app-config.js' "http://localhost:$ApiPort"
    Check-Url 'Rust API local' "http://localhost:$ApiPort/health" 'csv-profiling-api'
}

if (-not $LocalOnly -and $PublicFrontendUrl -and $PublicApiUrl) {
    $frontend = [Uri]$PublicFrontendUrl
    $api = [Uri]$PublicApiUrl
    if ($frontend.Scheme -ne 'https' -or $api.Scheme -ne 'https') {
        throw 'PublicFrontendUrl and PublicApiUrl must be HTTPS origins.'
    }
    if (-not $TunnelOnly) {
        Check-Url 'Frontend public' "$($frontend.GetLeftPart([UriPartial]::Authority))/" 'CSV'
        Check-Url 'Browser API config public' "$($frontend.GetLeftPart([UriPartial]::Authority))/app-config.js" $api.GetLeftPart([UriPartial]::Authority)
        Check-Url 'Rust API public' "$($api.GetLeftPart([UriPartial]::Authority))/health" 'csv-profiling-api'
        Write-Host 'NOTE: Cloudflare Access-protected URLs require an authenticated browser. A login page does not prove the backend is healthy.'
    }
    foreach ($hostname in @($frontend.Host, $api.Host)) {
        try {
            Resolve-DnsName -Name $hostname -ErrorAction Stop | Out-Null
            Write-Host "PASS DNS - $hostname" -ForegroundColor Green
        } catch {
            Write-Host "FAIL DNS - $hostname" -ForegroundColor Red
            $failed++
        }
    }
    $connectorService = Get-Service cloudflared -ErrorAction SilentlyContinue
    if ($connectorService -and $connectorService.Status -eq 'Running') {
        Write-Host 'PASS cloudflared Windows service - Running' -ForegroundColor Green
    } elseif (Get-Command cloudflared -ErrorAction SilentlyContinue) {
        Write-Host 'INFO cloudflared executable found; check whether your connector is running.' -ForegroundColor Yellow
    } else {
        Write-Host 'INFO no cloudflared service or executable found in this session.' -ForegroundColor Yellow
    }
    try {
        $metrics = "http://127.0.0.1:$ConnectorMetricsPort"
        $ready = Invoke-WebRequest "$metrics/ready" -UseBasicParsing -TimeoutSec 3
        if ($ready.StatusCode -ne 200) { throw 'connector is not ready' }
        Write-Host 'PASS tunnel connector - connected to Cloudflare' -ForegroundColor Green
        $current = Invoke-RestMethod "$metrics/config" -TimeoutSec 3
        $config = $current.config
        if ($config -is [string]) { $config = $config | ConvertFrom-Json }
        foreach ($expected in @(@{Host=$frontend.Host;Port=5173}, @{Host=$api.Host;Port=$ApiPort})) {
            $route = @($config.ingress | Where-Object { $_.hostname -eq $expected.Host -and -not $_.path })
            $matched = $false
            foreach ($item in $route) {
                $origin = $null
                if ([Uri]::TryCreate($item.service, [UriKind]::Absolute, [ref]$origin) -and
                    $origin.Scheme -eq 'http' -and $origin.Host -in @('localhost','127.0.0.1') -and
                    $origin.Port -eq $expected.Port) { $matched = $true }
            }
            if ($matched) {
                Write-Host "PASS route - $($expected.Host) -> loopback:$($expected.Port)" -ForegroundColor Green
            } else {
                Write-Host "FAIL route - add $($expected.Host) -> http://127.0.0.1:$($expected.Port) in your existing tunnel's Published application routes" -ForegroundColor Red
                $script:failed++
            }
        }
    } catch {
        Write-Host "FAIL connector diagnostics - $($_.Exception.Message). Use -ConnectorMetricsPort if cloudflared chose a different port." -ForegroundColor Red
        $script:failed++
    }
} elseif (-not $LocalOnly) {
    Write-Host 'Public checks skipped: pass -PublicFrontendUrl and -PublicApiUrl after configuring a valid domain.' -ForegroundColor Yellow
}

if ($failed) { exit 1 }
