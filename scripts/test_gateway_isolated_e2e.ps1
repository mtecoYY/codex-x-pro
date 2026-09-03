param(
    [string]$GatewayScript = $env:CODEX_X_GATEWAY_SCRIPT,
    [string]$UserScriptDir = '',
    [string]$EnableScriptId = '',
    [string]$ExpectedCustomId = 'fc_custom_123',
    [int]$GatewayPort = 18787,
    [int]$MockServerPort = 19090,
    [string]$MockServerJar = ''
)

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($GatewayScript)) {
    $GatewayScript = Join-Path $env:USERPROFILE '.codex-x\personal-gateway\codex_responses_repair_gateway.py'
}
if ([string]::IsNullOrWhiteSpace($MockServerJar)) {
    $MockServerJar = Join-Path $env:USERPROFILE '.codex\tools\gateway-testing\mockserver-netty-5.15.0-shaded.jar'
}
if (-not (Test-Path -LiteralPath $GatewayScript -PathType Leaf)) {
    throw "Gateway script not found: $GatewayScript"
}
if (-not (Test-Path -LiteralPath $MockServerJar -PathType Leaf)) {
    throw "MockServer JAR not found: $MockServerJar"
}
if ($GatewayPort -eq 8787 -or $MockServerPort -eq 8787) {
    throw 'The isolated test must not use the active gateway port 8787'
}

function Test-Listening([int]$Port) {
    return @(
        Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    ).Count -gt 0
}

function Wait-Listening([int]$Port, [int]$Seconds = 20) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        if (Test-Listening $Port) {
            return
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    throw "Port $Port did not start listening"
}

