//! 插件动作日志：每次调用一份 stdout/stderr 文件，单份超过 1 MiB 轮转保留两份，
//! 每个包另有总量与文件数上限，超出时删除最旧的请求日志。
//!
//! 内存里的输出由 [`super::runner`] 保留最近 64 KiB；这里负责落盘与查询，
//! 不参与命令协议。

use std::io::Write;
use std::path::{Path, PathBuf};

/// 单份日志的轮转阈值。
pub const ROTATE_BYTES: u64 = 1024 * 1024;
/// 每个包的日志总量上限。
pub const PACKAGE_BYTES: u64 = 8 * 1024 * 1024;
/// 每个包保留的日志文件数上限。
pub const PACKAGE_FILES: usize = 32;

fn request_dir(root: &Path, package: &str) -> PathBuf {
    root.join("logs").join(package)
}

fn request_path(root: &Path, package: &str, request: &str, stream: &str) -> PathBuf {
    request_dir(root, package).join(format!("{request}.{stream}.log"))
}

/// 追加一段输出；超过阈值时轮转到 `.1`。
pub fn append(
    root: &Path,
    package: &str,
    request: &str,
    stream: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    let dir = request_dir(root, package);
    std::fs::create_dir_all(&dir)?;
    let path = request_path(root, package, request, stream);
    if path.metadata().map(|meta| meta.len()).unwrap_or(0) > ROTATE_BYTES {
        let rotated = request_path(root, package, request, &format!("{stream}.1"));
        let _ = std::fs::remove_file(&rotated);
        std::fs::rename(&path, rotated)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(bytes)?;
    drop(file);
    enforce(&dir)
}

/// 包级配额：先按文件数、再按总字节数删最旧的日志，至少保留刚写入的那份。
fn enforce(dir: &Path) -> std::io::Result<()> {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        files.push((modified, meta.len(), entry.path()));
    }
    files.sort_by_key(|file| file.0);
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    let mut removable = files.len();
    let mut index = 0;
    while index < files.len() && (removable > PACKAGE_FILES || total > PACKAGE_BYTES) {
        if removable <= 1 {
            break;
        }
        let (_, len, path) = &files[index];
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*len);
            removable -= 1;
        }
        index += 1;
    }
    Ok(())
}

/// 读回一次调用的日志末尾；文件缺失时返回空字符串。
pub fn tail(root: &Path, package: &str, request: &str, lines: usize) -> String {
    let mut out = String::new();
    for stream in ["stdout", "stderr"] {
        let text = std::fs::read_to_string(request_path(root, package, request, stream))
            .or_else(|_| {
                std::fs::read_to_string(request_path(
                    root,
                    package,
                    request,
                    &format!("{stream}.1"),
                ))
            })
            .unwrap_or_default();
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(lines);
        if !text.is_empty() {
            out.push_str(&format!("--- {stream}\n"));
            out.push_str(&all[start..].join("\n"));
            out.push('\n');
        }
    }
    out
}

/// 删除一次调用的日志（预留给清理命令）。
pub fn remove(root: &Path, package: &str, request: &str) -> std::io::Result<()> {
    for stream in ["stdout", "stderr"] {
        let _ = std::fs::remove_file(request_path(root, package, request, stream));
        let _ = std::fs::remove_file(request_path(root, package, request, &format!("{stream}.1")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_both_streams_and_rotates() {
        let root = std::env::temp_dir().join(format!(
            "hctui-plugin-logs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        append(&root, "pkg", "p1", "stdout", b"hello\nworld\n").unwrap();
        append(&root, "pkg", "p1", "stderr", b"warn\n").unwrap();
        let text = tail(&root, "pkg", "p1", 1);
        assert!(text.contains("world"), "{text}");
        assert!(text.contains("warn"), "{text}");
        let big = vec![b'x'; (ROTATE_BYTES + 1) as usize];
        append(&root, "pkg", "p1", "stdout", &big).unwrap();
        append(&root, "pkg", "p1", "stdout", b"after\n").unwrap();
        assert!(tail(&root, "pkg", "p1", 5).contains("after"));
        assert!(request_path(&root, "pkg", "p1", "stdout.1").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_package_keeps_a_bounded_number_of_log_files() {
        let root = std::env::temp_dir().join(format!(
            "hctui-plugin-logs-bound-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        for index in 0..(PACKAGE_FILES + 20) {
            append(&root, "pkg", &format!("p{index}"), "stdout", b"x\n").unwrap();
            // 让最旧文件的 mtime 可区分。
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let files = std::fs::read_dir(request_dir(&root, "pkg"))
            .unwrap()
            .filter_map(Result::ok)
            .count();
        assert!(files <= PACKAGE_FILES, "{files} log files kept");
        let total: u64 = std::fs::read_dir(request_dir(&root, "pkg"))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|meta| meta.len())
            .sum();
        assert!(total <= PACKAGE_BYTES, "{total} bytes kept");
        std::fs::remove_dir_all(root).ok();
    }
}
