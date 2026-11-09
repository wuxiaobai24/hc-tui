未修改任何文件，也未创建 `review-plugins.md`，遵守你标为最重要的禁写要求。已完整阅读指定材料；以下是静态评审，未运行测试。

判断：**这套机制承担了包管理器、任务监督器和分布式终端管理器的复杂度，却没有把代码寿命、执行目标和进程寿命三个核心契约做对。当前不宜把 M5–M7 视为契约已经稳定的插件平台。**

**1）三个最严重的架构问题**

1. **[P1] 代际引用保护的是视图，实际依赖代码的却是进程。**

   位置：[src/app/plugin.rs:845](/home/wxb24/code/hc-tui/src/app/plugin.rs:845)、[src/plugin/lease.rs:153](/home/wxb24/code/hc-tui/src/plugin/lease.rs:153)、[src/plugin/registry.rs:384](/home/wxb24/code/hc-tui/src/plugin/registry.rs:384)。

   `release_plugin_terminal()` 在 Pane 消失时释放 lease；`live_generations()` 判断的是客户端 PID 是否存活。两者都不能证明 core 中的终端已经结束。`plugins pane close` 明确只断视图，程序继续运行，却会触发代码回收。

   **爆炸场景：**包内 Python 程序启动后卸载插件，再关闭 Pane；下一次延迟 import、打开资源或执行包内辅助程序时，代际目录已经被删。detach 后客户端 PID 消失，下次卸载同样会把仍运行的终端判成无引用。已加载的二进制暂时还能执行，不等于代码依赖已经消失。

   动作更糟：[src/plugin/registry.rs:44](/home/wxb24/code/hc-tui/src/plugin/registry.rs:44) 的 `ActionInfo` 没有 generation，[src/plugin/runner.rs:99](/home/wxb24/code/hc-tui/src/plugin/runner.rs:99) 的 `Spec` 也没有代际持有凭证。动作排队时卸载，代码目录可直接消失；运行期间卸载，则 cwd 尚在但目录内容被删除。

   M5 本身也没统一这条契约：[src/mods/store.rs:470](/home/wxb24/code/hc-tui/src/mods/store.rs:470) 卸载直接 `store.remove()`；[src/app/mods.rs:870](/home/wxb24/code/hc-tui/src/app/mods.rs:870) 回收只统计本进程表面，无法保护另一个 TUI 实例。

   最刺眼的是 `result-m7.md:257`：把“直到终端结束”用“关闭 Pane 后回收”作为证据。**交付记录把需要发现的错误写成了验收成功。**

2. **[P1] “受限子进程”没有覆盖完整生命周期，超时存在绕不过去的盲区。**

   位置：[src/plugin/runner.rs:479](/home/wxb24/code/hc-tui/src/plugin/runner.rs:479)、[src/plugin/runner.rs:506](/home/wxb24/code/hc-tui/src/plugin/runner.rs:506)。

   父进程先同步 `stdin.write_all(context)`，随后才启动 stdout/stderr reader，最后才建立超时 deadline。

   **爆炸场景：**上下文超过管道容量，子进程先大量输出再读 stdin。父进程堵在写 stdin，子进程堵在写 stdout，reader 尚未创建，取消标志和超时循环永远轮不到。八个这样的调用足以永久占满运行槽。

   另一条漏洞：[src/plugin/runner.rs:510](/home/wxb24/code/hc-tui/src/plugin/runner.rs:510) 发现直接子进程退出就结束；正常退出路径没有清理进程组。`sh -c 'sleep 9999 & exit 0'` 可以报告 done，同时留下后代和持管道 reader。reader 的线程句柄被直接丢弃，宿主没有监督它们的终止。

   [src/app.rs:6789](/home/wxb24/code/hc-tui/src/app.rs:6789) 的 shutdown 只等三秒，没有取消全部任务、等待清理或终止进程组。退出宿主后，独立进程组里的动作可能继续运行，负责超时的线程却消失了。

   **这不是恶意插件绕沙箱的问题，普通脚本就能违反宿主承诺。**