function Stop-Listening([int]$Port) {
    $connections = @(
        Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    )
    foreach ($connection in $connections) {
        $process = Get-Process -Id $connection.OwningProcess -ErrorAction SilentlyContinue
        if ($process) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

function Get-HeaderValues($Request, [string]$Name) {
    $property = $Request.headers.PSObject.Properties |
        Where-Object { $_.Name -ieq $Name } |
        Select-Object -First 1
    if ($null -eq $property) {
        return @()
    }
    return @($property.Value)
}

function Read-SharedText([string]$Path) {
    $stream = [IO.File]::Open(
        $Path,
        [IO.FileMode]::Open,
        [IO.FileAccess]::Read,
        [IO.FileShare]::ReadWrite
    )
    try {
        $reader = [IO.StreamReader]::new($stream)
        try {
            return $reader.ReadToEnd()
        }
        finally {
            $reader.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }
}

$configPath = Join-Path $env:USERPROFILE '.codex\config.toml'
$beforeConnection = Get-NetTCPConnection -LocalPort 8787 -State Listen -ErrorAction SilentlyContinue |
    Select-Object -First 1
$beforeTask = Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' -ErrorAction SilentlyContinue
$beforeTaskState = "$($beforeTask.State)|$($beforeTask.Settings.Enabled)"
$beforeConfigHash = if (Test-Path -LiteralPath $configPath) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $configPath).Hash
} else {
    'MISSING'
}

$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$root = [IO.Path]::GetFullPath(
    (Join-Path $tempBase ("codex-x-gateway-e2e-" + [Guid]::NewGuid().ToString('N')))
)
if (-not $root.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe temporary test path: $root"
}
$emptyScriptDir = Join-Path $root 'scripts'
$effectiveScriptDir = if ([string]::IsNullOrWhiteSpace($UserScriptDir)) {
    $emptyScriptDir
} else {
    [IO.Path]::GetFullPath($UserScriptDir)
}
$mockOutput = Join-Path $root 'mockserver.out.log'
$mockError = Join-Path $root 'mockserver.err.log'
$gatewayOutput = Join-Path $root 'gateway.out.log'
$gatewayError = Join-Path $root 'gateway.err.log'
$stateFile = Join-Path $root 'state.json'
$mockProcess = $null
$gatewayProcess = $null

try {
    foreach ($port in @($GatewayPort, $MockServerPort)) {
        if (Test-Listening $port) {
            throw "Test port $port is already in use"
        }
    }

    New-Item -ItemType Directory -Force -Path $emptyScriptDir | Out-Null

    $mockProcess = Start-Process -FilePath 'java.exe' `
        -ArgumentList @(
            '-Dmockserver.localBoundIP=127.0.0.1',
            '-jar',
            $MockServerJar,
            '-serverPort',
            $MockServerPort,
            '-logLevel',
            'WARN'
        ) `
        -WorkingDirectory $root `
        -WindowStyle Hidden `
        -RedirectStandardOutput $mockOutput `
        -RedirectStandardError $mockError `
        -PassThru
    Wait-Listening $MockServerPort

    $expectation = @{
        httpRequest = @{ method = 'POST'; path = '/v1/responses' }
        httpResponse = @{
            statusCode = 200
            headers = @{ 'Content-Type' = @('application/json') }
            body = '{"id":"synthetic-response","output":[]}'
        }
    } | ConvertTo-Json -Depth 10
    Invoke-RestMethod `
        -Uri "http://127.0.0.1:$MockServerPort/mockserver/expectation" `
        -Method Put `
        -ContentType 'application/json' `
        -Body $expectation | Out-Null

    $gatewayProcess = Start-Process -FilePath 'python.exe' `
        -ArgumentList @(
            $GatewayScript,
            '--listen',
            "127.0.0.1:$GatewayPort",
            '--upstream',
            "http://127.0.0.1:$MockServerPort",
            '--state-file',
            $stateFile,
            '--script-dir',
            $effectiveScriptDir
        ) `
        -WorkingDirectory $root `
        -WindowStyle Hidden `
        -RedirectStandardOutput $gatewayOutput `
        -RedirectStandardError $gatewayError `
        -PassThru
    Wait-Listening $GatewayPort

    if (-not [string]::IsNullOrWhiteSpace($EnableScriptId)) {
        $testResult = Invoke-RestMethod `
            -Uri "http://127.0.0.1:$GatewayPort/scripts/$EnableScriptId/test" `
            -Method Post `
            -ContentType 'application/json' `
            -Body '{"source":"default"}'
        if ($testResult.status -ne 'passed') {
            throw "Script test failed: $EnableScriptId"
        }
        Invoke-RestMethod `
            -Uri "http://127.0.0.1:$GatewayPort/scripts/$EnableScriptId/enable" `
            -Method Post `
            -ContentType 'application/json' `
            -Body '{}' | Out-Null
    }

    $body = '{"model":"synthetic-model","input":[{"type":"custom_tool_call","id":"fc_custom_123"},{"type":"function_call","id":"fc_function_789"}]}'
    $response = Invoke-WebRequest `
        -UseBasicParsing `
        -Uri "http://127.0.0.1:$GatewayPort/v1/responses" `
        -Method Post `
        -ContentType 'application/json' `
        -Headers @{
            Authorization = 'Bearer synthetic-token'
            Cookie = 'synthetic-cookie'
        } `
        -Body $body

    $query = @{
        httpRequest = @{ method = 'POST'; path = '/v1/responses' }
    } | ConvertTo-Json -Depth 10
    $requests = @(
        Invoke-RestMethod `
            -Uri "http://127.0.0.1:$MockServerPort/mockserver/retrieve?type=REQUESTS" `
            -Method Put `
            -ContentType 'application/json' `
            -Body $query
    )
    if ($requests.Count -ne 1) {
        throw "Expected one recorded upstream request, got $($requests.Count)"
    }

    $request = $requests[0]
    $forwardedBody = [Convert]::FromBase64String([string]$request.body.rawBytes)
    $forwardedJson = [Text.Encoding]::UTF8.GetString($forwardedBody) | ConvertFrom-Json
    $contentLengths = Get-HeaderValues $request 'Content-Length'

    if ($response.StatusCode -ne 200) {
        throw "Expected gateway HTTP 200, got $($response.StatusCode)"
    }
    if ($request.path -ne '/v1/responses') {
        throw "Unexpected upstream path: $($request.path)"
    }
    if ($forwardedJson.model -ne 'synthetic-model') {
        throw 'Provider model was changed unexpectedly'
    }
    if ($forwardedJson.input[0].id -ne $ExpectedCustomId) {
        throw "Unexpected custom tool-call ID: $($forwardedJson.input[0].id)"
    }
    if ($forwardedJson.input[1].id -ne 'fc_function_789') {
        throw 'Normal function-call ID was changed'
    }
    Write-Output "E2E_CONTENT_LENGTH_COUNT=$($contentLengths.Count)"
    Write-Output "E2E_CONTENT_LENGTH_VALUE=$($contentLengths -join ',')"
    Write-Output "E2E_BODY_BYTES=$($forwardedBody.Length)"
    $parsedContentLength = [int]($contentLengths | Select-Object -First 1)
    if ($contentLengths.Count -ne 1 -or
        $parsedContentLength -ne $forwardedBody.Length) {
        throw 'Content-Length is not the single final UTF-8 body length'
    }

    foreach ($path in @($mockOutput, $mockError, $gatewayOutput, $gatewayError)) {
        if (Test-Path -LiteralPath $path) {
            $log = Read-SharedText $path
            if ($log.Contains('synthetic-token') -or $log.Contains('synthetic-cookie')) {
                throw "Sensitive synthetic header leaked into test log: $path"
            }
        }
    }

    Write-Output "E2E_HTTP_STATUS=$($response.StatusCode)"
    Write-Output "E2E_REQUEST_COUNT=$($requests.Count)"
    Write-Output "E2E_PATH=$($request.path)"
    Write-Output "E2E_FINAL_CUSTOM_ID=$($forwardedJson.input[0].id)"
    Write-Output "E2E_FUNCTION_ID=$($forwardedJson.input[1].id)"
    Write-Output 'E2E_SINGLE_CONTENT_LENGTH=True'
    Write-Output 'E2E_LOG_REDACTION=True'
}
finally {
    if ($gatewayProcess) {
        Stop-Process -Id $gatewayProcess.Id -Force -ErrorAction SilentlyContinue
    }
    if ($mockProcess) {
        Stop-Process -Id $mockProcess.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
    Stop-Listening $GatewayPort
    Stop-Listening $MockServerPort
    if (Test-Path -LiteralPath $root) {
        [IO.Directory]::Delete($root, $true)
    }

    $afterConnection = Get-NetTCPConnection -LocalPort 8787 -State Listen -ErrorAction SilentlyContinue |
        Select-Object -First 1
    $afterTask = Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' -ErrorAction SilentlyContinue
    $afterTaskState = "$($afterTask.State)|$($afterTask.Settings.Enabled)"
    $afterConfigHash = if (Test-Path -LiteralPath $configPath) {
        (Get-FileHash -Algorithm SHA256 -LiteralPath $configPath).Hash
    } else {
        'MISSING'
    }
    if (($beforeConnection.OwningProcess -ne $afterConnection.OwningProcess) -or
        ($beforeTaskState -ne $afterTaskState) -or
        ($beforeConfigHash -ne $afterConfigHash)) {
        throw 'Production gateway baseline changed during isolated E2E test'
    }
    Write-Output 'BASELINE_8787_UNCHANGED=True'
    Write-Output 'BASELINE_TASK_UNCHANGED=True'
    Write-Output 'BASELINE_CONFIG_UNCHANGED=True'
}
