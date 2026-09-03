# Codex-X 本地网关旁路测试方案

> 本文同时是网关功能的测试规范和发布门禁。文档中标记为“自动化”的项目必须有可重复执行的测试入口；只通过编译、Rust 内部函数测试、前端纯函数测试或 Python HTTP 测试，不能宣称桌面网关功能通过。

## 0. 测试完整性门禁

### 0.1 必须验证的真实边界

网关测试必须覆盖下面的完整调用链，不能只验证其中一个局部：

```text
React 页面
  -> gatewayCommands adapter
  -> Tauri invoke 参数对象
  -> Rust #[tauri::command] 参数反序列化
  -> gateway.rs 控制 API
  -> 临时网关/本地假上游
  -> 响应回到 Rust
  -> 响应回到页面状态
```

当前 Tauri command 的参数契约是固定的：

```text
get_gateway_process_state({ listenPort })
start_gateway({ input: { listenHost, listenPort, upstream, configDir } })
stop_gateway()
gateway_request({ input: { listenPort, method, path, body } })
```

`get_gateway_process_state` 返回的 `running` 只表示目标网关控制接口可用；测试还必须检查
`managedByCodexX` 和 `codexRouteActive`。只有三者分别为 `true`、`true`、`true` 时，才可把
Codex 视为已接入网关。网关进程仍运行但 live `config.toml` 指向其他网站时，必须验证页面显示
“网关运行中但未接入 Codex”，并且 Provider/提示词热更新、实时观测和用户脚本控制不会发往网关。

特别注意：`gateway_request` 的 `input` 是 Tauri command 的必需外层键。页面不得直接写裸 `invoke("gateway_request", ...)`，必须经
`apps/desktop/src/gatewayCommands.ts` 的 `gatewayCommands.request()` 调用。

### 0.2 自动化测试层次和对应入口

| 层次 | 证明内容 | 当前入口 | 发布要求 |
| --- | --- | --- | --- |
| 前端 command adapter | command 名称、外层键、字段名、`null` body、无重复包裹 | `apps/desktop/tests/gatewayCommands.test.mjs` | 每次必跑 |
| 前端页面调用静态门禁 | 页面不能绕过 adapter 直接调用网关 command | 同上静态扫描 | 每次必跑 |
| React 页面交互 | 加载、启动、停止、观测操作、错误展示和状态刷新 | `apps/desktop/tests/GatewayPage.test.tsx`、`GatewayObservePage.test.tsx`、`GatewayScriptsPage.test.tsx` | 涉及 UI 必跑 |
| Rust 数据边界 | Tauri command 外层 `input`、缺字段、错误类型和 camelCase | `gateway::tests` | 每次必跑 |
| Rust 网关模块 | URL、状态、投影、watchdog、恢复、回滚和控制 API | `cargo test --lib` | 每次必跑 |
| 隔离 HTTP 全链路 | 临时网关、MockServer、请求路径/正文/错误传播 | `scripts/test_gateway_isolated_e2e.ps1` | 涉及外部网关运行时必跑 |
| 生命周期 | 启动、托盘隐藏、应用退出、重启、强杀、watchdog 恢复 | 本文第 3.7 节 | Windows 发布必跑 |
| 安装包验收 | 实际 MSI/便携版点击流程、无 `invalid args`、无黑框、无闪退 | 本文第 3.8 节 | 发布阻断 |

### 0.3 当前变更的最低回归集

以下命令必须在网关相关变更中执行：

```powershell
pnpm --dir apps/desktop typecheck
pnpm --dir apps/desktop test:gateway
pnpm --dir apps/desktop build:renderer
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --locked
```

`test:gateway` 必须至少包含：

1. `gatewayCommands.test.mjs` 的正向 IPC payload 测试；
2. 扁平 `gateway_request` 参数在 Tauri 边界失败的契约测试；
3. `get_gateway_process_state`、`start_gateway`、`stop_gateway` 的参数测试；
4. 页面源文件禁止裸 `gateway_request` 调用的静态测试；
5. `GET`、`POST`、`PUT`、`null` body 和端口 `8788` 的测试。

当前 `apps/desktop` 的 `test:gateway` 是单一网关门禁入口，依次执行前端 adapter/状态/详情测试、React 组件测试和
`cargo test --manifest-path src-tauri/Cargo.toml --lib --locked -- --test-threads=1`。发布流程不得只执行其中的前端部分；
如果需要单独定位 Rust 失败，可以直接运行后半条 Cargo 命令。

React 组件测试使用假的 Tauri `invoke`，用于证明页面按钮确实发出正确 command 和 payload，并验证错误展示、busy
清理及禁用状态。它不能证明 WebView、Windows 进程、计划任务、托盘和安装包行为；这些必须按第 3.8 节实际运行
当前构建产物验收。任何新增网关页面或按钮都必须同时补充正向、失败和禁用状态用例，且不能只断言按钮存在。

修改外部网关运行时或用户脚本协议时，还必须在 Windows 本机执行第 3.2 节的两组隔离 E2E。该测试依赖用户私有
网关和插件，不加入公开 CI；缺少外部脚本时必须报告“未执行”，不能用单元测试替代并宣称 HTTP 全链路通过。

### 0.4 失败注入必须验证“没有假成功”

所有网关 UI 和控制 API 的失败测试都必须同时断言：

- 后端返回稳定、可定位的错误码；
- 页面显示该错误，不能只打印到控制台；
- busy 状态被清理；
- 失败操作不会伪造“已启动”“已保存”或“已启用”；
- 后续状态刷新不会把失败覆盖成成功；
- 临时进程、任务和配置能够清理。

观测记录由后台队列异步组装。测试在 HTTP 响应返回后读取记录或详情时，必须等待
`retained_count`、目标序号或 SSE 事件达到预期条件，并设置明确超时；不得依赖“响应已经返回”
就立即读取，也不得用固定休眠掩盖竞态。

下列失败路径是发布阻断项：

```text
Tauri 参数缺失或类型错误
控制 API 不可达
端口占用
计划任务创建/启用失败
网关启动后健康检查失败
网关进程退出
网关进程运行但 Codex live 路由漂移到外部网站
控制 API 返回 400/409/500
SSE 断线和 history gap
观测上限非法或超限
脚本 manifest/协议/退出码/超时错误
应用退出、重启和强制终止
```

## 1. 目的与安全边界

本方案用于验证本地 Responses 网关的请求处理行为，同时保证正在使用的 Codex 会话不受影响。

