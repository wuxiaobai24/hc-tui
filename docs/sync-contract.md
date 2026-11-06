# 共享状态契约

产品定位：接 herdr-core、与 herdr-app 共享布局的终端工作台。兼容对象是共享数据与 core 能力，不是某一 Desktop 版本的控件目录。

| 状态 | 必须双向 | 存储与回归 |
|---|---|---|
| Space/Tab/Pane 树、稳定 ID、终端地址、名称、目录 | 是 | 每台机器的 `herdr-app` 文档；保留未知字段、模块 props 和未知 look；CAS 三方合并 |
| Agent 已读、置顶 | 是 | `herdr.seen` / `herdr.pinned` 终端 labels；GUI 与 TUI 相互可见 |
| 当前 Space/Tab/Pane、焦点、zoom | 否 | 客户端屏幕状态，不回写共享结构 |
| settle | 否，遵循当前 App 语义 | App 类型声明与行为说明明确是“put away on this screen”；TUI 存在自己的恢复文档。跨端同步需先改变 App 契约，不能单方添加假字段 |
| 主题、快捷键、命令面板、侧栏折叠与滚动 | 否 | 本机配置/屏幕状态 |
| Pi JSONL 正文 | 不写共享文档 | session label 提供路径，正文在该终端所在机器读取；标明本机映射/推断、远端映射/失败 |

App 的未知字段可随往返保留，但不代表 TUI 承诺理解或操作它们。屏幕改变不应为了“同步”改写 App 的选中项、主题和 settle。

## core/App 版本变化时的最小验收

```sh
mise exec -- cargo test -p hc-tui shared
mise exec -- just contract
```

`just contract` 在隔离的本地 core 和第二个 core 上验证稳定 ID、多机路由、树与名称双向修改、未知字段/样式保留、已读/置顶 labels 与本地 settle。没有端到端跑过的 core/App 组合不宣称兼容。

独立 UI 美化、主题选择、Desktop 原生窗口按钮和命令面板不作为每次 core 小版本变化的回归清单。
