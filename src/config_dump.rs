//! 配置的整份序列化：`config dump` 与 `--default-config` 的唯一实现。
//!
//! 取值一律来自传入的 [`Config`]（默认值由同一批 getter 作用于
//! `Config::default()` 得出），不另存常量表。段的顺序固定，跟
//! `docs/configuration.md` 的小节顺序一致；运行时字段（`config_path` /
//! `state_path` / `warnings` / `errors` / `positions` / `theme_name_position`）
//! 不写盘。注释一律单独一行，避免值里的 `#` 被当成注释截断。

use std::collections::BTreeMap;

use crate::config::{
    AgentSort, CollapsedSidebar, CommandKind, Config, Delivery, HostCursor, Indicators,
    KeyboardProtocol, Lang, NewCwd, PaneBorders, Span, TabBarPosition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 当前生效的配置。
    Effective,
    /// 内置默认值，与仓库里的 `docs/default-config.toml` 逐字节相同。
    Defaults,
    /// 只输出与默认值不同的键。
    Minimal,
}

/// 一个配置项的取值函数：`None` 表示这个可选键没有设置。
type Getter = fn(&Config) -> Option<String>;

struct KeyDef {
    key: &'static str,
    get: Getter,
    /// 用途与合法值；空字符串表示不额外解释（`[keys]` 的绑定）。
    note: &'static str,
}

struct Section {
    name: &'static str,
    doc: &'static [&'static str],
    keys: &'static [KeyDef],
}

/// 生成整份配置文本。
pub fn dump(config: &Config, mode: Mode) -> String {
    let defaults = Config::default();
    let source = if mode == Mode::Defaults {
        &defaults
    } else {
        config
    };
    let mut out = String::new();
    out.push_str(&header(mode));
    for section in [&UI, &TERMINAL, &RENDER, &THEME] {
        emit_section(&mut out, section, source, &defaults, mode);
    }
    emit_theme_custom(&mut out, source);
    for section in [&NOTIFICATIONS, &SESSION] {
        emit_section(&mut out, section, source, &defaults, mode);
    }
    emit_keys(&mut out, source, &defaults, mode);
    emit_commands(&mut out, source);
    emit_panels(&mut out, source);
    emit_section(&mut out, &MODS, source, &defaults, mode);
    out
}

fn header(mode: Mode) -> String {
    match mode {
        Mode::Effective => {
            "# hc-tui 当前生效配置\n# 由 `hc-tui config dump` 生成；用法见 docs/configuration.md\n"
                .to_string()
        }
        Mode::Defaults => concat!(
            "# hc-tui 默认配置\n",
            "# 由 `hc-tui config dump --defaults` 生成，与代码同源；勿手改。\n",
            "# 重新生成：HC_UPDATE_DEFAULT_CONFIG=1 cargo test -- config_default_file_is_current\n",
            "# 放到 ~/.config/hc-tui/config.toml 或用 `hc-tui config init` 写入。\n",
        )
        .to_string(),
        Mode::Minimal => {
            "# hc-tui 与默认值的差异\n# 由 `hc-tui config dump --minimal` 生成；未列出的键都是默认值\n"
                .to_string()
        }
    }
}

fn items(section: &Section, source: &Config, defaults: &Config) -> Vec<Item> {
    section
        .keys
        .iter()
        .map(|key| Item {
            key: key.key.to_string(),
            value: (key.get)(source),
            default: (key.get)(defaults),
            note: key.note,
        })
        .collect()
}

struct Item {
    key: String,
    value: Option<String>,
    default: Option<String>,
    note: &'static str,
}

impl Item {
    fn kept(&self, mode: Mode) -> bool {
        match mode {
            Mode::Minimal => self.value != self.default,
            _ => self.value.is_some(),
        }
    }

    fn default_text(&self) -> String {
        self.default.clone().unwrap_or_else(|| "未设置".to_string())
    }
}

fn emit_section(
    out: &mut String,
    section: &Section,
    source: &Config,
    defaults: &Config,
    mode: Mode,
) {
    let items = items(section, source, defaults);
    let kept: Vec<&Item> = items.iter().filter(|item| item.kept(mode)).collect();
    if kept.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str(&format!("[{}]\n", section.name));
    if mode != Mode::Minimal {
        for line in section.doc {
            out.push_str("# ");
            out.push_str(line);
            out.push('\n');
        }
    }
    for item in kept {
        emit_item(out, item);
    }
}

