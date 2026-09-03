# 网关停止时的配置冲突解决设计

## 1. 文档目的

本文汇总 Codex-X 本地网关与 config.toml 配置写入相关的现状、问题和改进方案，重点讨论：

- 为什么网关需要保存启动前配置和投影配置；
- 网关写入流程与其他模块写入流程的区别；
- 当前整文件 SHA-256 冲突保护的设计缺陷；
- 无需提供 Git diff 界面，而是通过明确的字段所有权规则确定性写回；
- 如何保证冲突处理期间不丢配置、不误停网关，并且最终配置仍然有效。

本文对应当前实现：停止事务采用字段所有权写回，不提供 diff 选择界面；实现细节和测试边界见同步文档。

## 2. 当前架构

### 2.1 直连模式与网关模式

直连模式下，Codex 直接使用 config.toml 中的真实 Provider：

~~~text
Codex -> config.toml/auth.json -> 真实 Provider
~~~

网关模式下，Codex 的 live 配置被投影到本地网关：

~~~text
Codex -> 127.0.0.1:<port>/v1 -> 本地网关 -> 真实 Provider
~~~

网关运行时负责保存当前 Provider、模型、认证转发策略、提示词状态和请求处理状态。此时 config.toml 中的 base_url 主要是“路由投影”，不再代表真实远端 Provider。

### 2.2 网关启动时的处理

网关启动流程位于 apps/desktop/src-tauri/src/gateway.rs：

1. 读取当前 config.toml、auth.json、提示词文件和 AGENTS.md；
2. 将启动前配置保存到 gateway-mode/；
3. 把 config.toml 投影到本地网关地址；
4. 启动网关进程和 watchdog；
5. 将 Provider 与提示词同步到网关运行时；
6. 保存网关模式元数据。

关键位置：

- 启动前配置快照：gateway.rs 中的 original_config、original_auth；
- 配置投影：project_config()；
- 模式元数据：GatewayModeMeta；
- 投影配置哈希：projected_config_sha256。

### 2.3 网关停止时的处理

停止流程目前大致为：

1. 读取持久化网关模式；
2. 验证运行中的网关仍由 Codex-X 管理；
3. 验证当前 config.toml 的 SHA-256 等于启动后投影配置的 SHA-256；
4. 从网关运行时读取 Provider 和提示词状态；
5. 终止 watchdog 和网关进程；
6. 将启动前配置恢复，并写回当前运行时 Provider；
7. 删除网关模式快照和 watchdog intent。

当前的整文件冲突判断位于：

~~~text
apps/desktop/src-tauri/src/gateway.rs:1505
~~~

如果哈希不一致，流程返回：

~~~text
DIRECT_CONFIG_WRITE_CONFLICT:
config.toml 已被外部修改，拒绝覆盖并保持网关模式
~~~

## 3. 为什么网关需要额外的快照设计

其他模块的配置修改通常是短事务：读取文件、修改文件、校验文件未被并发修改、原子写入，操作很快完成。

网关投影则是长事务：

~~~text
启动网关 -> 投影 config.toml -> 网关运行数小时 -> 停止网关 -> 恢复直连配置
~~~

Codex-X 进程退出后，watchdog 仍可能继续维持网关。因此网关不能只依赖内存中的回滚对象，必须把启动前快照和运行模式写到磁盘，供重启、异常退出和后续停止恢复使用。

所以，网关的“持久化快照”是长事务生命周期带来的特殊需求；它不是另一套完全独立的底层文件写入技术。

## 4. 其他模块的文件修改设计

普通 Provider、提示词、备份和恢复模块主要复用 live_config：

- acquire_live_config_lock()：防止两个 Codex-X 操作同时写 live 配置；
- read_file_snapshot()：读取写入前快照；
- atomic_write_if_unchanged()：只有当前文件仍等于预期快照时才写入；
- apply_file_change()：记录一次文件变更；
- restore_file_snapshot_if_unchanged()：在失败时安全回滚；
- rollback_after_failure()：协调配置、认证文件和数据库回滚。

Provider 模块的典型流程位于：

~~~text
apps/desktop/src-tauri/src/providers/live.rs
~~~

这些操作的特点是：

~~~text
短事务 + 操作完成后立即结束 + 冲突时回滚当前操作
~~~

网关流程也使用同一组底层原子写入和 live lock，但还增加了：

~~~text
长事务 + 持久化原始快照 + watchdog 生命周期 + 停止时恢复
~~~

因此两者不是“完全不统一”，而是“底层写入机制复用，上层生命周期和恢复语义不同”。

