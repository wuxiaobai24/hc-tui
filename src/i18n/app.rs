//! `src/app.rs` 的文案：提示行、浮层标题、帮助面板、状态与连接状态。
//!
//! 键是源码里的英文原文，值给中文界面用；带占位符的统一写 `{}`。

pub fn lookup(en: &str) -> Option<&'static str> {
    match en {
        // 连接状态与底部模式行
        "connecting" => Some("连接中"),
        "reconnecting" => Some("重连中"),
        "disconnected" => Some("已断开"),
        "COPY" => Some("复制"),
        "h/j/k/l w/b/e { } move  / ? search  n/N repeat  v/space select  y/enter copy  q/esc exit" => {
            Some("h/j/k/l w/b/e { } 移动  / ? 搜索  n/N 重复搜索  v/space 选择  y/enter 复制  q/esc 退出")
        }
        "RESIZE" => Some("调整尺寸"),
        "h/l width  j/k height  esc done" => Some("h/l 宽度  j/k 高度  esc 完成"),
        "MOVE" => Some("移动"),
        "1-9 target tab  t new tab  esc back" => Some("1-9 目标 Tab  t 新 Tab  esc 返回"),
        "NAVIGATE" => Some("导航"),
        "\u{2191}/\u{2193} workspace  1-9 switch  esc back" => {
            Some("↑/↓ 工作区  1-9 切换  esc 返回")
        }
        "PREFIX" => Some("前缀"),
        "? keybinds  s settings  c new tab  v/- split  hjkl focus  z zoom  x close  q detach" => {
            Some("? 键位  s 设置  c 新 Tab  v/- 分屏  hjkl 焦点  z 放大  x 关闭  q 断开")
        }

        // 提示行标题
        "command" => Some("命令"),
        "new tab" => Some("新建 Tab"),
        "new workspace" => Some("新建工作区"),
        "rename tab" => Some("重命名 Tab"),
        "rename workspace" => Some("重命名工作区"),
        "rename pane" => Some("重命名 Pane"),
        "send text" => Some("发送文本"),
        "send keys" => Some("发送按键"),
        "wait for output" => Some("等待输出"),
        "send file" => Some("发送文件"),
        "add machine" => Some("添加机器"),
        "rename machine" => Some("重命名机器"),
        "close tab" => Some("关闭 Tab"),
        "close workspace" => Some("关闭工作区"),
        "kill terminal" => Some("结束终端"),

        // 状态行提示
        "starting hc connect" => Some("正在连接 hc"),
        "Terminal theme: {}" => Some("终端主题：{}"),
        "core returned no layout; using the default" => Some("core 没有返回布局，改用默认布局"),
        "applied for this session; {}" => Some("本次会话已应用；{}"),
        "sort applied; {}" => Some("排序已应用；{}"),
        "tab name" => Some("Tab 名字"),
        "workspace name" => Some("工作区名字"),
        "close this tab and every pane in it? (y/N)" => {
            Some("关闭这个 Tab 和里面的所有 Pane？(y/N)")
        }
        "close workspace and all its tabs? (y/N)" => {
            Some("关闭工作区和它的所有 Tab？(y/N)")
        }
        "End the last terminal and close this tab? (y/N)" => {
            Some("结束最后一个终端并关闭这个 Tab？(y/N)")
        }
        "move: 1-{} 移到目标 tab，t 移成新 tab，Esc 取消" => {
            Some("移动：1-{} 移到目标 tab，t 移成新 tab，Esc 取消")
        }
        "bottom" => Some("已到底部"),
        "interrupting {}" => Some("正在中断 {}"),
        "cancelled" => Some("已取消"),
        "this pane has no terminal yet" => Some("这个 Pane 还没有终端"),
        "sending to {}" => Some("正在发送到 {}"),
        "sent {} keys" => Some("已发送 {} 个按键"),
        "waiting for: {}" => Some("等待输出包含：{}"),
        "sending {}" => Some("正在发送 {}"),
        "read failed: {}" => Some("读取失败：{}"),
        "adding machine {}" => Some("正在添加机器 {}"),
        "killing {}" => Some("正在结束 {}"),
        "floating split" => Some("浮动分屏"),
        "split" => Some("已分屏"),
        "layout · {}" => Some("布局 · {}"),
        "stacked {} panes · prefix+方向切换" => Some("已堆叠 {} 个 Pane · prefix+方向切换"),
        "{} keep = {}" => Some("{} 保留 = {}"),
        "{} is still running; kill it first with prefix shift+k" => {
            Some("{} 还在运行，先用 prefix shift+k 结束它")
        }
        "removed {}" => Some("已移除 {}"),
        "no link under cursor" => Some("光标下没有链接"),
        "opening {}" => Some("正在打开 {}"),
        "cannot open link: {}" => Some("打不开链接：{}"),
        "cannot write temp file: {}" => Some("写临时文件失败：{}"),
        "popup open" => Some("弹窗已打开"),
        "no agent needs attention" => Some("没有需要处理的 Agent"),
        "tab {}" => Some("Tab {}"),
        "workspace {}" => Some("工作区 {}"),
        "tab {} → {}" => Some("Tab {} → {}"),
        "invalid target tab" => Some("目标 Tab 无效"),
        "no room in the target tab" => Some("目标 Tab 里没有空间"),
        "pane moved to tab {}" => Some("Pane 已移到 Tab {}"),
        "pane moved into a new tab" => Some("Pane 已移到新 Tab"),
        "cannot create local VT: {}" => Some("无法创建本地终端：{}"),
        "running in background: {}" => Some("正在后台运行：{}"),
        "pane: {}" => Some("Pane：{}"),
        "no room: panes need at least {}x{}" => Some("没有空间：每格 Pane 至少要 {}x{}"),
        "popup: {}" => Some("弹窗：{}"),
        "popup closed; the program keeps running in core" => {
            Some("弹窗已关闭，程序仍在 core 里运行")
        }
        "reading {}…" => Some("正在读取 {}…"),
        "reader closed" => Some("读取窗口已关闭"),
        "removing machine" => Some("正在移除机器"),
        "connecting {}" => Some("正在连接 {}"),
        "opening a pane on {}" => Some("正在 {} 上打开 Pane"),
        "no room" => Some("没有空间"),
        "image was not uploaded: core accepts at most 8 MiB" => {
            Some("图片没有上传：core 最多接受 8 MiB")
        }
        "image was not uploaded: target terminal is not connected" => {
            Some("图片没有上传：目标终端没有连接")
        }
        "pasting clipboard image" => Some("正在粘贴剪贴板图片"),
        "subscribing" => Some("正在订阅"),
        "attached {} (resize notifications on)" => Some("已附加 {}（开启尺寸通知）"),
        "attached {}" => Some("已附加 {}"),
        // 插件终端入口
        "plugin terminal name is required" => Some("需要插件终端名"),
        "unknown plugin terminal {}" => Some("未知的插件终端 {}"),
        "space {} is gone" => Some("空间 {} 已不存在"),
        "terminal or name is required" => Some("需要终端地址或插件终端名"),
        "plugin terminal {} is gone" => Some("插件终端 {} 已不存在"),
        "terminal {} has no view in this client" => Some("本客户端没有终端 {} 的视图"),
        "no room to split" => Some("没有空间分屏"),
        "no room to float a pane" => Some("没有空间浮起 Pane"),
        "unknown placement {}" => Some("未知的放置方式 {}"),
        "plugin terminal {} opened" => Some("插件终端 {} 已打开"),
        "plugin terminal closed; {} generations {} reclaimed" => {
            Some("插件终端已关闭；{} 的代际 {} 已回收")
        }
        "popup ended: {}" => Some("弹窗结束：{}"),
        "Kill failed: {}" => Some("结束失败：{}"),
        "image attached · {}" => Some("图片已附加 · {}"),
        "image uploaded · chat closed · {}" => Some("图片已上传 · Chat 已关闭 · {}"),
        "image sent · {}" => Some("图片已发送 · {}"),
        "image uploaded · target pane closed · {}" => {
            Some("图片已上传 · 目标 Pane 已关闭 · {}")
        }
        "send image failed: core returned no file path" => {
            Some("图片发送失败：core 没有返回文件路径")
        }
        "send image failed: {}" => Some("图片发送失败：{}"),
        "cannot read layout: {}" => Some("读不到布局：{}"),
        "layout sync failed: {}" => Some("布局同步失败：{}"),
        "layout not saved: {}" => Some("布局没有保存：{}"),
        "read · {}" => Some("读取 · {}"),
        "read · {} · {} 行" => Some("读取 · {} · {} 行"),
        "read · {} · 部分内容" => Some("读取 · {} · 部分内容"),
        "screen" => Some("屏幕"),
        "history" => Some("历史"),
        "keys delivered · {} · turn {}" => Some("按键已发送 · {} · 轮次 {}"),
        "keys failed: {}" => Some("发送按键失败：{}"),
        "wait failed: {}" => Some("等待失败：{}"),
        "file delivered to {}" => Some("文件已送达 {}"),
        "send file failed: {}" => Some("发送文件失败：{}"),
        "templates（Enter 在新 tab 里运行）" => Some("模板（Enter 在新 tab 里运行）"),
        "templates failed: {}" => Some("模板列表失败：{}"),
        "Command timed out" => Some("命令超时"),
        "Command finished · exit {}" => Some("命令结束 · 退出码 {}"),
        "Workspace" => Some("工作区"),
        "machines · Enter/n 新 pane · a 添加 · c 连接 · r 改名 · d 移除" => {
            Some("机器 · Enter/n 新 pane · a 添加 · c 连接 · r 改名 · d 移除")
        }
        "machine list failed: {}" => Some("机器列表失败：{}"),
        "added {}" => Some("已添加 {}"),
        "add failed: {}" => Some("添加失败：{}"),
        "machine removed" => Some("机器已移除"),
        "remove failed: {}" => Some("移除失败：{}"),
        "machine state {}" => Some("机器状态 {}"),
        "connect failed: {}" => Some("连接失败：{}"),
        "machine renamed" => Some("机器已改名"),
        "rename failed: {}" => Some("改名失败：{}"),
        "sent; waiting for the turn to start" => Some("已发送，等待轮次开始"),
        "send failed: {}" => Some("发送失败：{}"),
        "interrupted · state {}" => Some("已中断 · 状态 {}"),
        "interrupt failed: {}" => Some("中断失败：{}"),
        "keep = {}" => Some("保留 = {}"),
        "keep failed: {}" => Some("保留设置失败：{}"),
        "terminal record removed" => Some("终端记录已移除"),
        "program finished; prefix c starts a new tab" => Some("程序已结束，prefix c 开新 Tab"),
        "kill failed: {}" => Some("结束失败：{}"),
        "resynced after missed events" => Some("漏掉事件后已重新同步"),
        "{} needs attention (prefix o)" => Some("{} 需要处理（prefix o）"),
        "pane {}" => Some("Pane {}"),
        "{} / Other terminals" => Some("{} / 其它终端"),

        // 侧栏分组
        "Needs you" => Some("需要你"),
        "Done" => Some("完成"),
        "Working" => Some("运行中"),
        "Idle" => Some("空闲"),

        // 设置面板分段
        "theme" => Some("主题"),
        "indicators" => Some("状态图标"),
        "toasts" => Some("通知"),
        "sound" => Some("声音"),

        // Keybinds 面板：分组标题、动作说明、复制模式按键
        "global" => Some("全局"),
        "navigation" => Some("导航"),
        "panes" => Some("Pane"),
        "tabs and workspaces" => Some("Tab 和工作区"),
        "agents and terminals" => Some("Agent 和终端"),
        "matches" => Some("匹配结果"),
        "prefix mode" => Some("前缀模式"),
        "keybinds" => Some("键位"),
        "settings" => Some("设置"),
        "detach" => Some("断开"),
        "reload config" => Some("重新加载配置"),
        "open notification target" => Some("打开通知目标"),
        "search agents and terminals" => Some("搜索 Agent 和终端"),
        "next tab" => Some("下一个 Tab"),
        "previous tab" => Some("上一个 Tab"),
        "move tab toward front" => Some("Tab 前移"),
        "move tab toward back" => Some("Tab 后移"),
        "workspace picker" => Some("工作区选择"),
        "next workspace" => Some("下一个工作区"),
        "previous workspace" => Some("上一个工作区"),
        "split vertical" => Some("垂直分屏"),
        "split horizontal" => Some("水平分屏"),
        "focus pane left" => Some("焦点移到左边的 Pane"),
        "focus pane down" => Some("焦点移到下面的 Pane"),
        "focus pane up" => Some("焦点移到上面的 Pane"),
        "focus pane right" => Some("焦点移到右边的 Pane"),
        "cycle pane next" => Some("循环到下一个 Pane"),
        "cycle pane previous" => Some("循环到上一个 Pane"),
        "last pane" => Some("上一个 Pane"),
        "zoom" => Some("放大"),
        "stack panes" => Some("堆叠 Pane"),
        "next layout preset" => Some("下一个布局预设"),
        "previous layout preset" => Some("上一个布局预设"),
        "float pane" => Some("浮动 Pane"),
        "pin floating pane" => Some("置顶浮动 Pane"),
        "hide floating panes" => Some("收起浮动 Pane"),
        "close pane" => Some("关闭 Pane"),
        "move pane" => Some("移动 Pane"),
        "resize mode" => Some("调整尺寸模式"),
        "toggle sidebar" => Some("开关侧栏"),
        "scroll to bottom" => Some("滚到底部"),
        "copy mode" => Some("复制模式"),
        "next agent in displayed order" => Some("按显示顺序切到下一个 Agent"),
        "chat view of the agent session" => Some("Agent 会话的 Chat 视图"),
        "send text to pane" => Some("发送文本到 Pane"),
        "interrupt turn" => Some("中断轮次"),
        "toggle keep" => Some("开关保留"),
        "remove finished terminal" => Some("移除已结束的终端"),
        "open link" => Some("打开链接"),
        "edit scrollback" => Some("编辑回滚缓冲"),
        "read pane" => Some("读取 Pane"),
        "send named keys" => Some("发送具名按键"),
        "templates" => Some("模板"),
        "machines" => Some("机器"),
        "paste image" => Some("粘贴图片"),
        "move" => Some("移动"),
        "word" => Some("词"),
        "paragraph" => Some("段落"),
        "search" => Some("搜索"),
        "repeat search" => Some("重复搜索"),
        "select" => Some("选择"),
        "copy" => Some("复制"),
        "exit" => Some("退出"),
        "not found" => Some("没找到"),
        "copy mode closed" => Some("复制模式已关闭"),
        "block selection" => Some("块选择"),
        "line selection" => Some("行选择"),
        "other clients" => Some("其他客户端"),
        "command + JSON arguments" => Some("命令加 JSON 参数"),
        "Command socket: {}" => Some("命令套接字：{}"),
        "blocked" => Some("需要你"),
        "error" => Some("出错"),
        "done" => Some("完成"),
        "working" => Some("工作中"),
        "idle" => Some("空闲"),
        "unknown" => Some("未知"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn resolves_plain_labels() {
        assert_eq!(lookup("connecting"), Some("连接中"));
        assert_eq!(lookup("Needs you"), Some("需要你"));
    }

    #[test]
    fn resolves_format_templates_unchanged() {
        assert_eq!(lookup("attached {}"), Some("已附加 {}"));
        assert_eq!(
            lookup("keys delivered · {} · turn {}"),
            Some("按键已发送 · {} · 轮次 {}")
        );
    }

    #[test]
    fn resolves_help_rows() {
        assert_eq!(lookup("copy mode"), Some("复制模式"));
        assert_eq!(lookup("tabs and workspaces"), Some("Tab 和工作区"));
    }

    #[test]
    fn leaves_unknown_source_text_alone() {
        assert_eq!(lookup("snapshot"), None);
        assert_eq!(lookup("herdr.pinned"), None);
    }
}
