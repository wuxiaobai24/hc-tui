//! 宿主键盘/鼠标事件到 ghostty 编码器的映射。
//!
//! 这里只做「crossterm 的东西 → libghostty 的东西」，真正的字节序列由
//! [`ghostty_vt::KeyEncoder`] 按子程序当前的键盘模式生成（kitty 键位、
//! modifyOtherKeys 等）。键位表依据 `bindings.rs` 里的 `GhosttyKey` 常量。

use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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

// ---- Kitty 键盘协议（渐进增强） ----
//
// 探测与启用由本模块负责，解码交给 `parse_kitty_key`；`Drop` 里一定会把压入的
// 标志弹回去，否则用户的 shell 会留在 disambiguate 状态。

/// 查询宿主终端当前的键盘增强标志：`CSI ? u`。
pub const KITTY_QUERY: &[u8] = b"\x1b[?u";
/// 压入 disambiguate 标志：`CSI > 1 u`。
pub const KITTY_ENABLE: &[u8] = b"\x1b[>1u";
/// 弹出一层标志：`CSI < u`。
pub const KITTY_RESTORE: &[u8] = b"\x1b[<u";
/// 探测超时；超过这个时长没有回复就按不支持处理。
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// 是否已经往宿主终端压过标志。恢复是幂等的：panic 钩子和 `Drop` 都调同一个入口。
static KEYBOARD_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn open_tty() -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()
}