旁路测试必须满足以下边界：

- 不修改 `C:\Users\aa\.codex\config.toml`、`auth.json` 或现有提示词文件；
- 不停止、重启或重配置当前 `127.0.0.1:8787` 网关；
- 不操作现有 `Codex Responses Repair Gateway` 计划任务；
- 默认不向真实上游发送请求。只有完成本节“真实上游测试”前置确认后，才允许发送专用的最小探针；不得发送真实会话内容或生产业务请求；
- 测试网关、假上游和日志使用独立端口与临时目录；
- 测试结束后必须确认生产端口和原进程仍保持原状态。

默认（假上游）拓扑如下：

```text
当前 Codex 会话
  -> 127.0.0.1:8787
  -> 现有网关
  -> 真实上游

旁路测试客户端
  -> 127.0.0.1:18787
  -> 临时测试网关
  -> 127.0.0.1:19090（本地假上游）
```

`18787` 和 `19090` 只是推荐值。运行前应确认端口空闲；如果被占用，改用其他临时端口，不得抢占 `8787`。

在显式批准真实上游测试后，临时网关的上游地址可以复用当前设置的 `https://newapi.gogogogoapp.mom`，但监听端口仍必须使用 `18787` 等临时端口，不能把当前 Codex 的 `base_url` 改到测试端口。

## 测试工具与 agent 使用约定

本机已安装一套专用于旁路测试的工具，agent 应优先复用这些入口，不要为一次测试临时修改系统 Python、全局代理或当前 Codex 配置。

### 工具位置

| 工具 | 版本 | 用途 | 入口 |
| --- | --- | --- | --- |
| MockServer | 5.15.0 | 固定假上游、验证请求、模拟状态码/延迟/错误 | `C:\Users\aa\.codex\skills\gateway-testing\scripts\start_mockserver.ps1` |
| mitmproxy/mitmdump | 12.2.3 | Python 流脚本观测、脱敏、请求变换和故障注入 | `C:\Users\aa\.codex\skills\gateway-testing\scripts\start_mitmproxy.ps1` |
| 测试端口清理器 | 本地脚本 | 按监听端口停止测试进程 | `C:\Users\aa\.codex\skills\gateway-testing\scripts\stop_test_port.ps1` |

工具安装目录为 `C:\Users\aa\.codex\tools\gateway-testing`。详细启动约定见全局 skill `$gateway-testing`（文件：`C:\Users\aa\.codex\skills\gateway-testing\SKILL.md`）及其参考文件 `C:\Users\aa\.codex\skills\gateway-testing\references\isolated-workflow.md`；如果 agent 无法加载 skill，直接使用本节和上表中的脚本路径。

### agent 的选择规则

agent 开始测试前应先判断测试目标：

| 测试目标 | 首选工具 | 上游地址 | 是否允许真实请求 |
| --- | --- | --- | --- |
| raw-text/正文、`Content-Length`、请求头和路径 | MockServer | `http://127.0.0.1:19090` | 否 |
| 固定响应、`401/429/5xx`、延迟和断开连接 | MockServer | `http://127.0.0.1:19090` | 否 |
| 需要 Python 逻辑检查或注入故障 | mitmproxy/mitmdump | 本地测试上游或临时网关 | 否 |
| DNS、TLS、认证和真实响应链路 | 临时网关 + 当前真实上游 | `https://newapi.gogogogoapp.mom` | 仅显式批准后 |

默认组合为：`临时网关 18787 -> MockServer 19090`。只有 MockServer 无法表达的观测或变换才使用 mitmproxy；只有需要验证真实服务行为时才启用真实上游测试。

### 标准启动与断言入口

启动 MockServer（仅回环监听）：

```powershell
& 'C:\Users\aa\.codex\skills\gateway-testing\scripts\start_mockserver.ps1' -Port 19090
```

通过 MockServer REST API 创建期望并验证请求。已验证的接口为：

```text
PUT http://127.0.0.1:19090/mockserver/expectation
PUT http://127.0.0.1:19090/mockserver/verify
```

创建固定响应的最小示例：

```powershell
$expectation = '{"httpRequest":{"method":"POST","path":"/v1/responses"},"httpResponse":{"statusCode":200,"headers":{"Content-Type":["application/json"]},"body":"{\"id\":\"resp_test\",\"output\":[]}"}}'
Invoke-WebRequest -UseBasicParsing `
  -Uri http://127.0.0.1:19090/mockserver/expectation `
  -Method Put `
  -ContentType 'application/json' `
  -Body $expectation
```

验证请求至少到达一次：

```powershell
$verification = '{"httpRequest":{"method":"POST","path":"/v1/responses"},"times":{"atLeast":1}}'
Invoke-WebRequest -UseBasicParsing `
  -Uri http://127.0.0.1:19090/mockserver/verify `
  -Method Put `
  -ContentType 'application/json' `
  -Body $verification
```

期望和验证正文只能使用虚构模型、ID 和文本。测试完成后清空 MockServer 的期望和已记录请求，再按端口停止：

```powershell
& 'C:\Users\aa\.codex\skills\gateway-testing\scripts\stop_test_port.ps1' -Port 19090
```

启动 mitmproxy（仅回环监听）：

```powershell
& 'C:\Users\aa\.codex\skills\gateway-testing\scripts\start_mitmproxy.ps1' -Port 19190
```

需要脚本时使用 `mitmdump` 的显式 flow 文件，并将匹配范围限制在测试端口、测试路径和测试主机。不要设置系统级代理，不要安装 mitmproxy CA 来处理普通 HTTP 旁路测试，不要让脚本修改真实上游地址或自动重放请求。

mitmproxy 推荐拓扑为：

```text
测试客户端 -> mitmproxy 19190 -> 临时网关 18787 -> MockServer 19090
```

如果目标只是验证网关转发后的最终请求，直接使用 MockServer 即可；只有需要观察客户端到网关之间的原始请求、对比改写前后差异或注入连接级错误时，才在链路前增加 mitmproxy。

启动临时网关：

```powershell
python <外部本地网关脚本路径> `
  --listen 127.0.0.1:18787 `
  --upstream http://127.0.0.1:19090
