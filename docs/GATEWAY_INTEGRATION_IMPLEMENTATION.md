# Codex-X 本地网关集成实施方案

## 1. 文档目的

本文定义 Codex-X 与本地 Responses 网关的集成方案，覆盖：

- 网关总开关；
- 监听地址和端口配置；
- 看门狗的启用与 Windows 登录自启动；
- Provider 列表操作在网关模式下的实时生效；
- 一键管理指令提示词在网关模式下的实时生效；
- 关闭网关后将当前状态安全回写到 Codex 原生配置；
- 实时请求观测页面及其有界内存保留；
- 失败回滚、并发保护和旧 session 的边界处理。

本文是实施设计。当前代码已落地独立网关页面、网关进程控制、live `config.toml` 原子投影/恢复、提示词文件运行时接管、结构化提示词目标锁定、Provider/提示词运行时接口、脱敏有界观测、用户脚本协议、Windows 计划任务/登录自启动、SSE 断线恢复、Responses `usage` Tokens 统计和 Provider 认证文件投影；仍需在目标 Windows 安装包中完成一次人工验收，不能以页面显示或旁路测试替代。前端网关 command 统一经过 `apps/desktop/src/gatewayCommands.ts`，其 IPC 参数契约和页面交互测试由 `docs/GATEWAY_BYPASS_TESTING.md` 第 0 节和第 3.8 节定义。

## 2. 当前系统事实

当前 Codex 配置通过 `C:\Users\aa\.codex\config.toml` 指向：

```toml
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"
```

实际运行的本地网关位于：

```text
本机用户私有目录中的外部本地网关脚本；路径通过 `CODEX_X_GATEWAY_SCRIPT`
或 `C:\Users\<user>\.codex-x\personal-gateway\codex_responses_repair_gateway.py` 确定
```

实际运行的看门狗位于：

```text
本机用户私有目录中的外部本地看门狗脚本；路径通过 `CODEX_X_GATEWAY_WATCHDOG`
或 `C:\Users\<user>\.codex-x\personal-gateway\codex_responses_repair_watchdog.ps1` 确定
```

现有链路为：

```text
Codex -> 127.0.0.1:8787 -> Python 网关 -> https://newapi.gogogogoapp.mom
```

现有链路中的 Python 网关是当前用户自己的独立工具，不是 Codex-X 仓库中的业务模块。Codex-X
负责启动、停止、接管、恢复和调用这个外部进程；实际的 HTTP 监听、上游转发和网关运行时逻辑由外部个人工具完成。

### 2.1 项目代码、个人网关和个人插件的边界

本项目必须明确区分以下三个对象：

| 对象 | 所有权和位置 | 负责内容 | 是否随项目发布 |
| --- | --- | --- | --- |
| Codex-X 网关控制层 | 仓库内，主要是 `apps/desktop/src-tauri/src/gateway.rs` | 网关/直连模式切换、live `config.toml` 投影与恢复、持久化状态、watchdog intent、Windows 登录自启动、外部进程生命周期和控制 API 转发 | 是 |
| 外部个人网关工具 | 本机用户私有目录 `~/.codex-x/personal-gateway/`；也可由 `CODEX_X_GATEWAY_SCRIPT` 和 `CODEX_X_GATEWAY_WATCHDOG` 指定 | 本地 HTTP 监听、Responses 请求转发、上游连接、网关运行时状态和 watchdog 实际执行 | 否 |
| 个人用户插件 | 本机用户脚本目录 `~/.codex-x/gateway-tools/<script-id>/`，由用户自行安装或维护 | 对 raw-text 请求执行个人定制处理，例如 tool-call ID 修复；遵循 manifest、测试、启用、priority、超时和失败阻断规则 | 否 |

这里的“用户脚本处理器”是项目允许外部个人网关提供的插件协议和管理界面，不表示
Codex-X 内置了一份特定脚本。项目代码可以通过控制 API 请求外部网关发现、测试、启用、
禁用和排序用户脚本，但项目不拥有这些脚本的具体内容，也不应把个人脚本复制到仓库、
安装包或普通用户可见的内置资源中。

原先位于仓库 `scripts/codex_responses_repair_gateway.py` 的文件属于第二类“外部个人网关工具”，
不是第一类“Codex-X 网关控制层”。它及其 watchdog 应放在用户私有的
`C:\Users\<user>\.codex-x\personal-gateway\` 目录中。若该文件中的 tool-call ID 修复逻辑
继续使用，则应作为第三类“个人用户插件”维护；该逻辑不是网关控制层的内置后置处理。

当前仓库代码与上述边界的对应关系如下：

- `gateway_script()` 和 `watchdog_script()` 只从环境变量或用户私有默认路径查找外部脚本，
  不从仓库路径回退加载个人脚本。
- `spawn_gateway_process()` 通过 Python 启动外部网关；`request_control()` 只向
  `127.0.0.1:<port>` 的外部控制 API 发送经过校验的管理请求。
- `start()`、`stop()`、`initialize_on_startup()`、`shutdown_on_exit()` 负责项目侧的生命周期和
  状态边界，不实现 tool-call ID 改写。
- 前端“用户脚本处理器”页面只管理外部网关暴露的脚本接口；脚本入口、内容和实际改写规则
  来自本机用户脚本目录。

当前本机实现已按该边界对齐：外部网关本体不再内置 tool-call ID 改写；空脚本目录下
`custom_tool_call.id=fc_*` 原样转发。私有 `tool-call-id-repair` 插件通过测试并启用后，才将
目标 ID 改为 `ctc_*`，普通 `function_call` 保持不变。两种状态由隔离 E2E 分别验证，不能用
个人网关本体的隐藏后置处理代替插件。

Codex-X 现有 Provider 操作主要通过修改 `config.toml` 和 `auth.json` 完成；指令提示词操作会写入 `.md` 文件并设置或删除 `model_instructions_file`。切换后，已存在的 Codex session 不保证立即重新读取配置，项目当前约定是创建或重新打开 session。

## 3. 设计原则

### 3.1 一个期望状态，两个运行投影

Codex-X 内部需要维护唯一的“用户期望状态”（Canonical State）：

```text
Provider:
  provider_id
  provider_name
  base_url
  model
  wire_api
  requires_openai_auth
  api_key

Instruction:
  enabled
  template_id
  content
  injection_mode: append | replace

Runtime:
  desired_mode: direct | gateway
  gateway_listen_host
  gateway_listen_port
  watchdog_desired
  watchdog_autostart

Observation:
  capture_limit
  capture_body_limit_bytes
  observe_queue_capacity
```

同一份期望状态根据网关总开关投影到不同运行目标：

```text
网关关闭：Canonical State -> config.toml + auth.json + *.md
网关开启：Canonical State -> Gateway Runtime State
```

不得把“当前 `config.toml`”同时当作网关模式下的 Provider 唯一真相，否则 `base_url` 会被本地网关地址遮蔽，Codex-X 无法可靠判断实际远程 Provider。

### 3.2 网关只绑定本机

默认监听 `127.0.0.1`。控制 API、SSE/WebSocket 和请求正文不得暴露到局域网或公网。

### 3.3 切换必须可回滚

网关开关切换、Provider 回写和提示词回写均视为事务。任何一步失败，都不得留下“界面显示已开启、实际请求走直连”或“配置只写了一半”的状态。

### 3.4 明确请求级与配置级边界

- Codex-X 负责用户期望、Provider 库、提示词模板、备份和模式切换。
- 网关负责当前请求的上游选择、模型覆盖、提示词注入和请求观测。
- 网关不应直接修改 Codex-X Provider 数据库。

## 4. 模式定义

### 4.1 `direct`（网关关闭）

保持现有 Codex-X 行为：

```text
Codex -> 真实 Provider
```

Provider 操作写入 live `config.toml`/`auth.json`；提示词操作写入 Codex-X 管理的提示词文件和 `model_instructions_file`。

### 4.2 `gateway`（网关开启）

Codex 的 live 配置只负责指向本地网关：

```toml
base_url = "http://127.0.0.1:<port>/v1"
```

网关运行时状态负责：

- 当前远程 Provider 地址；
- 当前模型；
- 当前认证信息的转发策略；
- 当前指令提示词及 append/replace 模式；
- 请求修改和观测。

网关模式下，Provider 或提示词操作必须在保存 Canonical State 后同步到网关，下一条请求即可使用新状态，不要求重启 Codex 或重新打开 session。

## 5. 独立网关页面

网关模式的按钮和设置放在 Codex-X 的独立“本地网关”页面中，不与 Provider 列表或“一键管理指令提示词”页面混在一起。Provider 和提示词页面仍负责编辑用户期望状态；它们在网关开启时通过统一运行时同步层反映到网关。

该页面最上方必须放置常驻提示条（位于脚本列表、测试按钮和其他设置之前）：

```text
用户脚本仅在网关模式下生效。当前为直连模式时，脚本不会处理 Codex 的真实请求。
```

提示条必须根据实际 `gateway_mode` 状态实时更新，并在 `direct` 模式使用醒目的警示样式；不得只在用户首次进入页面时显示一次。直连模式下允许用户查看脚本、刷新发现结果和运行协议测试，但这些操作只是配置/测试行为，不会拦截、修改或阻断直连请求，也不会写入真实请求的脚本观测或耗时统计。

### 5.1 页面职责

独立网关页面负责：

- 网关总开关；
- 监听地址和端口；
- 当前网关状态、上游状态和最近错误；
- 看门狗启用/停用；
- Windows 登录自启动；
- 当前已提交的网关运行时版本；
- 最后一条请求的脱敏快照和清除操作；
- 实时请求观测页面（启动/暂停、筛选、排序、详情和保留上限）；
- 用户脚本处理链（发现、手动刷新、测试、启用/停用和优先级）；
- 每个脚本的测试状态、测试数据/错误详情和最近 10 次真实调用平均耗时；
- 网关运行时配置的重新加载和诊断。

看门狗开关和“登录自启动”开关都仅在 `gateway_mode = true` 时可操作。当网关总开关为 `direct` 时，两个按钮必须显示为禁用状态，并附带明确原因（例如“请先开启网关”）；不得在后台暗中修改计划任务。该页面可展示任务的实际状态，但不将直连模式下的计划任务状态视为可操作权限。

该页面不重复实现 Provider 编辑器或提示词模板编辑器，也不直接修改 Provider 数据库。页面只展示当前 Canonical State 的运行投影结果，并通过控制接口提交网关设置。

### 5.2 页面状态

页面至少显示以下状态：

```text
网关总开关：关闭 / 开启中 / 已开启 / 关闭中 / 降级
监听：127.0.0.1:<port>，已监听 / 未监听
看门狗：任务不存在 / 已停用 / 运行中 / 最近启动失败
自启动：已启用 / 未启用
运行时同步：已同步版本 N / 同步失败
用户脚本：已发现 N 个 / 最近一次刷新时间
脚本链：未启用 / 已启用（按优先级串行）
脚本运行范围：仅网关模式 / 当前未生效（直连模式）
```

页面状态还应包含以下计算字段：

```text
watchdog_control_enabled = (gateway_mode == true)
watchdog_desired       = 用户是否要运行看门狗
watchdog_runtime       = 计划任务/看门狗进程的实际状态
watchdog_autostart     = 是否在 Windows 登录时触发该任务
```

`watchdog_desired` 和 `watchdog_runtime` 必须分开展示：用户已开启但任务未运行时，应显示“启动中”或具体失败原因，不得伪装为“运行中”。当 `gateway_mode = false` 时，`watchdog_control_enabled` 为 `false`，看门狗控件显示停用/未运行，登录自启动仅保留用户偏好供下次开启网关时参考，不应使计划任务实际触发。

“已开启”只有在网关端口就绪、Provider 和提示词状态均已提交、Codex live `base_url` 已切换并完成健康检查后才能显示。表单提交成功不等于运行时提交成功，页面应以网关返回的已提交状态为准。

网关进程运行、Codex-X 管理网关和 Codex 当前请求已接入网关是三个独立事实。状态接口必须分别提供
`running`、`managed_by_codex_x` 和 `codex_route_active`；只有三者满足
`running=true`、`managed_by_codex_x=true`、`codex_route_active=true` 时，才允许称为“网关已接入”
并启用 Provider/提示词热更新、实时观测和用户脚本。`codex_route_active` 必须根据持久化
`codex_dir` 中 live `config.toml` 的有效 Provider `base_url` 与
`http://127.0.0.1:<listen_port>/v1` 比较得出，不能只根据网关 `/state` 或 Tauri 子进程句柄推断。