/// 诊断输出：`HC_TUI_KEYS_TRACE` 指向文件时逐行追加原始字节。
fn trace(line: &str) {
    let Some(path) = std::env::var_os("HC_TUI_KEYS_TRACE") else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

fn trace_bytes(label: &str, bytes: &[u8]) {
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    trace(&format!("{label} {} 字节 {hex}", bytes.len()));
}

/// `CSI ? <flags> u` 回复里的标志位；没有完整回复返回 `None`。
pub fn parse_flags_reply(bytes: &[u8]) -> Option<u8> {
    let start = bytes.windows(3).position(|window| window == b"\x1b[?")? + 3;
    let rest = &bytes[start..];
    let end = rest.iter().position(|byte| *byte == b'u')?;
    let digits = std::str::from_utf8(&rest[..end]).ok()?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u16>().ok().map(|value| value as u8)
}

/// kitty 的修饰位掩码（从 1 起算）→ crossterm 修饰键。
pub fn kitty_modifiers(mask: u8) -> KeyModifiers {
    let bits = mask.saturating_sub(1);
    let mut out = KeyModifiers::NONE;
    if bits & 1 != 0 {
        out |= KeyModifiers::SHIFT;
    }
    if bits & 2 != 0 {
        out |= KeyModifiers::ALT;
    }
    if bits & 4 != 0 {
        out |= KeyModifiers::CONTROL;
    }
    if bits & 8 != 0 {
        out |= KeyModifiers::SUPER;
    }
    if bits & 16 != 0 {
        out |= KeyModifiers::HYPER;
    }
    if bits & 32 != 0 {
        out |= KeyModifiers::META;
    }
    out
}

fn kitty_event_kind(code: u8) -> KeyEventKind {
    match code {
        2 => KeyEventKind::Repeat,
        3 => KeyEventKind::Release,
        _ => KeyEventKind::Press,
    }
}

/// kitty 的键码 → crossterm 键码。
///
/// 功能键沿用 kitty 表里的码位；Esc / Enter / Tab / Backspace 用 `27` / `13` / `9` / `127`。
/// 字母送的是未加 Shift 的基准键，按 SHIFT 抬成大写，好让 `KeyInput.text` 与 legacy 一致。
fn kitty_keycode(code: u32, modifiers: KeyModifiers) -> Option<KeyCode> {
    Some(match code {
        27 => KeyCode::Esc,
        13 => KeyCode::Enter,
        9 if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        9 => KeyCode::Tab,
        127 => KeyCode::Backspace,
        57348 => KeyCode::Insert,
        57349 => KeyCode::Delete,
        57350 => KeyCode::Left,
        57351 => KeyCode::Right,
        57352 => KeyCode::Up,
        57353 => KeyCode::Down,
        57354 => KeyCode::PageUp,
        57355 => KeyCode::PageDown,
        57356 => KeyCode::Home,
        57357 => KeyCode::End,
        57364..=57398 => KeyCode::F((code - 57364 + 1) as u8),
        _ => {
            let character = char::from_u32(code)?;
            if character.is_control() {
                return None;
            }
            let character = if modifiers.contains(KeyModifiers::SHIFT) {
                character.to_ascii_uppercase()
            } else {
                character
            };
            KeyCode::Char(character)
        }
    })
}

/// 字节流末尾还有一个没收全的 `CSI` 序列时返回 true，调用方应该继续攒字节。
pub fn kitty_incomplete(bytes: &[u8]) -> bool {
    let Some(start) = bytes.iter().position(|byte| *byte == 0x1b) else {
        return false;
    };
    let body = &bytes[start + 1..];
    if body.is_empty() {
        return true;
    }
    if body[0] != b'[' {
        return false;
    }
    !body[1..].iter().any(|byte| (0x40..=0x7e).contains(byte))
}

/// legacy 单字节键 → `KeyEvent`。
///
/// kitty 协议下没有修饰的 Tab / Enter / Esc / Backspace 仍然走 legacy 编码，
/// 诊断命令要能把它们和 `CSI … u` 摆在一起比。
pub fn parse_legacy_key(byte: u8) -> Option<KeyEvent> {
    let key = match byte {
        0x09 => KeyCode::Tab,
        0x0d => KeyCode::Enter,
        0x1b => KeyCode::Esc,
        0x7f => KeyCode::Backspace,
        b' '..=b'~' => KeyCode::Char(byte as char),
        _ => return None,
    };
    Some(KeyEvent::new(key, KeyModifiers::NONE))
}

/// 从字节流里解一格键：`CSI <code>[:<alternate>] ; <mods>[:<event>] u`。
///
/// 返回解出的按键与消费的字节数；不是完整序列时返回 `None`，调用方继续攒字节。
pub fn parse_kitty_key(bytes: &[u8]) -> Option<(KeyEvent, usize)> {
    let start = bytes.iter().position(|byte| *byte == 0x1b)?;
    if bytes.get(start + 1) != Some(&b'[') {
        // 单独的 ESC：legacy 路径同样当成 Esc。
        return Some((KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), start + 1));
    }
    let body = &bytes[start + 2..];
    // CSI 的终止字节是 0x40..=0x7e，参数里不会出现。
    let end = body.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
    let consumed = start + 2 + end + 1;
    if body[end] != b'u' {
        return None;
    }
    let text = std::str::from_utf8(&body[..end]).ok()?;
    let mut fields = text.split(';');
    let code = fields.next()?.split(':').next()?.parse::<u32>().ok()?;
    let (modifiers, kind) = match fields.next() {
        Some(field) => {
            let mut parts = field.split(':');
            let mask = parts.next()?.parse::<u8>().ok()?;
            let kind = parts
                .next()
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(1);
            (kitty_modifiers(mask), kitty_event_kind(kind))
        }
        None => (KeyModifiers::NONE, KeyEventKind::Press),
    };
    let code = kitty_keycode(code, modifiers)?;
    Some((KeyEvent::new_with_kind(code, modifiers, kind), consumed))
}

/// 宿主键盘协议的守卫：`Drop` 里发 `CSI < u`。没启用过的终端上什么都不发。
pub struct KeyboardGuard {
    tty: Option<std::fs::File>,
    reply: Vec<u8>,
}

