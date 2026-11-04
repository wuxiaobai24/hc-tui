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
        Some("probe") => probe(),
        Some(other) => {
            eprintln!("unknown command: {other}");
            eprintln!("usage: hc-tui [probe|--version]");
            std::process::exit(2);
        }
        None => run_tui(),
    }
}

fn run_tui() -> Result<()> {
    // panic 时也要把外层终端还原，否则 shell 会留在 raw mode。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = hc_tui::tui::restore();
        default_hook(info);
    }));
    hc_tui::app::run()
}

/// P0 诊断：连上 core，打印握手结果。不进入 TUI。
fn probe() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut conn = hc_tui::hc::client::Conn::connect(None).await?;
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
