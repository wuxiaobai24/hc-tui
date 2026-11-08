use anyhow::Result;
use hc_tui::i18n::{t, t1, t2, t3};

mod cli {
    pub mod completion;
    pub mod config;
    pub mod config_dump;
}

fn main() -> Result<()> {
    hc_tui::i18n::set_zh(hc_tui::config::detect_zh());
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-V" | "--version") => {
            println!("hc-tui {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("session-hook") => {
            use hc_tui::chat::hook;
            match args.get(1).map(String::as_str) {
                Some("install") => println!("{}", t1("installed {}", hook::install()?.display())),
                Some("remove") => println!("{}", t1("removed: {}", hook::remove()?)),
                Some("status") => println!("{}", hook::describe(hook::status())),
                _ => anyhow::bail!(t(
                    "usage: hc-tui session-hook install|remove|status (runs on this machine)"
                )),
            }
            Ok(())
        }
        Some("update") => hc_tui::update::run(&args[1..]),
        Some("mods") => mods_command(&args[1..]),
        Some("theme") => theme_command(&args[1..]),
        Some("packages") => packages_command(&args[1..]),
        Some("plugins") => plugins_command(&args[1..]),
        Some("events") => events_command(&args[1..]),
        Some("probe") => probe(&args[1..]),
        Some("keys") => keys_command(&args[1..]),
        Some("commands") => {
            let config = parse_options(&args[1..])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&hc_tui::app::command_catalog_for(&config))?
            );
            Ok(())
        }
        Some("completion") => cli::completion::run(&args[1..]),
        Some("config") => cli::config::run(&args[1..]),
        Some("notify") => notify_command(&args[1..]),
        Some("ls") => {
            let config = parse_options(&args[1..])?;
            println!(
                "{}",
                hc_tui::control::call(&config, serde_json::json!({"method":"snapshot"}))?
            );
            Ok(())
        }
        Some("do") => {
            let command = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!(t("do requires a command")))?;
            let (value, start) = match args.get(2) {
                Some(value) if value.starts_with('{') => {
                    (serde_json::from_str::<serde_json::Value>(value)?, 3)
                }
                // 裸位置参数只对寻址终端的动作成立：`do terminal.park t1`。
                Some(target) if !target.starts_with('-') && targets_terminal(command) => {
                    (serde_json::json!({ "terminal": target }), 3)
                }
                _ => (serde_json::json!({}), 2),
            };
            let config = parse_options(&args[start..])?;
            let request = serde_json::json!({"method":"do","command":command,"args":value});
            // 没有窗口时走无窗口路径；有窗口但不应答仍然报错，避免两个写入者。
            match hc_tui::control::call(&config, request) {
                Ok(answer) => {
                    println!("{answer}");
                    Ok(())
                }
                Err(error) if hc_tui::app::winless::window_unavailable(&error) => {
                    std::process::exit(hc_tui::app::winless::run(&config, command, &value)?)
                }
                Err(error) => Err(error),
            }
        }
        Some("--help" | "-h") => {
            usage();
            Ok(())
        }
        Some("--default-config") => {
            print!("{}", hc_tui::config::default_config_text());
            Ok(())
        }
        Some(flag) if flag.starts_with('-') => {
            let config = parse_options(&args)?;
            run_tui(config)
        }
        Some(other) => {
            eprintln!("{}", t1("unknown command: {}", other));
            usage();
            std::process::exit(2);
        }
        None => run_tui(parse_options(&args)?),
    }
}

/// 裸位置参数是终端地址的动作：`hc-tui do terminal.park t1`。
fn targets_terminal(command: &str) -> bool {
    command.starts_with("terminal.") || matches!(command, "agent.focus" | "agent.rename")
}

fn usage() {
    println!("hc-tui {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("{}", t("usage: hc-tui [options]"));
    println!("{}", t("       hc-tui probe [options]"));
    println!(
        "{}",
        t("       hc-tui keys probe [--protocol auto|kitty|legacy] [--seconds <n>] | decode <hex>")
    );
    println!("       hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]");
    println!("       hc-tui session-hook install|remove|status");
    println!("       hc-tui theme list|show|reload [--config <path>]");
    println!("       hc-tui --default-config");
    println!("{}", t("       hc-tui commands | ls [options]"));
    println!(
        "{}",
        t("       hc-tui mods check <path|id>... [--config <path>]")
    );
    println!(
        "{}",
        t("       hc-tui plugins action list|invoke|status|cancel|log ...")
    );
    println!(
        "{}",
        t("       hc-tui plugins pane open|status|focus|close ...")
    );
    println!(
        "{}",
        t("       hc-tui packages list|install|update|link|unlink|uninstall|enable|disable|gc|log|info ...")
    );
    println!(
        "{}",
        t("       hc-tui packages update --check [<id>] [--json]")
    );
    println!(
        "{}",
        t("       hc-tui events [--plugin <id>] [--kinds a,b] [--count <n>]")
    );
    println!("       hc-tui completion zsh|bash|fish");
    println!("       hc-tui config check|reset-keys [--config <path>] [--yes]");
    println!(
        "       hc-tui config dump [--effective|--defaults|--minimal] [--config <path>] [--out <path>]"
    );
    println!("       hc-tui config init [--path <path>] [--force]");
    println!(
        "{}",
        t("       hc-tui notify <text> [--level info|warn|error] [--action <name>] [--target <terminal|pane>]")
    );
    println!(
        "{}",
        t("       hc-tui do <command> [<terminal>] [JSON arguments] [options]")
    );
    println!();
    println!(
        "{}",
        t("  --session <name>   hc session to connect to (default: HC_SESSION)")
    );
    println!(
        "{}",
        t("  --attach <t>       attach an existing terminal instead of starting a shell")
    );
    println!(
        "{}",
        t("  --config <path>    config file (default: ~/.config/hc-tui/config.toml)")
    );
    println!(
        "{}",
        t("  --no-restore       ignore the layout saved in the core doc")
    );
    println!(
        "{}",
        t("  --no-mods          builtin chrome, no module runtime")
    );
}

/// `hc-tui theme list|show|reload`：不用启动界面就能看当前解析出的配色。
fn theme_command(args: &[String]) -> Result<()> {
    let mut sub = "show";
    let mut path = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            other => sub = other,
        }
        index += 1;
    }
    match sub {
        "list" => theme_list(),
        "show" | "reload" => theme_show(path)?,
        other => anyhow::bail!(t1(
            "unknown theme command {}; usage: hc-tui theme list|show|reload [--config <path>]",
            other
        )),
    }
    Ok(())
}

fn theme_list() {
    println!("builtin");
    for name in hc_tui::theme::names() {
        println!("  {name}");
    }
    for (label, loaded) in [
        ("ghostty", hc_tui::theme::ghostty::load()),
        ("omarchy", hc_tui::theme::omarchy::load()),
    ] {
        match loaded {
            Ok(loaded) => {
                let files = loaded
                    .files
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("{label}  {} ({files})", loaded.label)
            }
            Err(error) => println!("{label}  {error}"),
        }
    }
}

fn theme_show(path: Option<std::path::PathBuf>) -> Result<()> {
    let config = hc_tui::config::Config::load(path);
    for warning in &config.warnings {
        eprintln!("warning: {warning}");
    }
    let theme = &config.theme;
    println!("name    {}", theme.name);
    match &theme.source {
        Some(source) => println!("source  {}", source.describe()),
        None => println!("source  builtin"),
    }
    for token in [
        "bg",
        "fg",
        "dim",
        "border",
        "accent",
        "selection_bg",
        "cursor_bg",
    ] {
        println!("{token:<12} {}", hc_tui::theme::hex(theme.color(token)));
    }
    for (index, color) in theme.terminal_palette().iter().enumerate() {
        println!("ansi{index:<8} {}", hc_tui::theme::hex(*color));
    }
    Ok(())
}

