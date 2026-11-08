//! 无窗口 `do`：没有 TUI 窗口时直连本地 core，用同一条布局副本算法改状态。
//!
//! 数据形状 = `SavedState`（schema 3）写在 `hctui-layout-v1`；写入复用
//! [`App::flush_layout`]，即「带刚读到的 version 做 CAS + 三方合并 + 冲突重读重放」。
//! 模块边界：这里只做动作分派、就绪等待与收尾，动作本身仍走
//! [`App::execute_action`]；`src/main.rs` 只在控制 socket 不可用时转交给这里。
//!
//! 不变量：屏幕类动作在连 core 之前就返回 `needs_window`；没有 core 时返回
//! `no_core`，不按需拉起（ADR-0002「会话数量失控」）；写入不落地或合并失败
//! 时报 `layout_error`，不静默成功。

use super::*;

/// 只改当前窗口屏幕的动作。清单来自 ADR-0002 的 D.2 表与动作实现：
/// 焦点/缩放/方向移动、space/tab/agent 的浏览式切换、侧栏与覆盖层、
/// 需要本地 VT 的滚动与搜索、需要模块运行时或渲染的面板。
const NEEDS_WINDOW: &[&str] = &[
    "agent.focus",
    "agent.next",
    "agent.prev",
    "app.palette",
    "app.settings",
    "app.help",
    "app.quit",
    "file.open",
    "files.open",
    "changes.open",
    "layout.hide",
    "layout.resize",
    "layout.show",
    "layout.switch",
    "layout.toggle",
    "machine.connect",
    "mod.open",
    "mods.chrome",
    "mods.close",
    "mods.dock",
    "mods.open",
    "mods.reload",
    "mods.undock",
    "overlay.close",
    "pane.focus",
    "pane.go",
    "pane.zoom",
    "space.next",
    "space.prev",
    "space.select",
    "tab.next",
    "tab.prev",
    "tab.select",
    "terminal.copy",
    "terminal.paste",
    "terminal.paste_image",
    "terminal.peek",
    "terminal.right_click",
    "terminal.scroll",
    "terminal.search",
    "terminal.show",
];

/// 交互式聊天面板只在窗口里存在。
const NEEDS_WINDOW_PREFIXES: &[&str] = &["chat."];

/// 读布局文档与等待共享文档的上限。
const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
/// 写文档与等待 RPC 回复的上限；与窗口退出时的收尾窗口一致。
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(5);

/// 这个动作在没有窗口时能不能落地。
pub fn needs_window(command: &str) -> bool {
    NEEDS_WINDOW.contains(&command)
        || NEEDS_WINDOW_PREFIXES
            .iter()
            .any(|prefix| command.starts_with(prefix))
}

/// 控制 socket 是不是「没有窗口」，而不是「有窗口但不该由第二个人写文档」。
/// 只有连接层失败才走无窗口路径；`UI did not answer` 之类的应答失败继续报错。
pub fn window_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            std::io::ErrorKind::NotFound
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
        )
    })
}

fn session_name(config: &Config) -> String {
    config
        .session
        .clone()
        .or_else(|| std::env::var("HC_SESSION").ok())
        .filter(|session| !session.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

/// core 的 unix socket：`$XDG_RUNTIME_DIR/herdr-core/<session>.sock`。
fn core_socket(config: &Config) -> Option<std::path::PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    Some(socket_in(
        std::path::Path::new(&runtime),
        &session_name(config),
    ))
}

fn socket_in(runtime: &std::path::Path, session: &str) -> std::path::PathBuf {
    runtime.join("herdr-core").join(format!("{session}.sock"))
}

fn core_alive(config: &Config) -> bool {
    core_socket(config).is_some_and(|path| std::os::unix::net::UnixStream::connect(path).is_ok())
}

/// prompt 需要的输入从 argv 取：确认类只认 `confirmed: true`（等价于窗口里敲
/// `y`），其余提示只认 `name`（改名与新建用的那条）。取不到就是 `None`。
fn prompt_text(prompt: &Prompt, args: &Value) -> Option<String> {
    if prompt.kind.needs_confirm() {
        return (args.get("confirmed").and_then(Value::as_bool) == Some(true))
            .then(|| "y".to_string());
    }
    args.get("name").and_then(Value::as_str).map(str::to_string)
}

fn refuse(command: &str, kind: &str, error: String) -> i32 {
    println!(
        "{}",
        json!({"error": error, "error_kind": kind, "method": command})
    );
    let _ = std::io::stdout().flush();
    1
}

/// 等布局副本可写：连接就绪、布局已读、`restore_pending` 已清。
fn wait_ready(app: &mut App) {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while Instant::now() < deadline {
        app.pump();
        let loading = !app.doc_loaded
            || app.restore_pending
            || app
                .inflight
                .keys()
                .any(|tag| tag.starts_with("shared.get:") || tag == "machines.sync");
        if !loading {
            break;
        }
        std::thread::sleep(POLL);
    }
}

/// 把这次动作引起的写入与 RPC 送完；布局文档的 CAS 全在 `flush_layout` 里。
fn settle(app: &mut App) {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    loop {
        app.pump();
        app.flush_layout();
        app.flush_shared();
        let busy = app.layout_dirty
            || app.layout.pending.is_some()
            || app.layout.needs_refresh
            || !app.inflight.is_empty();
        if !busy || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL);
    }
}

