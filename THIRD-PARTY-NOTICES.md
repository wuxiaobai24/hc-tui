# 第三方组件与许可

本仓库的二进制会链接下列组件。发布前需按实际链接结果复核本文件
（`cargo deny` / `hc --licenses` 可作为交叉检查）。

## 本项目

- **hc-tui** — Apache-2.0，见 [LICENSE](../LICENSE)

## vendored 的终端模拟器

- **ghostty-vt**（`crates/ghostty-vt`，Rust 封装）
  - 来源：herdr TUI
  - 许可：Apache-2.0
- **libghostty-vt / Ghostty**（`vendor/libghostty-vt`，C/Zig 库）
  - 上游：https://github.com/ghostty-org/ghostty
  - 固定 commit：`44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`（见 `vendor/libghostty-vt.vendor.json`）
  - 许可：**MIT**（Copyright © 2024 Mitchell Hashimoto, Ghostty contributors）
  - 注意：Ghostty 与 libghostty-vt 是 MIT，不是 Apache-2.0；两者都要保留声明。

## Rust 依赖

由 `Cargo.lock` 固定，主要包括：

| crate | 许可 |
|---|---|
| ratatui | MIT |
| crossterm | MIT |
| tokio | MIT |
| serde / serde_json | MIT / Apache-2.0 |
| tracing / tracing-subscriber | MIT |
| thiserror | MIT / Apache-2.0 |
| base64 | MIT / Apache-2.0 |
| png | MIT / Apache-2.0 |
| libc | MIT / Apache-2.0 |

## Ghostty 构建期依赖

`vendor/libghostty-vt/build.zig.zon` 声明并在链接时使用：

- `translate_c`
- `libxev`
- `vaxis`

这些会被静态链接进最终二进制，发布时同样需要声明许可。首次构建可能从网络拉取，
离线构建需预置 Zig 缓存。

## 未启用

`crates/ghostty-vt` 里的 kitty 图像落盘路径（`pane_graphics_files`、`native_image_sources`）
当前不启用；启用前需单独审计其文件写行为。
