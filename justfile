# 常用命令。没有 just 也可以直接看这里的 cargo 调用。

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

probe:
    cargo run -p hc-tui -- probe

# 升级 vendored libghostty-vt（流程见 docs/adr/0001-ghostty-vt.md §5）
vendor *ARGS:
    python3 scripts/vendor_libghostty_vt.py {{ARGS}}

bindings *ARGS:
    bash scripts/generate_libghostty_bindings.sh {{ARGS}}