3. **[P1] “目标冻结”只冻结了一部分数据，执行位置仍可能串到另一个 Space。**

   位置：[src/app/plugin.rs:123](/home/wxb24/code/hc-tui/src/app/plugin.rs:123)、[src/app/plugin.rs:151](/home/wxb24/code/hc-tui/src/app/plugin.rs:151)、[src/app/plugin.rs:214](/home/wxb24/code/hc-tui/src/app/plugin.rs:214)。

   动作收到指定 Pane 后，Space、Tab、workspace machine 仍取当前活动工作区。未知 `pane_or_terminal` 解析失败后还能回退到当前焦点；数字 Pane 不存在也不拒绝。

   **爆炸场景：**当前在本地 Space A，通过 CLI 指定远端 Space B 的终端。上下文里终端属于 B，folder 和 machine 却属于 A。脚本依据 `machine.workspace` 执行 Git 或部署操作，就可能操作错误机器。这里需要 `target_gone`，实现却继续提供可执行上下文。

   终端入口也有同类问题：[src/app/plugin.rs:480](/home/wxb24/code/hc-tui/src/app/plugin.rs:480) 保存的异步 `Plan` 没有 Space/Tab 身份；探测回复在 [src/app/plugin.rs:537](/home/wxb24/code/hc-tui/src/app/plugin.rs:537) 重新调用打开逻辑，后者使用当时活动工作区。

   **爆炸场景：**在远端 Space B 打开入口，探测期间切到 A。程序仍携带 B 的 machine/cwd，Pane 却创建在 A，生成的上下文还混入 A 的身份。所谓冻结目标没有成为接口不变量。

**2）设计取舍：复杂度花错了地方**

- **代际目录不算过度工程；当前这套 lease 算。**  
  热更新时保留旧代码有实际价值。但 `lease.rs:68` 用“包＋客户端 PID＋代际＋Pane 键”模拟资源所有权，`lease.rs:125` 又在安装记录存在时拒绝回收旧代际，结果是更新积累垃圾、关闭视图又过早删除。这套复杂度既没换来正确回收，也没换来一致的多实例语义。对单人工具，宁可旧代际保守保留、显式 GC，也好过现在自动误删。

- **存储宣称原子发布，实现却是分步改共享状态。**  
  [src/store.rs:171](/home/wxb24/code/hc-tui/src/store.rs:171) 先删除旧 `current`，再创建新链接；[src/store.rs:139](/home/wxb24/code/hc-tui/src/store.rs:139) 直接覆盖 record；[src/plugin/registry.rs:327](/home/wxb24/code/hc-tui/src/plugin/registry.rs:327) 先切指针，再写记录。中途崩溃会留下无指针、损坏记录或指针与 generation 不一致。`store.rs:163` 用 `create_dir_all` 分配代际，没有互斥，两个安装进程可能把内容复制进同一个代际。  
  `docs/plugins.md:204` 的“临时目录加 rename”不是实现事实。**既然选了包管理器复杂度，事务发布就是基本成本。**

- **模块／插件／终端不是同一维度，硬拆成互斥产品边界切错了。**  
  `manifest.rs:81` 的包只能贡献 actions/terminals；`app/plugin.rs:750` 又把“终端”组装成执行上下文和布局操作。终端应是呈现与执行载体，TSX 是另一种呈现载体，包才是分发单位。一个 Git 工具同时需要动作和 TSX 列表，现在必须拆包、拆安装信任、经 CLI 拼接。`docs/plugins.md:229` 明确把这种拼接变成作者责任。

- **重造的是终端外围控制面，不是 PTY。**  
  真正启动已经委托给 [src/hc/net.rs:951](/home/wxb24/code/hc-tui/src/hc/net.rs:951) 的 `terminal.run`；机器探测使用 `terminal.rs:190` 的 `machine.exec`；身份检索依靠 `app/plugin.rs:825` 的 core 标签。core 事件订阅也已在 `src/hc/net.rs:500`。  
  合理新增项是清单、命令贡献、参数校验、本地 placement。重复且危险的部分是另建终端身份映射和生命周期状态：`PluginTerminal`、`PaneRequest`、lease，再与 core terminal/keep/标签同步。`app/plugin.rs:838` 甚至到 attach 后才登记地址，终端身份取得受视图流程制约。**薄适配层正在长成第二个终端管理器。**

