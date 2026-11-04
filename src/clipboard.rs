//! 复制到宿主剪贴板。
//!
//! 两条路一起走：先写 OSC 52（无需任何外部程序，多数终端支持），再尽力调用
//! 平台剪贴板工具。任一条成立就算成功，调用方只把结果写进状态栏。

use std::io::Write;
use std::process::{Command, Stdio};

use crossterm::execute;

/// 复制文本，返回一句给状态栏用的人话。
pub fn copy(text: &str) -> String {
    if text.is_empty() {
        return "copy: nothing selected".to_string();
    }
    let mut ways = Vec::new();
    if osc52(text) {
        ways.push("OSC 52");
    }
    if tool(text) {
        ways.push("clipboard tool");
    }
    match ways.len() {
        0 => format!(
            "copied {} chars (host may not accept it)",
            text.chars().count()
        ),
        _ => format!(
            "copied {} chars · {}",
            text.chars().count(),
            ways.join(" + ")
        ),
    }
}

fn osc52(text: &str) -> bool {
    let encoded = base64(text.as_bytes());
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = std::io::stdout();
    execute!(stdout, crossterm::style::Print(sequence))
        .and_then(|()| stdout.flush())
        .is_ok()
}

fn tool(text: &str) -> bool {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[][..])]
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[
            ("wl-copy", &[][..]),
            ("xclip", &["-selection", "clipboard"]),
        ]
    } else if std::env::var_os("DISPLAY").is_some() {
        &[
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["-b", "-i"]),
        ]
    } else {
        &[
            ("wl-copy", &[][..]),
            ("xclip", &["-selection", "clipboard"]),
        ]
    };
    for (program, args) in candidates {
        let Ok(mut child) = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() {
                continue;
            }
        }
        if child.wait().map(|status| status.success()).unwrap_or(false) {
            return true;
        }
    }
    false
}

/// 最小的标准 base64：OSC 52 与 `terminal.put` 共用。
pub(crate) fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"hello").len(), 8);
        assert!(base64(b"hello").ends_with('='));
        assert_eq!(base64(b"hello").len(), 8);
        assert_eq!(&base64(b"hello")[..4], &base64(b"hell")[..4]);
    }
}