## 5. 当前整文件 SHA-256 设计的缺陷

整文件哈希的优点是简单、保守、容易证明不会静默覆盖未知修改。

但它把所有变化都视为同一种冲突。例如用户在网关运行期间只修改：

~~~toml
model = "gpt-5.6"
approval_policy = "never"
~~~

或者调整 mcp_servers、项目配置、注释和其他与网关无关的字段，停止时仍会因为整文件哈希变化而拒绝恢复。

这会带来三个问题：

1. 用户可能无法正常停止网关；
2. 用户无法知道到底是哪一个字段产生了冲突；
3. 为了避免覆盖一个字段，整个网关生命周期都会被锁定在网关模式。

如果简单删除哈希检查，风险更大：停止流程基于启动前原始文件构造恢复配置，用户在运行期间对其他字段的修改可能被整文件覆盖。

因此问题不在于“要不要保护”，而在于保护粒度过粗。

## 6. 目标设计：按字段所有权确定性写回（不做 diff 界面）

当前阶段不实现 Git diff 或逐字段选择界面。停止网关时采用明确的字段所有权规则，避免整文件盲目覆盖：

~~~text
网关托管字段   -> 以网关运行时状态为准
非网关托管字段 -> 保留停止时磁盘上的当前值
~~~

启动时仍然记录原始快照和本次实际投影路径，但它们主要用于恢复、异常处理和安全校验，不再用“整文件哈希变化”阻止正常停止。

### 6.1 config.toml 的写回规则

停止时先读取当前 `config.toml`，以当前文件为基础生成结果，然后只覆盖网关明确托管的完整 Provider 配置组：

- `model_provider`；
- `model`；
- 顶层 `base_url`；
- `model_providers.<实际 provider id>.name`；
- `model_providers.<实际 provider id>.base_url`；
- `model_providers.<实际 provider id>.wire_api`；
- `model_providers.<实际 provider id>.requires_openai_auth`。

真实 Provider URL 必须覆盖回非网关地址，不能留下即将停止的 `127.0.0.1:<port>/v1`。以下非托管内容保留停止时文件中的当前值：

- `approval_policy`；
- `sandbox_mode`；
- `mcp_servers`；
- `projects`；
- 其他未知字段、注释和用户格式。

### 6.2 auth.json 的写回规则

不使用运行时生成的 JSON 整体覆盖 `auth.json`。只覆盖当前 Provider 明确托管的认证键，例如 `OPENAI_API_KEY` 或该 Provider 声明的认证字段；其他键从停止时当前文件保留。

官方登录状态、刷新令牌和未知认证字段不能因为停止网关而被清除。认证内容在 UI、日志和备份清单中必须脱敏。

### 6.3 外部提示词文件和 AGENTS.md

- Replace 模式：网关运行时提示词覆盖 Codex-X 管理的目标 Markdown 文件，并恢复 `model_instructions_file`；
- Append 模式：只替换 `AGENTS.md` 中 Codex-X 的 BEGIN/END 管理区块，区块外用户内容原样保留；
- 禁用提示词：只移除 Codex-X 管理的提示词指针、目标文件或管理区块，不能删除无关用户内容；
- 网关临时生成的提示词文件，如果启动前不存在，停止时按明确策略保留或删除，并记录在停止前备份中。

### 6.4 用户手动修改托管字段时的规则

网关模式期间真正生效的是网关运行时状态。因此用户手动修改以下托管内容，不作为停止冲突：

- Provider 和模型；
- 真实 Provider URL；
- 当前 Provider 的托管认证键；
- Codex-X 管理的提示词文件；
- `AGENTS.md` 中 Codex-X 的管理区块。

停止时这些内容由网关运行时确定性覆盖。产品界面必须提前说明这一行为，并建议用户在网关模式中通过 Codex-X 修改这些设置。

## 7. 停止流程

停止网关不再需要单独的 diff 预览或用户选择命令，但仍必须按照安全顺序执行：

~~~text
1. 获取 live config lock
2. 读取当前 config.toml、auth.json、提示词文件和 AGENTS.md
3. 创建停止前备份
4. 读取网关运行时最终 Provider、认证和提示词状态
5. 按字段所有权生成候选文件
6. 校验候选配置完整且不再指向即将停止的本地网关
7. 确认源文件从读取后没有被其他程序再次修改
8. 禁用 watchdog 的重启资格并停止 watchdog
9. 停止网关
10. 原子写入全部直连配置
11. 清理网关模式快照和 intent
~~~

