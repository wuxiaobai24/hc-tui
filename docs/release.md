# 发布检查清单

发布对象：`hc-tui v<version>`，产物见 README「发行」。

## 1. 版本与文案

- [ ] `Cargo.toml` 的 `version` 已更新，`hc-tui --version` 与之一致。
- [ ] `CHANGELOG.md` 增加对应版本段落。
- [ ] `THIRD-PARTY-NOTICES.md` 与实际链接的 crate 一致（新增依赖时补行）。
- [ ] README 的键位、配置示例与「尚未做」列表与代码一致。

## 2. 构建环境

- [ ] Zig **0.16.0** 在 `PATH` 最前（0.17 编译失败，见 ADR-0001）。
- [ ] `rustup target add x86_64-unknown-linux-musl`。
- [ ] `cargo install cargo-zigbuild`（musl 交叉构建需要）。
- [ ] 本机 `hc>= 0.1.17` 可用于端到端与冒烟测试。

## 3. 验证

```sh
cargo fmt -p hc-tui -- --check
cargo clippy -p hc-tui --all-targets -- -D warnings
cargo test
HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane -- --nocapture
just smoke
```

- [ ] 以上全部通过。
- [ ] `just dist` 产出 tar.gz 与 sha256。
- [ ] 覆盖本机安装：`install -m 755 target/x86_64-unknown-linux-musl/release/hc-tui ~/.local/bin/hc-tui`，
      用 `hc-tui --default-config | grep 'settings ='` 确认 PATH 上的是新构建（版本号未变时
      只看 `--version` 分不出新旧）。
- [ ] `sha256sum -c` 通过；解包后 `./hc-tui --version`、`--default-config` 正常。
- [ ] 解包产物在干净环境的 `hc` 上能起 TUI（`hc ls` 有可 attach 的终端）。

## 4. 打标签与发布

```sh
git add -A
git commit -m "release: v<version>"
git tag -a v<version> -m "hc-tui v<version>"
git push origin main --tags
```

- [ ] 推送标签后 `.github/workflows/release.yml` 的 Linux musl 构建成功。
- [ ] GitHub Release 资产齐全：Linux musl 及其 `.sha256`。
      需要 macOS 产物时手动触发该工作流并勾选 `macos`，两个 target 各自也要有 `.sha256`。
- [ ] Release 正文取自 `CHANGELOG.md`。

## 5. 发布后

- [ ] 用发布包在干净机器上手工走一遍：attach、分屏、Copy Mode、重启恢复、`prefix i` 发消息。
- [ ] 记录已知问题：kitty 虚拟 placement 未渲染、`machine.exec` 无 UI 入口、
      worktree / Agent 会话续接 / 插件未做。
