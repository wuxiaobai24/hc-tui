//! 主题：一组具名颜色 token。
//!
//! 内置主题只覆盖基础 token，其余从 `default` 继承；`[theme.custom]` 在最后覆盖。
//! 渲染层只按 token 取色，不出现任何字面色值。

use std::collections::BTreeMap;

use ratatui::style::Color;

/// 全部 token 及默认主题的取值。顺序即 `default-config` 的输出顺序。
const DEFAULTS: &[(&str, Color)] = &[
    ("bg", Color::Rgb(18, 18, 18)),
    ("fg", Color::Rgb(224, 224, 224)),
    ("dim", Color::Rgb(120, 120, 120)),
    ("border", Color::Rgb(72, 72, 72)),
    ("border_focused", Color::Rgb(0, 200, 200)),
    ("accent", Color::Rgb(0, 200, 200)),
    ("tab_bar_bg", Color::Rgb(18, 18, 18)),
    ("tab_bar_fg", Color::Rgb(200, 200, 200)),
    ("tab_active_bg", Color::Rgb(0, 200, 200)),
    ("tab_active_fg", Color::Rgb(0, 0, 0)),
    ("status_bar_bg", Color::Rgb(24, 24, 24)),
    ("status_bar_fg", Color::Rgb(200, 200, 200)),
    ("sidebar_bg", Color::Rgb(18, 18, 18)),
    ("sidebar_title", Color::Rgb(0, 200, 200)),
    ("selection_bg", Color::Rgb(60, 90, 140)),
    ("selection_fg", Color::Rgb(240, 240, 240)),
    ("cursor_bg", Color::Rgb(200, 200, 200)),
    ("cursor_fg", Color::Rgb(0, 0, 0)),
    ("agent_blocked", Color::Rgb(240, 80, 80)),
    ("agent_error", Color::Rgb(240, 80, 80)),
    ("agent_working", Color::Rgb(230, 190, 60)),
    ("agent_done", Color::Rgb(90, 210, 120)),
    ("agent_idle", Color::Rgb(120, 120, 120)),
    ("agent_unknown", Color::Rgb(150, 150, 150)),
    ("badge_bg", Color::Rgb(200, 180, 60)),
    ("badge_fg", Color::Rgb(0, 0, 0)),
    ("toast_bg", Color::Rgb(40, 40, 40)),
    ("toast_fg", Color::Rgb(240, 240, 240)),
];

/// 一套主题。`overrides` 覆盖默认表。
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    tokens: BTreeMap<String, Color>,
}

impl Theme {
    pub fn color(&self, token: &str) -> Color {
        self.tokens.get(token).copied().unwrap_or(Color::Reset)
    }

    pub fn set(&mut self, token: &str, color: Color) {
        self.tokens.insert(token.to_string(), color);
    }

    /// 供 `--default-config` 与键位提示使用的 token 列表。
    pub fn tokens(&self) -> impl Iterator<Item = (&str, Color)> {
        self.tokens
            .iter()
            .map(|(name, color)| (name.as_str(), *color))
    }
}

fn base() -> Theme {
    Theme {
        name: "default".to_string(),
        tokens: DEFAULTS
            .iter()
            .map(|(name, color)| (name.to_string(), *color))
            .collect(),
    }
}

/// 主题的调色板：只列与默认不同的 token。
fn themed(name: &str, palette: &[(&str, Color)]) -> Theme {
    let mut theme = base();
    theme.name = name.to_string();
    for (token, color) in palette {
        theme.tokens.insert((*token).to_string(), *color);
    }
    theme
}