如果步骤 7 发现并发修改，不能覆盖刚写入的新内容；应中止停止操作、恢复 watchdog，并保持网关模式。这里防止的是“写入事务执行期间的并发修改”，而不是长期锁死用户在网关运行期间的历史修改。

步骤 8 以后发生失败时，不能只保留 `desired_mode = gateway` 的元数据而让网关进程实际处于停止状态。实现必须执行补偿恢复：回滚已经写入的文件、恢复 watchdog intent、重新启动并健康检查网关，然后才向 UI 返回“停止失败、网关仍在运行”。如果补偿恢复也失败，必须进入明确的 degraded 状态，保留快照和停止前备份，不得删除现场文件或谎报网关仍正常运行。

### 7.1 停止后的状态探测语义

停止命令的执行结果与停止后的健康探测是两个不同信号。显式停止事务成功后，网关端口不再监听是预期结果；前端随后请求 `/state` 得到连接拒绝，只能转换为 `running = false`，不能生成 `CONTROL_API_UNAVAILABLE`，也不能显示为“配置错误”。

| 持久化意图/进程所有权 | `/state` | 返回状态 | 用户可见错误 |
| --- | --- | --- | --- |
| 无 `desired_mode = gateway`，无 Codex-X 子进程 | 不可达 | `stopped`，`degraded = false` | 无 |
| `desired_mode = gateway` 仍有效 | 不可达 | `degraded = true` | `CONTROL_API_UNAVAILABLE` |
| Codex-X 管理的子进程仍存活 | 不可达 | `degraded = true` | `CONTROL_API_UNAVAILABLE` |
| 已存在 `degraded.json` | 不可达 | `degraded = true` | 优先显示持久化恢复错误 |

操作错误由停止命令本身返回，例如文件回滚或网关补偿恢复失败；状态错误只描述停止命令结束后仍持续存在的异常。UI 不得把普通 stopped 状态中的一次探测失败显示为持续性错误，只有 `degraded = true` 时才展示 `processState.error`。

## 8. 停止前备份与恢复

在任何确定性覆盖前，保存停止时当前文件：

~~~text
gateway-stop-backups/<timestamp>/
  config.toml
  auth.json
  <实际提示词文件>
  AGENTS.md
  manifest.json
~~~

两种快照用途不同：

- 启动前快照：记录进入网关模式之前的配置，是投影和异常恢复的基线；
- 停止前备份：记录确定性覆盖之前用户磁盘上的最新内容，供用户事后恢复。

停止成功后，界面应提示“网关运行时配置已写回直连配置，并已创建停止前备份”，同时提供备份入口。

## 9. 安全与有效性约束

### 9.1 最终配置必须有效

最终 `config.toml` 不能继续指向即将停止的本地网关。如果 Provider、模型、URL 或认证状态无法生成有效直连配置，停止操作必须失败并保持网关运行。

### 9.2 不允许整文件盲目覆盖

- 不用启动前 `config.toml` 整体覆盖当前文件；
- 不用网关生成的 `auth.json` 整体覆盖当前认证文件；
- 不用运行时提示词整体覆盖 `AGENTS.md`；
- 只覆盖明确声明且在本次会话中实际托管的字段、认证键、提示词文件或管理区块。

### 9.3 继续复用现有安全机制

确定性写回继续使用：

- `acquire_live_config_lock()`；
- `atomic_write_if_unchanged()`；
- 多文件事务回滚；
- watchdog 失败恢复；
- 文件格式和 UTF-8 校验。

新的逻辑只负责按照字段所有权生成候选文件，不绕过现有的原子写入和并发保护。

## 10. 与供应商页面的协调

前端当前根据 gatewayProcess 分支：

- 网关路由有效时写网关运行时；
- 否则写 config.toml 和 auth.json。

因此必须把“状态未知”与“直连模式”区分开：

~~~text
unknown/loading != direct
~~~

建议全局状态至少区分：

- direct：允许直连文件写入；
- gateway：供应商和提示词写入网关运行时；
- gateway-disconnected：禁止普通直写，要求重新接入或先停止网关；
- unknown：状态未确认，暂缓可能修改 live 配置的操作。

这样可以避免应用刚启动、网关状态尚未轮询完成时，供应商操作误走直写路径。

### 10.1 当前首屏竞态的实际根因

当前前端把 `gatewayProcess` 初始化为 `null`，并在独立的 `useEffect` 中异步调用 `get_gateway_process_state`。与此同时，读取 Codex 配置的另一条异步流程完成后就会让供应商页可操作。`gatewayRouteActive(null)` 当前返回 `false`，因此在网关状态查询尚未完成时，以下操作会被误认为直连操作：