当网关进程仍运行且由 Codex-X 管理，但 live `config.toml` 已指向其他网站时，状态为
`disconnected/degraded`：保留网关、watchdog、网关模式意图和外部配置，不自动覆盖配置，也不把
Provider/提示词/观测/脚本操作发送到网关运行时；页面必须明确显示“网关运行中但未接入 Codex”。

当目标端口上已经存在健康网关，但 `managed_by_codex_x = false` 时，必须将其识别为外部网关，而不是 Codex-X 的网关模式：

```text
状态：外部网关运行中
Codex-X 模式：direct / 未进入网关模式
所有权：Codex-X 未接管
```

外部网关与 Codex-X 的状态完全隔离。Codex-X 不得因检测到外部网关而创建或保留自己的网关模式快照，也不得在退出、重启或启动恢复时改变 `managed_by_codex_x` 的判断。只要 `gateway-mode/state.json` 不存在，或持久化快照与运行时端口、监听地址、网关状态和 `process_id` 不匹配，当前端口上的运行实例就不能被视为 Codex-X 网关。

外部网关运行时，页面必须允许编辑监听端口并保留“启动网关”按钮，但必须禁用停止网关、实时观测、脚本启用/禁用和其他会改变外部网关状态的操作。用户不修改端口直接启动时，后端返回 `GATEWAY_PORT_IN_USE`；用户将端口改为例如 `8888` 后再启动，Codex-X 才可以在新端口建立自己的网关模式和快照。端口输入只改变 Codex-X 的启动目标，不停止或修改外部网关。

状态探测必须区分“正常停机”和“应运行但失联”。显式停止成功并删除 `gateway-mode/state.json` 后，`/state` 不可达是正常的 stopped 状态，返回 `running = false`、`degraded = false`、`error = null`。只有持久化意图仍为 gateway、Codex-X 管理的子进程仍存在，或已经记录持久化 degraded 状态时，控制接口不可达才返回 `degraded = true` 和错误详情。若同时存在 `degraded.json` 和本次健康探测错误，优先展示持久化恢复错误。

前端必须分别处理操作错误和状态错误：启动、停止命令失败直接展示该命令返回的错误；`processState.error` 仅在 `processState.degraded = true` 时展示。正常停止后的连接拒绝不得显示为 `CONTROL_API_UNAVAILABLE` 或“配置错误”。

对于需要 Codex 重新读取 live 配置的网关改动，独立网关页面必须在设置区域显著显示红色边框提示框，明确提示用户“网关改动将在重启 Codex 后生效”。该提示不得仅通过短暂 Toast 或不可见日志提供；在深色主题下也必须保持足够的红色边框和文字对比度。提示文案应与实际生效条件一致，不得把“已保存”误报为“已生效”。

### 5.3 失败状态与明确原因

页面不得只显示“失败”或“降级”。每次失败必须返回并展示以下信息：

```json
{
  "state": "degraded",
  "phase": "enable",
  "code": "GATEWAY_PORT_IN_USE",
  "message": "监听端口 8787 已被其他进程占用",
  "retryable": true,
  "action": "更换端口或关闭占用该端口的进程"
}
```

`message` 和 `action` 是用户可见文本；`code` 用于日志、测试和问题定位；不得在这些字段中包含 API Key、Token、Cookie 或完整请求正文。

| 状态/阶段 | 原因码 | 明确失败原因 | 页面处理 |
| --- | --- | --- | --- |
| 开启失败 | `GATEWAY_INVALID_LISTEN` | 监听地址格式错误、端口不是整数、端口不在 `1-65535` 范围内，或地址不是允许的本机地址 | 标记为关闭，保留用户输入并提示修正地址/端口 |
| 开启失败 | `GATEWAY_PORT_IN_USE` | 目标端口已被其他进程占用，无法绑定监听 | 标记为关闭，提示查看占用进程或更换端口 |
| 开启失败 | `WATCHDOG_TASK_START_FAILED` | Windows 计划任务不存在、被禁用、权限不足或启动动作无效 | 标记为关闭，显示任务状态和系统返回原因，提供重试/修复任务入口 |
| 开启失败 | `GATEWAY_PROCESS_START_FAILED` | Python 可执行文件、网关脚本或脚本依赖不存在，进程未能启动 | 标记为关闭，显示实际脚本路径和进程错误，不自动改写配置 |
| 开启失败 | `GATEWAY_LISTEN_TIMEOUT` | 看门狗已启动但在规定时间内没有出现目标监听 | 标记为关闭，显示网关 stderr 摘要和最后退出码 |
| 开启失败 | `GATEWAY_RUNTIME_SYNC_FAILED` | Provider/提示词运行时状态被网关拒绝、控制接口不可达或提交版本过旧 | 标记为关闭，显示未提交的状态类别和网关返回原因 |
| 开启失败 | `GATEWAY_HEALTHCHECK_FAILED` | 网关已监听但健康检查未通过，例如控制接口异常、上游地址无法解析或测试响应格式不符合要求 | 标记为关闭，保留网关诊断状态并允许重试 |
| 开启失败 | `LIVE_CONFIG_WRITE_CONFLICT` | 写入 `config.toml` 前后文件被其他程序修改，原子写入保护拒绝覆盖 | 恢复开启前状态，提示刷新 live 配置后重试 |
| 开启失败 | `LIVE_CONFIG_INVALID` | 当前 `config.toml`/`auth.json` 无法解析或备份失败，无法安全建立回滚点 | 不切换模式，提示修复配置或查看备份 |
| 开启失败 | `GATEWAY_UNKNOWN_FAILURE` | 发生了未能归入其他原因码的异常；必须同时提供不含敏感信息的底层错误摘要 | 保留开启前模式，停止继续提交并提供诊断信息 |
| Provider 同步失败 | `PROVIDER_INVALID` | Provider 缺少名称、`base_url` 或模型，或使用了不允许的占位值 | 不更新网关 active provider，保留上一版本 |
| Provider 同步失败 | `PROVIDER_VERSION_CONFLICT` | 网关收到的状态版本早于当前已提交版本，拒绝旧写入覆盖新状态 | 刷新网关状态，重新提交最新 Provider |
| Provider 同步失败 | `PROVIDER_RUNTIME_REJECTED` | 网关无法解析 Provider 地址、wire API 不支持或运行时配置校验失败 | 保留上一 Provider，显示字段级校验原因 |
| 提示词同步失败 | `INSTRUCTION_TEMPLATE_NOT_FOUND` | 选中的内置/已保存模板不存在，或模板内容读取失败 | 保留上一提示词状态，提示重新加载模板 |
| 提示词同步失败 | `INSTRUCTION_PATH_UNSUPPORTED` | 提示词无法映射到允许的 Responses JSON 路径，或 append/replace 模式无效 | 拒绝更新，显示允许的注入模式和路径 |
| 提示词同步失败 | `INSTRUCTION_RUNTIME_REJECTED` | 网关检测到重复注入、内容超过限制或运行时版本冲突 | 保留上一提示词状态，显示具体校验结果 |
| 请求注入失败 | `INSTRUCTION_TARGET_AMBIGUOUS` | 请求中同时存在多个可用的 developer/system 候选位置，且没有已确认的目标路径 | 原请求不修改，显示候选 JSON Pointer，等待用户确认或重新探测 |
| 请求注入失败 | `INSTRUCTION_TARGET_CHANGED` | 已锁定的目标路径、角色或节点类型在后续请求中发生变化 | 原请求不修改，标记需要重新探测，不自动猜测新位置 |
| 请求注入失败 | `INSTRUCTION_TARGET_UNSUPPORTED` | 目标消息包含无法安全处理的非文本节点、未知 content 类型或不支持的 instructions 类型 | 原请求不修改，显示实际节点类型和支持范围 |
| 请求注入失败 | `INSTRUCTION_TARGET_NOT_FOUND` | 请求没有顶层 `instructions`，也没有可安全处理的 developer/system 消息 | append 可创建顶层 `instructions`；replace 默认保持原请求并提示目标不可用 |
| 关闭失败 | `DISABLE_STATE_UNAVAILABLE` | Canonical State 不存在、损坏或无法读取，无法确定应回写的 Provider/提示词 | 保持网关开启，提示恢复状态或从备份选择 |
| 关闭失败 | `DIRECT_CONFIG_WRITE_FAILED` | 回写 `config.toml`、`auth.json` 或提示词文件时发生权限、磁盘或原子写入错误 | 保持网关开启，显示具体文件和系统错误 |
| 关闭失败 | `DIRECT_CONFIG_WRITE_CONFLICT` | 确定性写回事务执行期间 live 文件被其他进程并发修改，无法确认候选结果仍基于最新内容 | 保持网关开启，重新读取文件并重试；不提供逐字段 diff 选择 |
| 关闭失败 | `DIRECT_CONFIG_INVALID_AFTER_WRITE` | 回写完成后 TOML/JSON/提示词文件校验失败，不能提交直连模式 | 触发回滚；回滚失败时进入降级并保留网关 |
| 端口迁移失败 | `NEW_PORT_IN_USE` | 新端口已被占用，无法启动第二个监听实例 | 保持旧端口和旧 `base_url`，提示更换端口 |
| 端口迁移失败 | `NEW_PORT_NOT_READY` | 新实例未在超时前完成监听或健康检查 | 保持旧端口，清理未就绪实例并允许重试 |
| 端口迁移失败 | `BASE_URL_SWITCH_FAILED` | 新端口就绪，但 Codex live `config.toml` 原子更新失败 | 保持旧端口和旧模式，提示外部修改或权限原因 |
| 看门狗异常 | `WATCHDOG_TASK_MISSING` | 计划任务不存在，无法执行登录自启动或状态监控 | 显示“任务不存在”，提供创建任务操作 |
| 看门狗异常 | `WATCHDOG_TASK_DISABLED` | 计划任务存在但被禁用，因此不会在登录时启动 | 显示“已停用”，提供启用操作 |
| 看门狗异常 | `WATCHDOG_TASK_STOP_FAILED` | 已撤销重启资格，但现有看门狗进程未能在超时前退出或计划任务仍报告运行 | 保持网关模式，显示进程/任务状态，禁止回写直连配置 |
| 看门狗异常 | `WATCHDOG_RESTART_LOOP` | 网关连续退出并被反复重启，超过规定次数 | 标记为降级，停止无上限重启并显示最近退出码/错误摘要 |
| 看门狗控制失败 | `WATCHDOG_GATEWAY_REQUIRED` | 请求在 `direct` 模式下启用、停用或修改看门狗 | 后端拒绝操作，返回“请先开启网关”，UI 保持按钮禁用 |
| 控制接口失败 | `CONTROL_API_UNAVAILABLE` | 网关进程未监听控制接口、连接被拒绝或请求超时 | 不改变本地已提交状态，显示重试入口 |
| 控制接口失败 | `CONTROL_API_INVALID_REQUEST` | 请求 JSON 缺少字段、字段类型错误或版本不兼容 | 不改变运行时状态，显示字段级错误 |
| 观测控制失败 | `OBSERVE_GATEWAY_REQUIRED` | 网关未启动或当前为 `direct` 模式，观测页面没有可用运行时 | 页面灰化并提示“请先进入网关模式”，后端拒绝启动/暂停/清除/设置 |
| 观测设置失败 | `OBSERVE_CAPTURE_LIMIT_INVALID` | `capture_limit` 为空、非整数、非正数或小于配置的最小值 | 保留旧上限和旧记录，显示允许的整数范围 |
| 观测设置失败 | `OBSERVE_CAPTURE_LIMIT_TOO_LARGE` | `capture_limit` 超过配置的最大值，可能造成超出内存预算 | 保留旧上限和旧记录，提示降低上限 |
| 观测详情失败 | `OBSERVE_REQUEST_EVICTED` | 请求已因环形队列上限被淘汰，不再提供磁盘回查 | 弹窗显示记录已淘汰及当前保留窗口 |
| 观测旁路状态 | `OBSERVE_CAPTURE_DISABLED` | 用户尚未启动采集或已点击暂停 | 列表不新增记录，主请求继续正常转发 |
| 观测旁路状态 | `OBSERVE_QUEUE_FULL` | 非阻塞观测队列已满，无法及时交给观测 worker | 丢弃该条观测并增加 `capture_dropped_count`，主请求不等待 |
| 观测详情状态 | `OBSERVE_DETAIL_TRUNCATED` | 发送包或接收包超过单条详情字节上限 | 展示原始/保留字节数和截断原因，主请求不受影响 |
| 脚本发现失败 | `SCRIPT_MANIFEST_INVALID` | 脚本目录中的 `manifest.json` 缺少协议版本、名称、简介或入口信息，或字段类型错误 | 保留其他脚本，显示具体文件和字段错误，该脚本不可测试/启用 |
| 脚本测试失败 | `SCRIPT_TEST_FAILED` | 脚本退出码非零、无有效出口帧、出口帧无法解析或协议结构校验失败 | 显示“测试失败”和“重试”，提供“测试数据”“错误详情”，保持启用按钮禁用 |
| 脚本未测试 | `SCRIPT_TEST_REQUIRED` | 脚本尚未通过当前版本的测试，或脚本内容/manifest 已变更 | 保持脚本停用，要求先执行测试 |
| 脚本执行失败 | `SCRIPT_EXECUTION_FAILED` | 真实请求中脚本退出、超时、输出无效或返回协议错误 | 停止当前请求，不访问上游；反馈错误并在实时观测中记录入口包和失败详情 |
| 脚本链配置失败 | `SCRIPT_CHAIN_INVALID` | 启用脚本不存在、优先级不是整数或链配置包含未通过测试的脚本 | 保留上一份已提交链配置，不改变正在运行的链 |

