//! 渲染层文案：侧栏、Tab 栏、浮层、Chat 视图里画到屏幕上的字。

pub fn lookup(en: &str) -> Option<&'static str> {
    match en {
        "Swap pane" => Some("交换窗格"),
        " Find " => Some(" 查找 "),
        "Actions" => Some("操作"),
        "Back" => Some("返回"),
        "Keybindings" => Some("键盘方案"),
        "Keybindings: default / Zellij" => Some("键盘方案：默认 / Zellij"),
        "Files" => Some("文件"),
        "Changes" => Some("变更"),
        "Wider" => Some("加宽"),
        "Narrower" => Some("缩窄"),
        "Taller" => Some("增高"),
        "Shorter" => Some("缩短"),
        "Done" => Some("完成"),
        " Confirm " => Some(" 确认 "),
        " Save " => Some(" 保存 "),
        " Cancel " => Some(" 取消 "),
        " Copy " => Some(" 复制 "),
        " Search " => Some(" 搜索 "),
        " Done " => Some(" 完成 "),
        " Open " => Some(" 打开 "),
        " Add " => Some(" 添加 "),
        " Connect " => Some(" 连接 "),
        " Rename " => Some(" 重命名 "),
        " Remove " => Some(" 移除 "),
        "Drag to another pane to swap, or to a tab to move" => {
            Some("拖到另一窗格交换位置，拖到 Tab 移动")
        }
        // 手机顶栏与状态行。
        " close " => Some(" 关闭 "),
        " switch " => Some(" 切换 "),
        " no workspace" => Some(" 无工作区"),
        "  tab {}/{}" => Some("  Tab {}/{}"),
        // 手机面板的分区标题。
        "workspaces" => Some("工作区"),
        "tabs" => Some("Tab"),
        "panes" => Some("Pane"),
        "agents" => Some("Agent"),
        "menu" => Some("菜单"),
        "  (no tabs)" => Some("  （无 Tab）"),
        "  (no panes)" => Some("  （无 Pane）"),
        "floating" => Some("浮动"),
        // Tab 栏右侧的条目。
        "zoom" => Some("放大"),
        // 工作区摘要：英文分单复数，中文一种说法。
        "{} tab · {} pane" => Some("{} Tab · {} Pane"),
        "{} tab · {} panes" => Some("{} Tab · {} Pane"),
        "{} tabs · {} pane" => Some("{} Tab · {} Pane"),
        "{} tabs · {} panes" => Some("{} Tab · {} Pane"),
        // 侧栏。
        " Spaces" => Some(" 工作区"),
        "This computer" => Some("本机"),
        " + New space there" => Some(" + 新建工作区"),
        " ↻ Reconnect" => Some(" ↻ 重连"),
        " new" => Some(" 新建"),
        " menu " => Some(" 菜单 "),
        "grouped" => Some("分组"),
        "priority" => Some("优先级"),
        " Agents" => Some(" Agent"),
        "  (no agents)" => Some("  （无 Agent）"),
        // Pane 标题。
        "pin " => Some("置顶 "),
        "closed: {}" => Some("已关闭 {}"),
        " starting " => Some(" 启动中 "),
        "popup" => Some("弹窗"),
        "VT render failed\n\nuse `hc read` to look at it" => {
            Some("VT 渲染失败\n\n用 `hc read` 查看内容")
        }
        // 输入提示框。
        "↵ confirm    esc cancel" => Some("↵ 确认    esc 取消"),
        "↵ save    ^c clear    esc cancel" => Some("↵ 保存    ^c 清空    esc 取消"),
        // 键位浮层。
        " keybinds" => Some(" 键位"),
        "esc close" => Some("esc 关闭"),
        "preset {}" => Some("预设 {}"),
        " press / to filter by command or shortcut" => Some(" 按 / 输入命令或快捷键筛选"),
        "search / · scroll j/k/↑↓/pgup/pgdn · close esc/enter" => {
            Some("搜索 / · 滚动 j/k/↑↓/pgup/pgdn · 关闭 esc/enter")
        }
        // 设置浮层。
        " Settings" => Some(" 设置"),
        " ↑↓ select   ←→ section   ↵ apply   esc close" => {
            Some(" ↑↓ 选择   ←→ 分区   ↵ 应用   esc 关闭")
        }
        " ↑↓ select  tab section  ↵ apply  esc close" => {
            Some(" ↑↓ 选择  tab 分区  ↵ 应用  esc 关闭")
        }
        "symbols" => Some("符号"),
        "dots" => Some("圆点"),
        "off" => Some("关闭"),
        "on" => Some("开启"),
        "toast" => Some("通知"),
        "system" => Some("系统"),
        // 跳转 / 命令面板。
        "commands" => Some("命令"),
        "terminals" => Some("终端"),
        "─ Commands " => Some("─ 命令 "),
        "─ Go to " => Some("─ 跳转 "),
        "search commands" => Some("搜索命令"),
        "search agents and terminals" => Some("搜索 Agent 与终端"),
        " No matching agents or terminals" => Some(" 没有匹配的 Agent 或终端"),
        " / search · ↑↓ rows · enter open · esc close" => {
            Some(" / 搜索 · ↑↓ 行 · enter 打开 · esc 关闭")
        }
        // Chat 视图顶栏。
        "chat · {} · {}" => Some("聊天 · {} · {}"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn translates_sidebar_labels() {
        assert_eq!(lookup(" Spaces"), Some(" 工作区"));
        assert_eq!(lookup(" Agents"), Some(" Agent"));
        assert_eq!(lookup("This computer"), Some("本机"));
        assert_eq!(lookup(" ↻ Reconnect"), Some(" ↻ 重连"));
    }

    #[test]
    fn translates_placeholders() {
        assert_eq!(lookup("closed: {}"), Some("已关闭 {}"));
        assert_eq!(lookup("chat · {} · {}"), Some("聊天 · {} · {}"));
        assert_eq!(lookup("  tab {}/{}"), Some("  Tab {}/{}"));
    }

    #[test]
    fn translates_every_plural_variant() {
        for key in [
            "{} tab · {} pane",
            "{} tab · {} panes",
            "{} tabs · {} pane",
            "{} tabs · {} panes",
        ] {
            assert_eq!(lookup(key), Some("{} Tab · {} Pane"), "{key}");
        }
    }

    #[test]
    fn unknown_text_is_left_alone() {
        assert_eq!(lookup("accent"), None);
        assert_eq!(lookup("catppuccin"), None);
    }
}
