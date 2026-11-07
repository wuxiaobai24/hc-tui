//! crossterm 事件到 nsh `Input` 的翻译，含修饰键与鼠标滚轮的步进语义。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use nsh_platform_tui::{Input, Modifiers, MouseButton, MouseInput, MouseKind};

/// 把按键翻译成 `Input::Key` / `Input::KeyRepeat`；`release` 按终端报告传递。
pub fn translate_key(key: &KeyEvent, release: bool) -> Option<Input> {
    let name = key_name(key.code)?;
    let modifiers = translate_modifiers(key.modifiers);
    if key.kind == crossterm::event::KeyEventKind::Repeat {
        Some(Input::KeyRepeat {
            key: name,
            modifiers,
        })
    } else {
        Some(Input::Key {
            key: name,
            modifiers,
            release,
        })
    }
}

/// 把鼠标事件翻译成 `Input::Mouse`；坐标为终端绝对列行。
pub fn translate_mouse(mouse: &MouseEvent) -> Option<Input> {
    let kind = match mouse.kind {
        MouseEventKind::Down(button) => MouseKind::Down(translate_button(button)),
        MouseEventKind::Up(button) => MouseKind::Up(translate_button(button)),
        MouseEventKind::Drag(_) | MouseEventKind::Moved => MouseKind::Move,
        MouseEventKind::ScrollUp => MouseKind::Wheel { x: 0, y: -3 },
        MouseEventKind::ScrollDown => MouseKind::Wheel { x: 0, y: 3 },
        MouseEventKind::ScrollLeft => MouseKind::Wheel { x: -3, y: 0 },
        MouseEventKind::ScrollRight => MouseKind::Wheel { x: 3, y: 0 },
    };
    let mut input = MouseInput::default();
    input.kind = kind;
    input.column = mouse.column;
    input.row = mouse.row;
    input.modifiers = translate_modifiers(mouse.modifiers);
    Some(Input::Mouse(input))
}

/// bracketed paste。
pub fn paste(text: String) -> Input {
    Input::Paste(text)
}

/// 终端焦点变化。
pub fn focus(focused: bool) -> Input {
    Input::Focus(focused)
}

fn translate_button(button: crossterm::event::MouseButton) -> MouseButton {
    match button {
        crossterm::event::MouseButton::Left => MouseButton::Left,
        crossterm::event::MouseButton::Right => MouseButton::Right,
        crossterm::event::MouseButton::Middle => MouseButton::Middle,
    }
}

fn translate_modifiers(modifiers: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: modifiers.contains(KeyModifiers::SHIFT),
        ctrl: modifiers.contains(KeyModifiers::CONTROL),
        alt: modifiers.contains(KeyModifiers::ALT),
        cmd: modifiers.contains(KeyModifiers::SUPER) || modifiers.contains(KeyModifiers::META),
    }
}

/// 按键名与 NSH keymap 对齐；不认识的键返回 `None`（宿主自己处理）。
pub fn key_name(code: KeyCode) -> Option<String> {
    let name: String = match code {
        KeyCode::Char(' ') => " ".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Up => "ArrowUp".to_string(),
        KeyCode::Down => "ArrowDown".to_string(),
        KeyCode::Left => "ArrowLeft".to_string(),
        KeyCode::Right => "ArrowRight".to_string(),
        KeyCode::F(n) => {
            return Some(format!("F{n}"));
        }
        _ => return None,
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    #[test]
    fn keys_map_to_nsh_names_and_modifiers() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let Some(Input::Key {
            key: name, release, ..
        }) = translate_key(&key, false)
        else {
            panic!("must translate");
        };
        assert_eq!(name, "Enter");
        assert!(!release);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let Some(Input::Key {
            key: name,
            modifiers,
            ..
        }) = translate_key(&ctrl_c, false)
        else {
            panic!("must translate");
        };
        assert_eq!(name, "c");
        assert!(modifiers.ctrl);
        assert!(!modifiers.cmd);

        let repeat = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        let key = KeyEvent {
            kind: KeyEventKind::Repeat,
            ..repeat
        };
        assert!(matches!(
            translate_key(&key, false),
            Some(Input::KeyRepeat { .. })
        ));

        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..ctrl_c
        };
        assert!(matches!(
            translate_key(&release, true),
            Some(Input::Key { release: true, .. })
        ));

        assert!(key_name(KeyCode::Null).is_none());
    }

    #[test]
    fn mouse_maps_kinds_and_wheel_steps() {
        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Right),
            column: 7,
            row: 9,
            modifiers: KeyModifiers::NONE,
        };
        let Some(Input::Mouse(input)) = translate_mouse(&down) else {
            panic!("must translate");
        };
        assert_eq!(input.kind, MouseKind::Down(MouseButton::Right));
        assert_eq!((input.column, input.row), (7, 9));

        let scroll = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let Some(Input::Mouse(input)) = translate_mouse(&scroll) else {
            panic!("must translate");
        };
        assert_eq!(input.kind, MouseKind::Wheel { x: 0, y: -3 });
    }
}