/// `hc-tui events`：连到运行中的 TUI，按行输出事件 NDJSON，断开即停。
fn events_command(args: &[String]) -> Result<()> {
    let mut config_path = None;
    let mut plugin = None;
    let mut kinds: Vec<String> = Vec::new();
    let mut count = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            "--plugin" => {
                index += 1;
                plugin = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--plugin needs a value")))?,
                );
            }
            "--kinds" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!(t("--kinds needs a value")))?;
                kinds.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|kind| !kind.is_empty())
                        .map(str::to_string),
                );
            }
            "--count" => {
                index += 1;
                count = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--count needs a value")))?
                        .parse::<u64>()
                        .map_err(|_| anyhow::anyhow!(t("--count needs a number")))?,
                );
            }
            other => anyhow::bail!(t1("unknown option: {}", other)),
        }
        index += 1;
    }
    let config = hc_tui::config::Config::load(config_path);
    let mut request = serde_json::json!({"method": "events"});
    if let Some(plugin) = plugin {
        request["plugin"] = serde_json::json!(plugin);
    }
    if !kinds.is_empty() {
        request["kinds"] = serde_json::json!(kinds);
    }
    subscribe_events(&config, request, count)
}

/// 长连接：先读一条确认，再按行输出，直到断开或达到 `count`。
#[cfg(unix)]
fn subscribe_events(
    config: &hc_tui::config::Config,
    request: serde_json::Value,
    count: Option<u64>,
) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(hc_tui::control::socket_path(config))?;
    writeln!(stream, "{request}")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let ack: serde_json::Value = serde_json::from_str(line.trim())?;
    if let Some(error) = ack.get("error").and_then(serde_json::Value::as_str) {
        anyhow::bail!("{error}");
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut seen = 0u64;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(line.trim())?;
        writeln!(out, "{}", serde_json::to_string(&value)?)?;
        out.flush()?;
        seen += 1;
        if count.is_some_and(|count| seen >= count) {
            return Ok(());
        }
    }
}

#[cfg(not(unix))]
fn subscribe_events(
    _config: &hc_tui::config::Config,
    _request: serde_json::Value,
    _count: Option<u64>,
) -> Result<()> {
    anyhow::bail!(t("local command sockets require Unix"))
}

/// `hc-tui plugins action` 子命令：列目录在无窗口时可用；调用、状态、
/// 取消与日志经本地控制 socket 转发给运行中的 TUI。
fn plugins_command(args: &[String]) -> Result<()> {
    let Some(group) = args.first().map(String::as_str) else {
        anyhow::bail!("{}", packages_usage("plugins"))
    };
    if wants_help(group) || args[1..].iter().any(|arg| wants_help(arg)) {
        println!("{}", packages_usage("plugins"));
        return Ok(());
    }
    if group == "pane" {
        return plugins_pane(&args[1..]);
    }
    if group != "action" {
        return packages_manage(group, &args[1..], Some("plugins"));
    }
    let Some(sub) = args.get(1).map(String::as_str) else {
        anyhow::bail!(t(
            "usage: hc-tui plugins action list|invoke|status|cancel|log"
        ));
    };
    let mut config_path = None;
    let mut json = false;
    let mut lines = 100usize;
    let mut pane = None;
    let mut targets: Vec<String> = Vec::new();
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            "--json" => json = true,
            "-n" => {
                index += 1;
                lines = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!(t("-n needs a value")))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!(t("-n needs a number")))?;
            }
            "--pane" => {
                index += 1;
                pane = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--pane needs a value")))?,
                );
            }
            other if other.starts_with('-') => anyhow::bail!(t1("unknown option: {}", other)),
            other => targets.push(other.to_string()),
        }
        index += 1;
    }
    let config = hc_tui::config::Config::load(config_path);
    let call = |body: serde_json::Value| -> Result<serde_json::Value> {
        hc_tui::control::call(&config, body)
    };
    match sub {
        "list" => {
            let registry = hc_tui::plugin::registry::Registry::load(&config);
            if json {
                let actions: Vec<serde_json::Value> = registry
                    .actions()
                    .into_iter()
                    .map(|action| {
                        serde_json::json!({
                            "name": action.name,
                            "package": action.package,
                            "action": action.action,
                            "title": action.title,
                            "description": action.description,
                            "contexts": action.contexts,
                            "args": action.args,
                            "source": format!("plugin:{}", action.package),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&actions)?);
                return Ok(());
            }
            if registry.actions().is_empty() {
                println!("{}", t("no plugin actions"));
            }
            for action in registry.actions() {
                println!(
                    "{:<40} {:<28} {}",
                    action.name,
                    action.contexts.join(","),
                    action.title
                );
            }
            for issue in registry.issues {
                eprintln!("{}", t1("warning: {}", issue.message));
            }
            Ok(())
        }
        "invoke" => {
            let name = targets.first().ok_or_else(|| {
                anyhow::anyhow!(t("usage: hc-tui plugins action invoke <name> [JSON]"))
            })?;
            let mut args: serde_json::Value = match targets.get(1) {
                Some(text) => serde_json::from_str(text)
                    .map_err(|error| anyhow::anyhow!(t1("arguments: {}", error)))?,
                None => serde_json::json!({}),
            };
            if let Some(pane) = pane {
                args["_target"] = serde_json::json!({"pane_or_terminal": pane});
            }
            let response = hc_tui::control::call_raw(
                &config,
                serde_json::json!({
                    "method": "do",
                    "command": name,
                    "args": args,
                }),
            )?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            if let Some(error) = response.get("error").and_then(serde_json::Value::as_str) {
                match response
                    .get("error_kind")
                    .and_then(serde_json::Value::as_str)
                {
                    Some(kind) => anyhow::bail!("{kind}: {error}"),
                    None => anyhow::bail!("{error}"),
                }
            }
            Ok(())
        }
        "status" | "cancel" | "log" => {
            let request = targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("a request ID is required")))?;
            let method = format!("plugins.{sub}");
            let response = call(serde_json::json!({
                "method": method,
                "request": request,
                "lines": lines,
            }))?;
            if let Some(error) = response.get("error").and_then(serde_json::Value::as_str) {
                anyhow::bail!("{error}");
            }
            if sub == "log" {
                if let Some(log) = response.get("log").and_then(serde_json::Value::as_str) {
                    print!("{log}");
                }
                return Ok(());
            }
            if json || sub == "status" {
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!("{}", serde_json::to_string(&response)?);
            }
            Ok(())
        }
        other => anyhow::bail!(t1("unknown plugins action: {}", other)),
    }
}

