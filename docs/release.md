# 发布检查清单

运行用户从 [Releases](https://github.com/wuxiaobai24/hc-tui/releases) 获取二进制，再用 `hc-tui update`；Rust/Zig 只属于贡献者的构建环境。

## 版本与节奏

- 后续版本按 `0.4.3 → 0.4.4 → 0.4.5` 递增，即每轮发布只增加 patch（`0.0.1`），不自动增加 minor。
- 同一轮功能和修复合并、完成验证后统一发布，不为每个修复连续发布补丁版本。
- 以最新已发布版本为基准，例如 `0.4.2` 的下一版是 `0.4.3`；保留已公开的版本与 tag。
- 开发期间在 CHANGELOG 的“未发布”中累计变更；准备发布时再统一更新包版本、日期并打 tag。

## 发布前

- 安装脚本变更先运行 `sh -n install.sh` 和 `python3 scripts/test-install.py`；发行包与 Release 均携带脚本。
- 更新 Cargo.toml / Cargo.lock 版本和 CHANGELOG，tag 必须是 `v<package version>`。
- 以 Zig 0.16.0 构建当前 vendored VT，四平台 CI 统一固定；不要让使用者自行安装它。
- `cargo fmt -p hc-tui -- --check`、Clippy、单元测试通过。
- `HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane` 通过。
- `just contract` 回归双 core 共享布局、labels 和远端 Pi Chat。
- `just smoke` 回归交互、窗口和机器折叠；nvim 固定回放见 tests/fixtures/README.md。
- 核对 [同步契约](sync-contract.md)，不要把本地主题、焦点或 settle 混进共享文档。

## 依赖与体积基线

M2 引入 nsh（`nsh-platform-tui` / `nsh-engine`，固定提交 `951556dc1af6f5acf593cfd28640a0fd467b71e3`）后的冷构建探针基线，供后续里程碑对比：

| 目标 | M1（无 nsh） | M2（含 nsh 探针） | M2 体积 |
|---|---|---|---|
| x86_64-unknown-linux-musl | 约 16 s，5.65 MiB | 约 106 s，5.78 MiB | 5,783,656 B |
| aarch64-unknown-linux-musl | 未测 | 约 182 s，5.22 MiB | 5,218,096 B |
| x86_64-apple-darwin / aarch64-apple-darwin | 未测 | 由 release CI 构建验证 | 未测 |

说明：

- 机器为 32 线程 Linux，release + zigbuild，冷目标目录；时间仅供趋势对比，不作绝对门限。
- nsh 尚未接线（M3/M4 才嵌入帧与宿主通道），链接器剔除未引用代码，发行二进制增量约 +136 KiB（约 +2.4%）；接线后需重测并更新本表。
- `oxc`、`quickjs-jit` 的编译开销主要在 dev-dependencies（探针测试），不影响发行构建时间；`nsh-engine` 的 `js`/`net` 特性带来 QuickJS、`ureq`/`rustls` 等运行时依赖。
- 体积或编译时间出现明显跳变时，先关未用 feature，再考虑裁剪 `nsh-base` 可选组件。

## M8 实测：四目标现状、体积与性能

测量机：本机 16 线程 Linux，`zig 0.16.0`，`cargo-zigbuild 0.24.1`（装在 `/tmp/m8-tools`，未改用户环境），
`--locked --release`。

| 目标 | 本机结果 | 说明 |
|---|---|---|
| x86_64-unknown-linux-musl | ✅ 构建 46,487,512 B（44.33 MiB），`file` 报告 statically linked；解包后 `--version`/`--default-config` 正常 | `cargo zigbuild` |
| aarch64-unknown-linux-musl | ✅ 构建 41,800,296 B（39.86 MiB），`qemu-aarch64 … --version` 输出 `hc-tui 0.4.3` | `cargo zigbuild` |
| aarch64-apple-darwin | ❌ | 交叉链接需要 Apple SDK：`cargo build` 在 `ring` 的 C 步骤因 Linux `cc` 不认 `-arch arm64` 失败；`cargo zigbuild` 走到链接阶段报 `unable to find framework 'CoreFoundation'`（无 `SDKROOT`/Xcode）。需 macOS runner。 |
| x86_64-apple-darwin | ❌ | 本机未安装该 rust target（`rustup target add` 也未做），且同样缺 Apple SDK。需 macOS runner。 |

打包与安装脚本：

- `bash scripts/package-release.sh x86_64-unknown-linux-musl` 生成 `dist/hc-tui-0.4.3-x86_64-unknown-linux-musl.tar.gz`（19,388,210 B）与 `.sha256`；解包后静态二进制可运行。
- `sh -n install.sh` 通过；`python3 scripts/test-install.py` 6 项全过（含平台/版本校验、截断管道无副作用）。

### 体积与性能对比（M1 → M8）

方法：`git archive 6917533` 到 `/tmp/hc-tui-m1` 构建同一 profile；`scripts/perf-baseline.sh` 在隔离
XDG/session 下测同一台机器。延迟为按键到该字符出现在 pane 输出中的端到端时间（默认 `render_fps = 30`，
理论下限约 33 ms）。

| 指标 | M1 | M8 | 结论 |
|---|---|---|---|
| 发行体积 musl amd64 | 5,647,592 B（5.39 MiB） | 46,487,512 B（44.33 MiB） | 8.2×；见下分解 |
| 发行体积 musl arm64 | 未测（M2 记录 5.22 MiB） | 41,800,296 B（39.86 MiB） | 同上 |
| 本机 GNU 发行体积 | 7,231,376 B | 66,427,456 B | 9.2×（未 strip，符号未剔除） |
| 常驻内存（空闲 2 s 后 VmRSS） | 8,052 / 8,088 / 8,152 KB | 14,312 / 14,476 / 14,448 KB | +约 6.2 MB：nsh + QuickJS 运行时 |
| 空闲 CPU（10 s，30 fps 循环） | 1.5 % / 1.7 % | 1.7 % / 1.7 % / 1.8 % | 无可见差异 |
| 按键延迟 p50（20 次） | 38.1 / 38.3 / 39.5 ms | 38.3 / 38.4 / 38.9 ms | 无可见退化 |
| 按键延迟 p90 | 39.0 / 39.0 / 39.9 ms | 38.8 / 39.2 / 38.9 ms | 无可见退化 |

体积增长分解（`cargo bloat --crates`，M8）：`nsh_base` 10.1 MiB、`nsh_core` 3.2 MiB、`quickjs_jit_stdlib`
2.5 MiB、`hc_tui` 2.2 MiB、`nsh_js` 1.7 MiB、`std` 1.6 MiB、`nsh_platform_tui` 1.1 MiB、`rustls` 0.55 MiB，
其余为依赖碎片；`.text` 合计约 32.8 MiB。也就是说 M8 的体积回归来自 M4 把 nsh 运行时真正接入（M2 只是
链接未引用代码，被链接器剔除），不是 M8 事件流引入。

已尝试的处理与结论：

- 把 `nsh-engine` 的 feature 从 `js` 降到 `js-core`：`nsh-platform-tui` 无条件启用 `nsh-engine/js`，
  Cargo 特性合并后无变化（构建 0.2 s，体积不变），本仓无法单方裁剪。
- `[profile.release] lto = "thin", codegen-units = 1`：musl amd64 46.5 → 42.4 MiB（−8.9%），但构建
  2 min → 4 min 56 s；未采纳，记录为可选处置。
- 二进制已由 zigbuild 产成 stripped；再执行 `strip` 体积不变。

## 四平台构建与打包

`.github/workflows/release.yml` 覆盖：

| 系统 | target | 验证 |
|---|---|---|
| Linux amd64 | x86_64-unknown-linux-musl | 静态链接、原生 --version/default-config |
| Linux arm64 | aarch64-unknown-linux-musl | 静态链接、QEMU --version |
| macOS Apple Silicon | aarch64-apple-darwin | 原生 --version/default-config |

macOS Intel（`x86_64-apple-darwin`）自 v0.5.0 起不再发布：GitHub 的 Intel runner 构建一次要十几分钟，
而实际使用面已经很小。Intel Mac 用户从源码构建即可（`cargo build --release --locked`），
`install.sh` 在该平台会直接给出这条提示，而不是去下载一个不存在的包。

macOS 的 VT 静态链接，但系统库不是完全静态。三套包都不依赖用户机器上的 Zig。
统一通过 `scripts/package-release.sh TARGET` 生成 tar.gz 及 `.sha256`，包内含二进制、许可、第三方声明、变更记录、README 和 install.sh。
Release 另外发布 `install.sh` / `install.sh.sha256`，固定入口为 `releases/latest/download/install.sh`；发布前在 Linux/macOS 跑安装器的隔离回归。
**四个构建全部成功且八个资产校验完整才发布**，不再因为 Linux 一项成功就发布缺平台的 Release。

推送 `v<version>` tag 触发发布；也可手动选择已存在的 tag 重跑。不移动已经公开发布的 tag。

## 发布后

- 从 Release 下载当前平台包，执行 `.sha256` 校验并实际解包运行。
- 用隔离的二进制副本测试 `update --from archive.tar.gz`；损坏 checksum 必须拒绝，原二进制保持不变。
- `update --check` 指向本仓库；`update` 校验包 hash、可执行版本后才在目标目录原子 rename，失败不覆盖旧版本。
- 本机安装采用“备份旧二进制 → 写新文件 → rename”，重开客户端生效，正在 core 中运行的程序不受影响。
- 不把自动回放称为所有物理终端、IME 或实体 SSH 主机已验收；未复现的输入报告继续保留。
