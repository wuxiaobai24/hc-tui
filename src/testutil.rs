//! 单元测试共用的临时目录。
//!
//! 只靠 `SystemTime` 当唯一来源在 macOS 上不够：那一侧墙钟精度到微秒，同一
//! 进程里并行的用例常常拿到同一个值，随后共用配置目录或暂存目录——一个用例
//! `remove_dir_all` 时另一个正在写或读 bundle，就会看到 0 字节或半份文件。
//! 序号是进程内原子计数，配合 pid 保证不同测试二进制、同一二进制的不同线程
//! 都不撞名。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 进程内递增序号。
pub(crate) fn serial() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 一次性的临时根目录：`<tmp>/<prefix>-<tag>-<pid>-<时间戳>-<序号>`。
pub(crate) fn unique_dir(prefix: &str, tag: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "{prefix}-{tag}-{}-{stamp}-{}",
        std::process::id(),
        serial()
    ))
}
