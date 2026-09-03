# Codex-X 进入与退出工作流

这份图把软件从启动、接收请求、关闭窗口、显式退出到异常恢复的路径放在一起。网关相关状态以持久化文件和实际 `/state` 健康响应为准，页面状态不能单独代表运行状态。

## 总树状流程

```text
Codex-X 进程
├─ 进入：程序启动
│  ├─ Tauri single-instance 检查
│  │  ├─ 已有实例：唤醒原主窗口，当前进程不重复初始化
│  │  └─ 首次实例：继续启动
│  ├─ gateway::initialize_on_startup()
│  │  ├─ 没有 gateway-mode/state.json
│  │  │  └─ 视为 direct：不启动网关，不改写 config.toml/auth.json/提示词
│  │  ├─ state.json 的 desired_mode = direct
│  │  │  └─ 保持直连，不恢复网关
│  │  └─ state.json 的 desired_mode = gateway
│  │     ├─ 保留/重写 watchdog intent 为 gateway + watchdog_desired=true
│  │     ├─ 确保唯一的项目 Windows 登录任务 Codex-X Local Gateway 已启用
│  │     ├─ /state 健康且 state/listen/process_id 与快照匹配
│  │     │  ├─ 识别为当前 Codex-X 网关，重新接管进程
│  │     │  └─ 确保 watchdog 任务正在运行
│  │     ├─ 端口有响应但身份不匹配
│  │     │  └─ 返回 GATEWAY_PORT_IN_USE，绝不终止未知进程
│  │     └─ 网关已崩溃或端口无响应
│  │        ├─ 从 watchdog intent 优先读取 upstream
│  │        ├─ 否则从 runtime-state.json/provider 恢复 upstream
│  │        ├─ 通过 watchdog 任务重新启动网关并等待健康检查
│  │        └─ 失败：返回 GATEWAY_HEALTHCHECK_FAILED，保留现场快照
│  ├─ 建立系统托盘与主窗口
│  └─ 软件进入可用状态
│
├─ 运行：客户端请求进入网关（gateway 模式）
│  ├─ proxy_request() 读取原始请求行、请求头和正文
│  ├─ global_entry_probe
│  ├─ Provider model 修改
│  ├─ prompt_injection_probe
│  ├─ 指令提示词注入
│  ├─ execute_scripts()：用户脚本 raw-text 串行链
│  │  ├─ 退出码 0：输出修改后的请求，继续转发
│  │  ├─ 退出码 10：输出完整响应，直接返回客户端
│  │  ├─ 退出码 11：丢弃请求，不访问上游
│  │  └─ 其他非零/超时/非法输出：SCRIPT_EXECUTION_FAILED
│  ├─ global_exit_probe
│  ├─ 固定 Provider 上游
│  │  ├─ upstream_target() 只使用运行时已提交的 Provider 目标
│  │  └─ 脚本不能通过 Host 或 raw-text 改变实际连接目标
│  ├─ response_probe
│  └─ 客户端收到上游响应、脚本响应或网关错误
│
├─ 离开：用户关闭主窗口
│  ├─ WindowEvent::CloseRequested
│  ├─ prevent_close()
│  ├─ 隐藏主窗口并移出任务栏
│  └─ 进程、网关、watchdog 和网关快照继续保留
│     └─ 托盘“显示 Codex-X”或再次激活：恢复窗口并继续使用
│
├─ 离开：托盘退出 / 应用重启
│  ├─ Tauri 触发 RunEvent::ExitRequested
│  ├─ gateway::shutdown_on_exit() 保持幂等 no-op
│  ├─ 不删除 watchdog intent
│  ├─ 不停止 watchdog 或 gateway
│  ├─ 不恢复 direct 配置，也不删除网关模式快照
│  └─ 只允许 Codex-X 管理进程退出；网关状态继续由独立组件维持
│
└─ 异常离开：程序卡退 / 被强制终止 / 操作系统杀进程
   ├─ Tauri 退出钩子可能来不及执行
   ├─ gateway-mode/state.json、runtime-state.json 和 intent 保留
   ├─ watchdog intent 仍为 gateway + watchdog_desired=true
   │  └─ 独立看门狗继续监控并按上限重启网关
   └─ 用户再次启动 Codex-X
      └─ 回到“进入：程序启动”的初始化恢复分支
```

## 两种“关闭”必须区分

```text
主窗口右上角关闭
└─ 隐藏到托盘
   ├─ 不停止网关
   ├─ 不删除快照
   └─ 可从托盘恢复

托盘“退出 Codex-X”或应用重启
└─ ExitRequested
   └─ 保持 gateway 状态并允许 Codex-X 退出/重启
      ├─ watchdog intent 保留
      ├─ watchdog 和 gateway 继续运行
      └─ 下次启动时重新接管或恢复网关

网关页面“停止网关”
└─ 显式执行 gateway -> direct
   ├─ 先撤销 watchdog intent、停止 watchdog 和项目登录任务
   ├─ 再停止 gateway
   ├─ 恢复 config.toml/auth.json/提示词
   ├─ 成功：删除快照和 intent，完成切回 direct
   └─ 失败：重新激活 watchdog，保留 gateway 快照
```

## 模式判断

```text
启动时读取 gateway-mode/state.json
├─ 文件不存在或 desired_mode = direct
│  └─ direct：客户端请求不经过用户脚本链，观测页面停用
└─ desired_mode = gateway
   ├─ 已有匹配网关：接管并继续，确保 watchdog 任务运行
   ├─ 网关已死：通过 watchdog 按持久化 Provider 上游恢复
   └─ 端口被陌生服务占用：报错并保护陌生进程
```

## 相关实现位置

| 工作阶段 | 主要实现 |
| --- | --- |
| 启动恢复 | `apps/desktop/src-tauri/src/lib.rs` 的 `setup`、`gateway::initialize_on_startup()` |
| 网关进程状态与启停 | `apps/desktop/src-tauri/src/gateway.rs` |
| 主窗口关闭与托盘 | `apps/desktop/src-tauri/src/desktop_lifecycle.rs` |
| 客户端请求链 | 外部本地网关脚本的 `proxy_request()` |
| 看门狗 | 外部本地看门狗脚本 |
| 目标链条定义 | `docs/项目工作链条.md` |
| 生命周期旁路测试 | `docs/GATEWAY_BYPASS_TESTING.md` 第 3.7 节 |