```

agent 必须在启动后确认 `18787` 正在监听，再发送测试请求；不得把 `C:\Users\aa\.codex\config.toml` 的 `base_url` 改为 `18787`。

### agent 执行清单

agent 应按以下顺序执行，并在报告中记录每一步结果：

1. 记录 `8787` 的监听 PID、当前配置哈希和现有计划任务状态；
2. 检查临时端口空闲；
3. 根据测试目标启动 MockServer 或 mitmproxy；
4. 启动临时网关并确认只绑定 `127.0.0.1`；
5. 发送最小、虚构、无敏感信息的测试请求；
6. 通过 MockServer REST API 或 mitmproxy 流脚本断言请求结果；
7. 遇到真实上游需求时，暂停并确认本节 3.3 的显式授权条件；
8. 停止测试端口上的进程、清理期望/记录和临时日志；
9. 重新检查 `8787` PID、配置哈希和计划任务状态与基线一致。

任何一步无法完成时，agent 应停止后续请求并报告具体阶段，不通过重试、切换 `8787` 或修改生产配置绕过失败。

## 2. 当前实现范围

截至当前版本，已落地的集成能力包括：独立网关页面、网关进程控制、`config.toml` 本地 `base_url` 原子投影与关闭恢复、提示词文件字段的运行时接管、Provider/提示词控制接口、结构化提示词目标锁定、脱敏请求快照、有界（默认 100 条）观测队列、SSE 接口以及用户脚本 raw-text 测试/启用链。Rust 侧在启动前保存 live 配置快照；目标方案要求关闭时以磁盘当前文件为基础、按字段所有权确定性写回，并仅在写入事务期间发生并发竞争时返回 `DIRECT_CONFIG_WRITE_CONFLICT`。普通外部修改不应触发整文件拒绝覆盖，具体边界见 `docs/GATEWAY_CONFLICT_RESOLUTION_DESIGN.md`。

仍需独立集成环境验证的项目：目标 Windows 安装包中的计划任务创建/登录自启动行为、无缝端口迁移、Codex 旧 session 对 live `base_url` 的缓存提示，以及真实 New API 响应头渠道识别。网关运行时已提供 SSE `after=<seq>` 恢复、Responses `usage` Tokens 统计和 Provider 认证投影；这些仍需结合安装包人工验收，但不再属于“接口尚未实现”。

外部本地网关脚本已实现：

- JSON 请求解析；
- raw-text 请求/响应协议的解析与序列化；
- 用户脚本对最终请求内容的完全覆盖；
- 普通 `function_call` ID 保持不变；
- 重新计算唯一的 `Content-Length`；
- 过滤 hop-by-hop、`Expect` 和原始 `Content-Length`；`Host` 保留用户脚本的最终值；
- 非 JSON 或无法解析的正文透传。

当前脚本已提供 `/health`、`/state`、`/observe/*`、`/scripts/*` 控制接口、运行时 Provider/提示词同步、脱敏快照、有界观测队列和用户脚本协议。Codex-X Rust 控制层负责启动/停止网关、写入本地 `base_url` 投影、保存快照并在关闭时做冲突校验和原子恢复。实时页面仍需在独立集成环境中验证 UI 灰化、端口迁移和 Windows 计划任务行为；本方案的旁路测试不能替代这些测试。

用户脚本处理器协议（脚本发现、raw-text 输入/输出、退出码控制、测试门禁、优先级链和脚本异常观测）已具备隔离运行测试；不得把真实上游探针结果当作协议测试结果。

## 3. 测试层次

### 3.1 模块级合同测试（零网络）

直接导入网关模块，使用内存对象验证 raw-text 协议和观测脱敏。

1. raw-text 请求行、headers、空行和正文完整往返；
2. 用户脚本可修改 method、path、headers 和正文；
3. Host 可修改，但 TCP 上游目标仍由 Provider 固定；
4. 非 JSON 正文透传且观测脱敏；
5. Content-Length 按最终正文重算且仅保留一个；
6. 五个探针支持 raw-text 和两种 JSON 视图；
7. tool-call ID 修复仅在个人用户脚本中验证，不属于网关内置功能。

在外部本地网关工具目录执行网关脚本语法检查和运行时回归测试：

```powershell
python -m py_compile <外部本地网关脚本文件>
python -m unittest <外部本地网关运行时测试模块>
```

### 3.2 隔离端到端测试（本地假上游）

该层验证 HTTP 头、路径、正文和错误传播。假上游默认使用本节定义的 MockServer，并通过其 REST API 创建期望和验证请求。MockServer 必须只绑定 `127.0.0.1`，收到请求后返回固定响应，不得转发到互联网。

执行顺序：

1. 检查 `18787`、`19090` 未被占用；
2. 启动本地假上游；
3. 启动临时网关，参数为 `--listen 127.0.0.1:18787 --upstream http://127.0.0.1:19090`；
4. 等待临时端口可连接；
5. 使用固定测试请求访问 `http://127.0.0.1:18787/v1/responses`；
6. 比较假上游收到的正文、方法、路径和关键非敏感请求头；
7. 结束临时网关和假上游；
8. 再次检查 `8787` 的监听进程、现有计划任务和当前配置摘要。

测试请求示例（仅使用虚构数据）：

```json
{
  "model": "test-model",
  "input": [
    {
      "type": "custom_tool_call",
      "id": "fc_custom_123"
    },
    {
      "type": "tool_search_call",
      "id": "fc_search_456"
    },
    {
      "type": "function_call",
      "id": "fc_function_789"
    }
  ]
}
```

假上游应观察到：

- 必须分别运行“空脚本目录”和“启用个人插件”两组测试。空脚本目录中 `custom_tool_call` 的 `fc_`
  ID 保持不变；启用个人 tool-call ID 插件后只允许该 ID 变为 `ctc_`，`function_call`、
  `tool_search_call` 和历史错误文本必须保持不变；
- JSON 以 UTF-8 转发，最终正文的字节长度与唯一的 `Content-Length` 一致；
- 原始 `Content-Length`、`Host` 和 `Expect` 不得原值重复转发；最终 `Host` 和
  `Content-Length` 由网关重建；
- `Authorization`、`Cookie` 等认证或会话头必须按当前网关传输策略测试，不能把“永不转发”
  写成无条件规则。无论是否转发，网关日志、观测快照和测试报告都不得出现其真实值；
- 请求路径仍为 `/v1/responses`，用户脚本输出的路由字段不能改变固定 TCP 上游。

仓库提供可重复执行的隔离 E2E。第一组使用空脚本目录，证明通用网关本体没有隐藏业务改写：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test_gateway_isolated_e2e.ps1 `
  -GatewayScript "$env:USERPROFILE\.codex-x\personal-gateway\codex_responses_repair_gateway.py" `
  -ExpectedCustomId "fc_custom_123"
```

第二组加载并启用本机个人插件，证明私有修复逻辑与通用网关边界对齐：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test_gateway_isolated_e2e.ps1 `
  -GatewayScript "$env:USERPROFILE\.codex-x\personal-gateway\codex_responses_repair_gateway.py" `
  -UserScriptDir "$env:USERPROFILE\.codex-x\gateway-tools" `
  -EnableScriptId "tool-call-id-repair" `
  -ExpectedCustomId "ctc_custom_123"
```

脚本只使用临时 `18787/19090`、临时状态目录和虚构请求，并在清理后断言当前 `8787` PID、
个人计划任务状态和 `config.toml` SHA-256 未变化。脚本还检查临时日志中不包含虚构的认证值；
任何一组失败都属于实现失败，不能修改期望值来迁就当前输出。

### 3.3 真实上游烟雾测试（显式启用）

本节用于验证临时网关到真实上游的 DNS、TLS、认证、路径、响应和超时行为。它不会改变本地 Codex 的路由，但可能产生真实上游日志、额度消耗、限流记录或模型调用费用，因此默认不执行。

#### 3.3.1 前置确认

开始前必须逐项确认并记录：

1. 当前 `127.0.0.1:8787` 的监听 PID、`C:\Users\aa\.codex\config.toml` 哈希和 `Codex Responses Repair Gateway` 计划任务状态；
2. 临时端口（例如 `18787`）未被占用；
3. 测试使用专用 Token。若确实要复用当前凭据，必须由用户明确批准，并通过临时环境变量注入，禁止从命令行参数、文档、日志或截图中暴露；
4. 上游允许测试请求，且已确认本次测试的额度、限流和审计影响；
5. 测试客户端不会读取、重放或拼接当前 Codex 会话正文、工具参数和设备标识；
6. 测试脚本关闭自动重试，并设置单次连接/读取超时。

可使用以下只读命令采集基线。输出只包含端口、PID、任务状态和文件哈希，不读取文件正文：

```powershell
Get-NetTCPConnection -LocalPort 8787 -State Listen |
  Select-Object LocalAddress,LocalPort,OwningProcess
Get-FileHash C:\Users\aa\.codex\config.toml -Algorithm SHA256
Get-ScheduledTask -TaskName 'Codex Responses Repair Gateway' |
  Select-Object TaskName,State
Get-ScheduledTaskInfo -TaskName 'Codex Responses Repair Gateway' |
  Select-Object LastTaskResult,LastRunTime
```

如果基线命令无法确认 `8787` 或计划任务状态，应先停止真实上游测试，不能以“测试结束后再检查”代替前置基线。

推荐使用环境变量保存测试凭据，而不是把 Token 写进命令：

```powershell
$env:CODEX_GATEWAY_TEST_TOKEN = '<由安全凭据存储临时注入的专用 Token>'
```

测试结束后应立即清除该进程环境变量：

```powershell
Remove-Item Env:CODEX_GATEWAY_TEST_TOKEN -ErrorAction SilentlyContinue
```

#### 3.3.2 启动临时网关

临时网关复用当前上游 URL，但不复用当前监听端口：

```powershell
python <外部本地网关脚本路径> `
  --listen 127.0.0.1:18787 `
  --upstream https://newapi.gogogogoapp.mom
```

启动前后都要检查 `18787` 的监听进程。不得修改 `C:\Users\aa\.codex\config.toml`，不得启动现有看门狗，也不得停止现有 `8787` 网关。

建议把“启动临时网关”和“发送真实请求”分成两个独立的终端步骤：先确认临时端口已经监听，再人工复核本节前置确认，最后才执行探针。这样可以避免复制命令时误把测试流量接入当前 Codex 会话。

#### 3.3.3 只读探针：`/v1/models`

优先执行只读探针，以确认认证和链路。客户端必须显式将专用 Token 放入请求头；临时网关会按现有逻辑转发该头，但不会把它写入日志。

```powershell
$headers = @{ Authorization = "Bearer $env:CODEX_GATEWAY_TEST_TOKEN" }
Invoke-WebRequest -UseBasicParsing `
  -Uri http://127.0.0.1:18787/v1/models `
  -Headers $headers `
  -TimeoutSec 15
```

结果分类：

| 结果 | 含义 | 后续动作 |
| --- | --- | --- |
| `200` | DNS、TLS、认证和路径基本可用 | 可选择执行一次最小 Responses 探针 |
| `401`/`403` | 凭据缺失、无效或无权限 | 停止测试，不重试，不改本地配置 |
| `429` | 上游限流 | 停止测试，记录 `Retry-After`，不自动重试 |
| `5xx` | 上游服务或网关链路异常 | 记录状态码和脱敏错误摘要，停止测试 |
| 超时/连接失败 | DNS、TLS、代理或网络不可达 | 记录错误类别，停止测试 |

`401`/`403` 不应被当作网关代码故障；它们只能说明真实上游拒绝了这次探针。

只读探针也必须经过“单次请求”约束：不要使用会自动重试的浏览器刷新、代理重试或脚本循环；不要并行启动多个临时网关连接同一真实上游。

#### 3.3.4 最小 Responses 探针（可选、有额度消耗）

只有 `/v1/models` 成功且已确认费用影响后，才可执行一次最小 Responses 请求。请求使用固定短文本、`store=false`、极小输出上限，不包含工具调用、文件、图片、真实会话内容或真实用户数据：

```powershell
$body = '{"model":"gpt-5.6-sol","input":"Reply with exactly OK.","store":false,"max_output_tokens":1}'
$headers = @{ Authorization = "Bearer $env:CODEX_GATEWAY_TEST_TOKEN" }
Invoke-WebRequest -UseBasicParsing `
  -Uri http://127.0.0.1:18787/v1/responses `
  -Method Post `
  -ContentType 'application/json' `
  -Headers $headers `
  -Body $body `
  -TimeoutSec 30
```

约束：

- 只发送一次；客户端和网关测试脚本均不得自动重试；
- `max_output_tokens` 保持最小，模型应使用当前 Provider 支持的测试模型；
- 不用伪造的工具调用 ID 验证改写是否成功；真实上游可能按协议拒绝伪造结构；
- 真上游测试只能证明真实链路和响应处理，不能替代假上游对 raw-text 修改和 `Content-Length` 的断言；
- 若响应为 `401`、`403`、`429` 或 `5xx`，记录分类后结束，不通过重复请求“撞成功”。

#### 3.3.5 真实上游测试的证据与清理

允许记录：测试时间、临时端口、上游主机名、HTTP 状态码、耗时、响应类型和脱敏错误摘要。禁止记录：Token、Authorization 值、Cookie、完整请求/响应正文、真实会话 ID 和设备标识。

测试结束必须：

1. 终止临时网关并清理临时日志；
2. 清除测试 Token 环境变量；
3. 确认 `18787` 未监听；
4. 确认 `8787` 仍由测试前 PID 监听；
5. 确认计划任务状态、`config.toml` 哈希和当前 Codex 会话路由与测试前一致；
6. 在报告中单独标记真实上游产生的请求次数、可能的额度消耗和任何 `429`/`5xx` 结果。

真实上游测试的成功不代表 Provider 切换、提示词注入、运行时热更新或旧 session 重载已经实现；这些仍需按第 4 节的独立集成环境要求验证。

### 3.4 实时请求观测与有界保留测试

本节验证实施方案第 11 节定义的实时请求观测页面。测试必须在独立 `CODEX_HOME`、临时网关端口（例如 `18787`）和本地假上游（例如 `19090`）中进行；不得连接当前 `8787` 或真实上游。若控制面在某个构建中尚未实现，这些用例应标记为“当前实现尚未提供”，不能通过修改生产配置来代替。

#### 3.4.1 页面状态和基础采集

1. 网关未启动或处于 `direct` 模式时打开观测页面，确认页面灰化并显示“请先进入网关模式”；启动、暂停、清除、筛选、排序和上限输入框均禁用。
2. 启动临时网关并完成健康检查，确认页面从灰化变为可用；网关启动中只显示启动状态，不提前允许采集操作。
3. 确认默认 `capture_enabled=false`、`capture_limit=100`，点击“启动采集”后状态变为已启动，再点击“暂停采集”后不再增加新记录。
4. 发送包含虚构模型、状态码和 JSON 请求/响应的测试流，确认列表字段依次包含 ID、渠道网站、状态码、模型、请求耗时、首字耗时和 Tokens；错误行使用浅红底色。
5. 点击一条仍保留的记录，确认发送包和接收包以格式化 JSON 显示，Authorization、Cookie、API Key 等字段已脱敏；对非 JSON 响应显示类型和文本摘要。
6. 点击每个表头，确认排序箭头和升/降序切换只作用于当前内存记录，不发起历史日志查询。
7. 修改需要 Codex 重新读取 live 配置的网关设置，确认设置区域持续显示红色边框提示“网关改动将在重启 Codex 后生效”；该提示不能只出现在 Toast 或日志中，深色主题下文字和边框仍清晰可见。

#### 3.4.2 上限裁剪和设置校验

1. 保持默认上限 100，发送至少 105 条请求，确认最终 `retained_count=100`，记录 ID 为最新的连续 100 条，最旧 5 条不可通过详情接口读取，`evicted_count` 增加 5。
2. 将上限从 100 改为 10，确认设置原子成功并立即裁剪到最近 10 条；裁剪期间不出现超过 10 条的可见状态。
3. 将上限从 10 改为 200，确认只影响后续新增记录，不恢复已淘汰记录，`next_seq` 保持单调递增。
4. 依次提交 `0`、负数、空值、非数字、小数和超过 `capture_limit_max` 的值，确认请求失败且返回 `OBSERVE_CAPTURE_LIMIT_INVALID` 或 `OBSERVE_CAPTURE_LIMIT_TOO_LARGE`，旧上限和记录均保持不变。
5. 在 `direct` 模式调用设置、启动、暂停和清除接口，确认后端统一返回 `OBSERVE_GATEWAY_REQUIRED`，即使绕过 UI 也不能改变状态。
6. 点击“清除”，确认列表为空、`retained_count=0`，后续请求从新的递增序号继续；累计 `evicted_count`/`capture_dropped_count` 的语义与页面标注一致。

#### 3.4.3 生命周期、SSE 和性能隔离

1. 断开并恢复 `/observe/events` SSE，使用 `after=<seq>` 补齐仍在环形队列内的记录；已淘汰序号必须返回 `history_gap=true`，不能伪造历史详情。
2. 重启临时网关，确认观测列表、详情、计数和 `next_seq` 清空，`capture_enabled=false`；若重启前已将上限设置为 20，确认经过校验的 `capture_limit=20` 持久保留，首次未配置时才使用默认 100。
3. 将采集暂停或观测队列填满，持续发送请求并比较假上游收到的数量、正文和状态码；确认仅观测数据丢弃并增加 `capture_dropped_count`，主请求仍成功转发且延迟没有因等待观测 worker 而增加。再使用慢 SSE 客户端填满单客户端队列，确认事件被丢弃并可用 `after=<seq>` 恢复，其他客户端和主转发不受阻塞。
4. 发送超过 `capture_body_limit_bytes` 的请求和响应，确认详情标记 `OBSERVE_DETAIL_TRUNCATED`、显示原始/保留字节数，主请求仍按原路径完成。
5. 让 Tokens 计数模块返回不可用或异常，确认列表显示 `unavailable` 和明确原因，不能阻塞响应或伪造 Tokens 数值。

#### 3.4.4 清理和证据

测试结束后确认：

```text
18787：未监听
19090：未监听
观测 worker/SSE：已退出
8787：仍由测试前的进程监听
计划任务：状态和启用状态与测试前一致
config.toml：字节级摘要与测试前一致
```

记录 `capture_limit`、发送总数、最终 `retained_count`、`evicted_count`、`capture_dropped_count`、SSE `history_gap` 结果以及假上游收到的请求数；不得记录完整敏感正文。

### 3.5 用户脚本处理器协议测试

本节以 `raw-text` 为唯一脚本协议。每次调用向脚本 stdin 写入完整请求文本并关闭 stdin；stdout 必须是完整请求文本或退出码 10 对应的完整响应文本。脚本通过 `CODEX_X_SCRIPT_MODE`、`CODEX_X_SCRIPT_REQUEST_ID` 和 `CODEX_X_SCRIPT_DIRECTION` 获取调用元数据。

本节验证《本地网关集成实施方案》第 16 节。测试使用独立临时脚本目录、临时网关端口和本地假上游；所有脚本、请求和响应均使用虚构数据。每个用例都要记录脚本版本指纹，避免旧测试结果被误用于启用。

#### 3.5.1 发现、刷新和 manifest

1. 在临时脚本目录放入合法 `manifest.json`、入口程序和测试夹具，点击“刷新脚本”，确认列表显示 `id`、名称、简介、入口、出口格式、版本和优先级。
2. 刷新前新增脚本，确认手动刷新后出现；新增脚本不会自动启用，也不会改变当前已提交脚本链。
3. 分别测试缺少 `protocol_version`、缺少名称/简介/入口、字段类型错误、JSON 非法和重复 `id`，确认对应脚本标记 `SCRIPT_MANIFEST_INVALID`，显示具体文件/字段错误，其他脚本仍可用。
4. 修改脚本或 manifest 后再次刷新，确认原 `passed` 状态变为 `not_tested`，旧版本指纹不能继续用于启用。

#### 3.5.2 测试数据、失败重试和启用门禁

1. 为脚本准备普通 JSON、深层嵌套 JSON、非 JSON 正文和较大正文夹具，确认这些数据能通过完整 raw-text stdin 传入并从 stdout 返回；测试判定只检查请求/响应文本解析、退出码和结构完整性，不检查业务字段值或功能结果。
2. 使用能修改任意请求字段的脚本，确认修改后的方法、路径、headers 和正文进入下一步；不得因业务字段变化被拒绝。
3. 使用退出码非零、无输出、多份输出、非法请求行、非法响应状态行和超时脚本，确认状态为 `failed`，页面显示固定文案“测试失败”并紧邻显示“重试”。
4. 点击“测试数据”，确认能查看本次实际入口帧和出口帧；若展示受到详情大小上限影响，必须明确标记截断。点击“错误详情”，确认能查看退出码、stderr 摘要、协议/结构校验错误和测试时间。
5. 在 `not_tested`、`testing`、`failed`、测试结果过期四种状态下确认启用按钮均不可用；失败后修改脚本并点击“重试”，确认执行新版本测试，成功后才允许启用。
6. 直接调用启用接口而不经过 UI，确认后端仍自动执行一次 `mode=test`；测试失败返回 `SCRIPT_TEST_FAILED`，原脚本链和运行时版本不改变。

#### 3.5.3 链执行、优先级和最终目标

1. 启用三个通过测试的脚本，设置不同优先级，确认请求严格按优先级升序串行执行；设置相同优先级时，确认按稳定脚本 ID 顺序执行，多次刷新结果一致。
2. 让每个脚本依次修改不同请求字段，确认下一支脚本收到上一支的 raw-text stdout，链结束后假上游收到最终出口文本；脚本可以覆盖 Provider model 和提示词注入结果。
3. 让脚本输出 `upstream_url`、`destination` 或代理地址等字段，确认这些字段不会改变假上游地址；网关始终使用 Provider 配置的固定目标，并在协议诊断中记录被忽略字段。
4. 脚本返回 `respond` 时确认客户端收到脚本返回包且假上游未收到请求；返回 `drop` 或 `error` 时确认当前请求停止并收到可读错误。

#### 3.5.4 真实异常、入口/出口/返回包和耗时

1. 在脚本测试通过后发送包含特殊结构的真实请求，使脚本运行异常或出口帧无效；确认请求在访问上游前停止，客户端收到 `SCRIPT_EXECUTION_FAILED`，假上游请求数不增加。
2. 打开实时观测采集，点击该错误行，确认详情包含已生成的阶段探针、失败脚本 ID/名称/优先级、退出码或 stderr、具体协议错误和实际发给客户端的网关错误响应，并明确显示 `global_exit_probe=null`。
3. 发送脚本链全部成功的请求，确认观测同时包含入口包、出口包和上游返回包；发送 `respond` 请求，确认包含脚本返回包并标记 `script_chain_status=responded`。
4. 确认观测不要求暴露每个脚本的中间包，但元数据包含各脚本耗时、链总耗时和结果；异常记录不得被成功记录覆盖或伪造成上游错误。
5. 连续发送至少 12 次真实请求，确认每个脚本和脚本链的平均耗时仅使用最近 10 次 `mode=live` 调用，成功/直接响应/异常均按实际结果记录；测试和重试耗时不计入，样本数不足 10 次时显示实际样本数。
6. 重启临时网关，确认脚本启用配置按持久化版本加载，最近 10 次耗时样本清空；再次发送真实请求后从第一条样本重新累计。

#### 3.5.5 raw-text 探针详情视图

1. 对 `global_entry_probe`、`provider_model_probe`、`prompt_injection_probe`、`global_exit_probe` 和 `response_probe` 分别发送包含 JSON 和非 JSON 正文的请求，确认每个探针都能打开统一详情查看器。
2. 在同一详情页面切换 `raw-text`、`request body JSON` 和 `response body JSON`，确认视图切换不依赖探针名称；不存在或不可解析的 JSON 显示为不可用，不伪造对象。
3. 确认 `raw-text` 展示请求行/状态行、请求头/响应头、空行和正文，并按统一规则脱敏；大正文必须显示 `OBSERVE_DETAIL_TRUNCATED`、`original_bytes` 和 `retained_bytes`。

### 3.6 故障与清理测试

每次故障测试都必须使用临时端口，且不得改变当前 Codex 的 `base_url`。

| 场景 | 预期结果 |
| --- | --- |
| 假上游未启动 | 临时网关返回 `502`，当前 `8787` 不受影响 |
| 临时端口已占用 | 临时网关启动失败，不能尝试接管占用端口 |
| 非 JSON 正文 | 请求原样透传，不尝试字符串级替换 |
| 非法 `Content-Length` | 返回 `400`，不向上游发送不完整正文 |
| 临时网关主动退出 | `8787` 仍监听，现有会话继续使用原网关 |
| 测试进程异常退出 | 临时日志和进程可定位，不能触发生产看门狗 |

### 3.7 启动、正常关闭和异常退出恢复

生命周期测试必须使用独立 `CODEX_HOME`、临时端口和临时看门狗 intent，不得借用生产 `8787` 或生产计划任务。

1. 首次启动：无 `gateway-mode/state.json` 时启动应用，确认初始化不误判为网关模式，不创建网关子进程，也不改写 live `config.toml`、`auth.json` 或提示词文件。
2. 正常窗口关闭：点击主窗口关闭，确认复用原项目托盘语义，仅隐藏窗口，不停止网关或删除网关快照；从托盘恢复窗口后状态和控制权仍可用。
3. 显式退出：在网关运行且看门狗启用时退出应用，确认不撤销看门狗运行意图、不停止看门狗或网关、不恢复 live direct 配置；网关快照和 intent 保留。
4. 应用闪退/强制终止：在网关运行后终止应用进程但保留 `gateway-mode/state.json`、`runtime-state.json` 和 watchdog intent，重新启动应用时确认能通过 `/state` 的 `process_id` 识别并重新接管现有网关；若网关已崩溃且 intent 仍为 gateway，确认启动初始化按持久化 Provider 上游重拉一次并等待健康检查。
5. 外部占用保护：启动恢复时若端口健康响应的 `state`、loopback `listen` 或 `process_id` 与持久化快照不匹配，确认返回 `GATEWAY_PORT_IN_USE`/`DISABLE_STATE_UNAVAILABLE`，不终止未知进程、不覆盖 live 文件。
6. 看门狗崩溃循环：让网关连续异常退出，确认达到重启上限后看门狗退出并记录最近退出信息；将 intent 改为 `direct` 或 `watchdog_desired=false` 后确认不会再次拉起。
7. 显式退出与重启：通过托盘“退出 Codex-X”和应用重启入口触发 `RunEvent::ExitRequested`，确认只退出 Codex-X，watchdog intent、watchdog、网关 PID、live gateway 配置和模式快照均保留；再次启动后确认可重新接管。
8. 设置与抓包持久化边界：修改 `capture_limit`、Provider、提示词和脚本启用状态后重启临时网关，确认设置恢复；启动采集并生成记录后重启网关，确认 `capture_enabled=false`、记录/详情/序号/计数清空，而 `capture_limit` 保留。主窗口关闭到托盘期间记录继续增加；显式退出、卡退或强杀后不得从状态文件恢复抓包正文。
9. 正常停止状态：显式停止事务成功、模式快照已删除且临时端口不再监听后，再次查询进程状态必须得到 `running=false`、`degraded=false`、`error=null`；页面显示 stopped，不出现 `CONTROL_API_UNAVAILABLE`。
10. 预期运行但失联：保留 `desired_mode=gateway` 或 Codex-X 管理的存活子进程，并使 `/state` 不可达，确认返回 `degraded=true` 且错误包含 `CONTROL_API_UNAVAILABLE`；不得误报为正常 stopped。
11. 持久化降级优先级：同时制造 `degraded.json` 恢复错误和 `/state` 连接失败，确认页面优先展示持久化恢复错误，避免瞬时探测错误覆盖根因。

已自动化覆盖：外部本地看门狗测试（5 项看门狗生命周期测试，含非法 intent 安全退出和无 intent 的个人看门狗模式）以及 Rust `gateway::tests` 中的持久身份校验、启动无快照、启动恢复、句柄丢失后接管、退出无快照、正常 stopped 不生成控制面错误、预期运行但失联进入 degraded、持久化 degraded 错误优先级、任务 XML 隐藏/登录/重启配置、长 action 参数、UTF-16LE 编码和开启失败状态回滚测试。前端组件测试同时覆盖正常停止后不显示 `CONTROL_API_UNAVAILABLE`，以及 degraded 失联仍显示错误。Windows 还提供一条默认忽略的隔离集成测试 `windows_schtasks_accepts_generated_watchdog_xml`：它使用唯一临时任务名执行真实“创建旧任务 -> 导出快照 -> 覆盖新任务 -> 恢复旧任务 -> 查询验证 -> 删除”闭环，不运行 action，也不触碰生产任务。窗口托盘交互、真实 Tauri `ExitRequested` 事件、Windows 终止信号和安装包登录自启动仍需目标 Windows 环境人工验收。

真实计划任务回归测试命令：

```powershell
cargo test --lib gateway::tests::windows_schtasks_accepts_generated_watchdog_xml -- --ignored --exact
```

该测试必须确认查询结果包含 `<Hidden>true</Hidden>`、`-WindowStyle Hidden` 和 `-StateFile`，并在任何成功或失败路径删除测试任务。测试前后仍需按本节清理标准确认生产任务和 `8787` 监听未变化。

清理完成的判定：

```text
18787：未监听
19090：未监听
8787：仍由测试前的进程监听
计划任务：状态和启用状态与测试前一致
config.toml：字节级摘要与测试前一致
```

### 3.8 Windows 安装包和桌面冒烟验收

自动化模块测试通过后，仍必须对实际 Windows 安装包或便携版执行一次桌面验收。该验收不能用 renderer build、Rust unit test 或 MockServer HTTP 测试替代，因为它要验证 WebView、Tauri IPC、Windows 进程创建、计划任务和托盘事件的组合行为。

#### 3.8.1 安装包测试前置

1. 使用当前提交构建的实际 `.msi` 或便携版，不使用旧安装目录中的 exe。
2. 记录安装包 SHA-256、应用版本、测试时间和测试使用的临时 `CODEX_HOME`。
3. 确认当前个人网关 `127.0.0.1:8787` 的 PID、个人计划任务和配置哈希；测试不得停止或改写它们。
4. 使用项目测试端口 `8788` 或动态分配的临时端口；若端口被占用，先记录占用者并换端口。
5. 使用本地假上游或测试网关，不把当前 Codex 配置的 `base_url` 改到测试端口。

#### 3.8.2 必须实际点击的流程

```text
首次启动
  -> 打开“本地网关”
  -> 输入 8788
  -> 启动网关
  -> 刷新状态
  -> 打开“实时请求观测”
  -> 启动采集、暂停采集、清除、修改上限、打开详情
  -> 打开“用户脚本处理器”
  -> 刷新脚本、打开协议说明、打开测试弹窗、执行测试、打开测试详情
  -> 返回“本地网关”
  -> 停止网关
  -> 再次启动网关
  -> 关闭主窗口到托盘
  -> 从托盘恢复
  -> 托盘退出
  -> 重新启动应用
```

每一个操作都必须检查对应的 Tauri command 是否成功完成，不能只检查页面按钮是否有点击反馈。重点检查：

- `gateway_request` 不出现 `invalid args`、`missing required key input`；
- `start_gateway` 不出现双层 `input.input`；
- 端口从 `8787` 改为 `8788` 后，所有后续控制请求都使用 `8788`；
- 启动失败时页面保留“启动网关”而不是显示“停止网关”；
- 控制 API 的 400/409/500 错误显示在页面 `role=alert` 中；
- 观测和脚本页面在 direct/stopped 状态下灰化，且点击禁用按钮不发送控制请求；
- 网关进程运行但 `codexRouteActive=false` 时，观测和脚本页面仍灰化，且不发送控制请求；
- 脚本测试、脚本启用和脚本优先级请求都使用 `{ input: ... }`；
- 主窗口关闭只隐藏到托盘；
- 托盘退出、应用重启不删除 watchdog intent，不停止独立网关，不恢复 direct 配置；
- 应用重新启动后能重新识别或接管仍在运行的网关；
- Windows 下无由轮询、PowerShell、`schtasks` 或 watchdog 产生的可见黑框；
- Windows 事件查看器和应用日志中没有本次测试新增的崩溃。

#### 3.8.3 Windows 进程和任务证据

验收完成后记录以下非敏感证据：

```text
应用版本和安装包 SHA-256
测试端口和所有测试进程 PID
Tauri command 名称及 payload shape，不记录 body 敏感内容
项目任务 Codex-X Local Gateway 的 State/LastTaskResult
个人任务 Codex Responses Repair Gateway 的 State 是否保持不变
8787 前后 PID
config.toml 前后 SHA-256
Windows Event Viewer/WER 是否新增崩溃
```

清理完成后必须满足：

```text
测试端口无监听
测试任务已删除或恢复到测试前快照
测试网关/watchdog 进程已退出
8787 仍由测试前 PID 监听
个人计划任务状态未改变
个人配置哈希未改变
```

#### 3.8.4 发布阻断规则

出现以下任一情况，Windows 构建不能发布：

```text
任一网关按钮没有执行到预期 Tauri command
任一 command payload 与本节契约不一致
出现 invalid args 或 missing required key
失败操作显示为成功
托盘退出/应用重启改变网关状态
计划任务或 watchdog 现场未清理
出现可见黑框
出现应用闪退或新增 WER 事件
```

## 4. 与完整集成验收的对应关系

旁路测试可以直接覆盖《本地网关集成实施方案》验收标准中的请求转发、raw-text 处理、透传、错误边界和不影响旧会话等低层行为；tool-call ID 修复仅由用户个人脚本验证。

以下项目必须在独立 `CODEX_HOME` 或虚拟机中测试：

- `direct`/`gateway` 原子切换和失败回滚；
- Provider 运行时切换及版本冲突；
- 提示词 `append`/`replace`、目标探测、幂等和透传诊断；
- `/state`、`/settings`、SSE 事件、请求快照和敏感字段脱敏；
- 实时请求观测页面的灰化状态、启动/暂停/清除、筛选/排序、默认 100 条有界保留、上限校验、淘汰计数、详情截断和非阻塞队列；
- 用户脚本目录刷新、manifest 错误隔离、测试失败/重试提示、测试数据与错误详情、测试通过后启用门禁；
- raw-text stdin/stdout、退出码控制、任意请求字段修改、按优先级串行执行、最终上游地址不可被脚本改变；
- 脚本真实异常时的停止转发、`SCRIPT_EXECUTION_FAILED` 错误以及入口包/出口包/返回包审计；
- 最近 10 次真实调用平均耗时的滚动刷新，以及测试耗时排除；
- 看门狗计划任务的创建、停止、自动恢复和重启上限；
- 关闭网关后的配置回写、端口迁移和旧 session 缓存提示；
- Rust 启动事务的 live `config.toml` 原子投影、提示词字段移除/恢复、状态快照丢失和外部修改冲突。

这些测试必须使用独立配置目录、独立端口和独立计划任务名称，不能复用个人外部工具任务 `Codex Responses Repair Gateway`，也不能复用项目任务 `Codex-X Local Gateway`。

## 5. 记录格式与证据

### 5.1 自动化网关回归测试

在项目根目录执行：

```powershell
$personalGatewayDir = Join-Path $env:USERPROFILE '.codex-x\personal-gateway'
Push-Location $personalGatewayDir
try {
  python -m py_compile .\codex_responses_repair_gateway.py
  if ($LASTEXITCODE -ne 0) { throw 'Personal gateway syntax check failed' }
  python -m unittest discover -s . -p 'test_gateway_*.py'
  if ($LASTEXITCODE -ne 0) { throw 'Personal gateway tests failed' }
} finally {
  Pop-Location
}
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test_gateway_isolated_e2e.ps1 `
  -ExpectedCustomId "fc_custom_123"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test_gateway_isolated_e2e.ps1 `
  -UserScriptDir "$env:USERPROFILE\.codex-x\gateway-tools" `
  -EnableScriptId "tool-call-id-repair" `
  -ExpectedCustomId "ctc_custom_123"
```

当前测试覆盖 Provider 认证覆盖、提示词目标与幂等、有界观测队列、SSE 断档恢复、Tokens 不可用原因、详情脱敏、控制 API 输入校验，以及用户脚本 manifest、raw-text forward/respond/drop/error、非法输出、超时和路由字段隔离。tool-call ID 修复只作为本机个人用户脚本验证，不作为仓库内置功能测试。

每次测试至少记录以下非敏感信息：

- 测试日期、代码版本和 Python 解释器路径；
- 测试模式（模块级/假上游/真实上游）、临时监听端口和上游类型；
- 测试用例名称、输入摘要和预期结果；
- HTTP 状态码、转发正文摘要和脚本动作/错误计数；
- 脚本协议测试的 manifest 版本指纹、测试状态、动作、退出码、协议错误摘要、失败脚本和入口/出口/返回包引用；
- 脚本链最近 10 次真实耗时的样本数、平均值和是否排除测试调用；
- 测试前后 `8787` 监听 PID、计划任务状态和配置文件哈希；
- 失败时的临时网关退出码和错误摘要。

真实上游模式还应记录：只读探针和 Responses 探针各自的请求次数、使用的模型名称、HTTP 状态码、是否收到 `Retry-After`，以及用户批准的额度/费用上限。不得记录 Token 本身，也不得以“请求成功”推断没有产生费用。

日志、报告和截图不得包含 API Key、Token、Cookie、Authorization 值、完整请求正文或真实设备标识。测试报告应明确区分“已由旁路测试验证”“当前实现尚未提供”和“需要独立集成环境验证”三类结论。
