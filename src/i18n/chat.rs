//! 会话视图的文案表：`src/chat/` 下渲染进 Chat 视图或作为提示行的文字。

pub fn lookup(en: &str) -> Option<&'static str> {
    Some(match en {
        // bash 工具卡的结果摘要。
        "exit {}" => "退出码 {}",
        // 会话文件或工具名缺失时的兜底名字。
        "tool" => "工具",
        "session" => "会话",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn chat_strings_have_chinese_text() {
        assert_eq!(lookup("exit {}"), Some("退出码 {}"));
        assert_eq!(lookup("tool"), Some("工具"));
        assert_eq!(lookup("session"), Some("会话"));
        assert_eq!(lookup("pane 没有 cwd"), None);
    }
}
