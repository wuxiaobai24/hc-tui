//! 命令分派、布局配置档、面板与共享布局的文案。
//!
//! 键是源码里的英文原文，带占位符的统一用 `{}`。

pub fn lookup(en: &str) -> Option<&'static str> {
    Some(match en {
        "Rename" => "重命名",
        "rename agent" => "重命名 Agent",
        "pane name (empty clears the alias)" => "Pane 名称（留空恢复默认）",
        "agent name (empty clears the alias)" => "Agent 名称（留空恢复默认）",
        "unknown terminal {}" => "未知终端 {}",
        "Rename a pane" => "重命名 Pane",
        "Rename an agent terminal" => "重命名 Agent 终端",

        // 命令参数与目标校验，出错时直接显示在提示行
        "command arguments must be an object" => "命令参数必须是对象",
        "unknown space {}" => "未知工作区 {}",
        "unknown tab {}" => "未知 Tab {}",
        "unknown pane {}" => "未知 Pane {}",
        "space index out of range" => "工作区序号超出范围",
        "tab index out of range" => "Tab 序号超出范围",
        "index required" => "缺少序号",
        "tree required" => "缺少外框树",
        "invalid chrome tree" => "外框树不合法",
        "mods are disabled (--no-mods)" => "模块已关闭（--no-mods）",
        "no space" => "没有工作区",
        "no tab" => "没有 Tab",
        "no pane" => "没有 Pane",
        "no neighbor" => "没有相邻的 Pane",
        "terminal required" => "需要终端",
        "terminal has no open view" => "终端没有打开的视图",
        "terminal is not attached" => "终端未连接",
        "text required" => "缺少文本",
        "name required" => "缺少名称",
        "path required" => "缺少路径",
        "px required" => "缺少像素值",
        "layout required" => "缺少布局",
        "child or index required" => "缺少子 Pane 或序号",
        "child index out of range" => "子 Pane 序号超出范围",
        "shown tab is not a stack" => "当前 Tab 不是堆叠",
        "to must be program or menu" => "参数 to 必须是 program 或 menu",
        "unknown theme" => "未知主题",
        "Unknown command: {}" => "未知命令：{}",
        // 确认问句与重命名提示
        "End every terminal in this space? (y/N)" => "结束此工作区的全部终端？(y/N)",
        "End every terminal in this tab? (y/N)" => "结束此 Tab 的全部终端？(y/N)",
        "space name" => "工作区名称",
        "tab name" => "Tab 名称",
        // 终端右键菜单
        "Terminal" => "终端",
        "Peek" => "读取",
        "Keep as tab" => "保留为 Tab",
        "Park (keep running)" => "收起并保持运行",
        "Pin" => "置顶",
        "Unpin" => "取消置顶",
        "End program and sub-agents" => "结束程序与子 Agent",
        // 布局配置档与面板
        "Saved layout {}" => "已保存布局 {}",
        "profile names use letters, digits, - or _" => "配置档名称只能用字母、数字、- 或 _",
        "layout not ready" => "布局尚未就绪",
        "layout contains a terminal that is not attached yet" => "布局里有尚未连接的终端",
        "layout must use each existing tab pane at most once" => {
            "布局里每个现有 Tab 的 Pane 最多只能出现一次"
        }
        "layout needs at least one content block" => "布局至少需要一个内容块",
        "unknown panel {}" => "未知面板 {}",
        "Command timed out" => "命令超时",
        "Output truncated" => "输出已截断",
        "Register a [[panels]] command with this name in hc-tui/config.toml to display this GUI module in the terminal." => {
            "在 hc-tui/config.toml 里为此名称注册 [[panels]] 命令，即可在终端中显示这个 GUI 模块"
        }
        "Its content and props are preserved in the shared layout." => {
            "其内容和属性仍保留在共享布局中"
        }
        // 侧栏与共享布局
        "sidebar resized; {}" => "侧栏已调整；{}",
        "Cannot rebase layout: {}" => "无法合并布局变更：{}",
        "Shared layout: {}; use layout.reload to accept the shared version" => {
            "共享布局：{}；用 layout.reload 接受共享版本"
        }
        "Cannot import layout: {}" => "无法导入布局：{}",
        "{} layout unavailable: {}" => "{} 布局不可用：{}",
        "Local" => "本地",
        "Shared layout write failed: {}" => "共享布局写入失败：{}",
        // 侧栏分组
        "Needs you" => "需要你",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::lookup;
    #[test]
    fn dispatched_errors_and_titles_have_chinese_text() {
        assert_eq!(lookup("terminal required"), Some("需要终端"));
        assert_eq!(lookup("unknown space {}"), Some("未知工作区 {}"));
        assert_eq!(lookup("Saved layout {}"), Some("已保存布局 {}"));
        assert_eq!(lookup("Needs you"), Some("需要你"));
        assert_eq!(lookup("not in this table"), None);
    }
}
