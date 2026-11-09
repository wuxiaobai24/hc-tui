# 测试会话守卫与 core 泄漏自检

## 为什么要守卫

对拍与集成测试每要一份干净状态，就会起一个隔离 runtime 的 `herdr-core`。
以前这些会话没有回收路径：跑完一套测试机器上留下几十个空转 core，每个几十
MB RSS，还占调度。会话由测试代码自己起，就只能由测试代码自己收——`Drop` 和
进程退出钩子比人工清理可靠。

## 新写对拍怎么起会话

集成测试进程里只有一个会话入口：`support::IsolatedCore`。

```rust
mod support;

#[test]
fn my_parity() {
    // 第一行就拿守卫：它建隔离 runtime、导出 XDG_* / HC_SESSION。
    let _core = support::IsolatedCore::new("my-parity");

    let app = App::new(config).expect("app builds");
    // ……断言……
}
```

规则：

- 一个测试进程只起一个会话。同一进程里多个用例各自 `new` 拿到同一个会话，
  tag 只用于会话名与目录名（`hctui-<tag>-<pid>`），首次调用的 tag 生效。
- 不要在 `Config.session` 里写死会话名，也不要再手工 `set_var("XDG_RUNTIME_DIR")`
  或 `HC_SESSION`；守卫已经把进程环境指到隔离目录。
- 守卫要活到测试结束（`let _core = …` 放函数开头）。最后一个守卫 `Drop`
  时回收；还有别的用例持有时不回收。
- 守卫同时是回收器：`Drop` 里先 `hc server stop`，再按 socket 路径找 core
  进程 `SIGTERM`/`SIGKILL`（`hc` 不可用时的退化路径），再杀
  `<session>.holder.*`，最后删隔离目录。两步都幂等，进程已死、目录已删、
  `hc` 不在都不会 panic，也不往 stderr 写东西。
- 进程正常退出（包括用例 panic 被 harness 收尾后继续跑完）时，`atexit`
  再兜一次：先杀掉本进程拉起的 `hc connect`，避免客户端重连把 core 拉起来。

## 测试不许依赖墙钟与固定帧数

`pump(app, N)` 里的 `N` 是帧数，不是时间：机器慢下来时一帧可能跨过一整秒，
拿帧数当秒表去等「通知该过期了」「props 该到了」，在慢 runner 上就是随机红。

- 等「某事发生」：用带超时的等待，条件成立就返回，超时才断言失败。
  单元测试（`src/app/mods/toast_tests.rs`）用 `wait_until`：

  ```rust
  // 超时门槛写秒数，条件写闭包；返回 false 就断言失败并带出消息。
  assert!(
      wait_until(&mut app, 3.0, |app| app.mod_notifications.len() == 1),
      "通知没有入队"
  );
  ```

  集成测试（`tests/m9g_toast_parity.rs`）用 `Harness::pump_until`：

  ```rust
  harness.pump_until("props arrive", |app| app.mod_notifications.len() == 1);
  ```

- 等「某事不会发生」：先等到安静（条数在一个时间窗口内不再变化）再记基线，
  断言只针对基线；同时把被测对象自己的超时设得够长（探针模块的 `notify`
  带 `timeout: 600`），别让「过期」伪装成「没有广播」。

- 断言「某条状态还在」的用例，超时要大于等待窗口：3 秒的等待配 4 秒过期
  的通知，在慢机器上仍然会先过期。

## 泄漏自检

```sh
scripts/check-core-leaks.sh            # 跑 HC_TUI_GRAPHICS=kitty cargo test
scripts/check-core-leaks.sh --lib      # 参数原样传给 cargo test
```

脚本记录跑批前后的 `pgrep -c -f 'herdr-core serve'`，结束后等 3 秒再数；
差值 > 1 或新增 core 超过 1 个就退出 1，并列出新 core 的 pid 与会话 socket
路径（会话名只能从 socket 读）。给 `hcp verify` 贴这个脚本的输出。

不要给脚本或测试导出 `HC_SESSION`：那会覆盖守卫设置的会话，把测试连到
别人的 core 上，还会把该会话算进泄漏。

共享机器上别的票同时在跑测试时，差值可能被它们的 core 干扰；看「新出现的
core」清单里的 `hctui-*` 会话更准确。

## kill -9 场景

`kill -9` 测试进程时没有任何 Rust 代码能跑，`Drop` 和 `atexit` 都不执行，
会话 core 会留下来。兜底：

```sh
XDG_RUNTIME_DIR=/tmp/hctui-<tag>-<pid>/run HC_SESSION=hctui-<tag>-<pid> hc server stop   # 会话名见上一条自检输出或 ss
scripts/reap-cores.sh                               # 干跑，确认分类
scripts/reap-cores.sh --apply                       # hctui-* 属 leak-name，会被清掉
```

`hctui-*` 以 `hctui-` 开头，`scripts/reap-cores.sh` 的 `leak-name` 分类
会接手，所以 kill -9 的残留不会永久堆积。
