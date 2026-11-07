//! 命令行文案表：`src/main.rs`、`src/update.rs` 与配置文件警告里的英文原文。

pub fn lookup(en: &str) -> Option<&'static str> {
    Some(match en {
        // session-hook 子命令的输出。
        "installed {}" => "已安装 {}",
        "removed: {}" => "已移除：{}",
        "usage: hc-tui session-hook install|remove|status (runs on this machine)" => {
            "用法：hc-tui session-hook install|remove|status（在本机执行）"
        }
        // `--help` 的用法块，保持原有缩进。
        "usage: hc-tui [options]" => "用法：hc-tui [选项]",
        "       hc-tui probe [options]" => "       hc-tui probe [选项]",
        "       hc-tui commands | ls [options]" => "       hc-tui commands | ls [选项]",
        "       hc-tui do <command> [JSON arguments] [options]" => {
            "       hc-tui do <command> [JSON 参数] [选项]"
        }
        "  --session <name>   hc session to connect to (default: HC_SESSION)" => {
            "  --session <name>   要连接的 hc 会话（默认：HC_SESSION）"
        }
        "  --attach <t>       attach an existing terminal instead of starting a shell" => {
            "  --attach <t>       附着已有终端，不新起 shell"
        }
        "  --config <path>    config file (default: ~/.config/hc-tui/config.toml)" => {
            "  --config <path>    配置文件（默认：~/.config/hc-tui/config.toml）"
        }
        "  --no-restore       ignore the layout saved in the core doc" => {
            "  --no-restore       忽略 core 文档里保存的布局"
        }
        "  --no-mods          builtin chrome, no module runtime" => {
            "  --no-mods          使用内置外框，不加载模块运行时"
        }
        // 参数解析的报错。
        "unknown command: {}" => "未知命令：{}",
        "do requires a command" => "do 需要命令参数",
        "--session needs a value" => "--session 需要一个值",
        "--attach needs a value" => "--attach 需要一个值",
        "--config needs a value" => "--config 需要一个值",
        "unknown option: {}" => "未知选项：{}",
        // `probe` 的握手输出：字段名保留，值前的对齐不变。
        "hc {} (generation {})" => "hc {}（generation {}）",
        "core    {} ({})" => "core    {}（{}）",
        "features {}" => "特性    {}",
        "methods  {}" => "方法    {}",
        "terminals {}" => "终端    {}",
        // `update` 的报错。
        "no release archive for {}/{}" => "没有适用于 {}/{} 的版本压缩包",
        "expected a release version such as 0.4.0" => "版本号应形如 0.4.0",
        "update needs curl on PATH" => "更新需要 PATH 中有 curl",
        "download failed: {}" => "下载失败：{}",
        "latest release redirected to an unexpected URL" => "最新版本的重定向地址不符合预期",
        "cannot resolve latest release: {}" => "无法确定最新版本：{}",
        "archive needs its .sha256 file" => "压缩包需要配套的 .sha256 文件",
        "empty SHA-256 file" => "SHA-256 文件为空",
        "invalid SHA-256 file" => "SHA-256 文件无效",
        "cannot calculate SHA-256" => "无法计算 SHA-256",
        "SHA-256 mismatch; installed executable was not changed" => {
            "SHA-256 不一致；已安装的可执行文件未改动"
        }
        "update needs tar on PATH" => "更新需要 PATH 中有 tar",
        "cannot list release archive" => "无法列出压缩包内容",
        "archive has no hc-tui executable at its root" => "压缩包根目录下没有 hc-tui 可执行文件",
        "archive executable is not a regular file" => "压缩包里的可执行文件不是普通文件",
        "executable has no parent directory" => "可执行文件没有上级目录",
        "cannot stage update beside executable; check directory permissions" => {
            "无法在可执行文件旁暂存更新；请检查目录权限"
        }
        "cannot atomically replace executable" => "无法原子替换可执行文件",
        "--version needs a value" => "--version 需要一个值",
        "--from needs a .tar.gz archive" => "--from 需要一个 .tar.gz 压缩包",
        "hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]\nArchives require a matching archive.tar.gz.sha256. Updates replace this executable atomically." => {
            "hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]\n压缩包需配套 archive.tar.gz.sha256。更新会原子替换当前可执行文件。"
        }
        "unknown update option: {}" => "未知的 update 选项：{}",
        "--check cannot be combined with --from" => "--check 不能与 --from 同时使用",
        "installed {} · release {}" => "已安装 {} · 版本 {}",
        "downloaded executable cannot run on this machine" => "下载的可执行文件无法在本机运行",
        "invalid executable version output" => "可执行文件输出的版本号无效",
        "executable version does not match release" => "可执行文件版本与目标版本不一致",
        "updated hc-tui to {}; reopen running clients to use it" => {
            "已更新 hc-tui 到 {}；重开正在运行的客户端后生效"
        }
        // 配置文件里剩下的两条英文警告。
        "Invalid panel {}" => "无效的 panel：{}",
        "[[panels]] requires name and command" => "[[panels]] 需要 name 和 command",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn cli_strings_have_chinese_text() {
        assert_eq!(lookup("unknown command: {}"), Some("未知命令：{}"));
        assert_eq!(
            lookup("updated hc-tui to {}; reopen running clients to use it"),
            Some("已更新 hc-tui 到 {}；重开正在运行的客户端后生效")
        );
        assert_eq!(lookup("terminals {}"), Some("终端    {}"));
        assert_eq!(
            lookup("[[panels]] requires name and command"),
            Some("[[panels]] 需要 name 和 command")
        );
        assert_eq!(lookup("--check"), None);
    }
}