发生 `degraded` 时必须保留最后一个已提交的有效模式、Provider、提示词和端口；页面同时显示“失败发生阶段”和“当前仍在生效的版本”。不可重试的错误应显示修复动作，而不是继续自动重试。

### 5.4 页面交互

- 点击网关总开关，执行第 6 节或第 9 节定义的开启/关闭事务；事务进行期间锁定重复操作。
- 修改端口时，网关关闭状态下保存为下次启动配置；网关开启状态下显示迁移提示，并按第 10 节的端口流程处理。
- 当网关为 `direct` 时，看门狗和登录自启动按钮必须禁用；任何来自前端的请求也必须在后端被拒绝为无效状态操作，不能仅依赖按钮禁用。
- 网关开启事务成功后自动持久化 `watchdog_desired = true`，启用并启动唯一的项目任务 `Codex-X Local Gateway`；watchdog 是网关模式的一部分，不再作为独立运行开关。该任务不得覆盖或控制个人外部工具的 `Codex Responses Repair Gateway` 任务。
- 网关关闭事务必须撤销看门狗的运行资格、停止正在运行的看门狗并禁用登录任务；应用退出或应用重启不属于网关关闭事务。
- 运行时同步失败时显示重试入口，不自动回写或覆盖用户的 Provider/提示词编辑内容。
- 脚本发现支持“刷新脚本”操作；刷新只读取约定脚本目录，不自动启用新发现的脚本。manifest 错误必须就地显示，不得让整个网关页面失败。
- 脚本模块的页面顶部必须持续显示“用户脚本仅在网关模式下生效。当前为直连模式时，脚本不会处理 Codex 的真实请求。”；切换到网关模式后提示同步显示已生效状态，不能依据本地按钮状态提前宣称生效。
- `direct` 模式下脚本可以被查看、刷新和测试，但不得进入真实请求处理链；脚本测试产生的输入/输出详情不写入实时请求观测，也不计入最近 10 次真实调用平均耗时。网关切换到 `gateway` 且运行时就绪后，已通过测试的脚本链才开始处理后续真实请求。
- 每次点击“测试”完成后，无论结果是成功还是失败，脚本卡片都必须在 `passed`/`failed` 状态旁显示“测试详情”入口。测试失败时还要固定显示“测试失败”和“重试”按钮；测试详情用于查看本次实际输入/输出包，错误详情用于查看退出码、stderr 摘要和协议校验错误。脚本内容或 manifest 变更后，原测试结果失效，必须重新测试，旧测试详情标记为已过期。
- 点击脚本卡片的“测试”按钮必须打开“测试脚本”弹窗。弹窗默认选中“默认测试包”并以只读格式化 JSON 展示；用户可切换到“自定义测试包”标签，粘贴或编辑完整入口帧后点击“运行测试”。弹窗必须提供“恢复默认”和“关闭”操作，并显示本次测试使用的包来源（默认/自定义）。
- 自定义测试包只要求符合第 16.2 节入口帧协议；业务字段、请求路径、请求头和正文内容由用户自行决定。网关仍不得接受任何最终上游地址控制字段，结构错误应在弹窗内定位到字段和行列。
- 只有最近一次测试通过的脚本才能启用；点击启用时必须自动再测试一次，自动测试未通过则保持停用并显示同一失败状态。启用操作不能绕过测试状态。
- 启用脚本按用户设置的整数优先级从小到大串行执行；同优先级按稳定的发现顺序/脚本 ID 排序。保存链配置前一次性校验所有脚本均已通过测试。
- 最后一条请求区域只显示脱敏快照，不提供重放、伪造请求或修改原始请求后重新发送功能。

### 5.5 与其他页面的联动

Provider 列表和指令提示词页面保留现有入口。它们提交操作后：

```text
网关关闭：继续调用现有 Codex-X 配置写入逻辑
网关开启：保存 Canonical State，再调用 Gateway Runtime Sync
```

网关页面应订阅同步事件并刷新当前 Provider、提示词、端口和健康状态。任何页面都不得仅根据本地 UI 状态推断网关已生效。

## 6. 网关开启流程

开启网关是一个两阶段提交过程：

1. 读取并校验 Canonical State、监听地址和端口。
2. 网关模式启用成功后自动持久化 `watchdog_desired = true`，启用唯一的登录任务并启动看门狗。
3. 将 Provider 和提示词运行时状态发送到网关。
4. 备份当前 `config.toml`、`auth.json` 以及 Codex-X 管理的提示词文件。
5. 将 Codex live `base_url` 原子改为本地网关地址。
6. 执行健康检查，确认网关可接收并转发测试请求；测试不得发送真实有副作用的业务请求。
7. 提交模式为 `gateway`。

如果任一步失败：

- 恢复开启前的 live 文件；
- 不提交 `gateway` 模式；
- UI 按第 5.3 节返回对应的 `phase`、`code`、`message` 和 `action`；
- 保留备份和诊断信息。

开启阶段不得把所有错误合并为一个“开启失败”提示：端口绑定问题使用 `GATEWAY_PORT_IN_USE`，看门狗启动问题使用 `WATCHDOG_TASK_START_FAILED`，运行时状态提交问题使用 `GATEWAY_RUNTIME_SYNC_FAILED`，健康检查问题使用 `GATEWAY_HEALTHCHECK_FAILED`，live 文件竞争修改使用 `LIVE_CONFIG_WRITE_CONFLICT`。如果实际原因无法分类，也必须返回 `GATEWAY_UNKNOWN_FAILURE` 并附带不含敏感信息的底层错误摘要。

网关模式下不能让 Codex 同时读取同一份 Codex-X 管理的 `model_instructions_file` 并由网关再次注入，否则会产生重复提示词。启用网关时应保存该文件的投影信息，并暂时移除或停用该字段；关闭网关时再恢复。

## 7. 网关模式下的 Provider 操作

操作链路：

```text
UI 选择 Provider
  -> 更新 Canonical State
  -> Gateway.set_provider
  -> 网关原子替换 active provider
  -> 下一条请求使用新 Provider
```

Provider 运行时状态至少包含：

```json
{
  "provider_id": "provider-b",
  "base_url": "https://provider-b.example/v1",
  "model": "model-b",
  "wire_api": "responses"
}
```

网关应根据 active provider 处理 `base_url` 和 `model`。如果请求体中已经有 `model`，应按照明确的覆盖策略处理，避免地址切换后仍使用旧 Provider 的模型。

API Key 默认不在 UI 和日志中展示。第一阶段可沿用 Codex 已发送的认证头；如果网关需要按 Provider 替换认证头，应使用受保护的本地存储，并限制控制 API 只能由本机桌面应用访问。

