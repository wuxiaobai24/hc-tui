# core 泄漏治理：判定、清理与回归

[返回 README](../README.md) · [开发、验证与本机部署](development.md)

## 为什么会有这么多 core

`herdr-core` 的常驻进程是 `herdr-core serve`，一个会话一个。它会由下面几条路径留下：

| cgroup | 来源 | 是否该留 |
|---|---|---|
| `hc-serve-<hash>.service` | 会话 core 本体的 transient 单元 | 只有还活着的会话该留 |
| `hc-hold-<hash>.service` | `herdr-core hold --socket <session>.holder.<hash>.sock`，保活连接、**持有终端的 pty 与子进程** | 只有还活着的会话该留 |
| `tmux-spawn-*.scope` / `dsh-subprocess-*.scope` | 测试脚本在隔离 runtime 里起的 core，脚本退出后没人收 | 不该留 |
| `docker-*.scope` | 容器内部的 core（`/usr/local/bin/herdr-core`） | 归容器，宿主不管 |

三条已经实测的结论，别推翻：

1. **kill holder 不会让 core 退出**。holder 只是保活连接；core 是独立进程/单元。反过来，holder 一死，它持有的那些终端（shell 子进程）才跟着结束——所以清理顺序是「先 core，后 holder」。
2. **`hc server status` 不能用来判断存活**。它是客户端命令，会话 core 不在时会按需拉起一个（`uptime_ms` 只有几毫秒就是这个）。
3. **core 的 cmdline 里没有会话名**。`/proc/<pid>/cmdline` 只有 `herdr-core serve`；会话名只能从它监听的 unix socket 路径读。

## 怎么判断一个 core 属于谁

会话 socket 的规则是 `<runtime>/herdr-core/<session>.sock`，其中 `runtime` 就是 core 启动时的 `XDG_RUNTIME_DIR`：

```sh
ss -xlp | grep herdr-core          # 拿到 pid 与它监听的 socket 路径
```

于是「会话 = (runtime, session)」而不是只有会话名。同一个 `default` 出现在 `/tmp/hc-tui-chat-smoke/run/herdr-core/default.sock` 时，那是测试用隔离 runtime 起的另一个 core，与 `/run/user/1000/herdr-core/default.sock` 无关。

**socket 所在目录还在不在，不能用来判死活**：本机实测 133 个 core 里 125 个的 socket 目录都还在（测试脚本没删 `/tmp` 目录，只是把 core 留下了）。能区分的是 runtime 目录：规范 runtime 之外的会话一律是测试残留。

holder 同理，路径是 `<runtime>/herdr-core/<session>.holder.<hash>.sock`；`herdr-core hold --socket <path>` 的 cmdline 里有完整路径，读它即可。

终端挂在 holder 上，所以「这个会话是不是真有人在用」看 holder 的子进程数（`cat /proc/<holder>/task/*/children`），不看 core。

## 清理

```sh
scripts/reap-cores.sh                        # 干跑：分类表 + 会杀谁，不动进程
scripts/reap-cores.sh --evidence-dir DIR     # 干跑并把清单写到 DIR/reap-plan.txt
scripts/reap-cores.sh --apply --evidence-dir DIR   # 执行，并把结果写到 DIR/reap-result.txt
```

分类规则（脚本头部注释与这里保持一致）：

| 分类 | 含义 | 处置 |
|---|---|---|
| `protect` | 规范 runtime 下的 `default`、`doc1`，以及任何 `hcp-*` 会话 | 不动 |
| `protect-unknown` | 规范 runtime 下的其它会话名 | 不动，打印出来人工确认 |
| `skip-docker` | cgroup 是 `docker-*.scope` | 不动 |
| `skip-nosock` | 拿不到会话 socket（刚启动还没 bind，或扫描时已退出） | 不动 |
| `leak-name` | 会话名是测试残留：`hctui-*`、`hcw*`、`probe*`、`osc-probe*`、`tmp*`、`pi-ext-test*` | 清理 |
| `leak-runtime` | 会话 socket 不在规范 runtime 目录下 | 清理 |

只有 `leak-*` 会被动刀。执行顺序：

1. 先 `systemctl --user stop` 那些只养着 leak 进程、且仍 active 的 `hc-serve-*` / `hc-hold-*` 单元——系统会把该单元的 cgroup 一起收掉，holder 持有的终端不会变成孤儿。同一个单元里只要还有 `protect` / `protect-unknown` / `skip-*` 进程，整个单元就不停。
2. 再对剩下的 core 发 `kill -TERM`（dead 单元的残留、scope 里的孤儿）。
3. 最后对剩下的 leak holder 发 `kill -TERM`。

安全边界写在脚本里：自己的祖先链（本进程 → shell → 所在 holder）一律跳过；只发 `kill -TERM`，不用 `kill -9` 兜底；pid 在发信号前还要复查 cmdline，防止 pid 复用打错人。

## 回归指标

```sh
pgrep -c -f 'herdr-core serve'
pgrep -c -f 'herdr-core hold'
HC_SESSION=default hc ls | python3 -c 'import sys,json; print(len(json.load(sys.stdin)["terminals"]))'
```

2026-11-08 本机一次清理：core 139 → 13，holder 17 → 8，default 30 个终端与 doc1 6 个终端在清理后照常列出。剩下的 13 个 = 10 个 `protect`（default、doc1、`hcp-*` 若干）+ 3 个容器内的 `skip-docker`。

注意清理时可能有别的进程正在跑测试：脚本扫到的新 core 会按规则重新分类，`hcp-*` 一律保留，刚启动还没有 socket 的 `skip-nosock` 也保留。清理完再跑一次干跑应当是 0 个 `leak-*`。