- **“严格 schema”也有名实落差。**  
  [manifest.rs:567](/home/wxb24/code/hc-tui/src/plugin/manifest.rs:567) 允许递归对象 schema，[manifest.rs:656](/home/wxb24/code/hc-tui/src/plugin/manifest.rs:656) 的运行期检查却只验证外层类型和 enum；默认值直接插入不验证，required 只遍历 properties 时检查。嵌套必填字段缺失、错误默认值都能通过。严格拒绝未知字段，却放过已声明契约，优先级倒了。

**3）安全模型：可以选择全信任，但不能把成本藏进“受限”二字**

“受信本地代码”适用于用户自己写、自己审阅、主动调用的个人自动化。**安装某个作者的插件，不等于信任它将处理的每个仓库、工作区配置和远端返回值。**目前 `runner.rs:446` 直接以用户身份执行，继承环境；`app/plugin.rs:249` 还提供配置、状态、socket 和 CLI 路径。工作区数据一旦进入危险命令，损失由运行 TUI 的用户及其可访问远端承担，宿主没有包级权限边界。

具体问题有三处：

- **显式启用并不是独立闸门。**`registry.rs:335` 安装即 `enabled: true`，`registry.rs:145` 扫描开发目录默认启用，`registry.rs:365` link 也启用。与 `docs/plugins.md:198` 的默认不启用相冲突。更糟的是，更新会把禁用状态重新设成 true。默认启用不等于安装时立即执行，但确实撤掉了规格承诺的第二道决策。
- **限制目录权限没有保护用户全部数据。**`app/plugin.rs:232` 只给配置和状态数据目录设置 0700；`logs.rs:29` 创建日志目录没有同等权限设置，实际依赖 umask。stdout/stderr 可能含令牌、远端路径或命令结果，`docs/plugins.md:211` 的统一权限承诺并未兑现。
- **禁用与撤销不是同一件事。**`registry.rs:287` 修改记录，`main.rs:588` 只调用这个持久化入口；既有 App 注册表不因此自动失效，动作调用 `app/plugin.rs:88` 仍查内存。终端打开也仅在名称找不到时重载。用户“已经 disable”的认知可能早于宿主实际停止提供入口。

相比 shell 脚本，多得到的是命令发现、参数入口、上下文、排队、状态查询和统一终端放置，**没有多得到权限隔离**。这些便利值得保留；但 `runner.rs:479` 的死锁、正常退出遗留后代和无包级日志配额，说明连运行治理也尚未兑现完整。`logs.rs:16` 每调用独立文件、`runner.rs:635` 只回收内存记录，磁盘总量仍随调用增长。

对于个人脚本，这种全信任模型可以接受；对于第三方分发和处理陌生仓库，应明确增加的是工作区信任或执行审批契约，不能拿 manifest 预览充当代码审计，也不能把签名等同安全。

**4）横向比较**

