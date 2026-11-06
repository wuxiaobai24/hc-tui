//! Optional Zellij-style keyboard modes.
use super::interaction::KeyboardMode;
use super::*;

impl App {
    pub(super) fn on_zellij_key(&mut self, key: KeyEvent) -> bool {
        if !self.config.zellij_keys {
            self.keyboard_mode = KeyboardMode::Normal;
            return false;
        }
        let mode = self.keyboard_mode;
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('g') {
            self.keyboard_mode = if mode == KeyboardMode::Locked {
                KeyboardMode::Normal
            } else {
                KeyboardMode::Locked
            };
            self.copy = None;
            self.prefix = false;
            self.dirty = true;
            return true;
        }
        if mode == KeyboardMode::Locked {
            self.forward_key(key);
            return true;
        }
        if key.modifiers == KeyModifiers::CONTROL {
            let next = match key.code {
                KeyCode::Char('p') => Some(KeyboardMode::Pane),
                KeyCode::Char('t') => Some(KeyboardMode::Tab),
                KeyCode::Char('n') => Some(KeyboardMode::Resize),
                KeyCode::Char('h') => Some(KeyboardMode::Move),
                KeyCode::Char('o') => Some(KeyboardMode::Session),
                KeyCode::Char('s') => {
                    self.enter_copy_mode();
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.prefix = false;
                    return true;
                }
                _ => None,
            };
            if let Some(next) = next {
                self.keyboard_mode = if mode == next {
                    KeyboardMode::Normal
                } else {
                    next
                };
                self.copy = None;
                self.prefix = false;
                self.dirty = true;
                return true;
            }
        }
        if mode == KeyboardMode::Normal {
            return false;
        }
        self.dirty = true;
        if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
            self.keyboard_mode = KeyboardMode::Normal;
            return true;
        }
        let direction = match key.code {
            KeyCode::Char('h') | KeyCode::Left => Some(Direction::Left),
            KeyCode::Char('j') | KeyCode::Down => Some(Direction::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Direction::Up),
            KeyCode::Char('l') | KeyCode::Right => Some(Direction::Right),
            _ => None,
        };
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return true;
        }
        match mode {
            KeyboardMode::Pane => {
                if let Some(direction) = direction {
                    self.move_focus(direction);
                    return true;
                }
                let action = match key.code {
                    KeyCode::Char('n' | 'r') => Some("split_vertical"),
                    KeyCode::Char('d') => Some("split_horizontal"),
                    KeyCode::Char('x') => Some("close_pane"),
                    KeyCode::Char('f') => Some("zoom"),
                    KeyCode::Char('e') => Some("float_pane"),
                    KeyCode::Char('w') => Some("float_hide"),
                    KeyCode::Char('c') => Some("rename_pane"),
                    KeyCode::Char('p') => Some("last_pane"),
                    _ => None,
                };
                if let Some(action) = action {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.run_named_binding(action);
                }
            }
            KeyboardMode::Tab => match key.code {
                KeyCode::Char('n') => {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.run_named_binding("new_tab");
                }
                KeyCode::Char('x') => {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.run_named_binding("close_tab");
                }
                KeyCode::Char('r') => {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.run_named_binding("rename_tab");
                }
                KeyCode::Char('b') => {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.move_pane_to_new_tab();
                }
                KeyCode::Char(digit @ '1'..='9') => {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.select_tab(digit as usize - '1' as usize);
                }
                _ => {
                    if let Some(direction) = direction {
                        self.cycle_tab(if matches!(direction, Direction::Left | Direction::Up) {
                            -1
                        } else {
                            1
                        });
                    }
                }
            },
            KeyboardMode::Resize => {
                self.on_resize_key(key);
            }
            KeyboardMode::Move => {
                if let Some(direction) = direction {
                    if let Err(error) = self.swap_direction(direction) {
                        self.note = error.to_string();
                    }
                }
            }
            KeyboardMode::Session => {
                let action = match key.code {
                    KeyCode::Char('d') => Some("detach"),
                    KeyCode::Char('w') => Some("workspace_picker"),
                    KeyCode::Char('c') => Some("settings"),
                    _ => None,
                };
                if let Some(action) = action {
                    self.keyboard_mode = KeyboardMode::Normal;
                    self.run_named_binding(action);
                }
            }
            _ => {}
        }
        true
    }

    pub(super) fn zellij_mode_line(&self) -> Option<(String, String)> {
        if !self.config.zellij_keys {
            return None;
        }
        let (label, hint) = match self.keyboard_mode {
            KeyboardMode::Normal => ("NORMAL", "Ctrl+p pane  Ctrl+t tab  Ctrl+n resize  Ctrl+h move  Ctrl+s scroll  Ctrl+o session  Ctrl+g lock"),
            KeyboardMode::Locked => ("LOCKED", "Ctrl+g unlock; keys go to the terminal"),
            KeyboardMode::Pane => ("PANE", "h/j/k/l focus  n/r right  d down  x close  f zoom  c rename  e float  esc done"),
            KeyboardMode::Tab => ("TAB", "h/l previous/next  n new  x close  r rename  b break pane  1-9 select  esc done"),
            KeyboardMode::Resize => ("RESIZE", "h/l width  j/k height  esc done"),
            KeyboardMode::Move => ("MOVE", "h/j/k/l swap pane  esc done"),
            KeyboardMode::Session => ("SESSION", "d detach  w workspaces  c settings  esc done"),
        };
        let hint = if self.note.is_empty() {
            hint.to_string()
        } else {
            format!("{} · {hint}", self.note)
        };
        Some((label.into(), hint))
    }
}