impl KeyboardGuard {
    /// 按策略探测并启用。`Legacy` 完全不碰终端；`Kitty` 跳过探测直接启用。
    pub fn open(policy: crate::config::KeyboardProtocol) -> Self {
        use crate::config::KeyboardProtocol;
        if policy == KeyboardProtocol::Legacy {
            trace("policy legacy");
            return Self {
                tty: None,
                reply: Vec::new(),
            };
        }
        let mut guard = Self {
            tty: open_tty(),
            reply: Vec::new(),
        };
        let Some(tty) = guard.tty.as_mut() else {
            trace("tty none");
            return guard;
        };
        if policy == KeyboardProtocol::Auto {
            // 回复要被立刻读到，先进 raw mode，否则会卡在行缓冲里。
            if crossterm::terminal::enable_raw_mode().is_err() {
                trace("raw error");
                return guard;
            }
            let Some((flags, reply)) = probe_reply(tty) else {
                trace("probe none");
                return guard;
            };
            trace(&format!("probe ok flags {flags}"));
            guard.reply = reply;
        }
        if tty
            .write_all(KITTY_ENABLE)
            .and_then(|()| tty.flush())
            .is_err()
        {
            trace("enable error");
            return guard;
        }
        KEYBOARD_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
        trace_bytes("enable", KITTY_ENABLE);
        guard
    }

    /// 探测回复的原始字节；`Kitty` 强开时为空。
    pub fn reply(&self) -> &[u8] {
        &self.reply
    }

