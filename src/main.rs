use anyhow::Result;
use hc_tui::i18n::{t, t1, t2, t3};

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
        Some("plugins") => plugins_command(&args[1..]),
        Some("events") => events_command(&args[1..]),
        Some("probe") => probe(&args[1..]),
        Some("commands") => {
            let config = parse_options(&args[1..])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&hc_tui::app::command_catalog_for(&config))?
            );
            Ok(())
        }
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
            let (value, start) = match args.get(2).filter(|s| s.starts_with('{')) {
                Some(value) => (serde_json::from_str::<serde_json::Value>(value)?, 3),
                None => (serde_json::json!({}), 2),
            };
            let config = parse_options(&args[start..])?;
            println!(
                "{}",
                hc_tui::control::call(
                    &config,
                    serde_json::json!({"method":"do","command":command,"args":value})
                )?
            );
            Ok(())
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

fn usage() {
    println!("hc-tui {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("{}", t("usage: hc-tui [options]"));
    println!("{}", t("       hc-tui probe [options]"));
    println!("       hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]");
    println!("       hc-tui session-hook install|remove|status");
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
        t("       hc-tui plugins list|install|link|uninstall|enable|disable|gc ...")
    );
    println!(
        "{}",
        t("       hc-tui events [--plugin <id>] [--kinds a,b] [--count <n>]")
    );
    println!(
        "{}",
        t("       hc-tui do <command> [JSON arguments] [options]")
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
        anyhow::bail!(t(
            "usage: hc-tui plugins action list|invoke|status|cancel|log"
        ))
    };
    if group == "pane" {
        return plugins_pane(&args[1..]);
    }
    if group != "action" {
        return plugins_manage(group, &args[1..]);
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
            let response = call(serde_json::json!({
                "method": "do",
                "command": name,
                "args": args,
            }))?;
            println!("{}", serde_json::to_string_pretty(&response)?);
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

/// `hc-tui plugins list|install|update|link|uninstall|enable|disable`：
/// 插件注册与代际管理，复用 M5 的通用存储。
fn plugins_manage(group: &str, args: &[String]) -> Result<()> {
    let mut config_path = None;
    let mut json = false;
    let mut yes = false;
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
            "--yes" | "-y" => yes = true,
            other if other.starts_with('-') => anyhow::bail!(t1("unknown option: {}", other)),
            other => targets.push(other.to_string()),
        }
        index += 1;
    }
    let config = hc_tui::config::Config::load(config_path);
    match group {
        "list" => {
            let records = hc_tui::store::Store::open(config.plugins_state_dir()).list();
            let registry = hc_tui::plugin::registry::Registry::load(&config);
            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
                return Ok(());
            }
            if records.is_empty() {
                println!("{}", t("no plugins installed"));
            }
            for record in records {
                println!(
                    "{:<32} {:<9} gen {:<3} {}",
                    record.id,
                    record.source.as_str(),
                    record.generation,
                    if record.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                if let Some(plugin) = registry
                    .plugins
                    .iter()
                    .find(|plugin| plugin.id == record.id)
                {
                    for terminal in &plugin.manifest.terminals {
                        println!(
                            "    {:<40} {} · {}",
                            format!("{}.{}", plugin.id, terminal.id()),
                            terminal.placement(),
                            terminal.title()
                        );
                    }
                }
            }
            Ok(())
        }
        "install" | "update" => {
            let dir = targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui plugins install <dir>")))?;
            let manifest =
                hc_tui::plugin::manifest::load(&std::path::Path::new(dir).join("plugin.toml"))
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            println!(
                "{}",
                t3(
                    "plugin {} ({} {})",
                    manifest.id(),
                    manifest.name(),
                    manifest.version()
                )
            );
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
            println!("{}", t1("  platforms {}", manifest.platforms().join(", ")));
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
            let record = hc_tui::plugin::registry::install(&config, std::path::Path::new(dir))
                .map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                t2("installed {} generation {}", record.id, record.generation)
            );
            Ok(())
        }
        "link" => {
            let dir = targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui plugins link <dir>")))?;
            let record = hc_tui::plugin::registry::link(&config, std::path::Path::new(dir))
                .map_err(anyhow::Error::msg)?;
            println!("{}", t1("linked {}", record.id));
            Ok(())
        }
        "uninstall" => {
            let id = targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui plugins uninstall <id>")))?;
            hc_tui::plugin::registry::uninstall(&config, id).map_err(anyhow::Error::msg)?;
            println!("{}", t1("uninstalled {}", id));
            Ok(())
        }
        "enable" | "disable" => {
            let id = targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui plugins enable|disable <id>")))?;
            let enabled = group == "enable";
            hc_tui::plugin::registry::set_enabled(&config, id, enabled)
                .map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                t2("{} {}", if enabled { "enabled" } else { "disabled" }, id)
            );
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
        other => anyhow::bail!(t1(
            "usage: hc-tui plugins list|install|link|uninstall|enable|disable|gc (unknown: {})",
            other
        )),
    }
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
        anyhow::bail!(t(
            "usage: hc-tui mods list|check|test|build|install|update|uninstall|link|unlink|log|open|close|reload|grant|enable|disable"
        ))
    };
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
        "install" => {
            let target = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods install <dir|.nshpkg>")))?
                .clone();
            install_or_update(&config, &parsed, &target, None)
        }
        "update" => {
            let (id, target) = match parsed.targets.as_slice() {
                [id, target] => (Some(id.clone()), target.clone()),
                [target] => (None, target.clone()),
                _ => anyhow::bail!(t("usage: hc-tui mods update <id> <dir|.nshpkg>")),
            };
            install_or_update(&config, &parsed, &target, id.as_deref())
        }
        "uninstall" => {
            let id = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods uninstall <id>")))?;
            hc_tui::mods::store::uninstall(&config, id).map_err(anyhow::Error::msg)?;
            println!("{}", t1("uninstalled {}", id));
            Ok(())
        }
        "link" => {
            let dir = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods link <dir>")))?;
            let record = hc_tui::mods::store::link(&config, std::path::Path::new(dir))
                .map_err(anyhow::Error::msg)?;
            println!("{}", t2("linked {} at {}", record.id, dir));
            Ok(())
        }
        "unlink" => {
            let id = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods unlink <id>")))?;
            hc_tui::mods::store::unlink(&config, id).map_err(anyhow::Error::msg)?;
            println!("{}", t1("unlinked {}", id));
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
        "enable" | "disable" => {
            let id = parsed
                .targets
                .first()
                .ok_or_else(|| anyhow::anyhow!(t("usage: hc-tui mods enable|disable <id>")))?;
            let enable = sub == "enable";
            let args = serde_json::json!({"name": id});
            if mods_do(&config, &format!("mods.{sub}"), args).is_ok() {
                return Ok(());
            }
            hc_tui::mods::store::set_enabled(&config, id, enable).map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                t1("{} {id}", if enable { "enabled" } else { "disabled" })
            );
            Ok(())
        }
        other => anyhow::bail!(t1("unknown mods command: {}", other)),
    }
}

fn install_or_update(
    config: &hc_tui::config::Config,
    parsed: &ModsArgs,
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
    if !parsed.yes {
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
        default_hook(info);
    }));
    hc_tui::app::run(config)
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
