# 发布检查清单

运行用户从 [Releases](https://github.com/wuxiaobai24/hc-tui/releases) 获取二进制，再用 `hc-tui update`；Rust/Zig 只属于贡献者的构建环境。

## 发布前

- 安装脚本变更先运行 `sh -n install.sh` 和 `python3 scripts/test-install.py`；发行包与 Release 均携带脚本。
- 更新 Cargo.toml / Cargo.lock 版本和 CHANGELOG，tag 必须是 `v<package version>`。
- 以 Zig 0.16.0 构建当前 vendored VT，四平台 CI 统一固定；不要让使用者自行安装它。
- `cargo fmt -p hc-tui -- --check`、Clippy、单元测试通过。
- `HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane` 通过。
- `just contract` 回归双 core 共享布局、labels 和远端 Pi Chat。
- `just smoke` 回归交互、窗口和机器折叠；nvim 固定回放见 tests/fixtures/README.md。
- 核对 [同步契约](sync-contract.md)，不要把本地主题、焦点或 settle 混进共享文档。

## 四平台构建与打包

`.github/workflows/release.yml` 覆盖：

| 系统 | target | 验证 |
|---|---|---|
| Linux amd64 | x86_64-unknown-linux-musl | 静态链接、原生 --version/default-config |
| Linux arm64 | aarch64-unknown-linux-musl | 静态链接、QEMU --version |
| macOS Intel | x86_64-apple-darwin | 原生 --version/default-config |
| macOS Apple Silicon | aarch64-apple-darwin | 原生 --version/default-config |

macOS 的 VT 静态链接，但系统库不是完全静态。四套包都不依赖用户机器上的 Zig。
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