fn emit_item(out: &mut String, item: &Item) {
    let Some(value) = &item.value else {
        return;
    };
    if item.note.is_empty() {
        if item.value != item.default {
            out.push_str(&format!("# 默认 {}\n", item.default_text()));
        }
    } else {
        match &item.default {
            Some(default) => out.push_str(&format!("# {}，默认 {default}\n", item.note)),
            None => out.push_str(&format!("# {}，默认未设置\n", item.note)),
        }
    }
    out.push_str(&format!("{} = {value}\n", item.key));
}

/// `[keys]`：预设加全部生效绑定；改过的绑定带默认值提示。
fn emit_keys(out: &mut String, source: &Config, defaults: &Config, mode: Mode) {
    let preset = |config: &Config| {
        Some(quote(if config.zellij_keys {
            "zellij"
        } else {
            "default"
        }))
    };
    let preset = Item {
        key: "preset".to_string(),
        value: preset(source),
        default: preset(defaults),
        note: "键位预设（default / zellij）",
    };
    let default_bindings: BTreeMap<&str, String> = defaults
        .keys
        .entries()
        .into_iter()
        .map(|(name, binding)| (name, binding.to_string()))
        .collect();
    let bindings: Vec<Item> = source
        .keys
        .entries()
        .into_iter()
        .map(|(name, binding)| Item {
            key: name.to_string(),
            value: Some(binding.to_string()),
            default: default_bindings.get(name).cloned(),
            note: "",
        })
        .collect();
    let kept: Vec<&Item> = std::iter::once(&preset)
        .chain(bindings.iter())
        .filter(|item| item.kept(mode))
        .collect();
    if kept.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str("[keys]\n");
    if mode != Mode::Minimal {
        out.push_str(
            "# 全部生效绑定：prefix+.. 需要先按前缀键，直接写 ctrl+alt+.. 表示终端模式下生效。\n",
        );
        out.push_str("# 同一按键绑到多个动作会告警，用 `hc-tui config check` 查看。\n");
    }
    for item in kept {
        emit_item(out, item);
    }
}

fn emit_commands(out: &mut String, source: &Config) {
    for command in &source.commands {
        out.push_str("\n[[keys.command]]\n");
        out.push_str(&format!("key = {}\n", command.key));
        out.push_str(&format!("type = {}\n", quote(kind_text(command.kind))));
        out.push_str(&format!("command = {}\n", quote(&command.command)));
        if let Some(width) = command.width {
            out.push_str(&format!("width = {}\n", span_text(width)));
        }
        if let Some(height) = command.height {
            out.push_str(&format!("height = {}\n", span_text(height)));
        }
    }
}

fn emit_panels(out: &mut String, source: &Config) {
    for panel in &source.panels {
        out.push_str("\n[[panels]]\n");
        out.push_str("# 脚本面板：也可以放在配置目录的 panels/*.json。\n");
        out.push_str(&format!("name = {}\n", quote(&panel.name)));
        out.push_str(&format!("command = {}\n", quote(&panel.command)));
        out.push_str(&format!("refresh = {}\n", panel.refresh));
    }
}

fn span_text(span: Span) -> String {
    match span {
        Span::Cells(cells) => cells.to_string(),
        Span::Percent(percent) => quote(&format!("{percent}%")),
    }
}

fn kind_text(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Shell => "shell",
        CommandKind::Pane => "pane",
        CommandKind::Popup => "popup",
        CommandKind::Action => "action",
    }
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|value| quote(value)).collect();
    format!("[{}]", items.join(", "))
}

macro_rules! get {
    ($name:ident, |$config:ident| $body:expr) => {
        fn $name($config: &Config) -> Option<String> {
            $body
        }
    };
}

