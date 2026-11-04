//! 宿主键盘/鼠标事件到 ghostty 编码器的映射。
//!
//! 这里只做「crossterm 的东西 → libghostty 的东西」，真正的字节序列由
//! [`ghostty_vt::KeyEncoder`] 按子程序当前的键盘模式生成（kitty 键位、
//! modifyOtherKeys 等）。键位表依据 `bindings.rs` 里的 `GhosttyKey` 常量。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ghostty_vt::ffi;

pub const MOD_SHIFT: u16 = 1 << 0;
pub const MOD_CTRL: u16 = 1 << 1;
pub const MOD_ALT: u16 = 1 << 2;
pub const MOD_SUPER: u16 = 1 << 3;

/// 一次按键在 ghostty 眼里的样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    pub key: ffi::GhosttyKey,
    pub mods: u16,
    /// 该键产生的文本，永远是「未做 Ctrl/Meta 变换」的字符。
    pub text: String,
    /// 已经体现在 `text` 里的修饰键，编码器不再额外加序列。
    pub consumed: u16,
}

/// 映射不了就返回 `None`，由调用方忽略这次按键。
pub fn map_key(key: KeyEvent) -> Option<KeyInput> {
    let mut mods = to_mods(key.modifiers);
    let mut consumed = 0;
    let (code, text) = match key.code {
        KeyCode::Char(character) => {
            if key.modifiers.contains(KeyModifiers::SHIFT) && character.is_ascii_uppercase() {
                consumed = MOD_SHIFT;
            }
            (key_for_char(character), character.to_string())
        }
        KeyCode::Backspace => named(ffi::GhosttyKey_GHOSTTY_KEY_BACKSPACE),
        KeyCode::Enter => named(ffi::GhosttyKey_GHOSTTY_KEY_ENTER),
        KeyCode::Left => named(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT),
        KeyCode::Right => named(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_RIGHT),
        KeyCode::Up => named(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_UP),
        KeyCode::Down => named(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_DOWN),
        KeyCode::Home => named(ffi::GhosttyKey_GHOSTTY_KEY_HOME),
        KeyCode::End => named(ffi::GhosttyKey_GHOSTTY_KEY_END),
        KeyCode::PageUp => named(ffi::GhosttyKey_GHOSTTY_KEY_PAGE_UP),
        KeyCode::PageDown => named(ffi::GhosttyKey_GHOSTTY_KEY_PAGE_DOWN),
        KeyCode::Tab => named(ffi::GhosttyKey_GHOSTTY_KEY_TAB),
        KeyCode::BackTab => {
            mods |= MOD_SHIFT;
            named(ffi::GhosttyKey_GHOSTTY_KEY_TAB)
        }
        KeyCode::Delete => named(ffi::GhosttyKey_GHOSTTY_KEY_DELETE),
        KeyCode::Insert => named(ffi::GhosttyKey_GHOSTTY_KEY_INSERT),
        KeyCode::Esc => named(ffi::GhosttyKey_GHOSTTY_KEY_ESCAPE),
        KeyCode::F(number) => (function_key(number), String::new()),
        _ => return None,
    };
    Some(KeyInput {
        key: code,
        mods,
        text,
        consumed,
    })
}

/// 只有键码、没有文本的键。
fn named(key: ffi::GhosttyKey) -> (ffi::GhosttyKey, String) {
    (key, String::new())
}

fn function_key(number: u8) -> ffi::GhosttyKey {
    match number {
        1..=12 => ffi::GhosttyKey_GHOSTTY_KEY_F1 + ffi::GhosttyKey::from(number - 1),
        _ => ffi::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED,
    }
}

/// 字符 → 逻辑键码。非 ASCII 字符按 UNIDENTIFIED 处理，靠 `text` 定正文。
fn key_for_char(character: char) -> ffi::GhosttyKey {
    match character {
        'a'..='z' => ffi::GhosttyKey_GHOSTTY_KEY_A + (character as u8 - b'a') as ffi::GhosttyKey,
        'A'..='Z' => ffi::GhosttyKey_GHOSTTY_KEY_A + (character as u8 - b'A') as ffi::GhosttyKey,
        '0'..='9' => {
            ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_0 + (character as u8 - b'0') as ffi::GhosttyKey
        }
        ' ' => ffi::GhosttyKey_GHOSTTY_KEY_SPACE,
        '`' => ffi::GhosttyKey_GHOSTTY_KEY_BACKQUOTE,
        '\\' => ffi::GhosttyKey_GHOSTTY_KEY_BACKSLASH,
        '[' => ffi::GhosttyKey_GHOSTTY_KEY_BRACKET_LEFT,
        ']' => ffi::GhosttyKey_GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => ffi::GhosttyKey_GHOSTTY_KEY_COMMA,
        '=' => ffi::GhosttyKey_GHOSTTY_KEY_EQUAL,
        '-' => ffi::GhosttyKey_GHOSTTY_KEY_MINUS,
        '.' => ffi::GhosttyKey_GHOSTTY_KEY_PERIOD,
        '\'' => ffi::GhosttyKey_GHOSTTY_KEY_QUOTE,
        ';' => ffi::GhosttyKey_GHOSTTY_KEY_SEMICOLON,
        '/' => ffi::GhosttyKey_GHOSTTY_KEY_SLASH,
        _ => ffi::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED,
    }
}

/// crossterm 的修饰键 → `GhosttyMods` 位掩码。
pub fn to_mods(modifiers: KeyModifiers) -> u16 {
    let mut out = 0;
    if modifiers.contains(KeyModifiers::SHIFT) {
        out |= MOD_SHIFT;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        out |= MOD_CTRL;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        out |= MOD_ALT;
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        out |= MOD_SUPER;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyInput {
        map_key(KeyEvent::new(code, modifiers)).unwrap()
    }

    #[test]
    fn letters_and_digits_map_to_logical_keys() {
        let input = press(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(input.key, ffi::GhosttyKey_GHOSTTY_KEY_Q);
        assert_eq!(input.text, "q");
        assert_eq!(input.mods, 0);

        let input = press(KeyCode::Char('7'), KeyModifiers::NONE);
        assert_eq!(input.key, ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_7);
    }

    #[test]
    fn shifted_letter_marks_shift_as_consumed() {
        let input = press(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(input.key, ffi::GhosttyKey_GHOSTTY_KEY_A);
        assert_eq!(input.consumed, MOD_SHIFT);
        assert_eq!(input.text, "A");
    }

    #[test]
    fn named_keys_cover_the_common_set() {
        assert_eq!(
            press(KeyCode::Enter, KeyModifiers::NONE).key,
            ffi::GhosttyKey_GHOSTTY_KEY_ENTER
        );
        assert_eq!(
            press(KeyCode::Esc, KeyModifiers::NONE).key,
            ffi::GhosttyKey_GHOSTTY_KEY_ESCAPE
        );
        assert_eq!(
            press(KeyCode::F(5), KeyModifiers::NONE).key,
            ffi::GhosttyKey_GHOSTTY_KEY_F1 + 4
        );
        assert_eq!(press(KeyCode::Up, KeyModifiers::CONTROL).mods, MOD_CTRL);
        assert_eq!(press(KeyCode::BackTab, KeyModifiers::NONE).mods, MOD_SHIFT);
    }
}
