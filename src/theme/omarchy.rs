//! Omarchy 主题来源：读 `~/.local/state/omarchy/current/theme/` 下的 `colors.toml`
//! 与 `alacritty.toml`，拼成来源无关的 [`Surface`]。
//!
//! 主题目录可以用环境变量 `OMARCHY_THEME_DIR` 覆盖（测试与临时切换用）。目录本身通常是
//! 符号链接；热重载按被读文件的规范化路径判断，切换软链即使时间戳相同也能发现。

use std::path::{Path, PathBuf};

use ratatui_core::style::Color;

use super::source::{Error, Loaded};
use super::Surface;

/// 配置里表示「跟随 Omarchy」的名字。
pub const NAME: &str = "omarchy";

const COLORS: &str = "colors.toml";
const ALACRITTY: &str = "alacritty.toml";

/// ANSI 0–7 的键名，`[colors.normal]` 与 `[colors.bright]` 都用这套。
const ANSI: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// 主题目录：`OMARCHY_THEME_DIR` 优先，否则 Omarchy 当前主题。
pub fn dir() -> PathBuf {
    if let Some(value) = std::env::var_os("OMARCHY_THEME_DIR") {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/state/omarchy/current/theme")
}

/// 读当前 Omarchy 主题。
pub fn load() -> Result<Loaded, Error> {
    load_dir(&dir())
}

/// 读指定目录的 Omarchy 主题。
pub fn load_dir(dir: &Path) -> Result<Loaded, Error> {
    // 目录根本不存在是「这台机器没装 Omarchy」；目录在但文件缺了才是要警告的问题。
    if !dir.is_dir() {
        return Err(Error::Absent(format!("{} 不存在", dir.display())));
    }
    let colors_path = dir.join(COLORS);
    let text = read(&colors_path).map_err(|error| match error {
        Error::Absent(reason) => Error::Broken(reason),
        error => error,
    })?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|error| Error::Broken(format!("{}: {error}", colors_path.display())))?;
    let required = |key: &str| {
        color(&table, key)
            .ok_or_else(|| Error::Broken(format!("{} 缺少颜色 {key}", colors_path.display())))
    };
    let background = required("background")?;
    let foreground = required("foreground")?;
    let accent = required("accent")?;
    let ansi = palette(dir, &table);
    let surface = Surface {
        bg: background,
        fg: foreground,
        dim: color(&table, "muted").unwrap_or(ansi[8]),
        border: color(&table, "lighter_background").unwrap_or(ansi[8]),
        panel: color(&table, "dark_background").unwrap_or(background),
        raised: color(&table, "lighter_background").unwrap_or(ansi[8]),
        accent,
        selection_bg: color(&table, "selection").unwrap_or(ansi[8]),
        selection_fg: foreground,
        ansi,
    };
    let mut theme = surface.theme(NAME);
    // Omarchy 的 orange 比 ANSI 黄更接近界面用色。
    if let Some(orange) = color(&table, "orange") {
        theme.set("space_orange", orange);
    }
    let mut files = vec![colors_path];
    let alacritty = dir.join(ALACRITTY);
    if alacritty.is_file() {
        files.push(alacritty);
    }
    Ok(Loaded {
        theme,
        label: label(dir),
        files,
    })
}

fn read(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::Absent(format!("{} 不存在", path.display()))
        } else {
            Error::Broken(format!("{}: {error}", path.display()))
        }
    })
}

fn read_table(path: &Path) -> Option<toml::Table> {
    read(path).ok()?.parse::<toml::Table>().ok()
}

fn color(table: &toml::Table, key: &str) -> Option<Color> {
    table.get(key)?.as_str().and_then(super::parse_color)
}

/// `[colors.<section>]` 下的一项。
fn section_color(table: &toml::Table, section: &str, key: &str) -> Option<Color> {
    let group = table.get("colors")?.as_table()?.get(section)?.as_table()?;
    color(group, key)
}