/// `hc-tui plugins pane open|status|focus|close`：经运行中的 TUI 创建与定位
/// 插件终端。`open` 等到拿到终端地址或明确错误才返回。
fn plugins_pane(args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        anyhow::bail!(t(
            "usage: hc-tui plugins pane open|status|focus|close <name|terminal>"
        ));
    };
    let mut config_path = None;
    let mut cwd = None;
    let mut machine = None;
    let mut placement = None;
    let mut space = None;
    let mut targets: Vec<String> = Vec::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            "--cwd" => {
                index += 1;
                cwd = args.get(index).cloned();
            }
            "--machine" => {
                index += 1;
                machine = args.get(index).cloned();
            }
            "--placement" => {
                index += 1;
                placement = args.get(index).cloned();
            }
            "--space" => {
                index += 1;
                space = args.get(index).cloned();
            }
            other if other.starts_with('-') => anyhow::bail!(t1("unknown option: {}", other)),
            other => targets.push(other.to_string()),
        }
        index += 1;
    }
    let config = hc_tui::config::Config::load(config_path);
    let target = targets.first().cloned().unwrap_or_default();
    let call = |body: serde_json::Value| hc_tui::control::call_raw(&config, body);
    match sub {
        "open" => {
            anyhow::ensure!(!target.is_empty(), t("a plugin terminal name is required"));
            let mut request = serde_json::json!({
                "method": "plugins.pane.open",
                "name": target,
            });
            for (key, value) in [
                ("cwd", cwd),
                ("machine", machine),
                ("placement", placement),
                ("space", space),
            ] {
                if let Some(value) = value {
                    request[key] = serde_json::json!(value);
                }
            }
            let reply = call(request)?;
            if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
                println!("{}", serde_json::to_string_pretty(&reply)?);
                anyhow::bail!("{error}");
            }
            let request_id = reply
                .get("request")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let final_status = wait_for_pane(&config, &request_id)?;
            println!("{}", serde_json::to_string_pretty(&final_status)?);
            if let Some(error) = final_status
                .get("error")
                .and_then(serde_json::Value::as_str)
            {
                anyhow::bail!("{error}");
            }
            Ok(())
        }
        "status" => {
            anyhow::ensure!(!target.is_empty(), t("a request ID is required"));
            let reply = call(serde_json::json!({
                "method": "plugins.pane.status",
                "request": target,
            }))?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
            if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
                anyhow::bail!("{error}");
            }
            Ok(())
        }
        "focus" | "close" => {
            anyhow::ensure!(!target.is_empty(), t("a terminal or name is required"));
            let reply = call(serde_json::json!({
                "method": format!("plugins.pane.{sub}"),
                "terminal": target,
            }))?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
            if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
                anyhow::bail!("{error}");
            }
            Ok(())
        }
        other => anyhow::bail!(t1("unknown plugins pane action: {}", other)),
    }
}

/// `open` 之后等终端地址：探测、attach 与标签都在下一轮界面循环里完成。
fn wait_for_pane(config: &hc_tui::config::Config, request: &str) -> Result<serde_json::Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let reply = hc_tui::control::call_raw(
            config,
            serde_json::json!({"method": "plugins.pane.status", "request": request}),
        )?;
        let status = reply
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let done = status == "failed"
            || (status == "opened"
                && reply
                    .get("terminal")
                    .and_then(serde_json::Value::as_str)
                    .is_some());
        if done
            || reply
                .get("error")
                .map(|error| !error.is_null())
                .unwrap_or(false)
        {
            return Ok(reply);
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(t1("plugin terminal {} did not open in time", request));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// `packages` 的子命令集合。
const PACKAGE_GROUPS: &str = "list|install|update|link|unlink|uninstall|enable|disable|gc|log|info";
/// `mods` 的自有子命令，其余与 `packages` 同名。
const MODS_GROUPS: &str = "list|check|test|build|install|update|uninstall|link|unlink|log|open|close|reload|grant|enable|disable";

/// 一个表面的用法行：`packages` 与 `plugins` / `mods` 两个别名各带自己的子命令。
/// `--help` 与未知子命令共用这一份文本。
fn packages_usage(surface: &str) -> String {
    match surface {
        "plugins" => format!("usage: hc-tui plugins action|pane|{PACKAGE_GROUPS}"),
        "mods" => format!("usage: hc-tui mods {MODS_GROUPS}"),
        _ => format!("usage: hc-tui packages {PACKAGE_GROUPS}"),
    }
}

/// `--help` / `-h`：打印用法并以 0 退出。
fn wants_help(arg: &str) -> bool {
    arg == "--help" || arg == "-h"
}

/// `hc-tui packages`：包注册与代际管理的唯一入口。
fn packages_command(args: &[String]) -> Result<()> {
    let Some(group) = args.first().map(String::as_str) else {
        anyhow::bail!("{}", packages_usage("packages"))
    };
    packages_manage(group, &args[1..], None)
}

/// `hc-tui packages list|install|update|link|unlink|uninstall|enable|disable|gc|log`：
/// 包注册与代际管理的唯一入口；`plugins` 与 `mods` 的同名子命令是别名，
/// 行为一致，只在 stderr 多打一行弃用提示。
fn packages_manage(group: &str, args: &[String], alias: Option<&str>) -> Result<()> {
    let surface = alias.unwrap_or("packages");
    if wants_help(group) || args.iter().any(|arg| wants_help(arg)) {
        println!("{}", packages_usage(surface));
        return Ok(());
    }
    let mut config_path = None;
    let mut json = false;
    let mut yes = false;
    let mut check = false;
    let mut lines = 100usize;
    let mut service: Option<String> = None;
    let mut policy = hc_tui::plugin::registry::EnablePolicy::Keep;
    let mut targets: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            "--json" => json = true,
            "--check" => check = true,
            "--yes" | "-y" => yes = true,
            "--service" => {
                index += 1;
                service = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--service needs a value")))?,
                );
            }
            "-n" => {
                index += 1;
                lines = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!(t("-n needs a value")))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!(t("-n needs a number")))?;
            }
            "--enable" => policy = hc_tui::plugin::registry::EnablePolicy::Enable,
            "--disable" => policy = hc_tui::plugin::registry::EnablePolicy::Disable,
            other if other.starts_with('-') => anyhow::bail!(t1("unknown option: {}", other)),
            other => targets.push(other.to_string()),
        }
        index += 1;
    }
    if let Some(alias) = alias {
        if PACKAGE_GROUPS.split('|').any(|known| known == group) {
            eprintln!("note: `{alias} {group}` is deprecated, use `packages {group}`");
        }
    }
    if check && group != "update" {
        anyhow::bail!(t1("--check only applies to update, not {}", group));
    }
    let config = hc_tui::config::Config::load(config_path);
    match group {
        "list" => packages_list(&config, json),
        "info" => {
            let target = package_target(&targets, "info")?;
            packages_info(&config, target, json)
        }
        "update" if check => packages_check(&config, targets.first().map(String::as_str), json),
        "install" | "update" => {
            let (id, target) = match targets.as_slice() {
                [id, target] => (Some(id.clone()), target.clone()),
                [target] => (None, target.clone()),
                _ => anyhow::bail!(t1(
                    "usage: hc-tui packages {} <dir|.nshpkg|<git url>|.tar.gz>",
                    group
                )),
            };
            let result = packages_install(&config, id.as_deref(), &target, policy, yes);
            if result.is_ok() {
                sync_services(&config, hc_tui::app::services::Trigger::Install);
            }
            result
        }
        "uninstall" => {
            let id = package_target(&targets, "uninstall")?;
            if plugin_record(&config, id).is_some() {
                hc_tui::plugin::registry::uninstall(&config, id).map_err(anyhow::Error::msg)?;
            } else {
                hc_tui::mods::store::uninstall(&config, id).map_err(anyhow::Error::msg)?;
            }
            println!("{}", t1("uninstalled {}", id));
            sync_services(&config, hc_tui::app::services::Trigger::Enable);
            Ok(())
        }
        "link" => {
            let dir = package_target(&targets, "link")?;
            let result = packages_link(&config, dir, policy);
            if result.is_ok() {
                sync_services(&config, hc_tui::app::services::Trigger::Install);
            }
            result
        }
        "unlink" => {
            let id = package_target(&targets, "unlink")?;
            let store = hc_tui::store::Store::open(config.mods_state_dir());
            match store.record(id) {
                Some(record) if record.source == hc_tui::store::Source::Linked => {
                    hc_tui::mods::store::unlink(&config, id).map_err(anyhow::Error::msg)?;
                }
                Some(_) => anyhow::bail!(t1("{} is not linked", id)),
                None => {
                    hc_tui::plugin::registry::uninstall(&config, id).map_err(anyhow::Error::msg)?
                }
            }
            println!("{}", t1("unlinked {}", id));
            sync_services(&config, hc_tui::app::services::Trigger::Enable);
            Ok(())
        }
        "enable" | "disable" => {
            let id = package_target(&targets, group)?;
            let enabled = group == "enable";
            hc_tui::pkg::set_enabled(&config, id, enabled).map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                t2("{} {}", if enabled { "enabled" } else { "disabled" }, id)
            );
            sync_services(&config, hc_tui::app::services::Trigger::Enable);
            Ok(())
        }
        "gc" => {
            // 显式回收：只删没有任何 pin 持有者、也没有记录指向的代际。
            let reports = hc_tui::plugin::lease::gc_all(
                &config,
                &hc_tui::plugin::lease::RetentionEvidence::default(),
            );
            if json {
                let entries: Vec<serde_json::Value> = reports
                    .iter()
                    .map(|(package, report)| {
                        serde_json::json!({
                            "package": package,
                            "generations": report.generations,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if reports.is_empty() {
                println!("{}", t("no generations to reclaim"));
            } else {
                for (package, report) in reports {
                    println!("{package}: {}", report.generations.len());
                }
            }
            Ok(())
        }
        "log" => {
            let id = package_target(&targets, "log")?;
            if let Some(service) = &service {
                return print_service_log(&config, id, service, lines);
            }
            let store = if plugin_record(&config, id).is_some() {
                hc_tui::store::Store::open(config.plugins_state_dir())
            } else {
                hc_tui::store::Store::open(config.mods_state_dir())
            };
            match store.log_tail(id, lines) {
                Ok(text) => println!("{text}"),
                Err(error) => println!("{}", t2("no log for {}: {}", id, error)),
            }
            Ok(())
        }
        other => anyhow::bail!("{} (unknown: {})", packages_usage(surface), other),
    }
}

/// 包集合变了就对账服务；core 不在时留给下一次窗口，问题打到 stderr 不静默。
fn sync_services(config: &hc_tui::config::Config, trigger: hc_tui::app::services::Trigger) {
    for issue in hc_tui::app::winless::sync_services(config, trigger) {
        eprintln!("{issue}");
    }
}

fn package_target<'a>(targets: &'a [String], command: &str) -> Result<&'a str> {
    targets
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!(t1("usage: hc-tui packages {} <id>", command)))
}