- 切换或保存当前 Provider；
- 切换官方 Provider；
- 启用或禁用提示词；
- 其他会修改 live `config.toml`、`auth.json` 或受管提示词文件的操作。

如果状态查询失败，前端同样把状态设回 `null`，也会重新落入直连分支。后端 `process_state()` 已经能够结合运行中的进程、持久化 `gateway-mode` 元数据和实际路由判断网关状态，所以问题不在于网关页才有能力检测，而在于检测结果没有成为应用启动阶段的全局前置条件。

此外，当前 `reset_official_provider` 以及删除当前供应商时调用的 `switch_official_provider` 存在绕过 `gatewayRouteActive` 的直连调用；`disable_external_instruction` 也没有统一经过网关分支。这些入口即使在首屏轮询完成后，仍可能绕过网关状态修改全局文件。

### 10.2 统一的启动闸门与写入路由

应用启动或切换 `CODEX_HOME` 时，必须先完成一次后端网关状态初始化，再允许任何可能写入全局配置的操作：

1. 状态初始值必须是 `unknown/loading`，不能用 `null` 表示“已停止”；
2. 启动阶段同时读取 Codex 状态和 `get_gateway_process_state`，但只有两者都完成且状态一致后，才把页面标记为可写；
3. 状态查询失败、超时、状态不一致或持久化网关元数据损坏时，进入 `unknown`/`degraded`，阻止所有 live 文件写入，并提供重试或修复入口；
4. 每个写入入口在执行前都必须再次通过统一的 `resolve_config_write_route()`（名称可调整）判断：`direct` 写文件，`gateway` 写网关运行时，其他状态拒绝写入；
5. 不能只依赖按钮的 disabled 外观。后端写入命令也应检查当前持久化网关模式，拒绝在网关托管期间执行直连 Provider、认证或提示词写入。

页面体验可以在检查期间显示加载态，但不能把加载态当成直连模式。检查完成后：

~~~text
direct               -> 允许 config.toml/auth.json/提示词直写
gateway              -> 只允许写网关运行时状态
gateway-disconnected -> 阻止普通写入，要求重新接入或停止网关
unknown/degraded     -> 阻止所有可能影响全局配置的写入
~~~

这样可以保证用户从应用刚打开、从其他页面返回、切换配置目录或网关进程刚重启时，都不会因为状态尚未刷新而误改全局配置。

## 11. 推荐实施顺序

1. 将网关状态初始化提升为全局启动状态，并将 `unknown` 作为阻塞态；
2. 记录本次网关会话实际投影的字段、Provider ID、认证键和提示词路径；
3. 将停止流程改为“当前文件为基础、按字段所有权确定性写回”；
4. 为 `config.toml` 实现 Provider 配置组覆盖，并保留其他字段；
5. 为 `auth.json` 实现托管认证键覆盖，禁止整文件覆盖；
6. 为外部提示词实现目标文件覆盖，为 `AGENTS.md` 实现管理区块覆盖；
7. 增加停止前备份、并发修改、无效 `base_url`、格式损坏和回滚失败测试；
8. 在网关页面明确提示：网关托管字段以运行时状态为准，停止时会写回；
9. 将 `reset_official_provider`、删除当前 Provider、外部提示词禁用等遗漏入口接入统一写入路由，并在 Rust 命令层增加网关模式复核。

## 12. 结论

网关保存启动前快照和投影状态是长生命周期网关事务的合理需求，但使用整份 config.toml 的 SHA-256 作为唯一停止条件，保护粒度过粗，确实构成设计缺陷。

推荐保留快照和并发保护，同时把停止流程升级为：

~~~text
当前文件作为基础
  + 网关托管字段由运行时覆盖
  + 非托管字段保留当前值
  + 停止前自动备份
  + 提交前并发复核
  + 原子写入与失败回滚
~~~

这样不会因为用户修改一个无关字段而无法停止网关，同时避免整文件恢复导致的意外覆盖。对于网关托管字段，产品规则必须明确告知用户：网关运行时状态拥有最终写回权；如未来需要用户逐项决定，再在此基础上增加 diff 界面。

## 13. 外部提示词文件与 AGENTS.md 的原生语义

### 13.1 外部提示词文件

外部提示词文件通常是 Markdown 文件，例如 `my-rules.md`、`gpt5.5-unrestricted.md`。Codex 通过 `config.toml` 中的 `model_instructions_file` 指向它：

~~~toml
model_instructions_file = "./my-rules.md"
~~~

Codex 读取该文件后，会把内容作为额外的模型指令加入请求上下文。它不是普通用户消息，也不是 Provider 配置，而是持久化的指令来源。可以抽象为：