Provider 操作失败必须返回第 5.3 节中的 `PROVIDER_INVALID`、`PROVIDER_VERSION_CONFLICT` 或 `PROVIDER_RUNTIME_REJECTED`，并保留上一份已提交 Provider。不得出现“Provider 列表已切换但网关仍使用未知配置”的状态。

## 8. 网关模式下的指令提示词操作

操作链路：

```text
UI 选择或禁用模板
  -> 更新 Canonical State
  -> Gateway.set_instruction
  -> 网关更新注入规则
  -> 下一条请求立即生效
```

必须保留现有 `append` 和 `replace` 语义：

- `append`：保留原请求提示词，并追加 Codex-X 提示词；
- `replace`：替换约定的 system/developer/instructions 内容。

注入应基于解析后的 Responses JSON 结构，只处理约定路径，例如 `instructions`、`input[*].content[*].text` 以及 system/developer 消息。不得对整个 JSON 做无差别字符串替换，不得修改工具参数、文件路径或无关字段。

网关解析失败、字段不存在或未匹配时，应保持原请求并返回可诊断状态；不能静默破坏请求。

提示词操作失败必须区分模板不存在（`INSTRUCTION_TEMPLATE_NOT_FOUND`）、注入路径/模式不支持（`INSTRUCTION_PATH_UNSUPPORTED`）和运行时提交被拒绝（`INSTRUCTION_RUNTIME_REJECTED`）。失败时继续使用上一份已提交提示词；只有明确选择“禁用”且禁用操作成功后，才清除运行时提示词。

### 8.1 目标定位原则

网关收到的是 Codex 已组装后的 Responses JSON，不包含 `model_instructions_file` 的来源路径。因此目标定位必须依赖 JSON 的语义字段和消息角色，不得依赖全文搜索、提示词标题、文件名或某段自然语言内容。

默认候选优先级如下：

```text
1. 顶层 /instructions
2. input 数组中 role=developer 的消息
3. input 数组中 role=system 的消息
4. 没有安全候选时，append 创建顶层 /instructions
```

`user` 消息、工具调用参数、文件路径、图片/文件引用和未知角色永远不是自动注入目标。

### 8.2 目标探测与路径锁定

网关第一次看到启用提示词的请求时，执行 `resolve_instruction_target`：

1. 验证请求是允许处理的 Responses JSON，并读取顶层 `instructions`、`input` 和消息 `role`。
2. 将候选位置转换为 JSON Pointer，例如 `/instructions`、`/input/2/content`、`/input/2/content/1/text`。
3. 对每个候选记录角色、节点类型、content 形状和结构指纹。
4. 按优先级选择唯一目标；同一优先级存在多个候选且无法证明唯一性时，返回 `INSTRUCTION_TARGET_AMBIGUOUS`，不修改请求。
5. 成功选择后，把 `target_path`、`target_kind`、`role`、`shape` 和 `fingerprint` 写入网关运行时状态。

后续请求不得重新猜测目标。网关必须验证已锁定的 JSON Pointer 仍存在，节点类型、角色和结构指纹仍兼容；否则返回 `INSTRUCTION_TARGET_CHANGED`，原请求保持不变，并要求用户在页面中执行“重新探测目标”。

页面应显示探测结果：目标 JSON Pointer、消息角色、内容形状、最近一次匹配时间和当前提示词版本。这样用户能直接看到提示词实际写入的位置，而不是只看到“已启用”。

### 8.3 append 的结构化注入

append 模式只向已确认的 system/developer 语义位置追加：

- `/instructions` 是字符串时，在末尾添加两个换行和受管提示词；
- 消息 `content` 是字符串时，在该消息末尾添加带分隔的受管提示词；
- 消息 `content` 是数组时，在同一条 system/developer 消息末尾增加一个 `input_text` 文本节点，保留原节点顺序和元数据；
- 没有安全目标时，在请求顶层新建 `/instructions`，不得把文本塞入第一条 user 消息。

追加操作只改变目标节点，其他字段按原结构保留。若目标数组含未知文本节点类型，先验证其可安全保留；无法验证时返回 `INSTRUCTION_TARGET_UNSUPPORTED`。

### 8.4 replace 的结构化替换

replace 模式只替换已确认目标的文本，不进行全 JSON 字符串替换：

- `/instructions` 是字符串时，替换该字符串；
- system/developer 消息的 `content` 是字符串时，替换字符串内容，保留 `role`、`type` 和其他元数据；
- content 数组只有在全部节点都是受支持的文本节点时才允许替换；包含图片、文件或未知节点时返回 `INSTRUCTION_TARGET_UNSUPPORTED`；
- 没有安全目标时，replace 默认返回 `INSTRUCTION_TARGET_NOT_FOUND` 并透传原请求，不把 replace 静默降级成 append。

第一版不自动替换多个 developer/system 消息。多个候选必须由用户在页面中确认目标，或通过显式的 `target_path` 设置锁定。

### 8.5 与现有文件投影的对应

进入网关模式时，应先把当前 Codex-X 文件状态转换为运行时提示词：

- 当前为 replace：读取当前生效提示词内容，网关接管注入；为防止重复，暂时移除 live `model_instructions_file`，但保存原字段和文件快照；
- 当前为 append：提取 `AGENTS.md` 中 `AGENTS_MANAGED_BEGIN/END` 区块的内容，网关接管该区块；移除受管区块但保留用户其他 `AGENTS.md` 内容，用户原有 `model_instructions_file` 继续由 Codex 加载；
- 当前为 disabled：网关不注入，原有用户配置保持不变。

网关模式中切换模板只更新运行时状态和必要的中性文件投影，不把同一份内容同时留在 Codex 文件和网关中。关闭网关时再按当前 Canonical State 反向生成：append 写回受管 `AGENTS.md` 区块，replace 写回 `.md` 并设置 `model_instructions_file`，disabled 清除 Codex-X 管理内容。

### 8.6 幂等、透传和诊断

每次注入记录 `target_path`、请求结构指纹、提示词内容哈希和注入结果。重复请求或网络重试必须满足：

- 已存在同一份受管内容时不重复追加；
- 只替换网关自己管理的内容，不删除用户相同文本；
- JSON 解析失败、非 JSON 请求或不支持的结构默认透传原请求，并记录“未注入”的明确原因；
- 透传不等于成功，UI 必须显示 `injected=false`、原因码和上一份仍生效的提示词版本。

控制页面可提供“目标探测预览”，但只读取最后一条请求并展示候选路径，不重放请求、不发送修改后的请求。

## 9. 关闭网关流程

关闭网关是反向投影事务：

1. 暂停新的运行时设置提交，等待正在进行的控制操作完成。
2. 在持久化状态中原子提交 `desired_mode = direct` 并撤销看门狗运行资格；可保留 `watchdog_autostart` 作为下次开启网关时的用户偏好。
3. 通知当前看门狗退出，并停止计划任务的运行实例；轮询确认不会再拉起网关。
4. 从 Canonical State 读取当前 Provider 和提示词状态。
5. 将 Provider 投影回真实 `config.toml` 和 `auth.json`，将提示词投影回 Codex-X 管理的 `.md` 文件及 `model_instructions_file`。
6. 原子恢复真实 Provider 的 `base_url`，校验 TOML/JSON/提示词文件可解析且字段完整。
7. 所有回写和校验成功后才提交模式为 `direct`。

如果回写失败，必须保持 `gateway` 模式和网关运行状态，等待用户重试；不得把系统标记为已关闭。

关闭阶段必须把具体错误映射为第 5.3 节中的 `DISABLE_STATE_UNAVAILABLE`、`DIRECT_CONFIG_WRITE_FAILED`、`DIRECT_CONFIG_WRITE_CONFLICT` 或 `DIRECT_CONFIG_INVALID_AFTER_WRITE`。其中 `DIRECT_CONFIG_WRITE_CONFLICT` 只表示写入事务期间的并发竞争；网关运行期间用户早已修改的非托管字段应按字段所有权规则保留，不应因此拒绝关闭。UI 还应显示失败发生在哪个文件（`config.toml`、`auth.json` 或提示词文件）以及是否已经完成回滚；不得只显示“关闭失败”。完整覆盖边界见 `docs/GATEWAY_CONFLICT_RESOLUTION_DESIGN.md`。

关闭后，新建或重新打开的 Codex session 会按真实 Provider 运行。已经创建的 session 可能缓存旧的本地网关地址，因此不能承诺所有旧 session 立即变为直连。可以保留一个短暂的网关宽限期，避免旧请求突然失败。

## 10. 端口和看门狗

### 10.1 端口修改

端口变更不是普通文本设置：

- 网关关闭时，保存为下次启动配置；
- 检测到外部网关时，端口输入保持可编辑；不修改端口直接启动必须返回 `GATEWAY_PORT_IN_USE`，修改到空闲端口后才允许 Codex-X 建立自己的网关模式；
- 网关开启时，优先启动新端口并健康检查，再切换 Codex `base_url`，最后关闭旧端口；
- 若暂不实现无缝迁移，针对网关进程本身的端口迁移必须明确提示“重启网关后生效”；涉及 Codex live 配置读取的改动按下一条规则提示重启 Codex。
- 对需要 Codex 重新读取 live 配置的改动，独立网关页面必须使用红色边框提示“网关改动将在重启 Codex 后生效”，并将该提示保持在设置内容附近。

端口迁移必须区分 `NEW_PORT_IN_USE`、`NEW_PORT_NOT_READY` 和 `BASE_URL_SWITCH_FAILED`。迁移失败时保留旧监听、旧 `base_url` 和旧模式；如果旧监听也已失效，应进入 `degraded` 并显示旧端口失效的实际错误。外部网关不属于 Codex-X 的端口迁移对象，不能被关闭、迁移或纳入回滚。

### 10.2 看门狗职责

看门狗继续作为独立后台组件，不作为 Tauri 子进程：

- Codex-X 负责创建、启用、停用和检查 Windows 计划任务；
- 看门狗负责监控端口、启动网关、异常重启和写日志；
- Codex-X 关闭、重启或卡住时，看门狗仍可独立运行，不因 Tauri 窗口退出而自动停止；
- 网关进程崩溃或退出后，只要 `desired_mode = gateway` 且 `watchdog_desired = true`，看门狗就应按重试策略重新拉起网关；
- 看门狗每次循环都重新读取持久化的运行意图，不以“端口当前没有监听”作为单独的重启允许。当 `desired_mode = direct` 或 `watchdog_desired = false` 时，必须停止自动拉起和重启。
- 主窗口关闭沿用原项目托盘语义，仅隐藏窗口；托盘“退出 Codex-X”和应用重启触发 `RunEvent::ExitRequested`，只退出 Codex-X 进程，不撤销 watchdog intent、不停止网关、不恢复直连配置。
- 只有用户明确点击“停止网关”时才复用网关关闭流程恢复直连配置；恢复失败时重新激活 watchdog 并保留网关现场。操作系统强制终止或崩溃由下一次启动恢复流程接管。