pub fn builtin(name: &str) -> Option<Theme> {
    let rgb = Color::Rgb;
    Some(match name {
        "default" | "terminal" => base(),
        "catppuccin" | "catppuccin-mocha" => themed(
            "catppuccin",
            &[
                ("bg", rgb(30, 30, 46)),
                ("fg", rgb(205, 214, 244)),
                ("dim", rgb(108, 112, 134)),
                ("border", rgb(69, 71, 90)),
                ("border_focused", rgb(137, 180, 250)),
                ("accent", rgb(137, 180, 250)),
                ("tab_bar_bg", rgb(24, 24, 37)),
                ("tab_bar_fg", rgb(166, 173, 200)),
                ("tab_active_bg", rgb(137, 180, 250)),
                ("tab_active_fg", rgb(30, 30, 46)),
                ("status_bar_bg", rgb(24, 24, 37)),
                ("status_bar_fg", rgb(186, 194, 222)),
                ("sidebar_bg", rgb(24, 24, 37)),
                ("sidebar_title", rgb(245, 194, 231)),
                ("selection_bg", rgb(88, 91, 112)),
                ("selection_fg", rgb(205, 214, 244)),
                ("agent_blocked", rgb(243, 139, 168)),
                ("agent_error", rgb(235, 160, 172)),
                ("agent_working", rgb(249, 226, 175)),
                ("agent_done", rgb(166, 227, 161)),
                ("agent_idle", rgb(127, 132, 156)),
                ("badge_bg", rgb(249, 226, 175)),
                ("badge_fg", rgb(30, 30, 46)),
                ("toast_bg", rgb(49, 50, 68)),
                ("toast_fg", rgb(205, 214, 244)),
            ],
        ),
        "catppuccin-latte" => themed(
            "catppuccin-latte",
            &[
                ("bg", rgb(239, 241, 245)),
                ("fg", rgb(76, 79, 105)),
                ("dim", rgb(140, 143, 161)),
                ("border", rgb(188, 192, 204)),
                ("border_focused", rgb(30, 102, 245)),
                ("accent", rgb(30, 102, 245)),
                ("tab_bar_bg", rgb(230, 233, 239)),
                ("tab_bar_fg", rgb(92, 95, 119)),
                ("tab_active_bg", rgb(30, 102, 245)),
                ("tab_active_fg", rgb(239, 241, 245)),
                ("status_bar_bg", rgb(230, 233, 239)),
                ("status_bar_fg", rgb(76, 79, 105)),
                ("sidebar_bg", rgb(230, 233, 239)),
                ("sidebar_title", rgb(136, 57, 239)),
                ("selection_bg", rgb(188, 192, 204)),
                ("selection_fg", rgb(76, 79, 105)),
                ("agent_blocked", rgb(210, 15, 57)),
                ("agent_error", rgb(210, 15, 57)),
                ("agent_working", rgb(223, 142, 29)),
                ("agent_done", rgb(64, 160, 43)),
                ("agent_idle", rgb(140, 143, 161)),
                ("badge_bg", rgb(223, 142, 29)),
                ("badge_fg", rgb(239, 241, 245)),
                ("toast_bg", rgb(220, 224, 232)),
                ("toast_fg", rgb(76, 79, 105)),
            ],
        ),
        "tokyo-night" => themed(
            "tokyo-night",
            &[
                ("bg", rgb(26, 27, 38)),
                ("fg", rgb(192, 202, 245)),
                ("dim", rgb(86, 95, 137)),
                ("border", rgb(41, 46, 66)),
                ("border_focused", rgb(122, 162, 247)),
                ("accent", rgb(122, 162, 247)),
                ("tab_bar_bg", rgb(22, 22, 30)),
                ("tab_bar_fg", rgb(169, 177, 214)),
                ("tab_active_bg", rgb(122, 162, 247)),
                ("tab_active_fg", rgb(26, 27, 38)),
                ("status_bar_bg", rgb(22, 22, 30)),
                ("status_bar_fg", rgb(169, 177, 214)),
                ("sidebar_bg", rgb(22, 22, 30)),
                ("sidebar_title", rgb(187, 154, 247)),
                ("selection_bg", rgb(51, 70, 124)),
                ("selection_fg", rgb(192, 202, 245)),
                ("agent_blocked", rgb(247, 118, 142)),
                ("agent_error", rgb(247, 118, 142)),
                ("agent_working", rgb(224, 175, 104)),
                ("agent_done", rgb(158, 206, 106)),
                ("agent_idle", rgb(86, 95, 137)),
                ("badge_bg", rgb(224, 175, 104)),
                ("badge_fg", rgb(26, 27, 38)),
                ("toast_bg", rgb(36, 40, 59)),
                ("toast_fg", rgb(192, 202, 245)),
            ],
        ),
        "dracula" => themed(
            "dracula",
            &[
                ("bg", rgb(40, 42, 54)),
                ("fg", rgb(248, 248, 242)),
                ("dim", rgb(98, 114, 164)),
                ("border", rgb(68, 71, 90)),
                ("border_focused", rgb(189, 147, 249)),
                ("accent", rgb(189, 147, 249)),
                ("tab_bar_bg", rgb(33, 34, 44)),
                ("tab_bar_fg", rgb(191, 191, 191)),
                ("tab_active_bg", rgb(189, 147, 249)),
                ("tab_active_fg", rgb(40, 42, 54)),
                ("status_bar_bg", rgb(33, 34, 44)),
                ("status_bar_fg", rgb(248, 248, 242)),
                ("sidebar_bg", rgb(33, 34, 44)),
                ("sidebar_title", rgb(255, 121, 198)),
                ("selection_bg", rgb(68, 71, 90)),
                ("selection_fg", rgb(248, 248, 242)),
                ("agent_blocked", rgb(255, 85, 85)),
                ("agent_error", rgb(255, 85, 85)),
                ("agent_working", rgb(241, 250, 140)),
                ("agent_done", rgb(80, 250, 123)),
                ("agent_idle", rgb(98, 114, 164)),
                ("badge_bg", rgb(241, 250, 140)),
                ("badge_fg", rgb(40, 42, 54)),
                ("toast_bg", rgb(68, 71, 90)),
                ("toast_fg", rgb(248, 248, 242)),
            ],
        ),
        "nord" => themed(
            "nord",
            &[
                ("bg", rgb(46, 52, 64)),
                ("fg", rgb(216, 222, 233)),
                ("dim", rgb(76, 86, 106)),
                ("border", rgb(67, 76, 94)),
                ("border_focused", rgb(136, 192, 208)),
                ("accent", rgb(136, 192, 208)),
                ("tab_bar_bg", rgb(41, 46, 57)),
                ("tab_bar_fg", rgb(216, 222, 233)),
                ("tab_active_bg", rgb(136, 192, 208)),
                ("tab_active_fg", rgb(46, 52, 64)),
                ("status_bar_bg", rgb(41, 46, 57)),
                ("status_bar_fg", rgb(216, 222, 233)),
                ("sidebar_bg", rgb(41, 46, 57)),
                ("sidebar_title", rgb(180, 142, 173)),
                ("selection_bg", rgb(67, 76, 94)),
                ("selection_fg", rgb(236, 239, 244)),
                ("agent_blocked", rgb(191, 97, 106)),
                ("agent_error", rgb(191, 97, 106)),
                ("agent_working", rgb(235, 203, 139)),
                ("agent_done", rgb(163, 190, 140)),
                ("agent_idle", rgb(76, 86, 106)),
                ("badge_bg", rgb(235, 203, 139)),
                ("badge_fg", rgb(46, 52, 64)),
                ("toast_bg", rgb(59, 66, 82)),
                ("toast_fg", rgb(236, 239, 244)),
            ],
        ),
        "gruvbox" => themed(
            "gruvbox",
            &[
                ("bg", rgb(40, 40, 40)),
                ("fg", rgb(235, 219, 178)),
                ("dim", rgb(146, 131, 116)),
                ("border", rgb(80, 73, 69)),
                ("border_focused", rgb(131, 165, 152)),
                ("accent", rgb(250, 189, 47)),
                ("tab_bar_bg", rgb(29, 32, 33)),
                ("tab_bar_fg", rgb(213, 196, 161)),
                ("tab_active_bg", rgb(250, 189, 47)),
                ("tab_active_fg", rgb(40, 40, 40)),
                ("status_bar_bg", rgb(29, 32, 33)),
                ("status_bar_fg", rgb(235, 219, 178)),
                ("sidebar_bg", rgb(29, 32, 33)),
                ("sidebar_title", rgb(211, 134, 155)),
                ("selection_bg", rgb(80, 73, 69)),
                ("selection_fg", rgb(251, 241, 199)),
                ("agent_blocked", rgb(251, 73, 52)),
                ("agent_error", rgb(251, 73, 52)),
                ("agent_working", rgb(250, 189, 47)),
                ("agent_done", rgb(184, 187, 38)),
                ("agent_idle", rgb(146, 131, 116)),
                ("badge_bg", rgb(250, 189, 47)),
                ("badge_fg", rgb(40, 40, 40)),
                ("toast_bg", rgb(60, 56, 54)),
                ("toast_fg", rgb(251, 241, 199)),
            ],
        ),
        "one-dark" => themed(
            "one-dark",
            &[
                ("bg", rgb(40, 44, 52)),
                ("fg", rgb(171, 178, 191)),
                ("dim", rgb(92, 99, 112)),
                ("border", rgb(62, 68, 81)),
                ("border_focused", rgb(97, 175, 239)),
                ("accent", rgb(97, 175, 239)),
                ("tab_bar_bg", rgb(33, 37, 43)),
                ("tab_bar_fg", rgb(171, 178, 191)),
                ("tab_active_bg", rgb(97, 175, 239)),
                ("tab_active_fg", rgb(33, 37, 43)),
                ("status_bar_bg", rgb(33, 37, 43)),
                ("status_bar_fg", rgb(171, 178, 191)),
                ("sidebar_bg", rgb(33, 37, 43)),
                ("sidebar_title", rgb(198, 120, 221)),
                ("selection_bg", rgb(62, 68, 81)),
                ("selection_fg", rgb(220, 223, 228)),
                ("agent_blocked", rgb(224, 108, 117)),
                ("agent_error", rgb(224, 108, 117)),
                ("agent_working", rgb(229, 192, 123)),
                ("agent_done", rgb(152, 195, 121)),
                ("agent_idle", rgb(92, 99, 112)),
                ("badge_bg", rgb(229, 192, 123)),
                ("badge_fg", rgb(40, 44, 52)),
                ("toast_bg", rgb(52, 58, 70)),
                ("toast_fg", rgb(220, 223, 228)),
            ],
        ),
        "solarized" | "solarized-dark" => themed(
            "solarized",
            &[
                ("bg", rgb(0, 43, 54)),
                ("fg", rgb(147, 161, 161)),
                ("dim", rgb(88, 110, 117)),
                ("border", rgb(7, 54, 66)),
                ("border_focused", rgb(38, 139, 210)),
                ("accent", rgb(38, 139, 210)),
                ("tab_bar_bg", rgb(7, 54, 66)),
                ("tab_bar_fg", rgb(147, 161, 161)),
                ("tab_active_bg", rgb(38, 139, 210)),
                ("tab_active_fg", rgb(253, 246, 227)),
                ("status_bar_bg", rgb(7, 54, 66)),
                ("status_bar_fg", rgb(147, 161, 161)),
                ("sidebar_bg", rgb(7, 54, 66)),
                ("sidebar_title", rgb(211, 54, 130)),
                ("selection_bg", rgb(7, 54, 66)),
                ("selection_fg", rgb(253, 246, 227)),
                ("agent_blocked", rgb(220, 50, 47)),
                ("agent_error", rgb(220, 50, 47)),
                ("agent_working", rgb(181, 137, 0)),
                ("agent_done", rgb(133, 153, 0)),
                ("agent_idle", rgb(88, 110, 117)),
                ("badge_bg", rgb(181, 137, 0)),
                ("badge_fg", rgb(0, 43, 54)),
                ("toast_bg", rgb(7, 54, 66)),
                ("toast_fg", rgb(253, 246, 227)),
            ],
        ),
        "kanagawa" => themed(
            "kanagawa",
            &[
                ("bg", rgb(31, 31, 40)),
                ("fg", rgb(220, 215, 186)),
                ("dim", rgb(114, 113, 105)),
                ("border", rgb(54, 54, 70)),
                ("border_focused", rgb(126, 156, 216)),
                ("accent", rgb(126, 156, 216)),
                ("tab_bar_bg", rgb(24, 24, 31)),
                ("tab_bar_fg", rgb(200, 195, 170)),
                ("tab_active_bg", rgb(126, 156, 216)),
                ("tab_active_fg", rgb(31, 31, 40)),
                ("status_bar_bg", rgb(24, 24, 31)),
                ("status_bar_fg", rgb(220, 215, 186)),
                ("sidebar_bg", rgb(24, 24, 31)),
                ("sidebar_title", rgb(210, 126, 153)),
                ("selection_bg", rgb(54, 54, 70)),
                ("selection_fg", rgb(220, 215, 186)),
                ("agent_blocked", rgb(232, 36, 36)),
                ("agent_error", rgb(232, 36, 36)),
                ("agent_working", rgb(192, 163, 110)),
                ("agent_done", rgb(152, 187, 108)),
                ("agent_idle", rgb(114, 113, 105)),
                ("badge_bg", rgb(192, 163, 110)),
                ("badge_fg", rgb(31, 31, 40)),
                ("toast_bg", rgb(43, 43, 54)),
                ("toast_fg", rgb(220, 215, 186)),
            ],
        ),
        "rose-pine" => themed(
            "rose-pine",
            &[
                ("bg", rgb(25, 23, 36)),
                ("fg", rgb(224, 222, 244)),
                ("dim", rgb(110, 106, 134)),
                ("border", rgb(64, 61, 82)),
                ("border_focused", rgb(196, 167, 231)),
                ("accent", rgb(235, 188, 186)),
                ("tab_bar_bg", rgb(38, 35, 58)),
                ("tab_bar_fg", rgb(224, 222, 244)),
                ("tab_active_bg", rgb(196, 167, 231)),
                ("tab_active_fg", rgb(25, 23, 36)),
                ("status_bar_bg", rgb(38, 35, 58)),
                ("status_bar_fg", rgb(224, 222, 244)),
                ("sidebar_bg", rgb(38, 35, 58)),
                ("sidebar_title", rgb(235, 188, 186)),
                ("selection_bg", rgb(64, 61, 82)),
                ("selection_fg", rgb(224, 222, 244)),
                ("agent_blocked", rgb(235, 111, 146)),
                ("agent_error", rgb(235, 111, 146)),
                ("agent_working", rgb(246, 193, 119)),
                ("agent_done", rgb(156, 207, 216)),
                ("agent_idle", rgb(110, 106, 134)),
                ("badge_bg", rgb(246, 193, 119)),
                ("badge_fg", rgb(25, 23, 36)),
                ("toast_bg", rgb(38, 35, 58)),
                ("toast_fg", rgb(224, 222, 244)),
            ],
        ),
        "vesper" => themed(
            "vesper",
            &[
                ("bg", rgb(16, 16, 16)),
                ("fg", rgb(214, 207, 195)),
                ("dim", rgb(122, 117, 110)),
                ("border", rgb(46, 46, 46)),
                ("border_focused", rgb(255, 199, 153)),
                ("accent", rgb(255, 199, 153)),
                ("tab_bar_bg", rgb(26, 26, 26)),
                ("tab_bar_fg", rgb(200, 194, 184)),
                ("tab_active_bg", rgb(255, 199, 153)),
                ("tab_active_fg", rgb(16, 16, 16)),
                ("status_bar_bg", rgb(26, 26, 26)),
                ("status_bar_fg", rgb(214, 207, 195)),
                ("sidebar_bg", rgb(26, 26, 26)),
                ("sidebar_title", rgb(255, 199, 153)),
                ("selection_bg", rgb(60, 58, 54)),
                ("selection_fg", rgb(240, 234, 224)),
                ("agent_blocked", rgb(255, 128, 128)),
                ("agent_error", rgb(255, 128, 128)),
                ("agent_working", rgb(255, 199, 153)),
                ("agent_done", rgb(153, 230, 153)),
                ("agent_idle", rgb(122, 117, 110)),
                ("badge_bg", rgb(255, 199, 153)),
                ("badge_fg", rgb(16, 16, 16)),
                ("toast_bg", rgb(36, 36, 36)),
                ("toast_fg", rgb(240, 234, 224)),
            ],
        ),
        _ => return None,
    })
}

