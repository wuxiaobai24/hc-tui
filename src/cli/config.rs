//! `hc-tui config check|reset-keys|dump|init`：配置文件诊断、`[keys]` 段重置、
//! 整份配置导出与默认配置落盘。
//!
//! `check` 只读配置文件，错误带 `文件:行:列`；有错误退出 1，只有警告退出 2，
//! 干净时退出 0。`reset-keys` 先备份再把 `[keys]` 段换成默认键位，其它段原样
//! 保留；不带 `--yes` 时只打印计划，不落盘。`dump` 与 `init` 见 [`crate::cli::config_dump`]。

use std::path::{Path, PathBuf};

use anyhow::Result;
use hc_tui::config::{self, Config, Keys};
use hc_tui::i18n::{t, t1};

/// `config` 的用法行；`--help` 与未知子命令共用。
fn usage() -> &'static str {
    "usage: hc-tui config check|reset-keys [--config <path>] [--yes]\n       hc-tui config dump [--effective|--defaults|--minimal] [--config <path>] [--out <path>]\n       hc-tui config init [--path <path>] [--force]"
}

pub fn run(args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        anyhow::bail!("{}", usage());
    };
    if crate::wants_help(sub) || args[1..].iter().any(|arg| crate::wants_help(arg)) {
        println!("{}", usage());
        return Ok(());
    }
    match sub {
        "check" => check(&args[1..]),
        "reset-keys" => reset_keys(&args[1..]),
        "dump" => crate::cli::config_dump::dump_command(&args[1..]),
        "init" => crate::cli::config_dump::init_command(&args[1..]),
        other => anyhow::bail!("{} (unknown: {})", usage(), other),
    }
}

#[derive(Default)]
struct Args {
    config: Option<PathBuf>,
    yes: bool,
}

fn parse_args(args: &[String]) -> Result<Args> {
    let mut parsed = Args::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                parsed.config =
                    Some(PathBuf::from(args.get(index).cloned().ok_or_else(
                        || anyhow::anyhow!(t("--config needs a value")),
                    )?));
            }
            "--yes" | "-y" => parsed.yes = true,
            other => anyhow::bail!(t1("unknown option: {}", other)),
        }
        index += 1;
    }
    Ok(parsed)
}

fn check(args: &[String]) -> Result<()> {
    let parsed = parse_args(args)?;
    let config = Config::load(parsed.config);
    let path = config.config_path.clone();
    println!("配置 {}", path.display());
    if !path.exists() {
        println!("    文件不存在，全部使用默认值");
    }
    if !config.errors.is_empty() {
        println!(
            "Error: 配置文件非法 {}（{} 处）",
            path.display(),
            config.errors.len()
        );
        for error in &config.errors {
            println!("  {}", config::describe(&path, error));
        }
        println!("  你的情况：这个文件不符合 config.toml 的语法或字段约束，hc-tui 按默认值启动");
        println!(
            "Possible fix: cp {path} {path}.bak && hc-tui --default-config > {path}",
            path = path.display()
        );
        std::process::exit(1);
    }
    let theme = match config.theme_name_position {
        Some(position) => format!(
            "{}（来自 {}:{}, 来源 {}）",
            config.theme.name,
            path.display(),
            position.line,
            config.theme_source
        ),
        None => format!(
            "{}（内置默认, 来源 {}）",
            config.theme.name, config.theme_source
        ),
    };
    println!("主题 {theme}");
    let conflicts = config.key_conflicts();
    println!(
        "键位 {} 个 · 冲突 {} 处",
        config.keys.entries().len(),
        conflicts.len()
    );
    println!("渲染 render_fps = {}", config.render_fps);
    println!(
        "mods 信任根 {} 个 · 撤销列表 {}",
        config.mods_trust_roots.len(),
        config.mods_revocations_path().display()
    );
    let plugins = config.plugins_dir();
    println!(
        "插件 目录 {}（{}）· [[panels]] {} 条 · 自定义命令 {} 条",
        plugins.display(),
        if plugins.is_dir() {
            "存在"
        } else {
            "不存在"
        },
        config.panels.len(),
        config.commands.len()
    );
    for conflict in &conflicts {
        println!("{}", config::describe(&path, conflict));
    }
    for warning in &config.warnings {
        println!("警告 {warning}");
    }
    let count = conflicts.len() + config.warnings.len();
    if count > 0 {
        println!("{count} 个警告");
        std::process::exit(2);
    }
    Ok(())
}

fn reset_keys(args: &[String]) -> Result<()> {
    let parsed = parse_args(args)?;
    let path = parsed
        .config
        .unwrap_or_else(|| Config::default().config_path);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    let Some(plan) = config::reset_keys(&text) else {
        println!("{}: 没有 [keys] 段，无需重置", path.display());
        return Ok(());
    };
    println!("配置 {}", path.display());
    println!("[keys] 段在第 {}–{} 行", plan.first_line, plan.last_line);
    if !plan.sections.is_empty() {
        println!("保留其它段 {}", plan.sections.join("、"));
    }
    println!("重置为默认键位 {} 个", Keys::default().entries().len());
    if !parsed.yes {
        println!("未写入；加 --yes 执行");
        return Ok(());
    }
    let backup = backup_path(&path);
    std::fs::write(&backup, &text)
        .map_err(|error| anyhow::anyhow!("{}: {error}", backup.display()))?;
    std::fs::write(&path, &plan.text)
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    println!("已备份 {}", backup.display());
    println!("已重置 {}", path.display());
    Ok(())
}

/// `config.toml.bak.<秒级时间戳>`；同一秒内重复执行时加序号，不覆盖旧备份。
pub(crate) fn backup_path(path: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let named = |suffix: String| {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    let first = named(format!(".bak.{stamp}"));
    if !first.exists() {
        return first;
    }
    let mut index = 1;
    loop {
        let candidate = named(format!(".bak.{stamp}-{index}"));
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}
