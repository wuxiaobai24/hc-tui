# hc 协议事实（实测基线）

日期：2026-11-04
基线：hc **0.1.17**（安装版）。本机源码 checkout 比安装版旧（缺 `resize_notifications`），因此**以 `hc schema` 和实测为准**。
用途：实现的直接依据。与官方文档冲突时，先在真实 core 上复测，再更新本文。

---

## 1. 连接

```sh
hc connect --session <name>     # stdin/stdout 上的裸协议
```

- core 未运行时自动拉起（`connect_or_start`），TUI 不需要自己管理守护进程。
- 第一条消息是**明文** JSON-RPC（一行一条），回复也是明文；回复之后切换为帧。
- 连接是双向字节流，不是 TTY；不要在其 stdout 上做任何终端控制。

### 1.1 hello

请求参数：`generation`（可选）、`framed`、`core`、`calls_you`、`features`。

> `features` 是「另一个 core 拨入时」声明自身能力用的，普通客户端不需要发；能力以返回值判断。

返回：

```json
{"generation":1,"version":"0.1.17","core":{"id":"…","name":"omarchy"},
 "framed":true,
 "methods":["hello","terminal.run",…],
 "features":["framed","attach","events","status","shell","resize_notifications"]}
```

- `generation` 当前为 **1**。不匹配时错误 `kind = needs_update`。
- 客户端应校验：generation、必需 methods（`terminal.run/list/attach/kill`、`events.subscribe`、`doc.get/set`）。

---

## 2. 帧

```
[u32 LE channel][u8 kind][u32 LE len][payload]
      4              1           4          len
```

| 常量 | 值 | 说明 |
|---|---|---|
| `HEADER` | 9 | |
| `MAX_PAYLOAD` | 16 MiB | 超出即错误 |
| `CHUNK` | 16 KiB | 单帧最大流数据 |
| `WINDOW` | 256 KiB | 每流初始 credit |
| `MAX_PENDING` | 4 MiB | 积压上限，超出触发重绘/missed |
| `MAX_CONTROL` | 64 MiB | channel 0 积压上限，超出断连 |

| kind | 名称 | 方向 | payload |
|---|---|---|---|
| 0 | DATA | 双向 | channel 0 为 JSON-RPC；其他为 VT 字节/输入 |
| 1 | CREDIT | 接收方→发送方 | `u32 LE` 已消费字节数 |
| 2 | CLOSE | 双向 | core→viewer 为 JSON reason；viewer→core 为空 |
| 3 | RESIZE | viewer→core | `u16 LE cols` + `u16 LE rows` |
| 4 | FOCUS | viewer→core | 1 = 获得，0 = 失去 |
| 5 | FACTS | viewer→core | JSON `Display{fg,bg,cursor,cell:[w,h]}` |

- channel 0 走 control 队列，**不吃 credit**（但有 64 MiB 上限，超出直接断）。
- 其他 channel 是流，初始 256 KiB credit，只有 `credit > 0` 才发送。
- 调度：control 优先；流之间轮转，单个高产 Pane 不会饿死其他 channel。

---

## 3. 流与 credit（最容易踩）

1. **每个 DATA 帧（每一条流）都要回 CREDIT**，值 = 该帧 payload 字节数。包括 **events channel**。
2. 不回 credit 的症状：**静默停住**——不报错、不断连、数据就是不来。
3. 积压超 `MAX_PENDING`：
   - attach 流：整段丢弃，core 在随后的 DATA 里**内联一份完整重绘**（不是特殊帧）。
   - events 流：保留整行后发 `missed` + 全量快照。
4. 因此 VT 解析器必须能处理「流中间的整屏复位」：重绘以 `\x18` 开头，后接软复位与清屏。

---

## 4. attach

### 4.1 参数

| 参数 | 说明 |
|---|---|
| `terminal` | `t7` / 名称 / `machine/t7` |
| `cols`,`rows` | viewer 内容区尺寸（不含边框） |
| `observe` | 只读镜像：不定尺寸、不参与输入 |
| `history` | 首帧前附带的滚动历史行数（仅主屏） |
| `display` | 颜色与 cell 像素（也可后续用 FACTS 补） |
| `resumed` | 断线恢复用：尺寸在重新激活后再生效 |

### 4.2 返回

```json
{"channel": 2, "resize_notifications": false, "terminal": {…}}
```

- `resize_notifications` 对普通客户端恒为 `false`（core-to-core 中继才为 true）。源码里它出现在返回而非参数。

### 4.3 顺序（源码保证）

```
apply_size()   → 若尺寸变化：发 size 事件（同一 outbox）
first_paint()  → 排入 attach channel
```

同一连接同一 outbox，所以 **`size` 事件先于首帧**。客户端按帧序处理即可。

### 4.4 首帧内容

```
\x18                     取消可能的中断序列
\x1b[!p                   软复位
\x1b[?2026l … \x1b[=0;1u  关掉同步输出、鼠标、焦点、粘贴、kitty 键位等
\x1b[?1049l …             退回主屏
\x1b[H\x1b[2J             清屏
\x1b[3g\x1b[9G\x1bH …      重建 tabstop
\x1b[0m + 内容             逐行重放（调色板色以 38;5;N 形式）
\x1b[<row>;<col>H          光标位置
\x1b[0m\x1b[?9001l         收尾
```

- 主屏请求了 `history` 时，core 会先把历史行格式化输出，再用 `\r\n` 把屏幕顶下去，最后清屏重画。
- 内容使用**调色板索引色**而非 RGB（`38;5;1`），最终颜色由 viewer 的调色板决定。