| 对照 | 本方案明显占优的范围 | 本方案明显落后的范围 |
|---|---|---|
| VSCode extension host | 针对一次性 CLI 工作流，`runner.rs:446` 直接运行任意语言，`terminal.rs:80` 接入已有 core 终端，作者不必先写 Node 扩展适配。优势是这个窄场景的接入成本。 | VSCode 明确区分 local/web/remote host，并用 `extensionKind` 表达运行位置；本方案动作固定本地，终端另走远端，`app/plugin.rs:249` 还把本机路径原样注入远端环境。缺少类型化的位置契约，也没有工作区信任闸门。[官方 host 文档](https://code.visualstudio.com/api/advanced-topics/extension-host)、[Workspace Trust](https://code.visualstudio.com/api/extension-guides/workspace-trust)。 |
| Zed extensions | 对“已有脚本＋真实交互终端”的扩展，`manifest.rs:109`、`:191` 的入口更直接；相较 Zed 扩展代码的 Rust/Wasm 路线，包装现有工具成本更低。这是开发体验判断。[Zed 开发文档](https://zed.dev/docs/extensions/developing-extensions)。 | Zed 对宿主 API 的 process/download/npm 操作有可撤销 capability 检查；本方案 `runner.rs:446` 一旦启动即获得用户权限。Zed 允许执行外部程序也不代表完整 OS 沙箱，但至少存在可执行的 API 权限层，本方案插件没有。[Zed capabilities](https://zed.dev/docs/extensions/capabilities)。 |
| Neovim 插件模型 | 相较直接在编辑器内执行的 Lua 插件，一次动作独立进程、统一 timeout/concurrency 的方向更适合隔离偶发脚本故障，入口在 `runner.rs:219`、`:455`；core 的持久终端也适合工作台场景。 | Neovim 普通插件无需清单注册，remote plugin 已支持任意语言、双向 RPC、事件和按需加载。“任意语言＋独立进程”不是本方案独有收益。本方案 `runner.rs:111` 只提供一次 stdin，`app/plugin.rs:300` 仅消费退出状态，持续交互还要另接 CLI 和待实现事件流。[Lua 插件](https://neovim.io/doc/user/lua-plugin/)、[remote plugin](https://neovim.io/doc/user/remote_plugin/)。 |

不能把 VSCode extension host 的进程隔离描述成权限沙箱，也不能把 Neovim 的 remote plugin 理解成自动远端主机部署。比较成立的尺度是运行位置、宿主协议、故障治理和作者成本。

**5）只允许改三件事**

1. **重做 `src/store.rs` 的发布与保留契约，停掉基于 Pane/PID 的自动代码回收。**

   对应第一项严重问题及非原子安装。代际目录不可变；record 是唯一发布真相，`current` 若保留则只是可恢复索引。包级锁覆盖代际分配和提交，record 临时文件写完后原子替换。安装与更新默认保持禁用或原启用状态。

   建议接口：

   ```rust
   fn install(
       &self, id: &PackageId, source: &Path, policy: EnablePolicy
   ) -> Result<InstalledGeneration, StoreError>;

   fn pin(
       &self, generation: &GenerationId, owner: ResourceOwner
   ) -> Result<GenerationPin, StoreError>;

   fn gc(
       &self, evidence: &RetentionEvidence
   ) -> Result<GcReport, StoreError>;
   ```

   `ResourceOwner` 区分 Invocation 与带 core/session 身份的 TerminalAddress；queued 动作也持 pin。Pane 关闭不释放终端依赖。**暂时拿不到可靠的 core 终结证据就保留代码，显式 GC；不要继续猜。**`plugin/lease.rs` 的 PID 存活判断退出正确性路径。

2. **把 `src/plugin/runner.rs` 改成监督完整 I/O 和进程组的执行器。**

   对应第二项严重问题。deadline 从启动阶段开始；stdin writer 与两个 reader 同时受取消和 deadline 管理。直接子进程退出后仍须收尾后代和管道，收尾完成才公布最终状态。正常关闭宿主也走同一条终止路径。

   建议接口：

   ```rust
   fn invoke(
       &self, spec: InvocationSpec, pin: GenerationPin
   ) -> Result<InvocationId, InvokeError>;

   fn cancel(&self, id: &InvocationId) -> Result<(), CancelError>;

   fn shutdown(&self, deadline: Instant) -> Result<(), ShutdownError>;
   ```

   `InvocationSpec` 明确 stdin 字节上限；`logs.rs` 加包级总量和保留数量策略。不要继续把“单文件轮转”叫作日志有界。

3. **抽出不可变执行目标和通用终端入口服务，让插件只贡献入口。**

   对应第三项严重问题及重复控制面。`plugin_target()` 必须由目标 Pane 反查所属 Space/Tab/machine；请求目标不存在立即失败，禁止焦点回退。异步探测保存完整目标，回复后只验证其仍存在。

   建议接口：

   ```rust
   fn resolve_target(
       &self, selector: TargetSelector
   ) -> Result<ExecutionTarget, TargetError>;

   fn prepare_terminal(
       &self, entry: &TerminalEntry, target: ExecutionTarget
   ) -> Result<PreparedTerminal, LaunchError>;

   fn open_terminal(
       &mut self, launch: PreparedTerminal, placement: Placement
   ) -> Result<OpenRequestId, LaunchError>;
   ```

   `ExecutionTarget` 包含稳定 Space/Tab 身份、限定终端地址、machine、cwd；上下文和环境都从它生成。PATH 检查应由实际执行位置负责，别让 `terminal.rs:144` 用 TUI 环境替 core 环境作最终判决。插件入口、脚本面板和普通终端共用这个服务，`app/plugin.rs` 删除自有的终端生命周期推断。

   先修这三件，再谈统一包同时贡献动作、TSX 和终端。现在继续堆入口，只会让错误的所有权关系成为兼容负担。
