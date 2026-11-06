//! 宿主剪贴板复制与图片路径粘贴。
//!
//! 两条路一起走：先写 OSC 52（无需任何外部程序，多数终端支持），再尽力调用
//! 平台剪贴板工具。任一条成立就算成功，调用方只把结果写进状态栏。

use std::io::Write;
use std::process::{Command, Stdio};

use crossterm::execute;

use crate::i18n::{t, t1, t2};

/// Read PNG bytes on the machine running the TUI, before terminal.put uploads them.
#[cfg(not(target_os = "macos"))]
pub(crate) fn image() -> std::result::Result<Vec<u8>, String> {
    let tools: &[(&str, &[&str])] = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[("wl-paste", &["--no-newline", "--type", "image/png"])]
    } else {
        &[
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            ),
            ("wl-paste", &["--no-newline", "--type", "image/png"]),
        ]
    };
    let mut available = false;
    for (program, args) in tools {
        let Ok(output) = std::process::Command::new(program).args(*args).output() else {
            continue;
        };
        available = true;
        if output.status.success() && output.stdout.starts_with(b"\x89PNG\r\n\x1a\n") {
            return checked_png(output.stdout);
        }
    }
    Err(if available {
        t("no PNG image available in the local clipboard").to_string()
    } else {
        t("image paste needs wl-paste (Wayland) or xclip (X11) on the TUI machine").to_string()
    })
}

fn checked_png(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(t("image was not uploaded: core accepts at most 8 MiB").into());
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(t("no PNG image available in the local clipboard").into());
    }
    Ok(bytes)
}