/// ANSI 16 色：`alacritty.toml` 优先，缺失的槽位用 `colors.toml` 拼，最后退回内置默认。
fn palette(dir: &Path, colors: &toml::Table) -> [Color; 16] {
    let mut out = super::builtin("default")
        .expect("default theme exists")
        .terminal_palette();
    let alacritty = read_table(&dir.join(ALACRITTY));
    for (index, key) in ANSI.iter().enumerate() {
        let normal = alacritty
            .as_ref()
            .and_then(|table| section_color(table, "normal", key))
            .or_else(|| compose(colors, key, false));
        if let Some(value) = normal {
            out[index] = value;
        }
        let bright = alacritty
            .as_ref()
            .and_then(|table| section_color(table, "bright", key))
            .or_else(|| compose(colors, key, true));
        if let Some(value) = bright {
            out[index + 8] = value;
        }
    }
    out
}

/// 从 `colors.toml` 拼一个 ANSI 槽位。文件里没有 black / white，用背景与前景补。
fn compose(colors: &toml::Table, key: &str, bright: bool) -> Option<Color> {
    if bright {
        if let Some(value) = color(colors, &format!("bright_{key}")) {
            return Some(value);
        }
    } else if let Some(value) = color(colors, key) {
        return Some(value);
    }
    match key {
        "black" if bright => color(colors, "muted").or_else(|| color(colors, "dark_foreground")),
        "black" => color(colors, "background"),
        "white" if bright => {
            color(colors, "bright_foreground").or_else(|| color(colors, "light_foreground"))
        }
        "white" => color(colors, "foreground"),
        _ if bright => color(colors, key),
        _ => None,
    }
}