启用“登录自启动”时应复用项目任务 `Codex-X Local Gateway`，不得重复创建同名或功能相同的项目任务。个人外部工具的任务 `Codex Responses Repair Gateway` 属于独立运行边界，Codex-X 不得查询后覆盖、停止、禁用或删除它。UI 应展示项目任务状态、项目端口状态和最近一次退出码。

Windows 任务必须通过 `schtasks /Create /XML <task-xml> /F` 创建，不得把完整 PowerShell action 放入 `/TR`。`schtasks` 对 `/TR` 有 261 字符上限，而 watchdog action 同时包含脚本、Python、upstream 和持久化 intent 路径，正常用户目录下就可能超过该上限。任务 XML 使用带 BOM 的 UTF-16LE 写入，并至少包含以下约束：

- `LogonTrigger` + `InteractiveToken`，仅在当前用户登录后启动；
- `<Hidden>true</Hidden>`，action 同时使用 `-NoProfile -NonInteractive -WindowStyle Hidden`；
- `StartWhenAvailable=true`、`MultipleInstancesPolicy=IgnoreNew`；
- `RestartOnFailure`，调度器失败后按一分钟间隔重试；
- action 显式传入 `-StateFile <gateway-mode/watchdog-intent.json>`，watchdog 每轮以持久 intent 决定是否允许拉起网关。

XML 中的脚本、解释器、upstream 和 intent 路径必须按 XML 文本节点规则转义。临时任务 XML 在创建成功或失败后都要删除；`schtasks` 返回失败时，后端应返回经过长度限制且不含凭据的 stdout/stderr 摘要，不能只显示统一的“无法创建”。重建任务前还必须导出原任务 XML 和运行状态；若创建或立即启动失败，恢复原任务定义和原运行状态，启动前无任务时删除本次新建任务。任务回滚失败必须追加 `WATCHDOG_TASK_ROLLBACK_FAILED`，不能静默忽略。应用内所有 `powershell.exe`、`schtasks.exe` 和 `taskkill.exe` 调用继续通过 Windows 隐藏进程封装执行。

网关模式下 `watchdog_desired = true`，唯一的 `Codex-X Local Gateway` 任务负责 Windows 登录触发和项目看门狗运行；登录后触发的任务仍必须先检查 `desired_mode` 和 `watchdog_desired`，不符合时立即退出。应用退出不会改变这两个值。

开启事务在写入任何 `gateway-mode` 文件前必须记录 `state.json`、`runtime-state.json`、watchdog intent 和原始文件备份的启动前快照。若 watchdog 任务创建/启动或更早阶段失败，应停止本次网关、恢复 live `config.toml` 和受管 `AGENTS.md`，再恢复这些快照；启动前不存在的文件应删除，启动前已存在的文件应恢复原字节。不得留下“`state.json` 指向新端口、live 配置仍指向旧端口”的半提交状态。

关闭网关的顺序必须固定为：

1. 原子撤销 watchdog intent 的重启资格；`gateway-mode/state.json` 在全部恢复成功前仍保留 `desired_mode = gateway`，以便失败时继续保护现场；
2. 通知当前看门狗退出，并禁用/停止计划任务的当前运行实例；
3. 轮询确认看门狗已退出且不会再拉起网关；
4. 再停止网关、回写 `config.toml`/`auth.json` 和提示词文件。

如果第 2–3 步超时，不得继续回写直连配置，应返回看门狗停止失败的明确原因并保持网关模式，以避免关闭后又被后台任务拉起。

看门狗相关异常必须区分任务不存在（`WATCHDOG_TASK_MISSING`）、任务已禁用（`WATCHDOG_TASK_DISABLED`）、脚本/解释器启动失败（`GATEWAY_PROCESS_START_FAILED`）和连续崩溃（`WATCHDOG_RESTART_LOOP`）。达到重启上限后必须停止无上限重试，保留最近一次退出码和 stderr 摘要。

## 11. 实时请求观测页面

实时请求观测是网关页面中的独立视图，只在网关实际运行时启用。它与“最后一条请求”脱敏快照使用不同的存储策略：最后一条快照用于诊断，观测列表用于短期实时分析；记录和计数只存在网关内存中，网关重启后清空。经过校验的观测设置属于本地配置，不包含请求正文，可在网关重启后保留。

网关页面设置的持久化边界必须明确区分：

| 设置/数据 | 持久化位置 | 网关重启后 | 程序显式退出后 |
| --- | --- | --- | --- |
| `capture_limit` | `runtime-state.json` | 恢复并重新校验 | 保留，下次网关启动继续使用 |
| Provider、模型、提示词、脚本启用/优先级 | `runtime-state.json` | 恢复 | 关闭网关时回写原生配置；失败则保留网关现场 |
| 监听端口 | 前端 `localStorage`；网关模式快照另存端口 | 页面使用已保存端口查询 | 保留前端端口偏好；实际网关由模式快照决定 |
| `capture_enabled` | 不持久化 | 默认恢复为 `false` | 不保留 |
| 观测列表、最后一条抓包、SSE 序号、淘汰/丢弃计数、脚本耗时窗口 | 仅网关进程内存 | 全部清空 | 全部清空 |

因此“关闭主窗口”和“退出程序”不能混为一谈：前者只是隐藏窗口，内存抓包继续累积；后者会停止网关，抓包不会写入 `runtime-state.json`，也不会在下次启动恢复。卡退或强制终止同样不保证保留抓包正文；仅持久化的设置和网关恢复现场会保留。

### 11.1 页面状态和操作

- `direct` 模式、网关未启动或网关正在关闭时，页面整体灰化，并显示“请先进入网关模式”；启动、暂停、清除、筛选、排序和上限设置均不可操作。
- 外部网关运行时，页面显示“外部网关运行中 / Codex-X 未接管”，按 `direct` 语义处理；监听端口和上游地址可以修改，启动按钮可用，停止、观测、脚本和其他运行时控制均不可操作。
- 网关启动中时，页面显示“网关启动中”，只允许查看启动阶段和明确的失败原因；健康检查通过后才解锁观测操作。
- 网关正常运行后，用户可显式点击“启动采集”或“暂停采集”。采集默认处于暂停状态，避免用户仅开启网关就产生额外观测开销。
- “清除”只清空当前保留的请求记录，不回退全局递增的请求序号；页面应显示当前保留数、保留上限以及累计淘汰/丢弃数。
- 提供成功/错误筛选。HTTP 状态码 `200-399` 为成功，`400` 及以上、连接错误、超时和网关内部处理错误为错误；筛选只改变视图，不删除记录。
- 每个表头均可点击排序，显示向上/向下箭头和当前排序列。排序在当前有界记录集上完成，不触发历史查询或磁盘扫描。

网关停止后应立即停止 SSE 推送并将页面灰化；已有记录可以只读查看，直到用户离开页面或执行清除。任何页面状态都必须以网关返回的实际状态为准，不能依据前端按钮的乐观状态判断采集已经启动。

### 11.2 列表数据和详情

列表行至少包含以下字段：

```text
id             网关内单调递增的请求序号
channel        当前 active provider 的网站/主机名
status_code    HTTP 状态码；未收到响应时显示 transport_error
model          请求中的模型，或网关最终采用的模型
request_time   请求总耗时，按网关计时结果显示
first_token    首字耗时，按 New API 的 frt/FirstResponseTime 语义显示
tokens         输入/输出/合计 Tokens
created_at     请求开始时间
```

`channel` 默认显示网关当前 Provider 的网站。网关无法知道 New API 内部实际选中的渠道时，不得伪造渠道名称，应显示上游主机并标记 `channel_source=provider_host`；若未来通过响应头获得内部渠道，再标记其来源。`request_time` 和 `first_token` 必须分别保留，不能用一个字段互相替代。当前实现优先读取 Responses 响应中的 `usage.total_tokens`，其次使用 `input_tokens + output_tokens`；响应未提供可识别字段时返回 `tokens_status=unavailable` 和 `tokens_error`，前端显示“不可用”及原因，不伪造估算值。若后续将 New API 的 tokenizer 模块编译为可调用组件，应保持该接口和不可用语义不变。

错误行使用浅红色底色，同时保留状态码或传输错误原因。点击任意行打开详情弹窗：发送包和接收包分别以格式化 JSON 展示，字段按统一脱敏规则处理；非 JSON 内容显示内容类型、长度和转义后的文本摘要，不尝试把任意文本伪装成 JSON。详情超过大小上限时必须显示 `OBSERVE_DETAIL_TRUNCATED` 及已保留字节数/原始字节数，不能静默截断。

启用用户脚本处理链后，每条真实请求的观测记录必须覆盖 Provider model 修改、提示词注入和脚本链，并设置可切换格式的阶段探针：

```text
客户端 -> [global_entry_probe] -> Provider model -> [provider_model_probe]
        -> 指令提示词注入 -> [prompt_injection_probe]
        -> raw-text 用户脚本链 -> [global_exit_probe]
        -> 固定 Provider 上游 -> [response_probe] -> 客户端
```

记录至少增加以下字段：

```text
script_chain_status   not_enabled / success / responded / error
global_entry_probe   客户端进入网关时的原始请求文本
provider_model_probe Provider model 修改后的请求文本
prompt_injection_probe 提示词注入后的请求文本，也是脚本链入口
global_exit_probe    所有脚本成功后的最终请求文本；未生成时为 null
response_probe       上游、脚本直接响应或网关错误响应的响应文本
probe_views          每个探针均支持 raw-text、request body JSON、response body JSON
script_failure        失败脚本 ID、名称、优先级、阶段和错误详情；成功时为 null
script_timings        各脚本本次耗时和脚本链总耗时
```

脚本异常、协议错误或启动失败时，必须在访问上游之前停止当前请求，返回用户可读错误（错误码为 `SCRIPT_EXECUTION_FAILED`），并将该请求标记为 `error`。此时仍记录各阶段探针、失败脚本、退出码/stderr 摘要和 `global_exit_probe=null`；`response_probe` 记录实际发给客户端的网关错误响应，不得伪造出口包或上游响应。脚本全部成功时记录出口文本；脚本返回 `respond` 时记录脚本生成的响应文本并标记 `responded`，不访问上游。测试请求不进入实时请求列表，也不计入真实调用耗时。

列表事件只传输行元数据和详情引用；点击行时再通过详情接口读取发送包和接收包，避免 SSE 携带大正文拖慢页面和网关。

### 11.3 有界保留和内存预算

观测记录必须使用有界内存环形队列，禁止使用无上限数组、全量日志订阅或把请求正文追加到长期集合。配置字段如下：

```text
capture_limit                 默认 100，用户可在本页面修改
capture_limit_min             1
capture_limit_max             配置化，初始建议 5000
capture_body_limit_bytes      单个发送包/接收包的最大保留字节数，配置化
observe_queue_capacity        旁路事件队列容量，配置化
sse_client_queue_capacity     每个 SSE 客户端待发送事件容量，配置化
```

