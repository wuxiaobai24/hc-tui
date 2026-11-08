---
title: 安装
summary: 用发行包装到 ~/.local/bin；升级原子替换，失败不覆盖旧版本。
order: 1
spec: development
---

## 前置条件

运行需要 herdr-core 的 `hc` 在 PATH 中；已验证的协议基线为 core 0.1.18。发行二进制不需要 Rust 或 Zig。

出处：`README.md` · 安装与升级。

## 安装

`install.sh` 同时支持 `sh install.sh` 与 `curl | sh`，自动识别 Linux/macOS 和 amd64/arm64：

```sh
sh install.sh
sh install.sh --version 0.4.2 --bin-dir "$HOME/.local/bin"
```

安装目录也可用 `HC_TUI_INSTALL_DIR` 指定，默认 `~/.local/bin`。脚本下载正式发行包，验证 SHA-256 与版本，成功后原子安装；不会改动现有配置或 core 中的程序。

出处：`README.md` · 安装与升级、`install.sh:1-60`。

## 发行目标

| 平台 | 发行 target |
| --- | --- |
| Linux amd64，musl 静态 | `x86_64-unknown-linux-musl` |
| Linux arm64，musl 静态 | `aarch64-unknown-linux-musl` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |

macOS 包独立携带 VT 静态库，仍使用系统库，不是完全静态的 Mach-O。

出处：`README.md` · 手动下载发行包。

## 升级

```sh
hc-tui update --check
hc-tui update
hc-tui update --version 0.4.2
```

升级使用 curl、tar 及 sha256sum（macOS 为 shasum），本地包也必须有同名 `.sha256`。已运行的客户端继续用旧版本，重开后生效。

出处：`README.md` · 安装与升级。

## 源码构建

```sh
mise install
mise exec -- cargo build --release -p hc-tui
```

需要 Rust stable 与 Zig 0.16.0；当前 vendored VT 不支持 0.17。

出处：`README.md` · 附录：源码构建、`docs/development.md`。
