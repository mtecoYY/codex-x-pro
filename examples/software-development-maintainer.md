## 基本原则

这是长期维护的正式项目，不是一次性 Demo。

修改代码时优先保证：

1. 正确性和安全性
2. 不破坏现有功能
3. 复用现有实现
4. 最小改动
5. 可维护性和可测试性

---

## 编码前

开始修改前，先搜索项目中是否已有类似实现，包括：

* 函数、类、组件
* Service、Repository、Hook
* API 封装
* 类型、常量、配置
* 校验、转换、错误处理逻辑

优先顺序：

1. 直接复用
2. 扩展现有实现
3. 抽取公共逻辑
4. 确实无法复用时再新增

禁止在未检查现有代码的情况下重新实现相同功能。

复杂任务开始前，先简要说明：

* 相关现有实现
* 可复用模块
* 准备修改的文件
* 是否需要新增文件
* 主要风险

小改动无需写冗长计划。

---

## 禁止重复造轮子

禁止：

* 创建与现有功能高度相似的新函数、类或组件
* 复制已有代码后只修改少量内容
* 重复定义类型、常量、枚举、错误码和接口地址
* 绕过现有 Service、Repository 或 API Client 重写调用逻辑
* 在多个位置分别实现同一业务规则
* 引入与现有依赖功能重复的新依赖

相同业务规则应只有一个权威实现。

---

## 文件与函数规模

行数只作为预警，不是硬性限制。禁止为了减少行数而机械拆分。

参考范围：

* 函数建议不超过 80 行
* 函数超过 100 行时检查是否职责过多
* 组件或类建议不超过 500 行
* 业务文件建议不超过 800 行
* 文件超过 1200 行时优先评估拆分
* 文件超过 1500 行且包含多个职责时应拆分

以下情况即使代码不长，也应考虑拆分：

* 一个模块承担多个无关职责
* 嵌套过深或条件分支过多
* 相同逻辑重复出现
* 难以测试、阅读或复用
* 修改一个功能经常影响无关功能

以下文件可适当放宽：

* 自动生成代码
* 类型声明
* 静态配置或数据
* Schema
* 数据库迁移
* 集中式路由或注册文件

拆分必须依据业务职责，不得创建大量无意义的小文件。

---

## 修改原则

* 优先最小改动
* 不做与当前需求无关的重构
* 不为小需求重写整个模块
* 不擅自修改公共接口、数据库结构或返回格式
* 保持现有目录、命名、风格和架构
* 不创建 `new`、`final`、`v2`、`copy`、`backup` 等重复文件
* 不保留废弃代码、注释代码、调试输出和无意义 TODO
* 不随意新增依赖

发现架构问题时，优先采用渐进式改造，不要一次性重写。

---

## 代码设计

每个函数、类、组件和文件应有明确职责。

避免：

* UI、请求、状态和业务逻辑全部写在一个组件
* Controller 同时处理校验、业务和数据库操作
* 巨型 `utils`、`constants`、`types` 文件
* 过深嵌套
* 大量布尔参数
* 隐藏副作用
* 硬编码配置和业务值
* 大量 `any`
* 空 `catch`
* 吞掉异常

不要过度抽象。只有在逻辑重复、稳定或需要复用时才抽取公共模块。

---

## 安全与错误处理

必须考虑：

* 参数非法
* 空值和边界值
* 权限校验
* 网络或数据库失败
* 外部服务超时
* 并发和重复提交
* 文件操作失败
* 敏感信息泄漏

禁止：

* 硬编码密码、Token 或密钥
* 关闭安全校验
* 记录敏感信息
* SQL 注入、命令注入、XSS、路径穿越等明显风险
* 捕获异常后不处理

优先复用项目现有错误类型、日志和权限机制。

---

## 测试与验证

修改完成后检查：

* 是否重复实现已有功能
* 是否出现复制粘贴代码
* 是否存在超长函数或职责混乱
* 是否破坏已有接口
* 是否存在未使用代码或导入
* 是否遗漏错误处理和边界条件
* 是否需要新增或更新测试

根据项目情况实际运行：

* 测试
* Lint
* 类型检查
* 构建
* 格式检查

没有实际运行的检查，不得声称已经通过。

---

## 输出要求

修改多个文件时，先列出文件变更清单。

输出结果应说明：

1. 复用了哪些现有实现
2. 修改、新增或删除了哪些文件
3. 核心改动是什么
4. 是否存在兼容性风险
5. 实际运行了哪些验证命令

不要输出冗长的过程说明，不要无意义重复整个文件。

## Secrets, API Targets, And Commit Boundary

- Never write or commit real API URLs, production or internal domains/IPs, long `sk-*` API keys, redeemable card codes, bearer tokens, cookies, passwords, private keys, or other credentials in source, tests, fixtures, docs, logs, screenshots, Playwright snapshots, reports, or prompt examples.
- Real local values may only be loaded at runtime from environment variables or ignored files such as `config/local-api.toml`, `config/local-api.json`, or `.env.local`; never add those files to Git.
- Use `example.test`, `127.0.0.1`, MockServer, and short synthetic placeholders such as `sk-test` in examples and tests. Do not use long strings that resemble live credentials. Short semantic test values are not secrets.
- Redact request/response bodies and headers before logging or exporting; never expose full authorization values. Keep only safe prefixes or suffixes when identification is necessary.
- Before every commit, scan the full worktree and staged diff (`git diff --cached`) for URLs, credentials, and secret-shaped values. Inspect relevant history when a leak is suspected.
- If a real credential or private target is found, stop propagating it, remove it from current files, revoke or rotate it, and report whether Git history requires cleanup. Removing a value from the latest file alone is not historical erasure.