/// 包集合变更后的服务对账：连上已存在的 core 改台账，没有 core 时留到下次窗口。
///
/// 返回本次对账发现的问题（`missing_program` 等），调用方负责打印，不静默。
pub fn sync_services(config: &Config, trigger: super::services::Trigger) -> Vec<String> {
    if !core_alive(config) {
        return Vec::new();
    }
    let mut config = config.clone();
    // 台账在文档里：无论窗口怎么配置都要先读文档再写。
    config.restore = true;
    config.attach = None;
    let Ok(mut app) = App::new(config) else {
        return vec![t("cannot connect to core").to_string()];
    };
    app.service_trigger = trigger;
    wait_ready(&mut app);
    if !app.doc_loaded {
        app.shutdown();
        return vec![t("cannot read layout; services were not reconciled").to_string()];
    }
    app.services_reconcile(trigger);
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while Instant::now() < deadline {
        app.pump();
        if !app.inflight.keys().any(|tag| tag.starts_with("svc:")) {
            break;
        }
        std::thread::sleep(POLL);
    }
    settle(&mut app);
    let issues = app.service_issues.clone();
    app.shutdown();
    issues
}

/// `hc-tui do` 的无窗口路径。返回进程退出码。
pub fn run(config: &Config, command: &str, args: &Value) -> Result<i32> {
    if needs_window(command) {
        return Ok(refuse(
            command,
            "needs_window",
            t1("{} needs a TUI window", command),
        ));
    }
    if !core_alive(config) {
        return Ok(refuse(
            command,
            "no_core",
            t1("no core for session {}", session_name(config)),
        ));
    }
    let mut config = config.clone();
    // 文档是权威：无论窗口怎么配置，这里都要先读文档再算新文档。
    config.restore = true;
    config.attach = None;
    let mut app = App::new(config)?;
    wait_ready(&mut app);
    if !app.doc_loaded {
        app.shutdown();
        return Ok(refuse(
            command,
            "no_layout",
            t1(
                "cannot read layout for session {}",
                session_name(&app.config),
            ),
        ));
    }
    if let Err(error) = app.execute_action(command, args.clone()) {
        app.shutdown();
        return Ok(refuse(command, "action_failed", error.to_string()));
    }
    // 动作要一行输入时，argv 里能拿到的就当场提交；拿不到就明确拒绝，
    // 不能报 `confirmation_pending: true` 再把 prompt 丢掉。
    if let Some(prompt) = app.prompt.take() {
        let Some(text) = prompt_text(&prompt, args) else {
            app.shutdown();
            return Ok(refuse(
                command,
                "needs_input",
                t1("{} needs input; pass it in the arguments", command),
            ));
        };
        let mut prompt = prompt;
        prompt.buffer = text;
        app.commit_prompt(prompt);
    }
    let mut response = json!({
        "accepted": true,
        "confirmation_pending": false,
    });
    if let Some(request) = app.plugin_request.take() {
        response["request"] = json!(request);
    }
    settle(&mut app);
    let settled = !app.layout_dirty && app.layout.pending.is_none() && !app.layout.needs_refresh;
    if app.layout.blocked || !settled {
        let reason = if app.layout.blocked {
            t("layout conflict: accept the shared version with layout.reload")
        } else {
            t("layout write did not settle before the timeout")
        };
        app.shutdown();
        return Ok(refuse(command, "layout_error", reason.to_string()));
    }
    app.shutdown();
    println!("{response}");
    let _ = std::io::stdout().flush();
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_only_actions_are_refused_without_a_window() {
        for command in ["pane.zoom", "pane.focus", "layout.show", "overlay.close"] {
            assert!(needs_window(command), "{command}");
        }
        assert!(needs_window("chat.send"));
    }

    #[test]
    fn document_actions_are_allowed_without_a_window() {
        for command in [
            "terminal.park",
            "terminal.keep",
            "terminal.kill",
            "pane.split",
            "tab.new",
            "space.rename",
            "plugins.action",
        ] {
            assert!(!needs_window(command), "{command}");
        }
    }

    #[test]
    fn prompts_take_their_answer_from_the_arguments_or_are_refused() {
        let prompt = |kind| Prompt {
            label: String::new(),
            buffer: String::new(),
            cursor: 0,
            history_pos: None,
            kind,
        };
        let close = prompt(PromptKind::CloseTab(0));
        assert_eq!(
            prompt_text(&close, &json!({"confirmed": true})),
            Some("y".to_string())
        );
        assert_eq!(prompt_text(&close, &json!({"confirmed": false})), None);
        assert_eq!(prompt_text(&close, &json!({})), None);
        let rename = prompt(PromptKind::RenameTab {
            workspace: 1,
            tab: 2,
        });
        assert_eq!(
            prompt_text(&rename, &json!({"name": "审查"})),
            Some("审查".to_string())
        );
        assert_eq!(prompt_text(&rename, &json!({})), None);
    }

    #[test]
    fn only_connection_failures_fall_back_to_the_windowless_path() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(window_unavailable(&anyhow::Error::new(missing)));
        let answered = anyhow::anyhow!("UI did not answer");
        assert!(!window_unavailable(&answered));
    }

    #[test]
    fn the_core_socket_is_derived_from_the_session() {
        assert_eq!(
            socket_in(std::path::Path::new("/run/user/1000"), "hcp-winless"),
            std::path::PathBuf::from("/run/user/1000/herdr-core/hcp-winless.sock")
        );
    }
}
