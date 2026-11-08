//! `hc-tui config dump|init`：整份配置的导出与默认配置的落盘。
//!
//! 序列化只有一处实现（[`hc_tui::config_dump::dump`]），`dump --defaults` 的输出
//! 与仓库里的 `docs/default-config.toml` 逐字节相同，由
//! `tests/config_dump.rs::config_default_file_is_current` 守住。`init` 用同一份
//! 文本写用户配置路径；目标已存在时先拒绝，`--force` 才覆盖并备份。

use std::path::PathBuf;

use anyhow::Result;
use hc_tui::config::Config;

pub use hc_tui::config_dump::{dump, Mode};

/// `hc-tui config dump [--effective|--defaults|--minimal] [--config <path>] [--out <path>]`
pub fn dump_command(args: &[String]) -> Result<()> {
    let mut mode = None;
    let mut config_path = None;
    let mut out = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--effective" => set_mode(&mut mode, Mode::Effective)?,
            "--defaults" => set_mode(&mut mode, Mode::Defaults)?,
            "--minimal" => set_mode(&mut mode, Mode::Minimal)?,
            "--config" => config_path = Some(value(args, &mut index, "--config")?),
            "--out" => out = Some(value(args, &mut index, "--out")?),
            other => anyhow::bail!("unknown option: {other}"),
        }
        index += 1;
    }
    let mode = mode.unwrap_or(Mode::Effective);
    let config = Config::load(config_path);
    if !config.errors.is_empty() {
        anyhow::bail!(
            "配置文件有 {} 个错误，先跑 `hc-tui config check` 修掉",
            config.errors.len()
        );
    }
    let text = dump(&config, mode);
    let Some(path) = out else {
        print!("{text}");
        return Ok(());
    };
    write_file(&path, &text)?;
    println!("已写入 {}（{}）", path.display(), mode_label(mode));
    Ok(())
}

/// `hc-tui config init [--path <path>] [--force]`
pub fn init_command(args: &[String]) -> Result<()> {
    let mut path = None;
    let mut force = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--path" => path = Some(value(args, &mut index, "--path")?),
            "--force" => force = true,
            other => anyhow::bail!("unknown option: {other}"),
        }
        index += 1;
    }
    let target = path.unwrap_or_else(|| Config::default().config_path);
    let text = dump(&Config::default(), Mode::Defaults);
    if target.exists() {
        if !force {
            anyhow::bail!(
                "{} 已存在；加 --force 覆盖（覆盖前会备份成 .bak.<时间戳>）",
                target.display()
            );
        }
        let original = std::fs::read(&target)
            .map_err(|error| anyhow::anyhow!("{}: {error}", target.display()))?;
        let backup = crate::cli::config::backup_path(&target);
        std::fs::write(&backup, original)
            .map_err(|error| anyhow::anyhow!("{}: {error}", backup.display()))?;
        println!("已备份 {}", backup.display());
    }
    write_file(&target, &text)?;
    println!("已写入 {}", target.display());
    Ok(())
}

fn set_mode(current: &mut Option<Mode>, mode: Mode) -> Result<()> {
    if current.is_some() {
        anyhow::bail!("--effective / --defaults / --minimal 只能选一个");
    }
    *current = Some(mode);
    Ok(())
}

fn value(args: &[String], index: &mut usize, name: &str) -> Result<PathBuf> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| anyhow::anyhow!("{name} needs a value"))?;
    Ok(PathBuf::from(value))
}

/// `--out` 的父目录必须已存在：不替用户建目录，也不静默写到别处。
fn write_file(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            anyhow::bail!("{}: 目录不存在", parent.display());
        }
    }
    std::fs::write(path, text).map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Effective => "生效配置",
        Mode::Defaults => "默认配置",
        Mode::Minimal => "与默认值的差异",
    }
}
