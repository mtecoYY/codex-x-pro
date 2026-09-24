param(
    [string]$CodexXExe = (Join-Path $PSScriptRoot '..\apps\desktop\src-tauri\target\release\codex-x-pro.exe'),
    [string]$CodexExe = '',
    [string]$GatewayScript = (Join-Path $env:USERPROFILE '.codex-x\personal-gateway\codex_responses_repair_gateway.py'),
    [string]$WatchdogScript = (Join-Path $env:USERPROFILE '.codex-x\personal-gateway\codex_responses_repair_watchdog.ps1'),
    [string]$Python = 'python',
    [int]$GatewayPort = 18787,
    [int]$MockServerPort = 19090,
    [string]$MockServerJar = (Join-Path $env:USERPROFILE '.codex\tools\gateway-testing\mockserver-netty-5.15.0-shaded.jar'),
    [string]$TauriDriver = 'tauri-driver',
    [string]$NativeDriver = '',
    [int]$WebDriverPort = 4444,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Require-File([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label not found: $Path"
    }
}

function Test-Port([int]$Port) {
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $connect = $client.ConnectAsync('127.0.0.1', $Port)
        return $connect.Wait(250) -and $client.Connected
    }
    catch {
        return $false
    }
    finally {
        $client.Dispose()
    }
}

function Wait-Port([int]$Port, [bool]$Expected, [int]$Seconds = 30) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        if ((Test-Port $Port) -eq $Expected) { return }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    throw "Port $Port did not reach expected state: $Expected"
}

function Stop-Port([int]$Port) {
    $connections = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)
    foreach ($connection in $connections) {
        Stop-Process -Id $connection.OwningProcess -Force -ErrorAction SilentlyContinue
    }
}

function Stop-ProcessTree([int]$RootId) {
    $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $RootId" -ErrorAction SilentlyContinue)
    foreach ($child in $children) {
        Stop-ProcessTree ([int]$child.ProcessId)
    }
    Stop-Process -Id $RootId -Force -ErrorAction SilentlyContinue
}

function Stop-IsolatedProcesses([string]$RootPath, [int]$Port) {
    $needle = $RootPath.ToLowerInvariant()
    $matches = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        $_.CommandLine -and $_.CommandLine.ToLowerInvariant().Contains($needle)
    })
    foreach ($match in $matches) {
        Stop-ProcessTree ([int]$match.ProcessId)
    }
    Stop-Port $Port
}

if ($SkipBuild) { Require-File $CodexXExe 'Codex-X-Pro executable' }
Require-File $GatewayScript 'Gateway script'
Require-File $WatchdogScript 'Watchdog script'
Require-File $MockServerJar 'MockServer JAR'
if (-not (Get-Command $TauriDriver -ErrorAction SilentlyContinue)) { throw "tauri-driver not found: $TauriDriver. Install with: cargo install tauri-driver --locked" }
if ([string]::IsNullOrWhiteSpace($NativeDriver)) {
    $NativeDriver = (Get-Command msedgedriver -ErrorAction SilentlyContinue | Select-Object -First 1).Source
}
if ([string]::IsNullOrWhiteSpace($NativeDriver) -or -not (Test-Path -LiteralPath $NativeDriver -PathType Leaf)) {
    throw 'Native WebDriver not found. Install the Edge WebDriver matching msedge.exe, then rerun with -NativeDriver C:\path\msedgedriver.exe'
}
if ($CodexExe) { Require-File $CodexExe 'Codex executable' }
if ((Test-Port 8787)) { $baselineGatewayPid = (Get-NetTCPConnection -LocalPort 8787 -State Listen | Select-Object -First 1).OwningProcess }
$baselineConfig = Join-Path $env:USERPROFILE '.codex\config.toml'
$baselineConfigHash = if (Test-Path $baselineConfig) { (Get-FileHash -Algorithm SHA256 $baselineConfig).Hash } else { 'MISSING' }
$baselineTask = Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' -ErrorAction SilentlyContinue
$baselineTaskState = if ($baselineTask) { "$($baselineTask.State)|$($baselineTask.Settings.Enabled)" } else { 'MISSING' }