/// Omarchy 的主题名：主题目录同级的 `theme.name`。
pub fn theme_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.parent()?.join("theme.name")).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 主题名：优先 `current/theme.name`，否则取主题目录名。
fn label(dir: &Path) -> String {
    if let Some(name) = theme_name(dir) {
        return name;
    }
    std::fs::canonicalize(dir)
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &str = r##"
mode = "dark"
accent = "#89b4fa"
selection = "#45475a"
muted = "#585b70"
background = "#1e1e2e"
dark_background = "#161622"
lighter_background = "#313244"
foreground = "#cdd6f4"
dark_foreground = "#6c7086"
light_foreground = "#bac2de"
bright_foreground = "#cdd6f4"
red = "#f38ba8"
yellow = "#f9e2af"
orange = "#f6b6ab"
green = "#a6e3a1"
cyan = "#94e2d5"
blue = "#89b4fa"
magenta = "#f5c2e7"
brown = "#7b5b55"
bright_red = "#ff0000"
bright_green = "#00ff00"
bright_yellow = "#ffff00"
bright_blue = "#0000ff"
bright_magenta = "#ff00ff"
bright_cyan = "#00ffff"
"##;

    fn write_theme(dir: &Path, colors: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(COLORS), colors).unwrap();
    }

    #[test]
    fn colors_toml_maps_tokens_and_palette() {
        let root = std::env::temp_dir().join("hc-tui-omarchy-map");
        let dir = root.join("current/theme");
        write_theme(&dir, THEME);
        let loaded = load_dir(&dir).unwrap();
        let theme = &loaded.theme;
        assert_eq!(theme.name, NAME);
        assert_eq!(theme.color("bg"), Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(theme.color("accent"), Color::Rgb(0x89, 0xb4, 0xfa));
        assert_eq!(theme.color("dim"), Color::Rgb(0x58, 0x5b, 0x70));
        assert_eq!(theme.color("border"), Color::Rgb(0x31, 0x32, 0x44));
        assert_eq!(theme.color("cursor_fg"), Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(theme.color("space_orange"), Color::Rgb(0xf6, 0xb6, 0xab));
        let palette = theme.terminal_palette();
        assert_eq!(palette[0], Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(palette[1], Color::Rgb(0xf3, 0x8b, 0xa8));
        assert_eq!(palette[7], Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(palette[8], Color::Rgb(0x58, 0x5b, 0x70));
        assert_eq!(palette[9], Color::Rgb(0xff, 0x00, 0x00));
        assert_eq!(palette[15], Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(loaded.label, "theme");
        assert_eq!(loaded.files, vec![dir.join(COLORS)]);
    }

    #[test]
    fn alacritty_wins_for_ansi() {
        let root = std::env::temp_dir().join("hc-tui-omarchy-alacritty");
        let dir = root.join("current/theme");
        write_theme(&dir, THEME);
        std::fs::write(
            dir.join(ALACRITTY),
            "[colors.normal]\nblack = \"#000000\"\nred = \"#110000\"\n\
             green = \"#001100\"\nyellow = \"#111100\"\nblue = \"#000011\"\n\
             magenta = \"#110011\"\ncyan = \"#001111\"\nwhite = \"#111111\"\n",
        )
        .unwrap();
        let loaded = load_dir(&dir).unwrap();
        let palette = loaded.theme.terminal_palette();
        assert_eq!(palette[1], Color::Rgb(0x11, 0x00, 0x00));
        assert_eq!(palette[8], Color::Rgb(0x58, 0x5b, 0x70));
        assert_eq!(loaded.files.len(), 2);
    }

    #[test]
    fn light_mode_lands_light() {
        let root = std::env::temp_dir().join("hc-tui-omarchy-light");
        let dir = root.join("current/theme");
        write_theme(
            &dir,
            "mode = \"light\"\naccent = \"#1e66f5\"\nbackground = \"#eff1f5\"\n\
             foreground = \"#4c4f69\"\nmuted = \"#8c8fa1\"\nselection = \"#bcc0cc\"\n\
             lighter_background = \"#dce0e8\"\ndark_background = \"#e6e9ef\"\n",
        );
        let theme = load_dir(&dir).unwrap().theme;
        assert_eq!(theme.color("bg"), Color::Rgb(0xef, 0xf1, 0xf5));
        assert_eq!(theme.color("fg"), Color::Rgb(0x4c, 0x4f, 0x69));
        assert_eq!(theme.color("tab_active_fg"), Color::Rgb(0xef, 0xf1, 0xf5));
        assert_eq!(theme.color("cursor_fg"), Color::Rgb(0xef, 0xf1, 0xf5));
    }

    #[test]
    fn missing_or_broken_files_are_reported() {
        let root = std::env::temp_dir().join("hc-tui-omarchy-broken");
        assert!(matches!(
            load_dir(&root.join("nothing")),
            Err(Error::Absent(_))
        ));
        write_theme(&root.join("bad"), "background = ");
        assert!(matches!(load_dir(&root.join("bad")), Err(Error::Broken(_))));
        write_theme(&root.join("partial"), "background = \"#000000\"\n");
        assert!(matches!(
            load_dir(&root.join("partial")),
            Err(Error::Broken(_))
        ));
        // 目录在、colors.toml 缺：这是要警告的问题，不是「没装 Omarchy」。
        std::fs::create_dir_all(root.join("empty")).unwrap();
        assert!(matches!(
            load_dir(&root.join("empty")),
            Err(Error::Broken(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn symlink_switch_changes_the_watched_path() {
        let root = std::env::temp_dir().join("hc-tui-omarchy-switch");
        write_theme(&root.join("aether"), THEME);
        write_theme(&root.join("other"), &THEME.replace("#89b4fa", "#ff8800"));
        let current = root.join("current");
        let _ = std::fs::remove_file(&current);
        std::os::unix::fs::symlink(root.join("aether"), &current).unwrap();
        let before = load_dir(&current).unwrap();
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(root.join("other"), &current).unwrap();
        let after = load_dir(&current).unwrap();
        assert_eq!(before.theme.color("accent"), Color::Rgb(0x89, 0xb4, 0xfa));
        assert_eq!(after.theme.color("accent"), Color::Rgb(0xff, 0x88, 0x00));
        let _ = std::fs::remove_dir_all(&root);
    }
}
