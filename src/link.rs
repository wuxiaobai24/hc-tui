//! OSC 8 链接的打开：只放行 http / https / mailto，交给系统 opener。
//!
//! 所有打开入口（Ctrl+Click、`open_link` 动作、Copy Mode 的 `o`、
//! 确认提示）都走 [`open`]，scheme 与长度校验因此只有一份。

use std::process::{Command, Stdio};

use unicode_width::UnicodeWidthChar;

use crate::i18n::{t, t1};

/// 打开前接受的最长 URI 字节数。
pub(crate) const MAX_URI_BYTES: usize = 4096;
/// 状态行里显示目标地址的最大列数。
const MAX_HINT_COLUMNS: usize = 80;

/// 打开 `uri`。macOS 用 `open`，其余平台用 `xdg-open`。
pub(crate) fn open(uri: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    open_with(opener, uri)
}

/// 用指定的 opener 打开；测试传入假程序，避免真的拉起浏览器。
pub(crate) fn open_with(opener: &str, uri: &str) -> Result<(), String> {
    validate(uri)?;
    Command::new(opener)
        .arg(uri)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| t1("cannot open link: {}", error))
}

/// scheme 与长度检查。
pub(crate) fn validate(uri: &str) -> Result<(), String> {
    if uri.is_empty() {
        return Err(t("link has no target").into());
    }
    if uri.len() > MAX_URI_BYTES {
        return Err(t1("link is longer than {} bytes", MAX_URI_BYTES));
    }
    let Some((scheme, _)) = uri.split_once(':') else {
        return Err(t("link has no scheme").into());
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "mailto" => Ok(()),
        _ => Err(t1("link scheme {} is not allowed", scheme)),
    }
}

/// 状态行里显示的目标地址：控制字符换成 `?`，超过 80 列截断并加 `…`。
pub(crate) fn hint(uri: &str) -> String {
    let chars: Vec<char> = uri
        .chars()
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect();
    let width: usize = chars.iter().map(|ch| ch.width().unwrap_or(0)).sum();
    if width <= MAX_HINT_COLUMNS {
        return chars.into_iter().collect();
    }
    let mut out = String::new();
    let mut columns = 0;
    for ch in chars {
        let width = ch.width().unwrap_or(0);
        if columns + width > MAX_HINT_COLUMNS - 1 {
            break;
        }
        out.push(ch);
        columns += width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use unicode_width::UnicodeWidthStr;

    use super::*;

    #[test]
    fn only_web_and_mail_schemes_pass() {
        for uri in [
            "https://example.com",
            "http://example.com/a?b=c",
            "mailto:someone@example.com",
            "HTTPS://EXAMPLE.COM",
        ] {
            assert_eq!(validate(uri), Ok(()), "{uri}");
        }
        for uri in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com",
            "/local/path",
            "://missing",
            "",
        ] {
            assert!(validate(uri).is_err(), "{uri}");
        }
    }

    #[test]
    fn oversized_targets_are_rejected_before_the_opener_runs() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URI_BYTES));
        let error = open_with("hc-tui-test-no-such-opener", &long).unwrap_err();
        assert!(error.contains(&MAX_URI_BYTES.to_string()), "{error}");
    }

    #[test]
    fn rejected_schemes_never_reach_the_opener() {
        let error = open_with("hc-tui-test-no-such-opener", "file:///etc/passwd").unwrap_err();
        assert!(error.contains("file"), "{error}");
    }

    #[test]
    fn hint_sanitizes_and_truncates_to_eighty_columns() {
        assert_eq!(hint("https://example.com"), "https://example.com");
        assert_eq!(
            hint("https://example.com/\u{1b}[31m"),
            "https://example.com/?[31m"
        );
        let long = format!("https://example.com/{}", "a".repeat(100));
        let short = hint(&long);
        assert!(short.width() <= MAX_HINT_COLUMNS, "{short}");
        assert!(short.ends_with('…'));
    }

    #[test]
    fn the_opener_receives_the_uri() {
        let dir = std::env::temp_dir().join(crate::shared::identity("link-opener-"));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("argv");
        let script = dir.join("opener.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s' \"$1\" > {}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        open_with(&script.to_string_lossy(), "https://example.com/ok").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !log.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "https://example.com/ok"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