$root = Join-Path ([IO.Path]::GetTempPath()) ("codex-x-user-e2e-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path "$root\codex-home","$root\codexx-home","$root\cc-switch-home","$root\logs" | Out-Null
$config = @"
model_provider = "local-test"
model = "test-model"
disable_response_storage = true

[model_providers.local-test]
name = "Local Test"
base_url = "http://127.0.0.1:$MockServerPort/v1"
wire_api = "responses"
request_max_retries = 0
"@
[IO.File]::WriteAllText((Join-Path $root 'codex-home\config.toml'), $config, [Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText((Join-Path $root 'codex-home\auth.json'), '{"OPENAI_API_KEY":"isolated-test-key"}', [Text.UTF8Encoding]::new($false))

$env:CODEX_HOME = Join-Path $root 'codex-home'
$env:CODEXX_HOME = Join-Path $root 'codexx-home'
$env:CC_SWITCH_HOME = Join-Path $root 'cc-switch-home'
$env:CODEX_X_GATEWAY_SCRIPT = $GatewayScript
$env:CODEX_X_GATEWAY_WATCHDOG = $WatchdogScript
$env:CODEX_X_PYTHON = $Python
$env:CODEX_X_WATCHDOG_TASK_NAME = "Codex-X-Pro-Isolated-$GatewayPort"
$mock = $null
$driver = $null
$codex = $null

try {
    foreach ($port in @($GatewayPort, $MockServerPort)) {
        if (Test-Port $port) { throw "Test port already in use: $port" }
    }
    $mock = Start-Process java.exe -ArgumentList '-Dmockserver.localBoundIP=127.0.0.1','-jar',$MockServerJar,'-serverPort',$MockServerPort,'-logLevel','WARN' -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput "$root\logs\mockserver.out.log" -RedirectStandardError "$root\logs\mockserver.err.log" -PassThru
    Wait-Port $MockServerPort $true
    $expectation = "{`"httpRequest`":{`"method`":`"POST`",`"path`":`"/v1/responses`"},`"httpResponse`":{`"statusCode`":200,`"headers`":{`"Content-Type`": [`"application/json`"]},`"body`":`"{\`"id\`":\`"isolated-response\`",\`"output\`":[]}`"}}"
    Invoke-RestMethod -Uri "http://127.0.0.1:$MockServerPort/mockserver/expectation" -Method Put -ContentType 'application/json' -Body $expectation | Out-Null
    if (-not $SkipBuild) {
        pnpm --dir (Join-Path $PSScriptRoot '..\apps\desktop') build:renderer | Out-Host
        cargo build --release --manifest-path (Join-Path $PSScriptRoot '..\apps\desktop\src-tauri\Cargo.toml') --locked | Out-Host
    }
    Require-File $CodexXExe 'Codex-X-Pro executable'

    $driver = Start-Process -FilePath $TauriDriver -ArgumentList '--port', $WebDriverPort, '--native-driver', $NativeDriver -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput "$root\logs\tauri-driver.out.log" -RedirectStandardError "$root\logs\tauri-driver.err.log" -PassThru
    Wait-Port $WebDriverPort $true
    $env:CODEX_X_EXE = $CodexXExe
    $env:CODEX_X_TEST_PORT = $GatewayPort
    $env:CODEX_X_MOCK_PORT = $MockServerPort
    $env:WEBDRIVER_URL = "http://127.0.0.1:$WebDriverPort"
    $env:E2E_TIMEOUT_MS = '60000'
    node (Join-Path $PSScriptRoot 'desktop_user_e2e.mjs') | Tee-Object -FilePath "$root\logs\e2e-result.log" | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Desktop WebDriver E2E failed with exit code $LASTEXITCODE" }
    Write-Output 'E2E_DRIVER_COMPLETED=True'
    if ($CodexExe) {
        $codex = Start-Process -FilePath $CodexExe -WorkingDirectory (Split-Path $CodexExe) -PassThru
        Start-Sleep -Seconds 2
        if ($codex.HasExited) { throw 'Codex exited during isolated startup' }
        Write-Output 'E2E_CODEX_RESTART_READY=True'
    } else {
        Write-Output 'E2E_CODEX_RESTART_READY=SKIPPED_NO_CODEX_EXE'
    }
    Write-Output 'E2E_UI_STEPS=ATTACH_WEBDRIVER_AND_RUN'
}
finally {
    if ($codex) { Stop-Process -Id $codex.Id -Force -ErrorAction SilentlyContinue }
    if ($driver) { Stop-ProcessTree $driver.Id }
    Stop-IsolatedProcesses $root $GatewayPort
    Stop-Port $MockServerPort
    Start-Sleep -Milliseconds 500
    $isolatedTask = Get-ScheduledTask -TaskName $env:CODEX_X_WATCHDOG_TASK_NAME -ErrorAction SilentlyContinue
    if ($isolatedTask) { Unregister-ScheduledTask -TaskName $env:CODEX_X_WATCHDOG_TASK_NAME -Confirm:$false }
    $afterGatewayPid = if (Test-Port 8787) { (Get-NetTCPConnection -LocalPort 8787 -State Listen | Select-Object -First 1).OwningProcess }
    $afterConfigHash = if (Test-Path $baselineConfig) { (Get-FileHash -Algorithm SHA256 $baselineConfig).Hash } else { 'MISSING' }
    $afterTask = Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' -ErrorAction SilentlyContinue
    $afterTaskState = if ($afterTask) { "$($afterTask.State)|$($afterTask.Settings.Enabled)" } else { 'MISSING' }
    if (($baselineGatewayPid -ne $afterGatewayPid) -or ($baselineConfigHash -ne $afterConfigHash) -or ($baselineTaskState -ne $afterTaskState)) {
        throw 'Production gateway baseline changed during isolated user E2E'
    }
    Write-Output 'BASELINE_8787_UNCHANGED=True'
    Write-Output 'BASELINE_CONFIG_UNCHANGED=True'
    Write-Output 'BASELINE_TASK_UNCHANGED=True'
}