get!(sidebar, |config| Some(config.sidebar.to_string()));
get!(sidebar_width, |config| Some(
    config.sidebar_width.to_string()
));
get!(sidebar_min_width, |config| Some(
    config.sidebar_min_width.to_string()
));
get!(sidebar_max_width, |config| Some(
    config.sidebar_max_width.to_string()
));
get!(mobile_width_threshold, |config| Some(
    config.mobile_width_threshold.to_string()
));
get!(lang, |config| Some(quote(lang_text(config.lang))));
get!(tab_bar_position, |config| Some(quote(
    tab_bar_position_text(config.tab_bar_position)
)));
get!(pane_borders, |config| Some(quote(pane_borders_text(
    config.pane_borders
))));
get!(pane_gaps, |config| Some(config.pane_gaps.to_string()));
get!(pane_outer_borders, |config| Some(
    config.pane_outer_borders.to_string()
));
get!(pane_scrollbars, |config| Some(
    config.pane_scrollbars.to_string()
));
get!(show_agent_labels_on_pane_borders, |config| Some(
    config.show_agent_labels_on_pane_borders.to_string()
));
get!(hide_tab_bar_when_single_tab, |config| Some(
    config.hide_tab_bar_when_single_tab.to_string()
));
get!(keyboard_protocol, |config| Some(quote(
    keyboard_protocol_text(config.keyboard_protocol)
)));
get!(mouse_capture, |config| Some(
    config.mouse_capture.to_string()
));
get!(mouse_scroll_lines, |config| Some(
    config.mouse_scroll_lines.to_string()
));
get!(copy_on_select, |config| Some(
    config.copy_on_select.to_string()
));
get!(confirm_open_link, |config| Some(
    config.confirm_open_link.to_string()
));
get!(confirm_close, |config| Some(
    config.confirm_close.to_string()
));
get!(collapsed_sidebar, |config| Some(quote(collapsed_text(
    config.collapsed_sidebar
))));
get!(host_cursor, |config| Some(quote(host_cursor_text(
    config.host_cursor
))));
get!(indicators, |config| Some(quote(indicators_text(
    config.indicators
))));
get!(agent_sort, |config| Some(quote(agent_sort_text(
    config.agent_sort
))));
get!(prompt_new_tab, |config| Some(
    config.prompt_new_tab.to_string()
));
get!(prompt_new_workspace, |config| Some(
    config.prompt_new_workspace.to_string()
));
get!(window_title, |config| Some(quote(&config.window_title)));
get!(tab_bar_right, |config| Some(array(&config.tab_bar_right)));
get!(tab_bar_right_separator, |config| Some(quote(
    &config.tab_bar_right_separator
)));
get!(new_cwd, |config| Some(quote(&new_cwd_text(
    &config.new_cwd
))));
get!(default_shell, |config| Some(quote(
    config.default_shell.as_str()
)));
get!(scrollback_bytes, |config| Some(
    config.scrollback_bytes.to_string()
));
get!(render_fps, |config| Some(config.render_fps.to_string()));
get!(theme_name, |config| Some(quote(&config.theme.name)));
get!(theme_source, |config| Some(quote(&config.theme_source)));
get!(delivery, |config| Some(quote(delivery_text(
    config.notifications
))));
get!(toast_limit, |config| Some(config.toast_limit.to_string()));
get!(sound, |config| Some(config.sound.to_string()));
get!(sound_path, |config| config.sound_path.as_deref().map(quote));
get!(sound_done_path, |config| config
    .sound_done_path
    .as_deref()
    .map(quote));
get!(sound_request_path, |config| config
    .sound_request_path
    .as_deref()
    .map(quote));
get!(restore, |config| Some(config.restore.to_string()));
get!(session, |config| config.session.as_deref().map(quote));
get!(mods_trust_roots, |config| Some(array(
    &config.mods_trust_roots
)));
get!(mods_revocations, |config| config
    .mods_revocations
    .as_deref()
    .map(quote));

fn lang_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Auto => "auto",
        Lang::En => "en",
        Lang::Zh => "zh",
    }
}

fn tab_bar_position_text(position: TabBarPosition) -> &'static str {
    match position {
        TabBarPosition::Top => "top",
        TabBarPosition::Bottom => "bottom",
    }
}

fn pane_borders_text(borders: PaneBorders) -> &'static str {
    match borders {
        PaneBorders::Auto => "auto",
        PaneBorders::Always => "always",
        PaneBorders::Titles => "titles",
        PaneBorders::Off => "off",
    }
}

fn keyboard_protocol_text(protocol: KeyboardProtocol) -> &'static str {
    match protocol {
        KeyboardProtocol::Auto => "auto",
        KeyboardProtocol::Kitty => "kitty",
        KeyboardProtocol::Legacy => "legacy",
    }
}

fn collapsed_text(mode: CollapsedSidebar) -> &'static str {
    match mode {
        CollapsedSidebar::Compact => "compact",
        CollapsedSidebar::Hidden => "hidden",
    }
}