    /// 是否已经把标志压给了宿主终端。
    pub fn active(&self) -> bool {
        KEYBOARD_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for KeyboardGuard {
    fn drop(&mut self) {
        restore_keyboard();
    }
}

/// 幂等恢复：只在真的压入过标志时发一次 `CSI < u`。
///
/// panic 钩子、正常退出和 SIGTERM 走的收尾路径都会调它。
pub fn restore_keyboard() {
    if !KEYBOARD_ACTIVE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Some(mut tty) = open_tty() {
        let _ = tty.write_all(KITTY_RESTORE).and_then(|()| tty.flush());
    }
    trace_bytes("restore", KITTY_RESTORE);
}

/// 探测：发 `CSI ? u`，最多等 [`PROBE_TIMEOUT`]，返回 flags 与回复原始字节。
#[cfg(unix)]
fn probe_reply(tty: &mut std::fs::File) -> Option<(u8, Vec<u8>)> {
    use std::os::fd::AsRawFd;

    tty.write_all(KITTY_QUERY).ok()?;
    tty.flush().ok()?;
    trace_bytes("query", KITTY_QUERY);
    let fd = tty.as_raw_fd();
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    let mut reply = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let mut ready = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = remaining.as_millis().min(i32::MAX as u128) as i32;
        if unsafe { libc::poll(&mut ready, 1, millis) } <= 0 {
            break;
        }
        let mut chunk = [0u8; 64];
        let read = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if read <= 0 {
            break;
        }
        reply.extend_from_slice(&chunk[..read as usize]);
        if let Some(flags) = parse_flags_reply(&reply) {
            trace_bytes("reply", &reply);
            return Some((flags, reply));
        }
        if reply.len() > 128 {
            break;
        }
    }
    trace_bytes("reply", &reply);
    None
}

#[cfg(not(unix))]
fn probe_reply(_tty: &mut std::fs::File) -> Option<(u8, Vec<u8>)> {
    None
}

/// 从 `/dev/tty` 读 `timeout` 内可用的字节，追加到 `buffer`；有新字节返回 true。
#[cfg(unix)]
pub fn read_tty(timeout: std::time::Duration, buffer: &mut Vec<u8>) -> bool {
    use std::os::fd::AsRawFd;

    let Some(tty) = open_tty() else {
        return false;
    };
    let fd = tty.as_raw_fd();
    let mut ready = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    if unsafe { libc::poll(&mut ready, 1, millis) } <= 0 {
        return false;
    }
    let mut chunk = [0u8; 256];
    let read = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
    if read <= 0 {
        return false;
    }
    buffer.extend_from_slice(&chunk[..read as usize]);
    true
}

#[cfg(not(unix))]
pub fn read_tty(_timeout: std::time::Duration, _buffer: &mut Vec<u8>) -> bool {
    false
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

    #[test]
    fn flags_reply_parses_the_query_answer() {
        assert_eq!(parse_flags_reply(b"\x1b[?0u"), Some(0));
        assert_eq!(parse_flags_reply(b"\x1b[?1u"), Some(1));
        assert_eq!(parse_flags_reply(b"\x1b[?31u"), Some(31));
        // 回复前面混进了别的字节也能认出来。
        assert_eq!(parse_flags_reply(b"junk\x1b[?5u"), Some(5));
        assert_eq!(parse_flags_reply(b"\x1b[?u"), None);
        assert_eq!(parse_flags_reply(b"\x1b[?4;2m"), None);
        assert_eq!(parse_flags_reply(b""), None);
    }

    #[test]
    fn kitty_modifiers_follow_the_protocol_bitmask() {
        assert_eq!(kitty_modifiers(1), KeyModifiers::NONE);
        assert_eq!(kitty_modifiers(2), KeyModifiers::SHIFT);
        assert_eq!(kitty_modifiers(3), KeyModifiers::ALT);
        assert_eq!(kitty_modifiers(5), KeyModifiers::CONTROL);
        assert_eq!(
            kitty_modifiers(8),
            KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL
        );
        assert_eq!(
            kitty_modifiers(26),
            KeyModifiers::SHIFT | KeyModifiers::SUPER | KeyModifiers::HYPER
        );
    }

    #[test]
    fn kitty_sequences_decode_to_key_events() {
        let decode = |bytes: &[u8]| parse_kitty_key(bytes).map(|(key, _)| key).unwrap();

        assert_eq!(
            decode(b"\x1b[105;5u"),
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            decode(b"\x1b[9u"),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
        );
        assert_eq!(
            decode(b"\x1b[9;2u"),
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
        );
        assert_eq!(
            decode(b"\x1b[27u"),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        );
        assert_eq!(
            decode(b"\x1b[57352;5u"),
            KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)
        );
        assert_eq!(
            decode(b"\x1b[57364u"),
            KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)
        );
        // Shift 送基准键，解码时抬成大写，跟 legacy 的 KeyInput.text 对齐。
        assert_eq!(
            decode(b"\x1b[97;2u"),
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)
        );
        // alternate keys 字段忽略，不参与判定。
        assert_eq!(
            decode(b"\x1b[97:65;2u"),
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)
        );
    }

    #[test]
    fn kitty_sequences_carry_the_event_kind() {
        let (key, used) = parse_kitty_key(b"\x1b[105;5:3u").unwrap();
        assert_eq!(used, 10);
        assert_eq!(key.kind, KeyEventKind::Release);
        assert_eq!(key.code, KeyCode::Char('i'));
        let (key, _) = parse_kitty_key(b"\x1b[97;1:2u").unwrap();
        assert_eq!(key.kind, KeyEventKind::Repeat);
        let (key, _) = parse_kitty_key(b"\x1b[97;1:1u").unwrap();
        assert_eq!(key.kind, KeyEventKind::Press);
    }

    #[test]
    fn kitty_parser_ignores_incomplete_and_foreign_sequences() {
        assert_eq!(parse_kitty_key(b"\x1b[105;5"), None);
        assert_eq!(parse_kitty_key(b"\x1b[1;5A"), None);
        assert_eq!(parse_kitty_key(b"abc"), None);
        assert_eq!(
            parse_legacy_key(0x09),
            Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        );
        assert_eq!(
            parse_legacy_key(0x7f),
            Some(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
        );
        assert_eq!(
            parse_legacy_key(b'q'),
            Some(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        );
        assert_eq!(parse_legacy_key(0x01), None);
        // 从流中间开始解，前面的字节不算在内。
        let (key, used) = parse_kitty_key(b"ab\x1b[9u").unwrap();
        assert_eq!(used, 6);
        assert_eq!(key.code, KeyCode::Tab);
    }
}