fn plugin_record(config: &hc_tui::config::Config, id: &str) -> Option<hc_tui::store::Record> {
    hc_tui::store::Store::open(config.plugins_state_dir()).record(id)
}

/// `packages list`：安装记录一行一个包，前缀徽章标出能力（A/T/M）。
fn packages_list(config: &hc_tui::config::Config, json: bool) -> Result<()> {
    let registry = hc_tui::plugin::registry::Registry::load(config);
    let terminals = service_terminals(config);
    let mut entries = registry.entries(config);
    for entry in &mut entries {
        entry.services = registry
            .services_of(&entry.id)
            .iter()
            .map(|info| {
                if entry.enabled {
                    hc_tui::plugin::service::status(info, terminals.as_deref().unwrap_or(&[]))
                } else {
                    hc_tui::plugin::service::ServiceStatus::stopped(info)
                }
            })
            .collect();
        // 文本与 `--json` 共用这一份 issues：服务汇总从刚填好的 `services` 推出。
        if let Some(issue) = hc_tui::plugin::service::issue_summary(&entry.services) {
            entry.issues.push(issue);
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("{}", t("no packages installed"));
    }
    if terminals.is_none() && entries.iter().any(|entry| !entry.services.is_empty()) {
        eprintln!(
            "{}",
            t("no core for this session; service state comes from declarations only")
        );
    }
    for entry in &entries {
        let mut line = format!(
            "{:<32} {:<9} gen {:<3} {:<8} {}",
            entry.id,
            entry.source,
            entry.generation,
            if entry.enabled { "enabled" } else { "disabled" },
            entry.capabilities.badges()
        );
        if let Some(summary) = service_summary(&entry.services) {
            line.push(' ');
            line.push_str(&summary);
        }
        println!("{line}");
        if let Some(plugin) = registry.get(&entry.id) {
            for terminal in &plugin.manifest.terminals {
                println!(
                    "    {:<40} {} · {}",
                    format!("{}.{}", plugin.id, terminal.id()),
                    terminal.placement(),
                    terminal.title()
                );
            }
        }
        for service in &entry.services {
            println!(
                "    {:<40} service · {}",
                service.id,
                service_detail(service)
            );
        }
        if let Some(module) = &entry.module {
            println!("    {:<40} module · {}", module.bundle, module.title);
        }
        for issue in &entry.issues {
            println!("    {:<40} issue · {}", "", issue);
        }
    }
    Ok(())
}

/// 已安装包的记录：两个 store 依次查，第一个有的就是这一份。
fn installed_record(config: &hc_tui::config::Config, id: &str) -> Option<hc_tui::store::Record> {
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        if let Some(record) = hc_tui::store::Store::open(state).record(id) {
            return Some(record);
        }
    }
    None
}

/// 一个包声明的服务及其状态；已停用的包不去探测程序，与 `packages list` 一致。
fn package_services(
    config: &hc_tui::config::Config,
    id: &str,
    enabled: bool,
) -> Vec<hc_tui::plugin::service::ServiceStatus> {
    let registry = hc_tui::plugin::registry::Registry::load(config);
    let terminals = service_terminals(config);
    registry
        .services_of(id)
        .iter()
        .map(|info| {
            if enabled {
                hc_tui::plugin::service::status(info, terminals.as_deref().unwrap_or(&[]))
            } else {
                hc_tui::plugin::service::ServiceStatus::stopped(info)
            }
        })
        .collect()
}