fn host_cursor_text(cursor: HostCursor) -> &'static str {
    match cursor {
        HostCursor::Native => "native",
        HostCursor::Drawn => "drawn",
    }
}

fn indicators_text(indicators: Indicators) -> &'static str {
    match indicators {
        Indicators::Dots => "dots",
        Indicators::Symbols => "symbols",
    }
}

fn agent_sort_text(sort: AgentSort) -> &'static str {
    match sort {
        AgentSort::Spaces => "spaces",
        AgentSort::Priority => "priority",
    }
}

fn delivery_text(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Off => "off",
        Delivery::Toast => "toast",
        Delivery::System => "system",
    }
}

fn new_cwd_text(new_cwd: &NewCwd) -> String {
    match new_cwd {
        NewCwd::Follow => "follow".to_string(),
        NewCwd::Home => "home".to_string(),
        NewCwd::Current => "current".to_string(),
        NewCwd::Path(path) => path.clone(),
    }
}

const UI: Section = Section {
    name: "ui",
    doc: &["界面、布局与输入。布尔项写 true / false；宽度单位是列。"],
    keys: &[
        KeyDef {
            key: "sidebar",
            get: sidebar,
            note: "侧栏是否显示",
        },
        KeyDef {
            key: "sidebar_width",
            get: sidebar_width,
            note: "侧栏初始宽度（列），实际值夹在 min..=max 之间",
        },
        KeyDef {
            key: "sidebar_min_width",
            get: sidebar_min_width,
            note: "侧栏最小宽度（列）",
        },
        KeyDef {
            key: "sidebar_max_width",
            get: sidebar_max_width,
            note: "侧栏最大宽度（列）",
        },
        KeyDef {
            key: "mobile_width_threshold",
            get: mobile_width_threshold,
            note: "终端宽度不超过此值时进入手机单列布局（0 关闭）",
        },
        KeyDef {
            key: "lang",
            get: lang,
            note: "界面语言（auto / en / zh）",
        },
        KeyDef {
            key: "tab_bar_position",
            get: tab_bar_position,
            note: "Tab 栏位置（top / bottom）",
        },
        KeyDef {
            key: "pane_borders",
            get: pane_borders,
            note: "Pane 边框策略（auto / always / titles / off）",
        },
        KeyDef {
            key: "pane_gaps",
            get: pane_gaps,
            note: "Pane 之间留空隙",
        },
        KeyDef {
            key: "pane_outer_borders",
            get: pane_outer_borders,
            note: "沿 Pane 区域外沿画分隔线",
        },
        KeyDef {
            key: "pane_scrollbars",
            get: pane_scrollbars,
            note: "在焦点 Pane 右缘画滚动条",
        },
        KeyDef {
            key: "show_agent_labels_on_pane_borders",
            get: show_agent_labels_on_pane_borders,
            note: "边框上显示检测到的 Agent 名字",
        },
        KeyDef {
            key: "hide_tab_bar_when_single_tab",
            get: hide_tab_bar_when_single_tab,
            note: "只有一个 Tab 时隐藏 Tab 栏",
        },
        KeyDef {
            key: "keyboard_protocol",
            get: keyboard_protocol,
            note: "宿主键盘协议（auto / kitty / legacy）",
        },
        KeyDef {
            key: "mouse_capture",
            get: mouse_capture,
            note: "捕获鼠标事件",
        },
        KeyDef {
            key: "mouse_scroll_lines",
            get: mouse_scroll_lines,
            note: "鼠标滚轮一次滚动的行数（至少 1）",
        },
        KeyDef {
            key: "copy_on_select",
            get: copy_on_select,
            note: "选中即复制",
        },
        KeyDef {
            key: "confirm_open_link",
            get: confirm_open_link,
            note: "打开 OSC 8 链接前先让用户输入 y 确认",
        },
        KeyDef {
            key: "confirm_close",
            get: confirm_close,
            note: "关闭前确认",
        },
        KeyDef {
            key: "sidebar_collapsed_mode",
            get: collapsed_sidebar,
            note: "侧栏折叠形态（compact / hidden）",
        },
        KeyDef {
            key: "host_cursor",
            get: host_cursor,
            note: "光标绘制方式（native / drawn）",
        },
        KeyDef {
            key: "status_indicators",
            get: indicators,
            note: "状态指示样式（dots / symbols）",
        },
        KeyDef {
            key: "agent_panel_sort",
            get: agent_sort,
            note: "Agents 段排序（spaces / priority）",
        },
        KeyDef {
            key: "prompt_new_tab_name",
            get: prompt_new_tab,
            note: "新建 Tab 时先问名字",
        },
        KeyDef {
            key: "prompt_new_workspace_name",
            get: prompt_new_workspace,
            note: "新建工作区时先问名字",
        },
        KeyDef {
            key: "window_title",
            get: window_title,
            note: "写给宿主终端的窗口标题模板，空字符串表示不改标题",
        },
        KeyDef {
            key: "tab_bar_right",
            get: tab_bar_right,
            note: "Tab 栏右侧显示的 token 列表",
        },
        KeyDef {
            key: "tab_bar_right_separator",
            get: tab_bar_right_separator,
            note: "右侧 token 之间的分隔符",
        },
    ],
};

