//! Ghostty 主题来源：解析 `~/.config/ghostty/config` 里的颜色、`theme = <Name>` 与
//! `config-file` include（只递归一层），拼成来源无关的 [`Surface`]。
//!
//! Ghostty 的实际配色往往来自 Omarchy 生成的 include（`?` 前缀表示可缺省），所以被包含的
//! 文件也进热重载的监视列表。路径可用 `HC_GHOSTTY_CONFIG` 与 `HC_GHOSTTY_THEME_DIR`
//! 覆盖，便于手工验证。

use std::path::{Path, PathBuf};

use ratatui_core::style::Color;

use super::source::{Error, Loaded};
use super::Surface;

/// 配置里表示「跟随 Ghostty」的名字。
pub const NAME: &str = "ghostty";

const CONFIG_ENV: &str = "HC_GHOSTTY_CONFIG";
const THEME_DIR_ENV: &str = "HC_GHOSTTY_THEME_DIR";

/// 原生主题目录：用户目录优先于系统目录。
fn theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(extra) = std::env::var_os(THEME_DIR_ENV).filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(extra));
    }
    dirs.push(config_dir().join("themes"));
    dirs.push(PathBuf::from("/usr/share/ghostty/themes"));
    dirs
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| home().join(".config"))
        .join("ghostty")
}

fn config_path() -> PathBuf {
    std::env::var_os(CONFIG_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| config_dir().join("config"))
}

/// 读当前 Ghostty 配置。
pub fn load() -> Result<Loaded, Error> {
    load_from(&config_path(), &theme_dirs())
}

/// 读指定配置文件；`theme_dirs` 是 `theme = <Name>` 的查找目录，按优先级排列。
pub fn load_from(config: &Path, theme_dirs: &[PathBuf]) -> Result<Loaded, Error> {
    let mut colors = Colors::default();
    let mut files = Vec::new();
    parse(config, &mut colors, &mut files, 0, theme_dirs)?;
    build(colors, files)
}

/// 一层配置里读到的颜色。
#[derive(Debug, Clone, Default)]
struct Colors {
    background: Option<Color>,
    foreground: Option<Color>,
    selection_bg: Option<Color>,
    selection_fg: Option<Color>,
    palette: [Option<Color>; 16],
    label: Option<String>,
    /// 最后一个读到的 include；Ghostty 常 include Omarchy 生成的 `ghostty.conf`。
    include: Option<PathBuf>,
}

impl Colors {
    fn overlay(&mut self, other: &Self) {
        if other.background.is_some() {
            self.background = other.background;
        }
        if other.foreground.is_some() {
            self.foreground = other.foreground;
        }
        if other.selection_bg.is_some() {
            self.selection_bg = other.selection_bg;
        }
        if other.selection_fg.is_some() {
            self.selection_fg = other.selection_fg;
        }
        for (slot, value) in self.palette.iter_mut().zip(other.palette) {
            if value.is_some() {
                *slot = value;
            }
        }
    }

    fn field(&mut self, key: &str, value: &str) {
        let slot = match key {
            "background" => &mut self.background,
            "foreground" => &mut self.foreground,
            "selection-background" => &mut self.selection_bg,
            "selection-foreground" => &mut self.selection_fg,
            _ => return,
        };
        if let Some(color) = super::parse_color(value) {
            *slot = Some(color);
        }
    }

    fn palette_value(&mut self, value: &str) {
        let Some((index, color)) = value.split_once('=') else {
            return;
        };
        let Ok(index) = index.trim().parse::<usize>() else {
            return;
        };
        if index < self.palette.len() {
            if let Some(color) = super::parse_color(color) {
                self.palette[index] = Some(color);
            }
        }
    }
}

fn parse(
    path: &Path,
    colors: &mut Colors,
    files: &mut Vec<PathBuf>,
    depth: usize,
    theme_dirs: &[PathBuf],
) -> Result<(), Error> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::Absent(format!("{} 不存在", path.display()))
        } else {
            Error::Broken(format!("{}: {error}", path.display()))
        }
    })?;
    if !files.iter().any(|seen| seen == path) {
        files.push(path.to_path_buf());
    }
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = strip_comment(value.trim());
        match key.trim() {
            // Ghostty 的 include：只跟一层；`?` 表示可缺省，缺失时忽略。
            "config-file" if depth == 0 => {
                let spec = value.strip_prefix('?').unwrap_or(value);
                let include = resolve(path.parent().unwrap_or(Path::new(".")), unquote(spec));
                if include.is_file() {
                    parse(&include, colors, files, depth + 1, theme_dirs)?;
                    if colors.label.is_none() {
                        colors.include = Some(include);
                    }
                }
            }
            "theme" => {
                let name = unquote(value);
                let theme_file = theme_dirs
                    .iter()
                    .map(|dir| dir.join(name))
                    .find(|path| path.is_file())
                    .ok_or_else(|| Error::Broken(format!("ghostty 主题 {name} 不存在")))?;
                let mut theme = Colors::default();
                parse(&theme_file, &mut theme, files, depth + 1, theme_dirs)?;
                colors.overlay(&theme);
                colors.label = Some(name.to_string());
            }
            "palette" => colors.palette_value(value),
            key => colors.field(key, value),
        }
    }
    Ok(())
}

/// 去掉行尾注释：`#` 前面有空白才算注释，颜色值本身以 `#` 开头不受影响。
fn strip_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'#' && index > 0 && bytes[index - 1].is_ascii_whitespace() {
            return value[..index].trim_end();
        }
    }
    value
}

