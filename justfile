# 常用命令。没有 just 也可以直接看这里的 cargo 调用。

version := `awk -F'"' '/^version = /{print $2; exit}' Cargo.toml`

default:
    @just --list

fmt:
    cargo fmt -p hc-tui

lint:
    cargo clippy -p hc-tui --all-targets -- -D warnings

test:
    cargo test -p hc-tui

run:
    cargo run -p hc-tui

# 真实 core 的端到端验收（隔离 session，默认在 cargo test 里跳过）
e2e:
    HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane -- --nocapture

probe:
    cargo run -p hc-tui -- probe

# 插件动作的端到端冒烟（隔离 XDG 与 hc session；需要 tmux、hc、jq）
plugins-smoke: build-smoke
    bash scripts/plugins-smoke.sh

# 真实 core + tmux 的 UI 冒烟测试（隔离 session；需要 hc、tmux）
smoke: build-smoke
    HC_TUI_BIN=target/debug/hc-tui scripts/ui-smoke.sh

# 与 GUI 共享文档 + 双 core 机器传输 + 控制命令的契约验收。
parity: build-smoke
    python3 scripts/herdr-parity-smoke.py

# Stable shared-data contract for core/App upgrades.
contract: parity

build-smoke:
    cargo build -p hc-tui

# 静态发行构建（需要 rustup target add x86_64-unknown-linux-musl 与 cargo-zigbuild）
release-musl:
    cargo zigbuild --release --target x86_64-unknown-linux-musl -p hc-tui

# Package a Linux static executable with the same script used by four-platform CI.
dist: release-musl
    bash scripts/package-release.sh x86_64-unknown-linux-musl

# 升级 vendored libghostty-vt（流程见 docs/adr/0001-ghostty-vt.md §5）
vendor *ARGS:
    python3 scripts/vendor_libghostty_vt.py {{ARGS}}

bindings *ARGS:
    bash scripts/generate_libghostty_bindings.sh {{ARGS}}
