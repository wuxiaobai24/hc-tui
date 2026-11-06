# 开发、验证与本机部署

[返回 README](../README.md) · [发布检查清单](release.md)

## 构建

需要 Rust stable、Zig **0.16.0**。mise.toml 固定 Zig 版本，系统默认 0.17 会导致 vendored VT 构建失败。
运行与真实 core 测试需要 PATH 中的 hc；当前完整功能验收基线为 core 0.1.18。
首次构建可能下载 Cargo/Zig 依赖，离线构建需要已有缓存。

```sh
mise install
mise exec -- cargo build -p hc-tui
mise exec -- cargo build --release -p hc-tui
# 没有 mise：ZIG=/absolute/path/to/zig-0.16.0 cargo build --release -p hc-tui
```

## 模块结构

| 位置 | 职责 |
|---|---|
| src/app.rs / src/ui/ | 事件循环、状态和 Ratatui 绘制、手机与 Chat |
| src/app/actions.rs | 命令面板、CLI、action 共用分发器及生命周期 |
| src/app/shared_sync.rs / src/shared.rs | 多机共享文档、稳定 ID、CAS 三方合并 |
| src/app/agent_tree.rs | Agent 父子树、优先级、折叠与环保护 |
| src/app/mouse_resize.rs / src/layout/ | 鼠标调整、BSP/Stack、布局预设 |
| src/app/panels.rs / layouts.rs | 脚本模块刷新、个人克隆 |
| src/control.rs | Unix 本地控制 socket，仅 UI 线程修改状态 |
| src/hc/ | hc connect 子进程、framed 协议、异步 RPC、attach、credit 与重连 |
| src/term/ | 每 Pane 的本地 VT、输入、主题颜色、Kitty 图形 |
| src/model.rs / config.rs / theme.rs | 对象、持久化 schema、配置与外观 |
| crates/ghostty-vt / vendor/libghostty-vt | Rust VT 包装与固定版本的 C/Zig 终端模拟器 |

core 持有终端和进程；TUI 持有布局、VT、attach 意图和显示状态。
收到流字节后先写入本地 VT，再回 CREDIT；事件流也要还 credit。
普通断线保持 attach 意图，程序退出则移除视图。keep 恢复属于 core，不在 TUI 静默换 shell。

## 验证入口

```sh
cargo fmt -p hc-tui -- --check
mise exec -- cargo clippy -p hc-tui --all-targets -- -D warnings
mise exec -- cargo test -p hc-tui
HC_TUI_E2E=1 mise exec -- cargo test -p hc-tui --test g1_attach --test multi_pane
mise exec -- just smoke
mise exec -- just parity
```

没有 just 时执行等价命令：

```sh
mise exec -- cargo build -p hc-tui
HC_TUI_BIN=target/debug/hc-tui bash scripts/ui-smoke.sh
python3 scripts/herdr-parity-smoke.py
```

普通 cargo test 中真实 core 测试会跳过主体；必须显式设置 HC_TUI_E2E=1 才算端到端验收。
UI 冒烟使用 tmux 和独立 session；可设 HC_TUI_SMOKE_ROOT 指向新的临时目录。
parity 启动两套隔离 core，验证远端布局、新建、GUI 双向 CAS、未知字段、settle、park/keep/close/quit。
不要让测试使用默认工作区。真实宿主图像、SSH 实机、macOS/Windows 不因 Linux tmux 测试通过而算已验收。

## 安装、升级与回滚

先完成验证，安装 release 产物。为兼容仍在运行的旧进程，使用新文件加 rename 原子替换：

```sh
mkdir -p ~/.local/bin
cp -p ~/.local/bin/hc-tui ~/.local/bin/hc-tui.previous  # 已有安装时备份
install -m 755 target/release/hc-tui ~/.local/bin/hc-tui.new
mv -f ~/.local/bin/hc-tui.new ~/.local/bin/hc-tui
command -v hc-tui
hc-tui --version
hc-tui commands
sha256sum target/release/hc-tui ~/.local/bin/hc-tui
```

已经运行的进程继续使用旧二进制；前缀 q detach 后重新启动才是新版。
不要为了升级杀掉 core 或业务终端。需要回滚时同样先复制备份到 .new，再 mv 覆盖。
只看 --version 不足以区分同版本的不同构建，部署时比较哈希。

## 发行包

just release-musl / just dist 使用 cargo-zigbuild 与 x86_64-unknown-linux-musl，
需先安装 cargo-zigbuild 和 Rust target。发行包为 hc-tui-<version>-<target>.tar.gz
及同名 .sha256，内含二进制、LICENSE、第三方声明与 CHANGELOG。
运行发行包仍需外部 hc；“静态链接”不表示无需 core。

当前 release 工作流由 v* 标签或 workflow_dispatch 触发，构建 Linux musl 和 macOS
两种架构；publish 至少要求 Linux 成功。平台支持与是否交互验证分开记录。
本机部署不等于 GitHub 发布，提交也不等于已经 push；发布步骤见发布清单。
