# 变更记录

## 0.1.0 — 2026-11-04

首个可发布版本：基于 herdr-core（`hc`）的终端工作台，客户端自带
Workspace / Tab / BSP Pane 模型、本地 VT 渲染与布局持久化。

### 核心协议

- framed 编解码（9 字节头，DATA/CREDIT/CLOSE/RESIZE/FOCUS/FACTS），单 reader 按帧序分发。
- 每条 attach 流与 events 流都回 credit；断线指数退避重连，重连后重订阅并按意图重新 attach。
- attach 后立即上报 FACTS；`size` 事件先改本地 VT 尺寸再解析后续字节；`missed` 走全量快照。
- 通用 JSON-RPC 通道：`terminal.label/kill/read/send/keys/wait/interrupt/put/keep/rm`、
  `machine.*`、`templates.list`、`doc.get/set`；连接层失败自动在重连后重放。

### 对象模型与布局

- Workspace → Tab → BSP Pane；Pane 跨 Tab 移动时 ID 与 attach 流保持不变。
- 布局写入 core 的 `doc`（CAS + 400 ms 防抖），重启后按终端地址恢复与重新 attach；
  `conflict` 时取最新版本重放本地意图，并监听 `doc` 事件刷新版本。
- 焦点按几何移动、Tab 循环与重排、缩放、分隔线比例调整、最小尺寸保护。
- 分隔线支持 `auto` / `always` / `off`；`pane_gaps = false` 时相邻 Pane 共用一条边。

### 交互

- 前缀键与全部动作可配置；支持直接绑定与自定义命令（`shell` / `pane` / `popup`），
  命令沿用当前 Pane 的前台目录，临时 Pane 在程序退出后自动折叠。
- Copy Mode：视口移动、按词、行首行尾、翻页、矩形选区、搜索与命中高亮、链接打开。
- 复制走 OSC 52 与平台剪贴板工具；`copy_on_select` 支持普通模式拖选即复制。
- 只读文本浮层（`terminal.read`）、`terminal.keys` / `wait` / `put` 输入行、剪贴板图片
  （`terminal.put` 后贴回路径）、`$EDITOR` 打开 ANSI 历史、滚动条。
- 提示行支持光标移动、删除与历史；侧栏支持滚轮滚动、折叠成窄栏或隐藏。

### Agent

- 消费 `status` / `turn` 事件，维护 `working` / `blocked` / `error` / `done` / `idle` / `unknown`。
- 完成未读用本地 `seen_turn` 判定并落盘到 `state.json`；侧栏按注意力排序，`prefix a` 跳转。
- 后台完成按配置投递 toast 或系统通知，可选提示音。

### 多机与图形

- 机器选择器：`machine.add/list/connect/rename/rm`，`terminal.run --on` 开远程 Pane，
  侧栏显示机器连接状态。
- 启用 libghostty 的 kitty graphics：查询由本地 VT 回答，非虚拟 placement 采样成单元格背景
  （RGB/RGBA/PNG）。

### 配置与主题

- `$XDG_CONFIG_HOME/hc-tui/config.toml`（回落 `~/.config`）；12 套内置主题与逐 token 覆盖；
  `--default-config` 输出全部字段；`prefix shift+r` 热重载。

### 发行

- `linux x86_64 musl` 静态产物（`just release-musl`），`just dist` 生成 tar.gz 与 sha256。
- CI：fmt / clippy / test（ubuntu + macOS）、musl 构建；标签 `v*` 触发
  `.github/workflows/release.yml` 发布 Linux 与 macOS 产物。

### 测试

- 65 个单元测试、2 个真实 core 端到端测试（attach → 首帧 → 输入回环 → 大流量 → 重连）。
- `scripts/ui-smoke.sh`：真实 core + tmux 的 UI 冒烟（绘制、Tab/分屏、跨重启恢复、
  Copy Mode、reader、三种自定义命令）。