`capture_limit` 只能是整数，且必须满足 `capture_limit_min <= capture_limit <= capture_limit_max`；不接受 `0`、负数、空值、非数字、小数或超过最大值的输入。后端和前端都要校验，后端返回明确的 `OBSERVE_CAPTURE_LIMIT_INVALID`、`OBSERVE_CAPTURE_LIMIT_TOO_LARGE` 或 `OBSERVE_GATEWAY_REQUIRED`，不能仅依赖前端禁用。

所有 `*_max`、字节上限和队列容量都必须是有限的正整数，并在网关启动及每次设置时校验；不得提供“无限制”或以特殊值绕过内存预算的配置。

新增记录时，如果数量超过上限，只从队首淘汰最旧记录，直到 `retained_count <= capture_limit`。因此默认配置始终只保留最近 100 条。设置上限的行为必须是原子的：

- 从 100 调小到 10：保存成功后立即裁剪到最近 10 条，并增加 `evicted_count`；
- 从 10 调大到 200：只放宽后续新增记录的容量，不伪造或恢复已经淘汰的历史记录；
- 设置失败：保留旧上限和旧记录，Canonical State 与网关运行时版本均不变化。

运行时状态至少返回：

```json
{
  "capture_enabled": true,
  "capture_limit": 100,
  "retained_count": 37,
  "evicted_count": 124,
  "capture_dropped_count": 2,
  "next_seq": 901
}
```

`retained_count` 是当前队列长度；`evicted_count` 和 `capture_dropped_count` 是本次网关运行期间的累计诊断计数，清除列表时不回退 `next_seq`，也不伪造历史记录。清除后 `retained_count=0`，新记录继续使用递增序号。网关重启后记录、计数和 `next_seq` 重新初始化，已保存的 `capture_limit` 等配置仍需经过边界校验后加载；`capture_enabled` 每次启动默认恢复为 `false`。

单条详情也必须受 `capture_body_limit_bytes` 限制，并在记录中保存 `truncated`、`original_bytes`、`retained_bytes` 和 `truncate_reason`。内存预算应按最坏情况计算并配置化，例如：

```text
总观测内存 <= capture_limit * (元数据 + 发送包上限 + 接收包上限)
             + observe_queue_capacity * 单事件上限
             + SSE 客户端/排序索引的固定开销
```

### 11.4 控制接口和事件流

在现有本地控制接口之外增加：

```text
GET  /observe/state
GET  /observe/requests?after=<seq>
GET  /observe/request/<id>
GET  /observe/events       (SSE)
POST /observe/start
POST /observe/pause
POST /observe/clear
PUT  /observe/settings     {"capture_limit": 100}
```

`/observe/requests` 只返回当前环形队列内仍保留的记录；`after` 使用单调序号，SSE 断线后可据此补齐仍存在的记录。序号已经被淘汰时，响应必须明确标记 `history_gap=true`，前端重新读取当前保留窗口。`/observe/request/<id>` 只允许读取仍在队列中的详情，已淘汰 ID 返回 `OBSERVE_REQUEST_EVICTED`，不进行磁盘回查。

所有接口均只监听 `127.0.0.1`，错误沿用统一结构并提供用户可读的 `message` 和 `action`。采集暂停、队列已满、详情超限等旁路问题分别记录 `OBSERVE_CAPTURE_DISABLED`、`OBSERVE_QUEUE_FULL`、`OBSERVE_DETAIL_TRUNCATED`；这些状态不能改写主请求的 HTTP 结果。

### 11.5 不影响网关转发的实现约束

- 在请求转发主路径上只做必要的时间戳和引用记录；请求/响应正文复制、脱敏、Tokens 计算和排序索引均投递到独立的非阻塞观测队列。
- 观测队列满时立即丢弃观测事件并递增 `capture_dropped_count`，不得等待消费者、阻塞上游读取或改变客户端响应。
- 每个 SSE 客户端必须使用固定容量的待发送队列；慢客户端达到容量后丢弃事件并提示客户端用 `after=<seq>` 恢复，不能为单个客户端无限累积内存或阻塞其他客户端。
- 发送包在转发前做一次受大小限制的旁路复制；接收包按响应块旁路复制并计算首字时间，原始块仍立即写回客户端。
- 观测 worker 崩溃、Tokens 模块异常或详情脱敏失败时，记录明确原因并透传原请求/响应；不得在主转发路径中重试或吞掉异常。
- 不轮询 New API 日志数据库，不同步写磁盘，不把完整请求正文写入普通网关日志。只有用户点击详情时才读取内存中的受限副本。

### 11.6 用户脚本链审计和最近 10 次耗时

`global_entry_probe` 在 Provider model 修改前复制客户端 raw-text；`provider_model_probe` 在模型覆盖后复制；`prompt_injection_probe` 在提示词注入后、第一支脚本启动前复制；`global_exit_probe` 在最后一支脚本成功返回后复制。所有探针都通过统一详情查看器提供 raw-text、请求体 JSON 和响应体 JSON 三种视图；为了定位错误，元数据仍须记录每个脚本的 ID、优先级、开始/结束时间、耗时和结果。

网关为每个脚本和整个脚本链维护独立的滚动统计：只保留最近 10 次 `mode=live` 的真实调用耗时（成功、`respond` 和异常调用均可计入，具体状态随样本保存），新样本到达后立即刷新平均值；样本不足 10 次时显示实际样本数。`mode=test` 的启用前测试、手动重试和启动自动测试一律排除，不得污染真实请求平均值。重启网关后滚动样本从空集开始。

脚本可以自由修改协议允许的请求方法、路径、请求头和正文，不设置业务字段白名单、业务筛选或功能正确性判断；但脚本输出不能改变最终上游目标。网关始终使用当前 Provider 的已提交目标地址，忽略输出帧中任何 `upstream_url`、`destination`、代理地址或同类字段。该约束只限制路由控制，不限制脚本对发送包内容的修改；脚本以用户权限运行，用户自行承担脚本行为后果。

## 12. 控制接口建议

控制接口只监听 `127.0.0.1`，建议提供：

```text
GET  /health
GET  /state
PUT  /state/provider
PUT  /state/instruction
PUT  /settings
GET  /events       (SSE)
POST /reload
POST /clear-last-request
GET  /scripts
POST /scripts/refresh
POST /scripts/{id}/test
POST /scripts/{id}/enable
POST /scripts/{id}/disable
PUT  /scripts/{id}/priority  {"priority": 10}
```

请求正文只在内存保留最后一条脱敏快照，默认不落盘。新请求覆盖旧快照；网关重启后快照清空。请求快照必须限制最大展示大小，并移除 Authorization、Cookie、API Key、设备标识等敏感数据。该快照与第 11 节的有界观测列表相互独立，不能用列表上限替代“最后一条”语义。

脚本控制接口返回脚本的发现状态、manifest 摘要、当前测试状态、测试版本指纹、启用状态、优先级和最近 10 次真实调用平均耗时。`POST /scripts/{id}/enable` 必须先执行一次 `mode=test` 自动测试，只有通过后才提交新的脚本链版本；测试失败返回 `SCRIPT_TEST_FAILED`，不改变原链配置。

`POST /scripts/{id}/test` 的请求体可省略：省略时使用第 16.3.1 节定义的默认测试包；指定自定义包时使用 `{"source":"custom","packet":{...}}`。接口响应必须返回本次实际使用的 `source`、入口帧详情引用、出口帧详情引用（如有）、测试状态、版本指纹和错误详情引用，便于弹窗与脚本卡片保持一致。详情接口必须能读取本次测试的具体输入/输出包内容，而不是只有摘要。

所有控制接口错误使用第 5.3 节的统一结构返回：连接失败或超时使用 `CONTROL_API_UNAVAILABLE`，字段缺失、类型错误或版本不兼容使用 `CONTROL_API_INVALID_REQUEST`。控制请求失败不得改变 Canonical State，也不得让 UI 乐观地显示未提交的运行时状态。

## 13. 状态机和并发约束

建议状态：

```text
direct
  -> enabling
  -> gateway
      +-> watchdog_stopped
      +-> watchdog_starting
      +-> watchdog_running
      +-> disabling -> direct
```

异常状态可标记为 `degraded`，但必须保留最后一个已提交的有效模式。进入 `degraded` 时必须同时记录第 5.3 节定义的 `phase`、`code`、`message`、`retryable` 和 `action`；没有明确原因的状态不得进入 UI，也不得写入“未知失败”之外的推测性原因。

约束：

- Provider、提示词和端口更新使用递增版本号；网关只接受不旧于当前版本的更新；
- 状态替换使用单次原子提交，避免 Provider 已切换而模型未切换；
- 网关请求处理和控制更新使用读写锁或等价机制；
- UI 显示以网关返回的已提交版本为准，不以本地表单提交成功作为最终依据；
- `direct` 模式不能进入 `watchdog_starting` 或 `watchdog_running`；只有先完成 `enabling -> gateway` 才能接受看门狗操作；
- `gateway -> disabling` 必须先撤销 watchdog intent，再停止看门狗和回写直连配置；全部恢复成功后才删除网关模式快照并进入 `direct`。Codex-X 进程退出不复用该清理流程；强制终止/崩溃则由下一次启动恢复接管。

## 14. 实施分期

### 第一阶段：最小闭环

- 网关总开关；
- 监听端口设置；
- 看门狗任务状态和登录自启动设置；
- 网关内存运行时 Provider/提示词状态；
- Provider 切换实时生效；
- 指令提示词 append/replace 实时生效；
- 用户脚本目录扫描、manifest 展示和手动刷新；
- 单脚本测试、失败重试和测试通过后启用；
- 按优先级串行执行脚本，脚本异常停止当前请求且不访问上游；
- 关闭网关后回写现有 Codex 配置。

### 第二阶段：可观测性

- 最后一条请求快照；
- 脱敏和大小限制；
- 实时请求观测页面、成功/错误筛选和逐列排序；
- 默认保留最近 100 条、可配置有界上限和缩容即时裁剪；
- 发送包/接收包详情、请求耗时、首字耗时和 Tokens；
- 脚本链入口/出口探针、脚本异常详情和最近 10 次真实调用平均耗时；
- SSE 实时状态；
- 替换前后差异预览；
- 最近错误和重启次数。

### 第三阶段：增强安全与迁移体验

- 认证信息按 Provider 的安全运行时切换；
- 端口无缝迁移；
- 关闭网关宽限期；
- 更细粒度的 JSON 路径规则和审计记录。

## 15. 验收标准

实施前和基础网关变更后的旁路验证步骤见：[本地网关旁路测试方案](GATEWAY_BYPASS_TESTING.md)。该方案定义了 MockServer 假上游、mitmproxy 流脚本和显式批准的真实上游烟雾测试，均使用独立临时端口，不修改当前 Codex 配置或 `127.0.0.1:8787` 网关；完整集成验收仍需按本节标准在独立配置环境中执行。

