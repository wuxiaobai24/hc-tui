//! 连接层、剪贴板、布局与本地命令的文案表。
//!
//! 这些文案来自 `hc/*`、`control`、`clipboard`、`shared` 与协议里的固定短语，
//! 多数会落到提示行、状态栏或命令行输出上。**协议字段、`kind`/`hint` 取值、
//! 方法名一律不翻**，只翻给人看的那一句。

/// 按英文原文取中文文案。
pub fn lookup(en: &str) -> Option<&'static str> {
    match en {
        // hc/client.rs：握手与 RPC 失败
        "cannot start `hc connect`; is herdr-core installed and on PATH?" => {
            Some("无法启动 `hc connect`；herdr-core 是否已安装并在 PATH 上？")
        }
        "hc connect has no stdin" => Some("hc connect 没有 stdin"),
        "hc connect has no stdout" => Some("hc connect 没有 stdout"),
        "core hello timed out" => Some("core 握手超时"),
        "Pane {}: input buffer full; {} new bytes were not sent" => {
            Some("窗格 {}：输入缓冲已满，本次 {} 字节未发送")
        }
        "core closed the connection during hello" => Some("握手期间 core 关闭了连接"),
        "hello reply is not json" => Some("hello 回复不是 json"),
        "hello failed: {}" => Some("hello 失败：{}"),
        "hello reply has no result" => Some("hello 回复缺少 result"),
        "hello result shape changed" => Some("hello 回复的结构变了"),
        "unsupported protocol generation {} (this client speaks 1); run `hc update`" => {
            Some("不支持的协议代次 {}（本客户端只说 1）；请运行 `hc update`")
        }
        "core refused framed mode; run `hc update`" => {
            Some("core 拒绝 framed 模式；请运行 `hc update`")
        }
        "core is missing `{}`; run `hc update`" => Some("core 缺少 `{}`；请运行 `hc update`"),
        "core closed the connection" => Some("core 关闭了连接"),
        "core closed the connection mid-frame" => Some("core 在帧中途关闭了连接"),
        "{} failed: {}" => Some("{} 失败：{}"),
        "unknown error" => Some("未知错误"),

        // hc/net.rs：后台网络线程的状态与 RPC 失败
        "cannot start tokio runtime: {}" => Some("无法启动 tokio 运行时：{}"),
        "events.subscribe failed: {}" => Some("events.subscribe 失败：{}"),
        "events.subscribe result shape changed" => Some("events.subscribe 回复的结构变了"),
        "no frames for a long time" => Some("长时间没有收到帧"),
        "{} timed out" => Some("{} 超时"),
        "too many pending calls" => Some("等待回复的调用过多"),
        "terminal.run returned no address" => Some("terminal.run 没有返回地址"),
        "attach result shape changed" => Some("attach 回复的结构变了"),
        "detached" => Some("已分离"),

        // hc/codec.rs
        "frame payload of {} bytes exceeds {}" => Some("帧载荷 {} 字节超过上限 {}"),

        // control.rs：本地命令套接字
        "another TUI already accepts commands for this session" => {
            Some("已有另一个 TUI 接管本会话的命令")
        }
        "command queue is full" => Some("命令队列已满"),
        "UI did not answer" => Some("界面没有应答"),
        "expected one JSON request line" => Some("期望一行 JSON 请求"),
        "local command sockets require Unix" => Some("本地命令套接字只能在 Unix 上使用"),

        // clipboard.rs：剪贴板与图片粘贴
        "no PNG image available in the local clipboard" => Some("本地剪贴板里没有 PNG 图片"),
        "no PNG image available in the local clipboard: {}" => {
            Some("本地剪贴板里没有 PNG 图片：{}")
        }
        "image paste needs wl-paste (Wayland) or xclip (X11) on the TUI machine" => {
            Some("粘贴图片需要 TUI 机器上的 wl-paste（Wayland）或 xclip（X11）")
        }
        "image was not uploaded: core accepts at most 8 MiB" => {
            Some("图片未上传：core 最多接受 8 MiB")
        }
        "cannot prepare clipboard image: {}" => Some("无法准备剪贴板图片：{}"),
        "cannot read macOS clipboard: {}" => Some("无法读取 macOS 剪贴板：{}"),
        "cannot read clipboard image: {}" => Some("无法读取剪贴板图片：{}"),
        "copy: nothing selected" => Some("复制：没有选中内容"),
        "copied {} chars (host may not accept it)" => {
            Some("已复制 {} 个字符（宿主终端可能不接受）")
        }
        "copied {} chars · {}" => Some("已复制 {} 个字符 · {}"),
        "clipboard tool" => Some("剪贴板工具"),
        "invalid image file URL" => Some("无效的图片文件 URL"),
        "invalid UTF-8 image path" => Some("图片路径不是有效的 UTF-8"),
        "image was not uploaded: source is unreadable on the TUI machine ({})" => {
            Some("图片未上传：源文件在 TUI 机器上不可读（{}）")
        }
        "image was not uploaded: source is not a regular file" => {
            Some("图片未上传：源文件不是普通文件")
        }
        "cannot read image: {}" => Some("无法读取图片：{}"),
        "image was not uploaded: file is not PNG, JPEG, GIF or WebP" => {
            Some("图片未上传：文件不是 PNG、JPEG、GIF 或 WebP")
        }

        // shared.rs：共享文档解码与三方合并
        "layout has no spaces array" => Some("布局缺少 spaces 数组"),
        "space has no id" => Some("工作区缺少 id"),
        "space has no tabs array" => Some("工作区缺少 tabs 数组"),
        "tab has no id" => Some("Tab 缺少 id"),
        "tab has no root" => Some("Tab 缺少 root"),
        "concurrent edits conflict; reload and reapply this change" => {
            Some("并发修改冲突；请重新加载后再应用这次改动")
        }

        // hc/proto.rs：侧边栏分组标题，由渲染方查表
        "Other terminals" => Some("其他终端"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn translated_entries_are_chinese() {
        assert_eq!(
            lookup("core closed the connection"),
            Some("core 关闭了连接")
        );
        assert_eq!(lookup("Other terminals"), Some("其他终端"));
    }

    #[test]
    fn placeholders_survive_translation() {
        assert!(lookup("{} timed out").unwrap().contains("{}"));
        assert!(lookup("hello failed: {}").unwrap().contains("{}"));
        assert!(
            lookup("copied {} chars · {}")
                .unwrap()
                .matches("{}")
                .count()
                == 2
        );
        assert!(
            lookup("frame payload of {} bytes exceeds {}")
                .unwrap()
                .matches("{}")
                .count()
                == 2
        );
    }

    #[test]
    fn unknown_and_protocol_only_text_is_not_translated() {
        assert_eq!(lookup("not in any table"), None);
        // 协议取值、方法名不查表。
        assert_eq!(lookup("offline"), None);
        assert_eq!(lookup("terminal.attach"), None);
    }
}