/// `packages info <id|src>`：已安装包读出记录与本代际清单，未安装来源读出清单。
fn packages_info(config: &hc_tui::config::Config, target: &str, json: bool) -> Result<()> {
    let record = installed_record(config, target);
    let mut origin = record.as_ref().and_then(|record| record.origin.clone());
    let staged = if record.is_none() {
        let staged = hc_tui::pkg::source::stage(target).map_err(anyhow::Error::msg)?;
        origin = Some(staged.origin().to_string());
        Some(staged)
    } else {
        None
    };
    let root = match &staged {
        Some(staged) => staged.path().to_path_buf(),
        None => hc_tui::pkg::resolve(config, target)
            .map_err(anyhow::Error::msg)?
            .map(|resolved| resolved.root)
            .ok_or_else(|| anyhow::anyhow!(t1("{} has no installed generation", target)))?,
    };
    let loaded = hc_tui::pkg::load_dir(&root).map_err(anyhow::Error::msg)?;
    let manifest = &loaded.manifest;
    let id = manifest.id().to_string();
    let local = record
        .as_ref()
        .and_then(|record| record.app.as_ref())
        .and_then(|app| app.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| manifest.version().to_string());
    let capabilities = record
        .as_ref()
        .and_then(|record| record.capabilities)
        .unwrap_or_else(|| manifest.capabilities());
    let module = record
        .as_ref()
        .and_then(|record| record.module.clone())
        .or_else(|| manifest.module_info());
    let grants = record
        .as_ref()
        .and_then(|record| record.grants.clone())
        .unwrap_or_else(|| manifest.grants());
    let enabled = record.as_ref().map(|record| record.enabled).unwrap_or(true);
    let services = package_services(config, &id, enabled);
    // 没安装就没有「本地版本」可比，`update` 一栏留空。
    let remote = match &record {
        None => serde_json::Value::Null,
        Some(record) => {
            let probe = match record.origin.as_deref() {
                Some(origin) if !origin.is_empty() => hc_tui::pkg::source::remote_version(origin),
                _ => Err(t("no source recorded").to_string()),
            };
            match &probe {
                Ok(version) => {
                    serde_json::json!({ "version": version, "outdated": *version != local })
                }
                Err(error) => serde_json::json!({ "error": error }),
            }
        }
    };
    if json {
        let value = serde_json::json!({
            "id": id,
            "name": manifest.name(),
            "version": local,
            "source": origin,
            "path": root.display().to_string(),
            "generation": record.as_ref().map(|record| record.generation),
            "enabled": record.as_ref().map(|record| record.enabled),
            "installedAt": record.as_ref().map(|record| record.installed_at),
            "capabilities": capabilities,
            "module": module,
            "services": services,
            "grants": grants,
            "remote": remote,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("{}", t3("{} ({} {})", &id, manifest.name(), &local));
    println!(
        "{}",
        t1("  source       {}", origin.as_deref().unwrap_or("-"))
    );
    match &record {
        Some(record) => println!(
            "{}",
            t2(
                "  generation   {} · {}",
                record.generation,
                if record.enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )
        ),
        None => println!("{}", t("  generation   not installed")),
    }
    println!("{}", t1("  path         {}", root.display()));
    println!("{}", t1("  capabilities {}", capabilities.badges()));
    if let Some(module) = &module {
        println!(
            "{}",
            t3(
                "  module       {} · {} · {}",
                &module.bundle,
                &module.place,
                &module.title
            )
        );
    }
    println!(
        "{}",
        t1(
            "  grants       {}",
            if grants.is_empty() {
                "none".to_string()
            } else {
                grants.join(", ")
            }
        )
    );
    println!(
        "{}",
        t1(
            "  services     {}",
            if services.is_empty() {
                "none".to_string()
            } else {
                services
                    .iter()
                    .map(service_detail)
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        )
    );
    if let Some(record) = &record {
        println!("{}", t1("  installed    {} (unix)", record.installed_at));
    }
    println!("{}", t1("  update       {}", remote_line(&local, &remote)));
    Ok(())
}

/// 一条来源端版本的人读形式。
fn remote_line(local: &str, remote: &serde_json::Value) -> String {
    if remote.is_null() {
        return "not installed".to_string();
    }
    match (
        remote.get("version").and_then(serde_json::Value::as_str),
        remote.get("error").and_then(serde_json::Value::as_str),
    ) {
        (Some(version), _) if version == local => format!("up to date ({local})"),
        (Some(version), _) => format!("local {local} → remote {version}"),
        (None, Some(error)) => format!("unknown: {error}"),
        (None, None) => "unknown".to_string(),
    }
}

/// 记录里的版本：安装时缓存的清单版本，旧记录回退到代际目录里的清单。
fn record_version(config: &hc_tui::config::Config, record: &hc_tui::store::Record) -> String {
    record
        .app
        .as_ref()
        .and_then(|app| app.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            let resolved = hc_tui::pkg::resolve(config, &record.id).ok()??;
            let loaded = hc_tui::pkg::load_dir(&resolved.root).ok()?;
            Some(loaded.manifest.version().to_string())
        })
        .unwrap_or_default()
}

/// `packages update --check [id]`：比较本地记录的版本与来源端的版本，不改动任何记录。
fn packages_check(config: &hc_tui::config::Config, id: Option<&str>, json: bool) -> Result<()> {
    let mut records: Vec<hc_tui::store::Record> = Vec::new();
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        for record in hc_tui::store::Store::open(state).list() {
            if records.iter().any(|known| known.id == record.id) {
                continue;
            }
            records.push(record);
        }
    }
    if let Some(id) = id {
        records.retain(|record| record.id == id);
        if records.is_empty() {
            anyhow::bail!(t1("{} is not installed", id));
        }
    }
    if records.is_empty() {
        println!("{}", t("no packages installed"));
        return Ok(());
    }
    let mut checked: Vec<(String, String, String)> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for record in &records {
        let local = record_version(config, record);
        let remote = match record.origin.as_deref() {
            Some(origin) if !origin.is_empty() => hc_tui::pkg::source::remote_version(origin),
            _ => Err(t("no source recorded").to_string()),
        };
        match remote {
            Ok(remote) => checked.push((record.id.clone(), local, remote)),
            Err(error) => failed.push(format!("{}: {error}", record.id)),
        }
    }
    if json {
        let entries: Vec<serde_json::Value> = checked
            .iter()
            .map(|(id, local, remote)| {
                serde_json::json!({
                    "id": id,
                    "local": local,
                    "remote": remote,
                    "outdated": remote != local,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for (id, local, remote) in &checked {
            if remote == local {
                println!("{}", t2("{} up to date ({})", id, local));
            } else {
                println!("{}", t3("{} local {} → remote {}", id, local, remote));
            }
        }
    }
    if !failed.is_empty() {
        anyhow::bail!(failed.join("; "));
    }
    Ok(())
}

/// 已存在 core 的终端清单（含已结束的记录）；`None` 表示本会话没有 core。
///
/// 不按需拉起 core：`hc connect` 在没有 core 时会启动一个。
fn service_terminals(config: &hc_tui::config::Config) -> Option<Vec<serde_json::Value>> {
    if !hc_tui::app::winless::core_alive(config) {
        return None;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let mut conn = hc_tui::hc::client::Conn::connect(config.session.as_deref())
            .await
            .ok()?;
        let reply = conn
            .call("terminal.list", serde_json::json!({ "all": true }))
            .await
            .ok()?;
        let terminals = reply.get("terminals")?.as_array()?;
        Some(terminals.clone())
    })
}

/// 一行里的服务状态摘要：在跑的 / 声明的总数，有失败时带 `✗`。
fn service_summary(services: &[hc_tui::plugin::service::ServiceStatus]) -> Option<String> {
    if services.is_empty() {
        return None;
    }
    let running = services
        .iter()
        .filter(|service| {
            service.state == hc_tui::plugin::service::RUNNING
                || service.state == hc_tui::plugin::service::STARTING
        })
        .count();
    let failed = services
        .iter()
        .any(|service| service.state == hc_tui::plugin::service::FAILED);
    Some(if failed {
        format!("s:{running}/{} ✗", services.len())
    } else {
        format!("s:{running}/{}", services.len())
    })
}

/// 一条服务的明细行：状态、终端地址、PID、原因。
fn service_detail(service: &hc_tui::plugin::service::ServiceStatus) -> String {
    let mut detail = service.state.to_string();
    if let Some(terminal) = &service.terminal {
        detail.push_str(&format!(" · {terminal}"));
    }
    if let Some(pid) = service.pid {
        detail.push_str(&format!(" · pid {pid}"));
    }
    if let Some(reason) = &service.reason {
        detail.push_str(&format!(" · {reason}"));
    }
    detail
}

/// `packages log <package> --service <id>`：读服务声明的日志文件尾部。
fn print_service_log(
    config: &hc_tui::config::Config,
    id: &str,
    service: &str,
    lines: usize,
) -> Result<()> {
    let registry = hc_tui::plugin::registry::Registry::load(config);
    let info = registry
        .services_of(id)
        .into_iter()
        .find(|info| info.service == service || info.name == service)
        .ok_or_else(|| anyhow::anyhow!(t2("{} declares no service {}", id, service)))?;
    let path = hc_tui::plugin::service::log_path(&info, &config.plugins_state_dir())
        .ok_or_else(|| anyhow::anyhow!(t1("service {} declares no log", info.name.clone())))?;
    match hc_tui::plugin::service::log_tail(&path, lines) {
        Ok(text) => println!("{text}"),
        Err(error) => println!("{}", t2("no log for {}: {}", info.name, error)),
    }
    Ok(())
}

/// 安装或更新一个包：v2 / v1 清单走包注册表，旧模块目录与签名包走模块存储。
fn packages_install(
    config: &hc_tui::config::Config,
    id: Option<&str>,
    target: &str,
    policy: hc_tui::plugin::registry::EnablePolicy,
    yes: bool,
) -> Result<()> {
    let path = std::path::Path::new(target);
    if !path.is_dir() {
        let from_source = matches!(
            hc_tui::pkg::source::kind(target),
            hc_tui::pkg::source::Kind::Git(_) | hc_tui::pkg::source::Kind::Archive(_)
        );
        if !from_source {
            return install_or_update(config, yes, target, id);
        }
        let staged = hc_tui::pkg::source::stage(target).map_err(anyhow::Error::msg)?;
        return install_staged(config, id, &staged, policy, yes);
    }
    let loaded = hc_tui::pkg::install_ready(path).map_err(anyhow::Error::msg)?;
    if loaded.origin == hc_tui::pkg::Origin::LegacyMod {
        return install_or_update(config, yes, target, id);
    }
    let manifest = loaded.manifest;
    if let Some(id) = id {
        if manifest.id() != id {
            anyhow::bail!(t3(
                "{} declares package id {}, not {}",
                target,
                manifest.id(),
                id
            ));
        }
    }
    print_package_preview(&manifest);
    confirm_install(yes)?;
    let record =
        hc_tui::plugin::registry::install(config, path, policy).map_err(anyhow::Error::msg)?;
    println!(
        "{}",
        t3(
            "installed {} generation {} ({})",
            record.id,
            record.generation,
            if record.enabled {
                "enabled"
            } else {
                "disabled"
            }
        )
    );
    Ok(())
}

/// 从暂存目录安装：先校验清单与 bundle，再按清单形态发布；任何一步失败都不
/// 改动原记录与指针，暂存目录随来源一起删除。
fn install_staged(
    config: &hc_tui::config::Config,
    id: Option<&str>,
    staged: &hc_tui::pkg::source::Staged,
    policy: hc_tui::plugin::registry::EnablePolicy,
    yes: bool,
) -> Result<()> {
    let loaded = hc_tui::pkg::install_ready(staged.path()).map_err(anyhow::Error::msg)?;
    if let Some(id) = id {
        if loaded.manifest.id() != id {
            anyhow::bail!(t3(
                "{} declares package id {}, not {}",
                staged.origin(),
                loaded.manifest.id(),
                id
            ));
        }
    }
    let legacy = loaded.origin == hc_tui::pkg::Origin::LegacyMod;
    print_package_preview(&loaded.manifest);
    println!("{}", t1("  source {}", staged.origin()));
    confirm_install(yes)?;
    let record = if legacy {
        hc_tui::mods::store::install_local_from(config, staged.path(), Some(staged.origin()))
            .map_err(anyhow::Error::msg)?
    } else {
        hc_tui::plugin::registry::install_from(config, staged.path(), Some(staged.origin()), policy)
            .map_err(anyhow::Error::msg)?
    };
    println!(
        "{}",
        t3(
            "installed {} generation {} ({})",
            record.id,
            record.generation,
            if record.enabled {
                "enabled"
            } else {
                "disabled"
            }
        )
    );
    Ok(())
}

fn print_package_preview(manifest: &hc_tui::plugin::manifest::Manifest) {
    println!(
        "{}",
        t3(
            "package {} ({} {})",
            manifest.id(),
            manifest.name(),
            manifest.version()
        )
    );
    println!("{}", t1("  provides {}", manifest.capabilities().badges()));
    if !manifest.actions.is_empty() {
        println!(
            "{}",
            t1(
                "  actions {}",
                manifest
                    .actions
                    .iter()
                    .map(|action| manifest.action_name(action))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        );
    }
    for terminal in &manifest.terminals {
        println!(
            "{}",
            t2(
                "  terminal {} ({})",
                format!("{}.{}", manifest.id(), terminal.id()),
                terminal.placement()
            )
        );
        if !terminal.requires_programs().is_empty() {
            println!(
                "{}",
                t1("    requires {}", terminal.requires_programs().join(", "))
            );
        }
    }
    if let Some(module) = manifest.module_info() {
        println!("{}", t2("  module {} ({})", module.bundle, module.place));
    }
    let platforms = manifest.platforms().join(", ");
    if !platforms.is_empty() {
        println!("{}", t1("  platforms {}", platforms));
    }
    let grants = manifest.grants();
    if !grants.is_empty() {
        println!("{}", t1("  grants {}", grants.join(", ")));
    }
}

fn confirm_install(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        print!("{} [y/N] ", t("install?"));
        use std::io::Write as _;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if matches!(answer.trim(), "y" | "Y" | "yes") {
            return Ok(());
        }
        anyhow::bail!(t("cancelled"));
    }
    anyhow::bail!(t("pass --yes to install without a terminal"))
}

/// 链接一个包：旧模块目录走模块存储，v2 / v1 清单走包注册表。
fn packages_link(
    config: &hc_tui::config::Config,
    target: &str,
    policy: hc_tui::plugin::registry::EnablePolicy,
) -> Result<()> {
    let path = std::path::Path::new(target);
    let loaded = hc_tui::pkg::install_ready(path).map_err(anyhow::Error::msg)?;
    let record = if loaded.origin == hc_tui::pkg::Origin::LegacyMod {
        hc_tui::mods::store::link(config, path).map_err(anyhow::Error::msg)?
    } else {
        hc_tui::plugin::registry::link(config, path, policy).map_err(anyhow::Error::msg)?
    };
    println!(
        "{}",
        t2(
            "linked {} ({})",
            record.id,
            if record.enabled {
                "enabled"
            } else {
                "disabled"
            }
        )
    );
    Ok(())
}

/// `hc-tui mods` 子命令：安装分发与只读检查，不依赖运行中的窗口；
/// `open` / `close` / `reload` / `grant` / `enable` / `disable` 优先经本地
/// 控制 socket 转发给运行中的 TUI。
#[derive(Default)]
struct ModsArgs {
    targets: Vec<String>,
    config: Option<std::path::PathBuf>,
    json: bool,
    yes: bool,
    lines: usize,
    host: Vec<String>,
    events: Vec<String>,
    revoke: bool,
    props: Option<String>,
    place: Option<String>,
}

fn parse_mods_args(args: &[String]) -> Result<ModsArgs> {
    let mut parsed = ModsArgs {
        lines: 100,
        ..ModsArgs::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                parsed.config = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            "--json" => parsed.json = true,
            "--yes" | "-y" => parsed.yes = true,
            "-n" => {
                index += 1;
                parsed.lines = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!(t("-n needs a value")))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!(t("-n needs a number")))?;
            }
            "--host" => {
                index += 1;
                parsed.host.push(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--host needs a value")))?,
                );
            }
            "--events" => {
                index += 1;
                parsed.events.push(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--events needs a value")))?,
                );
            }
            "--revoke" => parsed.revoke = true,
            "--props" => {
                index += 1;
                parsed.props = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--props needs a value")))?,
                );
            }
            "--place" => {
                index += 1;
                parsed.place = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--place needs a value")))?,
                );
            }
            other if other.starts_with('-') => anyhow::bail!(t1("unknown option: {}", other)),
            other => parsed.targets.push(other.to_string()),
        }
        index += 1;
    }
    Ok(parsed)
}