至少覆盖以下回归场景：

1. 网关关闭时，Provider 和提示词行为与现有 Codex-X 一致。
2. 开启网关后，Codex `base_url` 指向本地端口且网关状态为已就绪。
3. 网关模式下切换 Provider，下一条请求使用新地址和模型，无需重启 Codex。
4. 网关模式下切换、追加和替换提示词，下一条请求结果符合预期且不重复注入。
5. 关闭网关后，当前 Provider、认证和提示词正确回写到原有配置文件。
6. 开关过程任一步失败都能回滚，且不会留下半切换状态。
7. 看门狗任务只存在一份，网关模式启用时任务自动登录自启动。
8. 请求快照只保留最后一条，重启后清空，敏感字段不出现在 UI、日志或快照中；它与观测列表相互独立。
9. 实时观测默认暂停；启动采集后默认保留最近 100 条，超过上限只淘汰最旧记录且内存占用保持有界。
10. 用户可设置合法的 `capture_limit`；缩小上限立即裁剪，放大上限不恢复历史，非法值失败并显示明确原因。
11. 实时观测页面在网关未启动时灰化并提示“请先进入网关模式”；运行时可启动/暂停/清除、按成功/错误筛选、逐列排序，错误行显示浅红底色。
12. 点击观测行可读取受脱敏和大小限制保护的发送包/接收包 JSON；淘汰记录、详情截断和 Tokens 不可用均显示明确状态，不影响主请求转发。
13. 观测队列满、worker 异常或 Tokens 计算失败时只丢弃/降级观测数据，原始请求和响应仍按原路径完成，不因观测阻塞。
14. 并发 Provider/提示词更新不会产生旧版本覆盖新版本。
15. 旧 session 的缓存行为在 UI 中有明确提示，不把“网关热更新”误报为 Codex 全部 session 已重载配置。
16. 网关为关闭时，看门狗和登录自启动按钮均禁用，禁用原因可见且后端拒绝违规操作。
17. 网关开启后自动启用看门狗和登录自启动；关闭 Codex-X 主窗口只隐藏到托盘，退出或重启 Codex-X 也不改变网关状态。
18. 模拟网关崩溃，在 `desired_mode = gateway` 且 `watchdog_desired = true` 时可被自动恢复；连续崩溃超过上限时停止重试并展示明确原因。
19. 只有显式关闭网关时才撤销看门狗重启资格、停止任务并回写直连配置；清理失败时重新激活 watchdog 并保留网关模式。
20. Windows 重新登录时，仅在持久化 `desired_mode = gateway` 且 `watchdog_desired = true` 时触发看门狗；`direct` 模式不会自动拉起。
21. 网关页面对需要重启 Codex 才能读取的改动显示红色边框提示，并明确说明“网关改动将在重启 Codex 后生效”。
22. 手动刷新能够发现新增脚本；无效 manifest 只使对应脚本标记为不可用，并显示具体错误。
23. 脚本卡片显示名称、简介、入口、出口协议、优先级和测试状态；每次测试完成后，无论成功还是失败，`passed`/`failed` 旁均可打开“测试详情”查看具体输入包和输出包；失败时额外显示“测试失败”及“重试”。
24. 测试数据/判断只校验协议帧、编码、字段类型和包结构完整性，不校验业务字段是否达到某种功能结果；脚本可以修改任意协议允许的请求字段。
25. 测试未通过、测试结果过期或启用自动测试失败时，启用控件保持禁用；修改脚本后可通过“重试”生成新的测试结果，测试通过后才能启用。
26. 多个已启用脚本严格按优先级串行执行，同优先级排序稳定；脚本输出不能改变最终 Provider 上游地址，网关始终使用已提交 Provider 目标。
27. 脚本异常、退出码非零、超时或 raw-text 输出无效时，当前请求立即停止且不访问上游，客户端收到 `SCRIPT_EXECUTION_FAILED` 的可读错误；实时观测记录已生成阶段探针、失败脚本、错误详情和 `global_exit_probe=null`。
28. 脚本链全部成功时，实时观测记录入口包、出口包和返回包；脚本直接响应时记录脚本返回包并标记 `responded`。
29. 测试通过但真实请求因特殊结构失败时，观测列表显示错误行；点击该请求可读取入口包、失败脚本和具体错误，并明确标记未生成出口包。
30. 每个脚本及脚本链的最近 10 次真实调用平均耗时持续刷新，测试/重试耗时不计入；网关重启后样本清空并重新累计。
31. 点击脚本“测试”按钮打开弹窗，默认显示完整默认 raw-text 测试请求；切换到自定义 raw-text 可编辑并执行，能够恢复默认文本，并显示本次测试来源。测试完成后，脚本卡片旁的“测试详情”可再次打开本次输入文本、输出文本和结果。
32. 默认测试文本包含嵌套 JSON、非敏感测试标记且不包含真实认证、用户数据或最终上游地址；脚本通过 `CODEX_X_SCRIPT_MODE=test` 获知测试模式。
33. 自定义测试只做 raw-text 请求/响应解析、退出码和结构校验；非法文本在弹窗内显示错误且不启动脚本，合法文本不因业务字段或功能结果不同而失败；成功和失败详情均能查看实际输出或明确的 `null` 状态。
34. 脚本页面最上方持续显示“用户脚本仅在网关模式下生效。当前为直连模式时，脚本不会处理 Codex 的真实请求。”；直连模式下的查看、刷新和测试不修改或拦截真实请求，也不写入真实请求观测和耗时统计。
35. 外部网关运行且 `managed_by_codex_x = false` 时，页面显示“外部网关运行中 / Codex-X 未接管”，仍按直连模式处理；端口可编辑，启动按钮可用，停止、观测和脚本控制不可用。
36. 外部网关占用当前端口时，直接启动 Codex-X 返回 `GATEWAY_PORT_IN_USE`；修改到空闲端口后启动成功，外部网关原端口继续运行且不受影响。
37. Codex-X 退出或重启后，外部网关仍可被识别为外部网关；`managed_by_codex_x` 不依赖 Tauri 进程是否仍持有子进程句柄。
38. 网关进程运行、Codex-X 管理网关但 live `config.toml` 指向外部网站时，返回 `codex_route_active = false`，页面显示“网关运行中但未接入 Codex”；保留网关和 watchdog，不覆盖外部配置，且 Provider/提示词热更新、实时观测和用户脚本控制均不可用。

## 16. 用户脚本处理器协议（旧 JSONL 版本，已废弃）

本节保留历史 JSONL 设计用于迁移参考，不再是当前调用协议。当前唯一协议见本文第 17 节 raw-text 规范。脚本由用户自行编写并以当前用户权限运行，不提供沙箱、业务字段白名单、业务筛选或功能正确性限制；用户自行承担脚本改包、阻断请求以及脚本依赖带来的后果。

### 16.1 发现目录和 manifest

网关只扫描约定的用户目录，建议为：

```text
C:\Users\<user>\.codex-x\gateway-tools\<script-id>\
  manifest.json
  main.py                  # 入口示例，实际由 manifest 指定
  tests\*.json             # 可选测试夹具
```

用户点击“刷新脚本”时重新扫描该目录；实现也可以提供文件变更提示，但不得因为发现新脚本就自动启用。每个脚本目录必须包含 `manifest.json`，至少声明：

```json
{
  "protocol_version": 1,
  "id": "my-rewriter",
  "name": "My Rewriter",
  "description": "Rewrite request packets",
  "version": "1.0.0",
  "entry": {
    "program": "python",
    "args": ["main.py"]
  },
  "exit": {
    "format": "jsonl",
    "one_frame_per_request": true
  },
  "directions": ["request"],
  "test": {
    "fixtures_dir": "tests"
  }
}
```

`id` 在目录和控制接口中唯一；`name`、`description` 用于脚本卡片；`entry` 是网关启动脚本的程序和参数，工作目录固定为该脚本目录；`exit` 声明标准输出的出口帧格式。manifest 缺失、无法解析或字段类型错误时，仅该脚本进入 `SCRIPT_MANIFEST_INVALID`，其他脚本仍可用。

### 16.2 入口帧和出口帧

网关以 UTF-8 JSONL 调用脚本：一次调用向 stdin 写入一个入口帧并以换行结束，脚本必须向 stdout 写出一个出口帧并以换行结束；调试信息只能写 stderr。网关不得把最终上游地址放入入口帧，也不接受脚本通过出口帧选择上游地址。入口帧示例：

脚本只有在 `gateway_mode = gateway` 且网关运行时健康就绪后才会接入真实请求路径。`direct` 模式永远不调用脚本链；即使脚本已测试通过或配置为启用，直连请求也必须直接走现有 Codex Provider 路径。`mode=test` 是唯一例外，仅用于用户主动测试脚本，不代表脚本已对真实请求生效。

```json
{
  "protocol_version": 1,
  "request_id": "req-123",
  "mode": "live",
  "direction": "request",
  "method": "POST",
  "path": "/v1/responses",
  "headers": {"content-type": "application/json"},
  "body_base64": "eyJtb2RlbCI6InRlc3QtbW9kZWwifQ==",
  "body_json": {"model": "test-model"}
}
```

`body_base64` 是原始正文的唯一字节来源；正文可为任意字节。`body_json` 仅在正文可解析为 JSON 时附带，脚本可以只修改 `body_base64`，也可以同时更新 `body_json`。`headers`、`method`、`path` 和正文没有业务字段限制，脚本可按自身逻辑任意修改。

出口帧的 `action` 决定网关后续动作：

```json
{"protocol_version":1,"action":"forward","packet":{...}}
{"protocol_version":1,"action":"respond","response":{"status_code":200,"headers":{},"body_base64":"..."}}
{"protocol_version":1,"action":"drop","message":"filtered by user script"}
{"protocol_version":1,"action":"error","code":"USER_RULE_FAILED","message":"cannot parse special shape"}
```

- `forward`：将 `packet` 作为下一支脚本的入口；脚本链结束后作为发送包访问固定 Provider 上游。
- `respond`：脚本直接生成客户端响应，不访问上游；响应包进入实时观测的 `response_packet`。
- `drop`：脚本主动终止当前请求；网关返回可读的脚本终止错误并记录观测，不访问上游。
- `error`：脚本明确报告处理失败，等价于 `SCRIPT_EXECUTION_FAILED`，停止当前请求。

出口帧中不得出现用于路由的 `upstream_url`、`destination`、代理地址或同类控制字段。即使脚本输出这些未知字段，网关也只能忽略并记录协议诊断，最终目标仍由当前 Provider 的已提交地址决定；这些字段不能改变路由。未知的业务字段和正文内容则原样保留。

脚本退出码非零、无出口帧、多帧、非 UTF-8、JSONL 无法解析、`protocol_version` 不匹配、动作或必需字段缺失，均视为脚本执行失败。网关必须保存退出码、stderr 摘要和协议错误，停止请求并返回 `SCRIPT_EXECUTION_FAILED`；不得把半成品包发送给上游。

