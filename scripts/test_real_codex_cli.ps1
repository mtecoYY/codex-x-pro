param(
    [Parameter(Mandatory = $true)][string]$CodexExe,
    [string]$GatewayScript = (Join-Path $env:USERPROFILE '.codex-x\personal-gateway\codex_responses_repair_gateway.py'),
    [string]$MockServerJar = (Join-Path $env:USERPROFILE '.codex\tools\gateway-testing\mockserver-netty-5.15.0-shaded.jar'),
    [int]$GatewayPort = 18787,
    [int]$MockServerPort = 19090
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $CodexExe -PathType Leaf)) { throw "Codex executable not found: $CodexExe" }
if (-not (Test-Path -LiteralPath $GatewayScript -PathType Leaf)) { throw "Gateway script not found: $GatewayScript" }
if (-not (Test-Path -LiteralPath $MockServerJar -PathType Leaf)) { throw "MockServer JAR not found: $MockServerJar" }
if ($GatewayPort -eq 8787 -or $MockServerPort -eq 8787) { throw 'The real CLI test must not use port 8787' }

function Test-Listening([int]$Port) {
    return $null -ne (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1)
}
function Wait-Listening([int]$Port) {
    $deadline = (Get-Date).AddSeconds(30)
    do { if (Test-Listening $Port) { return }; Start-Sleep -Milliseconds 250 } while ((Get-Date) -lt $deadline)
    throw "Port $Port did not start listening"
}
function Stop-Port([int]$Port) {
    foreach ($connection in @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $connection.OwningProcess -Force -ErrorAction SilentlyContinue
    }
}
function Get-TaskFingerprint($Task) {
    if ($null -eq $Task) { return 'MISSING' }
    $action = @($Task.Actions | Select-Object -First 1)
    return "{0}|{1}|{2}" -f [bool]$Task.Settings.Enabled, [string]$action.Execute, [string]$action.Arguments
}