---

## 5. 尺寸与多 viewer

- 定尺寸者：**最近活动的非 observe viewer**。
- observe viewer 的 RESIZE 被忽略；尺寸变化时它**收不到重绘**，也收不到专门的尺寸帧。
- 它唯一的尺寸来源是 events channel 的 `size` 事件：

```json
{"type":"size","terminal":"t1","data":{"cols":90,"rows":20},"seq":2}
```

- 要求：先 resize 本地 VT，再处理该 channel 后续帧；否则宽字符/软换行错位。
- 两个非 observe 客户端会互抢尺寸（最后活动者赢）。UI 需要为此给提示。

---

## 6. 事件

`events.subscribe` 参数：`terminal` / `children_of` / `label`（均可选，用于过滤）。

返回：`{seq, terminals[], channel}`。之后该 channel 收到一行一条 JSON：

```json
{"jsonrpc":"2.0","method":"event","params":{"type":"status","terminal":"t1","data":{…},"seq":7}}
```

| type | data 关键字段 | 用途 |
|---|---|---|
| `created` | 完整 record | 新终端 |
| `restarted` | 完整 record | `keep` 后换了新 shell |
| `lost` | 结束信息 | 程序丢失，等待恢复 |
| `finished` | `exit_code`,`signal`,`reason` | 已结束 |
| `removed` | — | 记录被删 |
| `keep` | `{keep:bool}` | keep 开关变化 |
| `status` | `status`,`records`,`reported` | OSC 7501 状态 |
| `turn` | `turn`,`state` | 一轮结束 |
| `title` | `title` | 标题 |
| `labels` | `labels` | 标签 |
| `size` | `cols`,`rows` | 尺寸变化 |
| `foreground` | `argv`,`cwd`,`pid`,`name` | 前台程序 |
| `doc` | doc 名 | 文档变更（只发无过滤订阅者） |
| `missed` | 全量快照 | 落后，需整体重同步 |
| `heartbeat` | `seq` | 每 15 s |

- `seq` 单调递增；跨 core 重启不可比，重连后重建基线。
- `missed` 之后丢弃可能不连续的本地缓存，用快照重建。

---

## 7. 终端查询的归属

| 查询 | 谁回答 | 是否转发给 viewer |
|---|---|---|
| DA1（`\x1b[c`） | core 的 Screen | 否 |
| DSR 光标位置（`\x1b[6n`） | core | 否 |
| 默认前景/背景/光标色（OSC 10/11/12） | core | 否 |
| 窗口像素尺寸（`\x1b[48;…t` 等） | 被 core 丢弃 | 否 |
| kitty graphics（`\x1b_G…a=q`） | **viewer** | 是 |
| 其他 core 不认识的查询 | viewer | 是 |

- 客户端对透传查询必须应答（不支持就回否定），否则程序可能一直等。
- 应答作为 attach channel 的 DATA 回写。
- **不要**重复回答 core 已答的查询——那些序列根本不会出现在流里。

---

## 8. 输入

| 场景 | 用法 |
|---|---|
| 逐键、组合键、鼠标 | attach DATA（由本地 VT 的当前模式编码） |
| 给 Agent 发一条消息 | `terminal.send`（= 粘贴 + Enter） |
| 命名键（脚本/Agent） | `terminal.keys` |
| 中断 | `terminal.interrupt`（发送停止键并报告结果） |

- bracketed paste 依据 VT 当前模式决定是否包裹 `\x1b[200~ … \x1b[201~`。
- 鼠标编码需要 FACTS 提供的 cell 像素尺寸与 VT 的鼠标模式。

---

## 9. 文档

- `doc.get{name}` → `{value, version}`（version=0 表示从未写过）。
- `doc.set{name, value, version}` → `{version}`；版本不匹配 → `kind = conflict`。
- `value = null` 删除内容，version 保留。
- name 规则同终端名：1–32 字符，`a-z0-9-_.`；value ≤ 1 MiB。
- 变更会发 `doc` 事件（仅无过滤订阅者）。

---

## 10. 客户端必做清单

- [ ] 明文 `hello{framed:true}` → 校验 generation 与 methods
- [ ] 9 B 帧编解码，payload 上限校验，半包/粘包
- [ ] channel 0 RPC 按 `id` 关联，支持并发与超时
- [ ] 每个 DATA 帧回 CREDIT（**含 events channel**）
- [ ] attach：cols/rows 用内容区尺寸；`observe` 语义正确
- [ ] attach 后立即发 FACTS（颜色 + cell 像素）
- [ ] 首帧与流内重绘都当作「整屏复位」处理
- [ ] `size` 事件 → 先 resize 本地 VT，再解析后续字节
- [ ] 透传查询的最小应答（kitty graphics 至少回否定）
- [ ] `missed` → 全量重同步
- [ ] CLOSE → 只断视图，不杀进程
- [ ] 断线重连：重新 hello → subscribe → attach
- [ ] 逐键走 attach DATA，不用 `terminal.send`

---

## 11. 复测方法

隔离环境，不碰本机真实 core：

```sh
export HC_SESSION=hctui-probe
export XDG_RUNTIME_DIR=/tmp/hctui/run XDG_STATE_HOME=/tmp/hctui/state
export XDG_CONFIG_HOME=/tmp/hctui/config XDG_CACHE_HOME=/tmp/hctui/cache
hc connect --session hctui-probe
# 结束后
hc server stop
```

帧用 `struct.pack("<IBI", channel, kind, len)` + payload；读时先读 9 字节头。