### 16.3 测试和启用状态

每个脚本卡片维护 `not_tested`、`testing`、`passed`、`failed` 四态，并显示测试版本指纹。测试运行使用 `mode=test` 和 `tests` 目录中的夹具；没有夹具时由网关生成包含嵌套 JSON、非 JSON 正文和较大正文的确定性测试包。测试判断只做协议和包结构完整性校验：能否完成 JSONL 帧交换、正文 Base64 是否可解码、方法/路径/headers 类型是否正确、出口动作和必需字段是否匹配、是否意外提供路由控制字段。不得检查脚本是否实现某项业务功能，也不得因为业务字段缺失或值不同而判失败。

每次测试完成后都必须保存一份最近测试详情，并在脚本卡片的 `passed` 或 `failed` 状态旁提供“测试详情”按钮。点击后打开详情弹窗，至少分为“输入包”“输出包”“测试结果”三个区域：

- “输入包”显示实际发送给脚本的完整入口帧，包括 `mode=test`、测试来源和 `body_base64`；用户可以切换格式化 JSON 与原始文本视图。
- “输出包”显示脚本实际写出的完整出口帧。脚本没有写出有效出口帧时显示 `null`，并说明是无输出、协议解析失败还是进程异常；不得用输入包代替输出包。
- “测试结果”显示 `passed`/`failed`、测试时间、脚本版本指纹、退出码、stderr 摘要和协议/结构校验结果。成功时错误字段显示“无错误”，失败时显示可定位的具体错误。

测试详情必须同时适用于默认测试包和自定义测试包，并标注 `source=default` 或 `source=custom`。详情内容遵循第 11 节的大小上限：未超限时显示具体完整包内容，超限时必须同时显示 `original_bytes`、`retained_bytes` 和截断原因，不能静默改成摘要。详情至少保留到下一次测试或脚本版本指纹失效；版本失效后仍可打开旧详情，但必须显著标记“已过期”，且不能再用于启用判断。

测试失败时 UI 还必须显示固定文案“测试失败”，其后紧邻“重试”按钮；“重试”生成新的输入/输出包和测试详情。脚本文件或 manifest 的内容/版本指纹发生变化后，原 `passed` 结果立即失效并回到 `not_tested`，该脚本不得继续留在可执行链中。

启用流程必须执行一次 `mode=test` 自动测试；只有自动测试通过且结果对应当前版本指纹时，才能把脚本加入已提交链配置。测试失败、结果过期或自动测试失败时，启用按钮保持禁用，原有脚本链继续运行；用户修改脚本后可点击“重试”，无需删除脚本或重启网关。多脚本按整数 `priority` 升序串行执行，同优先级按脚本 ID 稳定排序。

### 16.3.1 默认测试包和自定义测试包

“测试”按钮打开的弹窗必须内置以下默认测试包。该包是协议级基线，不代表任何业务功能预期；用户可以在弹窗中查看完整内容、复制内容或切换到自定义测试包。

默认入口帧（`source=default`）：

```json
{
  "protocol_version": 1,
  "request_id": "test-default-001",
  "mode": "test",
  "direction": "request",
  "method": "POST",
  "path": "/v1/responses",
  "headers": {
    "content-type": "application/json",
    "x-codex-script-test": "1"
  },
  "body_base64": "eyJtb2RlbCI6ImNvZGV4LXgtdGVzdC1tb2RlbCIsImlucHV0IjpbeyJ0eXBlIjoibWVzc2FnZSIsInJvbGUiOiJ1c2VyIiwiY29udGVudCI6W3sidHlwZSI6ImlucHV0X3RleHQiLCJ0ZXh0IjoiZ2F0ZXdheSBzY3JpcHQgdGVzdCJ9XX1dLCJtZXRhZGF0YSI6eyJnYXRld2F5X3NjcmlwdF90ZXN0Ijp0cnVlfX0=",
  "body_json": {
    "model": "codex-x-test-model",
    "input": [
      {
        "type": "message",
        "role": "user",
        "content": [
          {
            "type": "input_text",
            "text": "gateway script test"
          }
        ]
      }
    ],
    "metadata": {
      "gateway_script_test": true
    }
  }
}
```

默认包的 `body_base64` 必须是 `body_json` UTF-8 紧凑序列化后的逐字节 Base64；实现不得把两个字段生成成互不对应的内容。默认包固定包含：POST 方法、Responses 路径、JSON 请求头、可解析的嵌套 JSON、普通用户消息、布尔 metadata 和稳定测试标记。默认包不包含真实 Provider 地址、认证值、Cookie、用户数据或会产生副作用的工具调用。

默认测试至少执行以下协议级检查：

1. 入口帧是单个 UTF-8 JSON 对象，`protocol_version`、`request_id`、`mode`、`direction`、`method`、`path`、`headers` 和 `body_base64` 类型正确。
2. `body_base64` 可以解码为原始正文；若提供 `body_json`，其 UTF-8 序列化内容必须与解码正文一致。
3. 脚本返回单个出口帧，动作是 `forward`、`respond`、`drop` 或 `error` 之一，动作对应的 `packet`/`response`/`message` 字段类型正确。
4. 测试结果只判断协议帧和包结构是否完整，不判断模型是否存在、字段是否满足业务语义、脚本是否完成用户期望的改写。

自定义测试包：

- 用户在弹窗中编辑完整入口帧 JSON，点击“运行测试”后以 `source=custom` 提交；自定义包不覆盖或修改默认包，关闭弹窗后默认包仍可通过“恢复默认”找回。
- 自定义包可以使用任意方法、路径、请求头、正文和嵌套结构，也可以提供非 JSON 正文（此时只保留 `body_base64`，不得伪造 `body_json`）。网关不做业务字段筛选。
- 自定义包必须通过入口帧协议校验；JSON 非法、Base64 非法、字段类型错误或包含最终上游地址控制字段时，弹窗在对应字段/行列显示错误，不启动脚本调用。
- 自定义包仅用于当前一次测试，默认不持久化正文；如果用户选择“保存为夹具”，只保存到当前脚本的 `tests/` 目录并显示文件名、版本指纹和保存时间。保存失败不影响脚本测试结果。
- 测试详情弹窗展示的是实际提交给脚本的完整入口帧和脚本实际返回的完整出口帧；大包遵循第 11 节详情上限并明确显示截断，不得用摘要替代协议判断。无论脚本成功、`respond`、`drop`、`error` 还是进程异常，都必须显示本次实际输出或明确的 `null` 状态。

### 16.4 运行时审计契约

实时观测把整个请求链视为一个宏观模块，依次采集 `global_entry_probe`、`provider_model_probe`、`prompt_injection_probe`、`global_exit_probe` 和 `response_probe`。所有探针都保存 raw-text，并由同一详情查看器切换 raw-text、请求体 JSON 和响应体 JSON；脚本中间包不要求作为独立记录，但必须记录脚本 ID、优先级、耗时和结果。异常请求必须保留已生成的阶段探针和具体错误；若脚本未成功输出，`global_exit_probe` 必须为 `null`。

网关分别维护每个脚本和整个脚本链最近 10 次 `mode=live` 调用的耗时滚动窗口，成功、直接响应和异常调用均按实际结果记录；窗口更新后立即刷新平均值，测试和重试耗时永不计入。观测记录仍遵循第 11 节的有界保留、脱敏和详情展示策略；这些展示/存储处理不得改变脚本实际收到或发出的包，也不得篡改入口包、出口包、返回包或错误事实。

## 17. raw-text 用户脚本协议（当前版本）

### 17.1 输入和输出

每次请求为每个脚本启动一个进程。网关把完整 raw-text 请求按原始字节写入 `stdin`，写完后关闭 `stdin`；脚本读到 EOF 后把完整的修改后请求 raw-text 写入 `stdout`。一次调用只能输出一份协议文本，调试信息只能写入 `stderr`。

请求文本格式与 Reqable 抓包一致：

```text
METHOD /path?query HTTP/2\r\n
header-name: header-value\r\n
content-type: application/json\r\n
content-length: <body bytes>\r\n
\r\n
<request body bytes>
```

响应文本使用 `HTTP/1.1 <status> <reason>` 状态行、响应头、空行和响应正文。正文可以是 JSON 或任意二进制字节；网关不得因为正文不是 JSON 就替换、丢弃或拒绝它。脚本通过环境变量 `CODEX_X_SCRIPT_MODE`（`live`/`test`）、`CODEX_X_SCRIPT_REQUEST_ID` 和 `CODEX_X_SCRIPT_DIRECTION` 获取调用元数据，元数据不混入 raw-text。

### 17.2 修改权限和固定路由

用户脚本可以修改请求行、路径、查询参数、HTTP 版本、所有请求头和完整正文，包括 Provider `model`、提示词内容以及其他业务字段。用户脚本链位于 Provider model 修改和提示词注入之后，因此脚本输出拥有最高的请求内容优先级，可以覆盖这两步的结果。网关只在发送前重新计算 `Content-Length`、整理 hop-by-hop headers 等传输层字段。

最终 Provider 目标仍由网关运行时配置固定决定。脚本可以修改 raw-text 中的 `Host`，但不能借此改变网关实际建立连接的 Provider 地址。

### 17.3 退出码控制语义

```text
0       stdout 是修改后的请求 raw-text，继续 forward
10      stdout 是完整 HTTP 响应 raw-text，直接响应客户端
11      丢弃请求，stderr 为可展示原因
其他非零  执行失败，stderr 为错误详情并返回 SCRIPT_EXECUTION_FAILED
```

脚本超时、退出码异常、stdout 为空、请求/响应 raw-text 无法解析或协议字段不完整时，当前请求立即停止，不得把半成品发送给上游。多脚本按 priority 升序串行执行，同优先级按脚本 ID 稳定排序；启用仍必须经过测试通过和版本指纹校验。

### 17.4 探针和详情视图

观测记录包括 `global_entry_probe`、`provider_model_probe`、`prompt_injection_probe`、`global_exit_probe` 和 `response_probe`。所有探针使用同一详情查看器，用户可以在页面直接切换 `raw-text`、`request body JSON` 和 `response body JSON`，不根据探针名称限制视图；不可解析或不存在的 JSON 视图显示为不可用。raw-text 仍遵循统一脱敏和大小上限，超限必须显示 `OBSERVE_DETAIL_TRUNCATED`、`original_bytes` 和 `retained_bytes`。

### 17.5 个人 tool-call ID 修复脚本

tool-call ID 修复是当前用户的个人本地用户脚本，不是仓库内置功能。它必须遵循 manifest、测试、启用、priority、超时和失败阻断规则，并实现：仅将 `custom_tool_call` 的 `fc_` 改为 `ctc_`；`tool_search_call`、`function_call` 和历史错误消息不改。脚本只存放在本机用户脚本目录，不随项目发布、不加入 Git，也不能作为其他用户安装后的依赖。