fn mods_config(args: &ModsArgs) -> hc_tui::config::Config {
    hc_tui::config::Config::load(args.config.clone())
}

fn mods_do(config: &hc_tui::config::Config, command: &str, args: serde_json::Value) -> Result<()> {
    let answer = hc_tui::control::call(
        config,
        serde_json::json!({"method": "do", "command": command, "args": args}),
    )?;
    println!("{}", serde_json::to_string_pretty(&answer)?);
    Ok(())
}

fn mods_command(args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        anyhow::bail!("{}", packages_usage("mods"))
    };
    if wants_help(sub) || args[1..].iter().any(|arg| wants_help(arg)) {
        println!("{}", packages_usage("mods"));
        return Ok(());
    }
    // 安装类子命令统一走 `packages`；`mods` 只保留一行弃用提示。
    if matches!(
        sub,
        "install" | "update" | "uninstall" | "link" | "unlink" | "enable" | "disable"
    ) {
        return packages_manage(sub, &args[1..], Some("mods"));
    }
    let parsed = parse_mods_args(&args[1..])?;
    let config = mods_config(&parsed);
    match sub {
        "list" => {
            let records = hc_tui::store::Store::open(config.mods_state_dir()).list();
            if parsed.json {
                println!("{}", serde_json::to_string_pretty(&records)?);
                return Ok(());
            }
            if records.is_empty() {
                println!("{}", t("no mods installed"));
                return Ok(());
            }
            for record in records {
                let version = record
                    .app
                    .as_ref()
                    .and_then(|app| app.get("version"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("-");
                println!(
                    "{:<24} {:<9} gen {:<3} {:<8} {}",
                    record.id,
                    record.source.as_str(),
                    record.generation,
                    if record.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    version
                );
            }
            Ok(())
        }
        "check" => {
            if parsed.targets.is_empty() {
                anyhow::bail!(t("usage: hc-tui mods check <path|id>..."));
            }
            for target in &parsed.targets {
                let report =
                    hc_tui::mods::check::check(&config, target).map_err(anyhow::Error::msg)?;
                println!("{} {}", report.id, t("ok"));
                println!("{}", t1("  path {}", report.path.display()));
                if !report.grants.is_empty() {
                    println!("{}", t1("  grants {}", report.grants.join(", ")));
                }
                for warning in report.warnings {
                    println!("{}", t1("  warning: {}", warning));
                }
            }
            Ok(())
        }
        "test" | "build" => {
            let target = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods test|build <path|id>")))?;
            let dir =
                hc_tui::mods::check::resolve_dir(&config, target).map_err(anyhow::Error::msg)?;
            let mut command = std::process::Command::new("nsh");
            command.arg(sub);
            if sub == "build" {
                command.arg("-o").arg(dir.join("dist"));
            }
            let status = command
                .arg(&dir)
                .status()
                .map_err(|error| anyhow::anyhow!(t2("nsh {}: {}", sub, error)))?;
            if !status.success() {
                anyhow::bail!(t1("nsh {} failed", sub));
            }
            Ok(())
        }
        "log" => {
            let id = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods log <id> [-n 100]")))?;
            let store = hc_tui::store::Store::open(config.mods_state_dir());
            match store.log_tail(id, parsed.lines) {
                Ok(text) => println!("{text}"),
                Err(error) => println!("{}", t2("no log for {}: {}", id, error)),
            }
            Ok(())
        }
        "open" | "close" | "reload" => {
            let mut payload = serde_json::Map::new();
            if let Some(name) = parsed.targets.first() {
                payload.insert("name".into(), serde_json::json!(name));
            }
            if let Some(place) = &parsed.place {
                payload.insert("place".into(), serde_json::json!(place));
            }
            if let Some(props) = &parsed.props {
                payload.insert(
                    "props".into(),
                    serde_json::from_str(props)
                        .map_err(|error| anyhow::anyhow!(t1("--props: {}", error)))?,
                );
            }
            if sub == "close" {
                payload.remove("props");
                payload.remove("place");
            }
            let command = match sub {
                "open" => "mods.open",
                "close" => "mods.close",
                _ => "mods.reload",
            };
            mods_do(&config, command, serde_json::Value::Object(payload))
        }
        "grant" => {
            let id = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods grant <id> --host ...")))?;
            let mut names = parsed.host.clone();
            names.extend(parsed.events.clone());
            if names.is_empty() {
                anyhow::bail!(t("pass --host or --events"));
            }
            let args = serde_json::json!({
                "name": id,
                "host": parsed.host,
                "events": parsed.events,
                "revoke": parsed.revoke,
            });
            if mods_do(&config, "mods.grant", args).is_ok() {
                return Ok(());
            }
            // 没有运行中的窗口：直接写持久化授予。
            let mut grants = hc_tui::mods::store::load_grants(&config, id);
            if parsed.revoke {
                grants.retain(|name| !names.contains(name));
            } else {
                for name in names {
                    if !grants.contains(&name) {
                        grants.push(name);
                    }
                }
                grants.sort();
            }
            hc_tui::mods::store::save_grants(&config, id, &grants).map_err(anyhow::Error::msg)?;
            println!("{}", t2("grants for {}: {}", id, grants.join(", ")));
            Ok(())
        }
        other => anyhow::bail!("{} (unknown: {})", packages_usage("mods"), other),
    }
}

fn install_or_update(
    config: &hc_tui::config::Config,
    yes: bool,
    target: &str,
    id: Option<&str>,
) -> Result<()> {
    let path = std::path::Path::new(target);
    let preview = if path.is_dir() {
        hc_tui::mods::store::preview_local(path).map_err(anyhow::Error::msg)?
    } else {
        hc_tui::mods::store::verify_package(config, path)
            .map_err(anyhow::Error::msg)?
            .0
    };
    println!(
        "{}",
        t3("mod {} ({} {})", preview.id, preview.name, preview.version)
    );
    println!("{}", t1("  source {}", preview.source.as_str()));
    if !preview.grants.is_empty() {
        println!("{}", t1("  grants {}", preview.grants.join(", ")));
    }
    println!("{}", t1("  files {}", preview.files));
    for warning in &preview.warnings {
        println!("{}", t1("  warning: {}", warning));
    }
    if !yes {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            print!("{} [y/N] ", t("install?"));
            use std::io::Write as _;
            std::io::stdout().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim(), "y" | "Y" | "yes") {
                anyhow::bail!(t("cancelled"));
            }
        } else {
            anyhow::bail!(t("pass --yes to install without a terminal"));
        }
    }
    let record = if id.is_some() || path.is_dir() {
        hc_tui::mods::store::update(config, id, path).map_err(anyhow::Error::msg)?
    } else {
        hc_tui::mods::store::install_package(config, path).map_err(anyhow::Error::msg)?
    };
    println!(
        "{}",
        t2("installed {} generation {}", record.id, record.generation)
    );
    Ok(())
}

