use anyhow::Result;

fn main() -> Result<()> {
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
                Some("install") => println!("installed {}", hook::install()?.display()),
                Some("remove") => println!("removed: {}", hook::remove()?),
                Some("status") => println!("{}", hook::describe(hook::status())),
                _ => anyhow::bail!(
                    "usage: hc-tui session-hook install|remove|status (runs on this machine)"
                ),
            }
            Ok(())
        }
        Some("update") => hc_tui::update::run(&args[1..]),
        Some("probe") => probe(&args[1..]),
        Some("commands") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&hc_tui::app::command_catalog())?
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
                .ok_or_else(|| anyhow::anyhow!("do requires a command"))?;
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
            eprintln!("unknown command: {other}");
            usage();
            std::process::exit(2);
        }
        None => run_tui(parse_options(&args)?),
    }
}

fn usage() {
    println!("hc-tui {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("usage: hc-tui [options]");
    println!("       hc-tui probe [options]");
    println!("       hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]");
    println!("       hc-tui session-hook install|remove|status");
    println!("       hc-tui --default-config");
    println!("       hc-tui commands | ls [options]");
    println!("       hc-tui do <command> [JSON arguments] [options]");
    println!();
    println!("  --session <name>   hc session to connect to (default: HC_SESSION)");
    println!("  --attach <t>       attach an existing terminal instead of starting a shell");
    println!("  --config <path>    config file (default: ~/.config/hc-tui/config.toml)");
    println!("  --no-restore       ignore the layout saved in the core doc");
}

/// 先读 `--config`，再让文件里的值成为基线，最后应用命令行覆盖。
fn parse_options(args: &[String]) -> Result<hc_tui::config::Config> {
    let mut index = 0;
    let mut path = None;
    let mut session = None;
    let mut attach = None;
    let mut restore = None;
    while index < args.len() {
        match args[index].as_str() {
            "--session" => {
                index += 1;
                session = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("--session needs a value"))?,
                );
            }
            "--attach" => {
                index += 1;
                attach = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("--attach needs a value"))?,
                );
            }
            "--config" => {
                index += 1;
                path = Some(std::path::PathBuf::from(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("--config needs a value"))?,
                ));
            }
            "--no-restore" => restore = Some(false),
            other => anyhow::bail!("unknown option: {other}"),
        }
        index += 1;
    }
    let mut config = hc_tui::config::Config::load(path);
    if let Some(session) = session {
        config.session = Some(session);
    }
    if let Some(attach) = attach {
        config.attach = Some(attach);
    }
    if let Some(restore) = restore {
        config.restore = restore;
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
                "hc {} (generation {})",
                conn.hello.version, conn.hello.generation
            );
            println!("core    {} ({})", conn.hello.core.name, conn.hello.core.id);
            println!("framed  {}", conn.hello.framed);
            println!("features {}", conn.hello.features.join(" "));
            println!("methods  {}", conn.hello.methods.len());

            let terminals = conn.call("terminal.list", serde_json::json!({})).await?;
            let count = terminals
                .get("terminals")
                .and_then(|t| t.as_array())
                .map(|t| t.len())
                .unwrap_or(0);
            println!("terminals {count}");
            Ok::<(), anyhow::Error>(())
        })
}