$baselinePid = (Get-NetTCPConnection -LocalPort 8787 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess
$baselineConfig = Join-Path $env:USERPROFILE '.codex\config.toml'
$baselineHash = if (Test-Path $baselineConfig) { (Get-FileHash -Algorithm SHA256 $baselineConfig).Hash } else { 'MISSING' }
$baselineTask = Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' -ErrorAction SilentlyContinue
$baselineTaskState = Get-TaskFingerprint $baselineTask
$root = Join-Path ([IO.Path]::GetTempPath()) ('codex-x-real-cli-' + [Guid]::NewGuid().ToString('N'))
$codexHome = Join-Path $root 'codex-home'
$codexxHome = Join-Path $root 'codexx-home'
$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force -Path $codexHome,$codexxHome,$logs,(Join-Path $root 'scripts') | Out-Null
$config = @(
    'model_provider = "local-test"'
    'model = "test-model"'
    'disable_response_storage = true'
    ''
    '[model_providers.local-test]'
    'name = "Local Test"'
    ('base_url = "http://127.0.0.1:{0}/v1"' -f $GatewayPort)
    'wire_api = "responses"'
    'request_max_retries = 0'
) -join [Environment]::NewLine
[IO.File]::WriteAllText((Join-Path $codexHome 'config.toml'), $config, [Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText((Join-Path $codexHome 'auth.json'), '{"OPENAI_API_KEY":"isolated-real-cli-key"}', [Text.UTF8Encoding]::new($false))
$mock = $null; $gateway = $null; $cli = $null
try {
    foreach ($port in @($GatewayPort,$MockServerPort)) { if (Test-Listening $port) { throw "Test port already in use: $port" } }
    $mock = Start-Process java.exe -ArgumentList @('-Xms64m','-Xmx256m','-Dmockserver.localBoundIP=127.0.0.1','-jar',$MockServerJar,'-serverPort',$MockServerPort,'-logLevel','WARN') -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logs 'mock.out.log') -RedirectStandardError (Join-Path $logs 'mock.err.log') -PassThru
    Wait-Listening $MockServerPort
    $sse = @'
data: {"type":"response.created","response":{"id":"real-cli-synthetic","object":"response","status":"in_progress","model":"test-model","output":[]}}

data: {"type":"response.completed","response":{"id":"real-cli-synthetic","object":"response","status":"completed","model":"test-model","output":[]}}

data: [DONE]

'@
    $expectation = @{ httpRequest = @{ method='POST'; path='/v1/responses' }; httpResponse = @{ statusCode=200; headers=@{'Content-Type'=@('text/event-stream')}; body=$sse } } | ConvertTo-Json -Depth 10
    Invoke-RestMethod -Uri "http://127.0.0.1:$MockServerPort/mockserver/expectation" -Method Put -ContentType 'application/json' -Body $expectation | Out-Null
    $gateway = Start-Process python.exe -ArgumentList @($GatewayScript,'--listen',"127.0.0.1:$GatewayPort",'--upstream',"http://127.0.0.1:$MockServerPort",'--state-file',(Join-Path $root 'state.json'),'--script-dir',(Join-Path $root 'scripts')) -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logs 'gateway.out.log') -RedirectStandardError (Join-Path $logs 'gateway.err.log') -PassThru
    Wait-Listening $GatewayPort
    $env:CODEX_HOME = $codexHome
    $env:CODEXX_HOME = $codexxHome
    $env:CC_SWITCH_HOME = Join-Path $root 'cc-switch-home'
    $cliOut = Join-Path $logs 'codex.out.log'; $cliErr = Join-Path $logs 'codex.err.log'
    $emptyStdin = Join-Path $root 'empty.stdin'; [IO.File]::WriteAllText($emptyStdin, '', [Text.UTF8Encoding]::new($false))
    $prompt = 'Reply with the exact word ROUTED_TEST and do not use tools.'
    $cli = Start-Process -FilePath $CodexExe -ArgumentList @('exec','--ephemeral','--skip-git-repo-check','--ignore-rules','--sandbox','read-only','--json',('"{0}"' -f $prompt)) -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardInput $emptyStdin -RedirectStandardOutput $cliOut -RedirectStandardError $cliErr -PassThru
    $deadline = (Get-Date).AddSeconds(90)
    do { Start-Sleep -Milliseconds 500; $cli.Refresh() } while (-not $cli.HasExited -and (Get-Date) -lt $deadline)
    if (-not $cli.HasExited) { Stop-Process -Id $cli.Id -Force; throw 'Codex CLI timed out after 90 seconds' }
    $cli.WaitForExit()
    $exitCode = $cli.ExitCode
    $cliOutputText = if (Test-Path $cliOut) { Get-Content -Raw $cliOut } else { '' }
    if ($cliOutputText -notmatch '"type":"turn.completed"') { throw 'Codex CLI did not emit turn.completed' }
    $query = @{ httpRequest = @{ method='POST'; path='/v1/responses' } } | ConvertTo-Json -Depth 10
    $requests = @(Invoke-RestMethod -Uri "http://127.0.0.1:$MockServerPort/mockserver/retrieve?type=REQUESTS" -Method Put -ContentType 'application/json' -Body $query)
    Write-Output "CODEX_VERSION=$((& $CodexExe --version) -join ' ')"
    Write-Output "CODEX_EXIT_CODE=$exitCode"
    Write-Output "UPSTREAM_REQUEST_COUNT=$($requests.Count)"
    if ($requests.Count -gt 0) {
        $body = $requests[0].body
        $raw = $null
        if ($body.rawBytes) {
            try { $raw = [Convert]::FromBase64String([string]$body.rawBytes) } catch { $raw = [Text.Encoding]::UTF8.GetBytes([string]$body.rawBytes) }
        }
        elseif ($body.string) { $raw = [Text.Encoding]::UTF8.GetBytes([string]$body.string) }
        if ($raw) {
            $json = [Text.Encoding]::UTF8.GetString($raw) | ConvertFrom-Json
            Write-Output "UPSTREAM_MODEL=$($json.model)"
            Write-Output "UPSTREAM_BODY_BYTES=$($raw.Length)"
        }
    }
    Write-Output "TEST_ROOT=$root"
    if ($cliOutputText) { $cliOutputText }
    if ($requests.Count -ne 1) { throw "Expected one upstream request, got $($requests.Count)" }
}
finally {
    if ($cli -and -not $cli.HasExited) { Stop-Process -Id $cli.Id -Force -ErrorAction SilentlyContinue }
    Stop-Port $GatewayPort; Stop-Port $MockServerPort
    Remove-Item Env:CODEX_HOME,Env:CODEXX_HOME,Env:CC_SWITCH_HOME -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    $afterPid = (Get-NetTCPConnection -LocalPort 8787 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess
    $afterHash = if (Test-Path $baselineConfig) { (Get-FileHash -Algorithm SHA256 $baselineConfig).Hash } else { 'MISSING' }
    $afterTask = Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' -ErrorAction SilentlyContinue
    $afterTaskState = Get-TaskFingerprint $afterTask
    if ($baselinePid -ne $afterPid -or $baselineHash -ne $afterHash -or $baselineTaskState -ne $afterTaskState) { throw 'Production gateway baseline changed during real CLI test' }
    Write-Output 'BASELINE_8787_UNCHANGED=True'
    Write-Output 'BASELINE_CONFIG_UNCHANGED=True'
    Write-Output 'BASELINE_TASK_UNCHANGED=True'
}