/// 解析颜色：`#rrggbb`、`#rgb`、`rgb(r,g,b)`、具名色、`reset`。
pub fn parse_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let mut parts = hex
                    .chars()
                    .map(|character| character.to_digit(16).map(|value| (value * 17) as u8));
                let r = parts.next()??;
                let g = parts.next()??;
                let b = parts.next()??;
                Some(Color::Rgb(r, g, b))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::Rgb(r, g, b))
            }
            _ => None,
        };
    }
    if let Some(inner) = text
        .strip_prefix("rgb(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let mut parts = inner.split(',').map(|part| part.trim().parse::<u8>().ok());
        let r = parts.next()??;
        let g = parts.next()??;
        let b = parts.next()??;
        return Some(Color::Rgb(r, g, b));
    }
    Some(match text.to_ascii_lowercase().as_str() {
        "reset" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "white" => Color::White,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_name_resolves() {
        for name in [
            "default",
            "catppuccin",
            "catppuccin-latte",
            "tokyo-night",
            "dracula",
            "nord",
            "gruvbox",
            "one-dark",
            "solarized",
            "kanagawa",
            "rose-pine",
            "vesper",
        ] {
            assert!(builtin(name).is_some(), "{name}");
        }
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("#89b4fa"), Some(Color::Rgb(0x89, 0xb4, 0xfa)));
        assert_eq!(parse_color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Color::Rgb(1, 2, 3)));
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("reset"), Some(Color::Reset));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("not-a-color"), None);
    }

    #[test]
    fn custom_tokens_override_the_template() {
        let mut theme = builtin("nord").unwrap();
        let accent = theme.color("accent");
        theme.set("accent", Color::Rgb(1, 2, 3));
        assert_ne!(theme.color("accent"), accent);
        assert_eq!(theme.color("accent"), Color::Rgb(1, 2, 3));
    }
}