/// Standard Additions reads screenshot PNG data without Homebrew or GUI automation.
#[cfg(target_os = "macos")]
pub(crate) fn image() -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::DirBuilderExt;

    let directory = std::env::temp_dir().join(crate::shared::identity("hc-tui-clipboard-"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|error| t1("cannot prepare clipboard image: {}", error))?;
    let path = directory.join("screenshot.png");
    let result = (|| {
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", MAC_READ_PNG])
            .arg(&path)
            .output()
            .map_err(|error| t1("cannot read macOS clipboard: {}", error))?;
        if !output.status.success() {
            return Err(t1(
                "no PNG image available in the local clipboard: {}",
                String::from_utf8_lossy(&output.stderr).trim(),
            ));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|file| {
                file.take(MAX_IMAGE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| t1("cannot read clipboard image: {}", error))?;
        checked_png(bytes)
    })();
    let _ = std::fs::remove_dir_all(&directory);
    result
}

#[cfg(target_os = "macos")]
const MAC_READ_PNG: &str = r#"
on run argv
    set imageData to the clipboard as «class PNGf»
    set outputFile to open for access (POSIX file (item 1 of argv)) with write permission
    try
        set eof outputFile to 0
        write imageData to outputFile
        close access outputFile
    on error errorMessage number errorNumber
        close access outputFile
        error errorMessage number errorNumber
    end try
end run
"#;

/// 复制文本，返回一句给状态栏用的人话。
pub fn copy(text: &str) -> String {
    if text.is_empty() {
        return t("copy: nothing selected").to_string();
    }
    let mut ways = Vec::new();
    if osc52(text) {
        ways.push("OSC 52");
    }
    if tool(text) {
        ways.push(t("clipboard tool"));
    }
    match ways.len() {
        0 => t1(
            "copied {} chars (host may not accept it)",
            text.chars().count(),
        ),
        _ => t2(
            "copied {} chars · {}",
            text.chars().count(),
            ways.join(" + "),
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

pub(crate) const MAX_IMAGE_BYTES: usize = 8 << 20;

/// A host terminal may paste a file path instead of image bytes (e.g. macOS file copies).
pub(crate) struct ImageAttachment {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
}

/// None means ordinary text. A recognized image path must upload or report an error;
/// it must not silently become an inaccessible attachment path in another machine.
pub(crate) fn image_file_paste(text: &str) -> Option<Result<ImageAttachment, String>> {
    use std::io::Read;
    let text = text.trim();
    if text.contains(['\r', '\n']) {
        return None;
    }
    let text = if text.len() >= 2
        && ((text.starts_with('\'') && text.ends_with('\''))
            || (text.starts_with('"') && text.ends_with('"')))
    {
        &text[1..text.len().saturating_sub(1)]
    } else {
        text
    };
    let lower = text.to_ascii_lowercase();
    if ![".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
    {
        return None;
    }
    let mut path = String::new();
    if let Some(url) = text.strip_prefix("file://") {
        let url = url
            .strip_prefix("localhost/")
            .map(|path| format!("/{path}"))
            .unwrap_or_else(|| url.to_string());
        if !url.starts_with('/') {
            return None;
        }
        let mut bytes = Vec::new();
        let mut encoded = url.as_bytes().iter().copied();
        while let Some(byte) = encoded.next() {
            if byte == b'%' {
                let high = encoded.next().and_then(|b| (b as char).to_digit(16));
                let low = encoded.next().and_then(|b| (b as char).to_digit(16));
                let (Some(high), Some(low)) = (high, low) else {
                    return Some(Err(t("invalid image file URL").into()));
                };
                bytes.push((high * 16 + low) as u8);
            } else {
                bytes.push(byte);
            }
        }
        path = match String::from_utf8(bytes) {
            Ok(path) => path,
            Err(_) => return Some(Err(t("invalid UTF-8 image path").into())),
        };
    } else {
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                path.push(chars.next().unwrap_or(ch));
            } else {
                path.push(ch);
            }
        }
    }
    let path = std::path::Path::new(&path);
    if !path.is_absolute() {
        return None;
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return None;
    }
    Some((|| {
        let unreadable = |error| {
            t1(
                "image was not uploaded: source is unreadable on the TUI machine ({})",
                error,
            )
        };
        let mut bytes = Vec::new();
        let metadata = std::fs::metadata(path).map_err(unreadable)?;
        if !metadata.is_file() {
            return Err(t("image was not uploaded: source is not a regular file").into());
        }
        let file = std::fs::File::open(path).map_err(unreadable)?;
        // terminal.put accepts at most 8 MiB; bound the read if the file grows.
        file.take((MAX_IMAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| t1("cannot read image: {}", error))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(t("image was not uploaded: core accepts at most 8 MiB").into());
        }
        let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "jpg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "webp"
        } else {
            return Err(t("image was not uploaded: file is not PNG, JPEG, GIF or WebP").into());
        };
        Ok(ImageAttachment { bytes, extension })
    })())
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn clipboard_png_rejects_invalid_and_oversized_data() {
        assert!(checked_png(b"text".to_vec())
            .unwrap_err()
            .contains("no PNG"));
        let mut oversized = vec![0; MAX_IMAGE_BYTES + 1];
        oversized[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(checked_png(oversized).unwrap_err().contains("8 MiB"));
    }

    // Explicit opt-in: this test replaces the host clipboard. Run only on a disposable Mac runner.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "replaces the host clipboard; macOS CI only"]
    fn macos_clipboard_roundtrip() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[20, 40, 60, 255])
                .unwrap();
        }
        let directory = std::env::temp_dir().join(crate::shared::identity("clipboard-test-"));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("系统 截图.png");
        std::fs::write(&path, &bytes).unwrap();
        let output = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                r#"on run argv
                set the clipboard to (read (POSIX file (item 1 of argv)) as «class PNGf»)
            end run"#,
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(image().unwrap(), bytes);
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", r#"set the clipboard to "ordinary text""#])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(image().unwrap_err().contains("no PNG"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn image_paths_quotes_escapes_and_file_urls_read_the_original_bytes() {
        let root = std::env::temp_dir().join(crate::shared::identity("image-paste-"));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("企业微信截图 (one).png");
        let bytes = b"\x89PNG\r\n\x1a\noriginal";
        std::fs::write(&path, bytes).unwrap();
        let plain = path.to_str().unwrap();
        let encoded: String = plain
            .bytes()
            .map(|byte| {
                if byte == b'/' || byte == b'.' || byte.is_ascii_alphanumeric() {
                    (byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect();
        for text in [
            plain.to_string(),
            format!("'{plain}'"),
            format!("\"{plain}\""),
            plain
                .replace(' ', "\\ ")
                .replace('(', "\\(")
                .replace(')', "\\)"),
            format!("file://{encoded}"),
            format!("file://localhost{encoded}"),
        ] {
            let attachment = image_file_paste(&text).unwrap().unwrap();
            assert_eq!(attachment.bytes, bytes);
            assert_eq!(attachment.extension, "png");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_mac_images_are_errors_and_ordinary_text_is_untouched() {
        let path = format!(
            "/Users/no-such-user/{}/企业微信截图.png",
            crate::shared::identity("clipboard-")
        );
        let error = image_file_paste(&path).unwrap().err().unwrap();
        assert!(error.contains("image was not uploaded"));
        assert!(error.contains("TUI machine"));
        for text in [
            "",
            "'",
            "\"",
            "normal text",
            "please inspect /tmp/image.png",
            "relative.png",
            "file:///tmp/100%notes.md",
            "one\ntwo",
            "/tmp/main.rs",
        ] {
            assert!(image_file_paste(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn invalid_and_oversized_images_are_not_uploaded() {
        use std::io::Write;
        let root = std::env::temp_dir().join(crate::shared::identity("image-paste-invalid-"));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("image.png");
        std::fs::write(&path, b"not an image").unwrap();
        assert!(image_file_paste(path.to_str().unwrap())
            .unwrap()
            .err()
            .unwrap()
            .contains("not PNG"));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        file.set_len((8 << 20) + 1).unwrap();
        assert!(image_file_paste(path.to_str().unwrap())
            .unwrap()
            .err()
            .unwrap()
            .contains("8 MiB"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
