# ADR-0001：终端模拟器采用原 Herdr TUI 的 ghostty-vt 封装

- 状态：**Accepted**
- 日期：2026-11-04
- 决策：vendor 原 Herdr TUI 的 `crates/ghostty-vt`，用 `term-view` 适配层隔离；上游 libghostty-vt 按 vendor 元数据固定；**不**依赖 herdr-core 自带的 `ghostty-vt` crate。
- 相关：[plan-v1.1.md](../plan-v1.1.md)、[review-v1.0.md](../review-v1.0.md)

---

## 1. 背景

hc 的 RPC 只发**原始 VT 字节**和首帧 ANSI，没有「带样式的 cell 流」。因此客户端必须自己维护一份 VT 才能画 Pane。可选的模拟器实现有两套，都包同一套 C 库 `libghostty-vt`：

| | hc 自带（`crates/ghostty-vt`） | 原 TUI（`crates/ghostty-vt`） |
|---|---|---|
| 体量 | ~1.2k 行，32 KB | ~12k 行，716 KB |
| Cell 级样式 | ✗（只有 `screen_rows()` 文本、`paint()` ANSI） | ✓ `RenderState` / `RowIterator` / `RowCells` / `CellStyle` |
| 键鼠编码 | 有 FFI 声明，公开 API 很少 | ✓ `KeyEncoder` / `MouseEncoder` |
| 滚动/链接/图像 | 部分 | ✓ |
| 钉住的 Ghostty | `e5cdb86`（submodule，较新） | `44f2a44`（1.3.2-dev，tarball 在树） |
| 许可 | Apache-2.0（crate）；libghostty-vt 为 MIT | 同 |

结论很直接：hc 的封装不足以渲染 Pane，原 TUI 的封装是唯一现成的 cell 级实现。

---

## 2. 决策

采用原 TUI 的封装，但**按「自维护 fork」管理**，不是「复制粘贴一段代码」：

1. 整个 crate 原样迁入 `crates/ghostty-vt`（Apache-2.0），连同 `vendor/libghostty-vt.vendor.json`、`scripts/vendor_libghostty_vt.py`、`scripts/generate_libghostty_bindings.sh`。
2. 上游源码 `vendor/libghostty-vt`（MIT）按元数据固定为 `44f2a44`。
3. 所有对它的使用收敛到 `term-view` 一个模块，上层代码不出现 `ghostty_vt::` 类型。
4. 不引入 hc 的 `ghostty-vt`：两者都声明 `links = "ghostty-vt"`，Cargo 不允许同一图里出现两份。

---

## 3. 依据

- **能力**：唯一能直接给出 cell 级样式与键鼠编码的现成实现，桥接代码（约 700 行）已在原 TUI 里验证过。
- **可维护**：附带 vendor 元数据、下载脚本、bindgen 生成脚本和一致性测试，升级路径是可重复的，不是手工改 465 KB 生成代码。
- **已验证（2026-11-04）**：
  - Zig 0.16.0 下编译通过，release 全量 **1m38s**。
  - 用真实 hc 首帧（392 B）还原：调色板红、下划线绿、真彩 `RGB(12,34,56)`、反显、宽字符列宽、光标 `(0,4)` 全部正确。

---

## 4. 风险与缓解

| # | 风险 | 表现 | 缓解 |
|---|---|---|---|
| 1 | 工具链 | Zig 是 pre-1.0；0.17 删了 `Build.build_root`，编译硬失败 | `mise.toml` 钉 `zig@0.16.0`；CI 前置版本检查；build.rs 已有明确报错 |
| 2 | 版本错配 | core 用较新 Ghostty 生成首帧，客户端用较旧版本解析；新序列/宽度表/图形协议可能错位 | 录制真实语料做差分回归；需要时按 §5 升级 |
| 3 | 所有权 | 12k 行 FFI 代码需自己升级、自己背 render state 语义 | 收敛到 `term-view`；升级有脚本；每次升级跑语料 |
| 4 | 构建/发行 | 静态库 + Zig 交叉编译；musl/macOS/Windows 各自验证 | CI 三平台；缓存 `zig-cache` 与 `OUT_DIR` |
| 5 | 供给链/许可 | 22 MB 上游源码；MIT 需单独声明；Zig 首次构建会拉 `translate_c`/`libxev`/`vaxis` | `THIRD-PARTY-NOTICES`；离线构建用已拉取的 Zig 缓存；vendor 一致性测试 |
| 6 | 安全面 | `pane_graphics_files` 会把 kitty 图像落盘 | 不启用该路径；若启用需单独审计 |

---

## 5. 升级流程

触发：渲染错位无法解释 / core 侧新增序列 / 需要 kitty 图像。

1. `scripts/vendor_libghostty_vt.py --source-repo <ghostty 检出> --source-commit <sha>`
   → 重新生成 `vendor/libghostty-vt` 与 `.vendor.json`
2. `scripts/generate_libghostty_bindings.sh`
   → 用 bindgen 重新生成 `src/bindings.rs`（allowlist：`Ghostty.*`）
3. `cargo build`，按编译错误修 `lib.rs` 里直接用到的结构与枚举
4. `tests/fixtures` 语料全绿
5. 更新 `THIRD-PARTY-NOTICES` 与版本记录

预算：C API 无大改时 0.5–1 天/次。

**为什么现在不同步到 core 的 `e5cdb86`**：那会同时改两个变量（C API + wrapper 代码），而 44f2a44 正是桥接与测试所针对的版本。顺序是「先用旧版把桥、语料、适配层跑绿，再单独做一次同步」，否则出问题分不清是桥写错还是版本不兼容。

---

## 6. 兜底判据

任一成立即切换方案，不要并行：

- G1 期间 5 个工作日内，仍不能在 sh / nvim / fzf / htop / pi 五个程序上稳定渲染与输入；
- 连续 3 个渲染 bug 的根因都是「解析不了 core 的新序列」。

**B 方案**：fork hc 的 `ghostty-vt`，补一层安全的 cell 读取（`grid_ref` / `CellStyle` / grapheme）。它已声明 `ghostty_terminal_grid_ref`、`KeyEncoder`、`MouseEncoder` 的 FFI，只差公开封装。代价：自写约 800–1500 行 FFI，换来与 core 完全同版本、不背 22 MB。只有在 A 方案被证伪时才值得。

---

## 7. 未验证

- 真实程序（nvim/fzf/htop/pi）的渲染与输入兼容
- 连续流吞吐（4 MiB/s）下的 credit 与重绘
- linux-musl / macOS-aarch64 交叉构建
- Zig 首次构建的离线可行性
