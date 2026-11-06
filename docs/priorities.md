# 工作主线

定位：接 herdr-core、与 herdr-app 共享布局的终端工作台。按多机 + Pi 日用体验排序。

1. **P0 输入与渲染**：nvim 全屏与分屏、英文重复字符、同步重绘/半帧、SSH 延迟下的按键和 IME、大段 bracketed paste。新增真实问题优先固定原始字节回放，保留 core → VT → Ratatui → 宿主的检查链；无法复现的报告不标成已修复。
2. **远端 Pi Chat**：沿 `machine/` 地址用 core 读取准确的 session JSONL，显式标明映射、推断、未映射或失败。没有 hook 时宁可显示远端未映射，不猜本机会话。
3. **共享布局边界**：只承诺 [同步契约](sync-contract.md) 中的数据；不追旧 Desktop 每个按钮。settle 当前属于屏幕本地，若改变必须有 App 端契约配合。
4. **二进制交付**：Linux amd64/arm64 musl、macOS Intel/Apple Silicon，归档、校验和、原子 update；普通使用者不用安装 Zig。源码构建作为维护者附录。

远端入口使用 `ssh box 'hc-tui --session work'`，或本地 core 的 `hc machine` 路由；不另建传输协议。手机 switch 与新增主题后置。

Diff 审阅交给 revdiff / Plannotator 等外部工具，TUI 只通过 `mod.open` 或自定义命令接一个外部 Pane；不增加内置审阅引擎。
