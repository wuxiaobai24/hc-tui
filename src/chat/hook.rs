//! 会话映射 hook：往 pi 的扩展目录里写一个小模块，把会话文件路径以 label 报到 core。
//!
//! hc 没有 agent 会话的接口，只给 OSC 7501 的状态词；因此"这个 Pane 是哪份会话"
//! 只能由 agent 自己上报。pi 的扩展在 `session_start` 里能拿到 `getSessionFile()`，
//! 调到 `hc label <t> session=<path>` 之后 hc-tui 就能读到确定映射。
//!
//! 安装是幂等的：文件带版本标记，内容变化时重写。

use std::path::PathBuf;

/// 扩展文件名。一个文件，装进 `<agent-dir>/extensions/`。
pub const NAME: &str = "hc-tui-session.ts";
/// 版本标记：用来判断已安装的副本是否过期。
const MARKER: &str = "hc-tui hook version 1";

const TEMPLATE: &str = r#"// hc-tui hook version 1 — 由 hc-tui 写入，重新安装会覆盖。
// 作用：把 pi 当前的会话文件路径报到 hc 的终端 label，Chat 视图就能精确对上
// "这个 Pane 是哪份会话"，而不是按写入时间推断。
import { execFile } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const LABEL = "session";

function report(value: string): void {
  const terminal = process.env.HC_TERMINAL_ID;
  if (!terminal) return;
  execFile("hc", ["label", terminal, `${LABEL}=${value}`], () => {});
}

export default function (pi: ExtensionAPI) {
  const sync = (_event: unknown, ctx: { sessionManager: { getSessionFile(): string | undefined } }) => {
    report(ctx.sessionManager.getSessionFile() ?? "");
  };
  pi.on("session_start", sync);
  pi.on("agent_start", sync);
  pi.on("session_shutdown", () => report(""));
}
"#;

/// pi 的配置目录：`PI_CODING_AGENT_DIR` 优先，其次 `~/.pi/agent`。
pub fn agent_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".pi/agent"))
}

/// 扩展文件的目标路径。
pub fn path() -> Option<PathBuf> {
    Some(agent_dir()?.join("extensions").join(NAME))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 没装。
    Missing,
    /// 已安装且是最新版本。
    Current,
    /// 装了但版本标记对不上。
    Stale,
}

pub fn status() -> Status {
    let Some(path) = path() else {
        return Status::Missing;
    };
    match std::fs::read_to_string(path) {
        Ok(text) if text.contains(MARKER) => Status::Current,
        Ok(_) => Status::Stale,
        Err(_) => Status::Missing,
    }
}

/// 写入扩展文件，返回目标路径。
pub fn install() -> std::io::Result<PathBuf> {
    let path =
        path().ok_or_else(|| std::io::Error::other("找不到 pi 的配置目录（HOME 未设置）"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, TEMPLATE)?;
    Ok(path)
}

/// 删掉扩展文件；返回是否真的删了。
pub fn remove() -> std::io::Result<bool> {
    let Some(path) = path() else {
        return Ok(false);
    };
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// 给提示行用的一句话。
pub fn describe(status: Status) -> &'static str {
    match status {
        Status::Missing => "会话映射 hook 未安装",
        Status::Current => "会话映射 hook 已安装",
        Status::Stale => "会话映射 hook 需要更新",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::pi::ENV_LOCK;

    #[test]
    fn install_status_and_remove_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join("hc-tui-hook-test");
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PI_CODING_AGENT_DIR", &root);

        assert_eq!(status(), Status::Missing);
        let path = install().expect("install");
        assert_eq!(path, root.join("extensions").join(NAME));
        assert_eq!(status(), Status::Current);
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains(MARKER));
        assert!(text.contains("getSessionFile"));

        assert!(remove().expect("remove"));
        assert_eq!(status(), Status::Missing);
        assert!(!remove().expect("remove twice"));

        std::env::remove_var("PI_CODING_AGENT_DIR");
        let _ = std::fs::remove_dir_all(&root);
    }
}