/// `hc-tui notify`：把一条通知交给运行中的窗口，走与模块 `notify` 相同的通道。
///
/// 没有窗口时按退出码 3 说明原因：core 文档里没有通知槽位，写了也是静默丢弃。
fn notify_command(args: &[String]) -> Result<()> {
    let mut text = None;
    let mut level = String::from("info");
    let mut action = None;
    let mut target = None;
    let mut options: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--level" | "--action" | "--target" | "--session" | "--config" => {
                let flag = args[index].clone();
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!(t1("{} needs a value", &flag)))?;
                match flag.as_str() {
                    "--level" => level = value,
                    "--action" => action = Some(value),
                    "--target" => target = Some(value),
                    _ => options.extend([flag, value]),
                }
            }
            other if other.starts_with('-') => {
                anyhow::bail!(t1("unknown option: {}", other));
            }
            other => {
                if text.replace(other.to_string()).is_some() {
                    anyhow::bail!(t("notify takes exactly one text argument"));
                }
            }
        }
        index += 1;
    }
    let Some(text) = text else {
        anyhow::bail!(t(
            "usage: hc-tui notify <text> [--level info|warn|error] [--action <name>] [--target <terminal|pane>]"
        ));
    };
    if text.trim().is_empty() {
        anyhow::bail!(t("notify requires a text"));
    }
    if !matches!(level.as_str(), "info" | "warn" | "error") {
        anyhow::bail!(t1("unknown level {}; use info, warn or error", &level));
    }
    let config = parse_options(&options)?;
    let mut request = serde_json::json!({"method":"notify","text":text,"level":level});
    if let Some(action) = action {
        request["action"] = serde_json::json!({"name": action});
    }
    if let Some(target) = target {
        request["target"] = serde_json::json!(target);
    }
    match hc_tui::control::call(&config, request) {
        Ok(answer) => {
            println!("{answer}");
            Ok(())
        }
        Err(error) if hc_tui::app::winless::window_unavailable(&error) => {
            let session = config
                .session
                .clone()
                .or_else(|| std::env::var("HC_SESSION").ok())
                .filter(|session| !session.is_empty())
                .unwrap_or_else(|| "default".to_string());
            eprintln!(
                "{}",
                t1(
                    "no hc-tui window is running on session {}; notification not delivered (exit 3).",
                    &session
                )
            );
            eprintln!(
                "{}",
                t("the core doc stores layout only, so there is no place to keep the notification; start hc-tui and retry")
            );
            std::process::exit(3)
        }
        Err(error) => Err(error),
    }
}