fn unquote(value: &str) -> &str {
    value
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
}

/// `config-file` 的值：`~/` 展开到家目录，相对路径按所在配置文件目录解析。
fn resolve(base: &Path, value: &str) -> PathBuf {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("~/") {
        return home().join(rest);
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn build(colors: Colors, files: Vec<PathBuf>) -> Result<Loaded, Error> {
    let background = colors
        .background
        .ok_or_else(|| Error::Broken("ghostty: 没有 background".to_string()))?;
    let foreground = colors
        .foreground
        .ok_or_else(|| Error::Broken("ghostty: 没有 foreground".to_string()))?;
    let mut ansi = super::builtin("default")
        .expect("default theme exists")
        .terminal_palette();
    for (index, value) in colors.palette.iter().enumerate() {
        if let Some(color) = value {
            ansi[index] = *color;
        }
    }
    let surface = Surface {
        bg: background,
        fg: foreground,
        dim: ansi[8],
        border: ansi[8],
        panel: ansi[0],
        raised: ansi[8],
        accent: ansi[4],
        selection_bg: colors.selection_bg.unwrap_or(ansi[8]),
        selection_fg: colors.selection_fg.unwrap_or(foreground),
        ansi,
    };
    let label = colors
        .label
        .or_else(|| {
            colors
                .include
                .as_deref()
                .and_then(Path::parent)
                .and_then(super::omarchy::theme_name)
        })
        .unwrap_or_else(|| "config".to_string());
    Ok(Loaded {
        theme: surface.theme(NAME),
        label,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hc-tui-ghostty-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const PALETTE: &str = "palette = 0=#1e1e2e\npalette = 1=#f38ba8\npalette = 4=#89b4fa\n\
                           palette = 7=#cdd6f4\npalette = 8=#585b70\npalette = 15=#cdd6f4\n";

    #[test]
    fn inline_colors_and_palette() {
        let dir = temp("inline");
        let config = dir.join("config");
        std::fs::write(
            &config,
            format!("background = #1e1e2e\nforeground = #cdd6f4\n{PALETTE}"),
        )
        .unwrap();
        let loaded = load_from(&config, &[]).unwrap();
        assert_eq!(loaded.theme.color("bg"), Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(loaded.theme.color("fg"), Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(loaded.theme.color("accent"), Color::Rgb(0x89, 0xb4, 0xfa));
        assert_eq!(
            loaded.theme.terminal_palette()[1],
            Color::Rgb(0xf3, 0x8b, 0xa8)
        );
        assert_eq!(loaded.label, "config");
        assert_eq!(loaded.files, vec![config]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_is_followed_and_watched() {
        let dir = temp("include");
        let include = dir.join("theme/ghostty.conf");
        std::fs::create_dir_all(include.parent().unwrap()).unwrap();
        std::fs::write(
            &include,
            format!("background = #101010\nforeground = #eeeeee\n{PALETTE}"),
        )
        .unwrap();
        let config = dir.join("config");
        std::fs::write(
            &config,
            "config-file = ?\"theme/ghostty.conf\"\nfont-size = 9\n",
        )
        .unwrap();
        let loaded = load_from(&config, &[]).unwrap();
        assert_eq!(loaded.theme.color("bg"), Color::Rgb(0x10, 0x10, 0x10));
        assert_eq!(loaded.files, vec![config.clone(), include]);
        // 缺省的 include 不报错。
        std::fs::write(
            &config,
            "config-file = ?\"nope.conf\"\nbackground = #202020\nforeground = #dddddd\n",
        )
        .unwrap();
        let loaded = load_from(&config, &[]).unwrap();
        assert_eq!(loaded.theme.color("bg"), Color::Rgb(0x20, 0x20, 0x20));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn named_theme_wins_at_its_position() {
        let dir = temp("named");
        let themes = dir.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(
            themes.join("Catppuccin Mocha"),
            format!("background = #1e1e2e\nforeground = #cdd6f4\n{PALETTE}"),
        )
        .unwrap();
        let config = dir.join("config");
        std::fs::write(
            &config,
            "theme = \"Catppuccin Mocha\"\nbackground = #000000\n",
        )
        .unwrap();
        let loaded = load_from(&config, std::slice::from_ref(&themes)).unwrap();
        // theme 文件在前面，后面的 inline 覆盖它。
        assert_eq!(loaded.theme.color("bg"), Color::Rgb(0x00, 0x00, 0x00));
        assert_eq!(loaded.theme.color("fg"), Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(loaded.label, "Catppuccin Mocha");
        assert_eq!(loaded.files.len(), 2);
        // 不存在的主题名是解析失败，调用方回退下一个来源。
        std::fs::write(&config, "theme = missing\n").unwrap();
        assert!(matches!(
            load_from(&config, &[themes]),
            Err(Error::Broken(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_config_is_absent() {
        let dir = temp("absent");
        assert!(matches!(
            load_from(&dir.join("config"), &[]),
            Err(Error::Absent(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn comments_and_quotes_are_handled() {
        assert_eq!(strip_comment("#1e1e2e"), "#1e1e2e");
        assert_eq!(strip_comment("#1e1e2e # note"), "#1e1e2e");
        assert_eq!(unquote("\"#1e1e2e\""), "#1e1e2e");
        assert_eq!(unquote("'#1e1e2e'"), "#1e1e2e");
    }
}