const TERMINAL: Section = Section {
    name: "terminal",
    doc: &["新建终端与 scrollback。"],
    keys: &[
        KeyDef {
            key: "new_cwd",
            get: new_cwd,
            note: "新建 Pane 的起始目录（follow / home / current / 固定路径）",
        },
        KeyDef {
            key: "default_shell",
            get: default_shell,
            note: "新建 Pane 用的可执行文件，空表示交给 core 的默认 shell",
        },
        KeyDef {
            key: "scrollback_bytes",
            get: scrollback_bytes,
            note: "每个终端保留的 scrollback 字节数",
        },
    ],
};

const RENDER: Section = Section {
    name: "render",
    doc: &["渲染节流。"],
    keys: &[KeyDef {
        key: "fps",
        get: render_fps,
        note: "渲染帧率上限（1..=240）",
    }],
};

const THEME: Section = Section {
    name: "theme",
    doc: &["主题。name 是内置主题名，或 auto / omarchy / ghostty 跟随终端主题。"],
    keys: &[
        KeyDef {
            key: "name",
            get: theme_name,
            note: "主题名（内置主题名 / auto / omarchy / ghostty）",
        },
        KeyDef {
            key: "source",
            get: theme_source,
            note: "跟随来源（auto / ghostty / omarchy / builtin），只在 name = \"default\" 时生效",
        },
    ],
};

const NOTIFICATIONS: Section = Section {
    name: "notifications",
    doc: &["通知投递。sound_path / done_path / request_path 未设置时用内置声音。"],
    keys: &[
        KeyDef {
            key: "delivery",
            get: delivery,
            note: "投递方式（off / toast / system）",
        },
        KeyDef {
            key: "toast_limit",
            get: toast_limit,
            note: "提示表面同时保留的条数（1..=20）",
        },
        KeyDef {
            key: "sound",
            get: sound,
            note: "播放通知声音",
        },
        KeyDef {
            key: "sound_path",
            get: sound_path,
            note: "通知音文件路径",
        },
        KeyDef {
            key: "done_path",
            get: sound_done_path,
            note: "完成提示音文件路径",
        },
        KeyDef {
            key: "request_path",
            get: sound_request_path,
            note: "请求提示音文件路径",
        },
    ],
};

const SESSION: Section = Section {
    name: "session",
    doc: &["启动时的会话行为。"],
    keys: &[
        KeyDef {
            key: "restore",
            get: restore,
            note: "启动时从 core 的 doc 恢复布局",
        },
        KeyDef {
            key: "session",
            get: session,
            note: "目标 hc session，未设置时用环境里的 HC_SESSION",
        },
    ],
};

const MODS: Section = Section {
    name: "mods",
    doc: &["模块签名。"],
    keys: &[
        KeyDef {
            key: "trust_roots",
            get: mods_trust_roots,
            note: "签名包信任根列表（base64 Ed25519 公钥或 .pub 文件路径）",
        },
        KeyDef {
            key: "revocations",
            get: mods_revocations,
            note: "撤销列表 JSON 路径，未设置时读配置目录的 mods-revocations.json",
        },
    ],
};

/// 主题覆盖的 token 与颜色：`[theme.custom]`，紧跟 `[theme]` 段。
fn emit_theme_custom(out: &mut String, source: &Config) {
    if source.theme_custom.is_empty() {
        return;
    }
    out.push_str(
        "\n[theme.custom]\n# 覆盖主题 token 的颜色，token 列表见 docs/configuration.md。\n",
    );
    for (token, value) in &source.theme_custom {
        out.push_str(&format!("{token} = {}\n", quote(value)));
    }
}