/// 先读 `--config`，再让文件里的值成为基线，最后应用命令行覆盖。
fn parse_options(args: &[String]) -> Result<hc_tui::config::Config> {
    let mut index = 0;
    let mut path = None;
    let mut session = None;
    let mut attach = None;
    let mut restore = None;
    let mut no_mods = None;
    while index < args.len() {
        match args[index].as_str() {
            "--session" => {
                index += 1;
                session = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--session needs a value")))?,
                );
            }
            "--attach" => {
                index += 1;
                attach = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--attach needs a value")))?,
                );
            }
            "--config" => {
                index += 1;
                path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(t("--config needs a value")))?,
                ));
            }
            "--no-restore" => restore = Some(false),
            "--no-mods" => no_mods = Some(true),
            other => anyhow::bail!(t1("unknown option: {}", other)),
        }
        index += 1;
    }
    let mut config = hc_tui::config::Config::load(path);
    hc_tui::i18n::set_zh(config.zh());
    if let Some(session) = session {
        config.session = Some(session);
    }
    if let Some(attach) = attach {
        config.attach = Some(attach);
    }
    if let Some(restore) = restore {
        config.restore = restore;
    }
    if let Some(no_mods) = no_mods {
        config.no_mods = no_mods;
    }
    Ok(config)
}

fn run_tui(config: hc_tui::config::Config) -> Result<()> {
    // panic 时也要把外层终端还原，否则 shell 会留在 raw mode。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = hc_tui::tui::restore();
        hc_tui::term::input::restore_keyboard();
        default_hook(info);
    }));
    // 探测与启用宿主键盘协议；未启用时不动终端。
    let keyboard = hc_tui::term::input::KeyboardGuard::open(config.keyboard_protocol);
    let result = hc_tui::app::run(config);
    drop(keyboard);
    result
}

/// `hc-tui keys probe [--seconds <n>]`：打印宿主键盘协议的探测回复与原始按键字节。
/// `hc-tui keys decode <hex>`：把一段十六进制字节按 kitty 协议解成按键。
fn keys_command(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("probe") => keys_probe(&args[1..]),
        Some("decode") => {
            let text = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!(t("decode needs hex bytes")))?;
            let bytes = parse_hex(text)?;
            let (key, used) = hc_tui::term::input::parse_kitty_key(&bytes)
                .or_else(|| {
                    let byte = *bytes.first()?;
                    let key = hc_tui::term::input::parse_legacy_key(byte)?;
                    Some((key, 1))
                })
                .ok_or_else(|| anyhow::anyhow!(t("not a key sequence")))?;
            println!("used {used} key {key:?}");
            Ok(())
        }
        _ => anyhow::bail!(t("usage: hc-tui keys probe [--protocol auto|kitty|legacy] [--seconds <n>] | decode <hex>")),
    }
}

fn keys_probe(args: &[String]) -> Result<()> {
    use std::time::{Duration, Instant};

    let mut seconds = 5u64;
    let mut policy = hc_tui::config::KeyboardProtocol::Auto;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--seconds" => {
                index += 1;
                seconds = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!(t("--seconds needs a value")))?
                    .parse()?;
            }
            "--protocol" => {
                index += 1;
                let text = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!(t("--protocol needs a value")))?;
                policy = hc_tui::config::KeyboardProtocol::parse(text)
                    .ok_or_else(|| anyhow::anyhow!(t1("unknown protocol: {}", text)))?;
            }
            other => anyhow::bail!(t1("unknown option: {}", other)),
        }
        index += 1;
    }

    crossterm::terminal::enable_raw_mode()?;
    let guard = hc_tui::term::input::KeyboardGuard::open(policy);
    println!("policy {policy:?}");
    println!("probe reply {:02x?}", guard.reply());
    println!("enabled {}", guard.active());
    println!(
        "{}",
        t1("typing window: {}s, press Ctrl+I then Tab", seconds)
    );
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut pending: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        let mut chunk = Vec::new();
        if !hc_tui::term::input::read_tty(Duration::from_millis(50), &mut chunk) {
            continue;
        }
        println!("bytes {:02x?}", chunk);
        pending.extend_from_slice(&chunk);
        while !pending.is_empty() {
            if hc_tui::term::input::kitty_incomplete(&pending) {
                break;
            }
            match hc_tui::term::input::parse_kitty_key(&pending) {
                Some((key, used)) => {
                    println!("key {key:?}");
                    pending.drain(..used);
                }
                None => {
                    let byte = pending[0];
                    match hc_tui::term::input::parse_legacy_key(byte) {
                        Some(key) => {
                            println!("key {key:?}");
                            pending.remove(0);
                        }
                        None => println!("raw byte {:02x}", pending.remove(0)),
                    }
                }
            }
        }
    }
    drop(guard);
    crossterm::terminal::disable_raw_mode()?;
    println!("restored");
    Ok(())
}

fn parse_hex(text: &str) -> Result<Vec<u8>> {
    let digits: String = text
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    if !digits.len().is_multiple_of(2) {
        anyhow::bail!(t1("odd hex length: {}", digits.len()));
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.as_bytes().chunks(2) {
        let byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// P0 诊断：连上 core，打印握手结果。不进入 TUI。
fn probe(args: &[String]) -> Result<()> {
    let config = parse_options(args)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut conn = hc_tui::hc::client::Conn::connect(config.session.as_deref()).await?;
            println!(
                "{}",
                t2(
                    "hc {} (generation {})",
                    &conn.hello.version,
                    conn.hello.generation
                )
            );
            println!(
                "{}",
                t2(
                    "core    {} ({})",
                    &conn.hello.core.name,
                    &conn.hello.core.id
                )
            );
            println!("framed  {}", conn.hello.framed);
            println!("{}", t1("features {}", conn.hello.features.join(" ")));
            println!("{}", t1("methods  {}", conn.hello.methods.len()));

            let terminals = conn.call("terminal.list", serde_json::json!({})).await?;
            let count = terminals
                .get("terminals")
                .and_then(|t| t.as_array())
                .map(|t| t.len())
                .unwrap_or(0);
            println!("{}", t1("terminals {}", count));
            Ok::<(), anyhow::Error>(())
        })
}