~~~text
Codex 内部指令
  + model_instructions_file 内容
  + 项目上下文
  + 用户当前输入
~~~

实际优先级和重新读取时机由 Codex 版本决定；Codex-X 不负责解析这些指令，只负责维护文件和配置指针。修改后通常需要新建或重新打开 session 才能确保生效。

Codex-X 当前支持两种管理方式：

- Replace：把选中的 Markdown 文件设置为 `model_instructions_file`，替换原来的主要提示词入口；
- Append：不替换原有外部提示词，而是把 Codex-X 管理内容写入 `AGENTS.md` 的受管区块。

### 13.2 AGENTS.md

`AGENTS.md` 是项目或目录级的 Agent 指令文件约定。Codex 在项目工作目录及其适用的父级目录中寻找 `AGENTS.md`，并将相关文件内容作为该项目或目录范围的工作规则。

它与 `model_instructions_file` 的差异是：

~~~text
model_instructions_file：配置级、通常范围较广的持久化指令
AGENTS.md：项目/目录级、随工作目录变化的指令
~~~

当前 Codex-X 的 `agents_path(codex_dir)` 指向 Codex 配置目录下的 `AGENTS.md`。这通常属于用户级或全局级文件，可能影响使用同一 Codex 配置目录的多个项目。因此不能把它简单当作某个项目的普通 Markdown 文件。

`AGENTS.md` 还包含两类内容：

~~~text
用户自己的规则
Codex-X 使用 BEGIN/END 标记维护的受管区块
~~~

禁用或停止网关时，只能删除或替换 Codex-X 自己的受管区块，必须保留其他用户内容。

### 13.3 网关为什么暂时接管提示词

网关运行时也可以在请求发送前注入提示词。如果 Codex 继续从 `model_instructions_file` 或 `AGENTS.md` 加载同一内容，同时网关再次注入，就会出现重复指令或来源不确定：

~~~text
Codex 原生加载一次
网关运行时再注入一次
~~~

因此网关启动时需要保存原始文件和配置状态，并做中性投影：

- Replace 模式：暂时移除 `model_instructions_file`，把内容提交给网关运行时；
- Append 模式：暂时移除 `AGENTS.md` 中 Codex-X 的受管区块，但保留用户其他内容；
- 网关关闭时：按运行时最新提示词恢复文件和指针。

这里的“接管”只是为了避免重复注入，不代表 Codex 原生不支持这些文件。

### 13.4 停止网关时的写回规则（无 diff 界面）

外部提示词文件和 `AGENTS.md` 遵循与 `config.toml` 相同的总原则：网关托管的部分由运行时状态决定，非托管的部分保留停止时磁盘上的最新内容。不做文本三方合并，也不要求用户逐段选择。

外部提示词文件：

1. Replace 模式只托管本次会话选中的文件。停止时将运行时生成的最新内容写回该文件；如果该文件在启动前不存在，按会话元数据中的创建策略保留或删除。
2. Replace 模式不接管用户未选中的其他 Markdown 文件；这些文件停止时原样保留。

`AGENTS.md`：

1. 只识别并替换 Codex-X 的 `BEGIN/END` 受管区块；区块之外的所有用户内容以停止时的当前文件为准。
2. 如果受管区块被用户修改，仍按受管区块所有权规则由运行时状态覆盖；不会因此拒绝停止。
3. 如果标记缺失、重复或结构损坏，无法安全定位受管区块，则停止失败并保持网关运行；用户可以先根据停止前备份修复文件。

只有一种情况仍然需要中止：写入事务执行期间出现并发修改，且无法确认当前文件未被其他进程覆盖。这是为了避免原子写入丢失正在发生的新修改，与用户在网关运行期间早已作出的合法修改不同。

如果提示词文件不是有效 UTF-8，或者无法安全解析 `AGENTS.md` 标记，不能直接覆盖；应保持网关运行，提供原文和停止前备份修复路径。

### 13.5 与普通用户消息的区别

~~~text
用户消息：影响当前请求或当前对话
model_instructions_file：持久化的配置级额外指令
AGENTS.md：持久化的项目/目录级工作规则
网关运行时提示词：只对经过该网关的请求生效
~~~

因此，网关停止时的目标不是简单删除提示词，而是把网关运行时状态安全地转换回 Codex 原生能够读取的文件形式：受管文件或受管区块由运行时状态写回，非受管文件和区块外内容保留用户在网关运行期间所做的修改；被确定性覆盖的停止前版本仍可从备份恢复。
