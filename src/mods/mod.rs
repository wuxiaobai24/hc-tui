//! 界面模块（mods）运行时。
//!
//! 外框（chrome）是本机配置的区域树，见 [`chrome`]；[`ModRuntime`] 管理模块
//! 表面的实例、代际、授予与请求。`--no-mods` 时使用内置外框，不加载任何模块。

pub mod check;
pub mod chrome;
pub mod host;
pub mod input;
pub mod render;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use nsh_platform_tui::{AppOptions, AppView};
use ratatui_core::layout::Rect;
use serde_json::{json, Value};

/// 沙箱限制：内存 128 MiB、调用 500 ms、加载 10 s、节点 200000。
/// 宿主按实例设置；配置只能下调。
pub fn embedded_limits() -> nsh_platform_tui::Limits {
    let mut limits = nsh_platform_tui::Limits::default();
    if limits.memory_bytes == 0 || limits.memory_bytes > 128 * 1024 * 1024 {
        limits.memory_bytes = 128 * 1024 * 1024;
    }
    if limits.call_budget > Duration::from_millis(500) {
        limits.call_budget = Duration::from_millis(500);
    }
    if limits.load_budget > Duration::from_secs(10) {
        limits.load_budget = Duration::from_secs(10);
    }
    if limits.max_nodes > 200_000 {
        limits.max_nodes = 200_000;
    }
    limits
}

/// 表面的位置形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePlace {
    /// 外框 `mod` 块占用的区域。
    Region,
    /// 占一个 Pane 位。
    Pane,
    /// 内容区之上的居中对话框，宿主压暗内容区。
    Dialog,
    /// 靠近指针或指定位置的弹出层。
    Popup,
    /// 可拖动、记忆位置的窗口。
    Window,
}

impl SurfacePlace {
    /// `mods.open` 的 `place` 参数解析。
    pub fn parse(value: &str) -> Option<SurfacePlace> {
        match value {
            "pane" => Some(SurfacePlace::Pane),
            "region" | "left" | "right" | "top" | "bottom" => Some(SurfacePlace::Region),
            "dialog" => Some(SurfacePlace::Dialog),
            "popup" => Some(SurfacePlace::Popup),
            "window" => Some(SurfacePlace::Window),
            _ => None,
        }
    }

    /// 序列化名，与启动参数 `place` 一致。
    pub fn as_str(&self) -> &'static str {
        match self {
            SurfacePlace::Region => "region",
            SurfacePlace::Pane => "pane",
            SurfacePlace::Dialog => "dialog",
            SurfacePlace::Popup => "popup",
            SurfacePlace::Window => "window",
        }
    }
}

/// 实例生命周期；见 docs/mods.md §11。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceState {
    /// 引擎加载与首帧渲染中。
    Loading,
    /// 正常运行。
    Running,
    /// 新代际在后台加载；宿主方法返回 `staging`。
    Staging,
    /// 连续崩溃或加载失败；`mods.reload` 恢复。
    Failed,
}

/// 一条模块表面：一个 `AppView` 与它的矩形、代际与状态。
pub struct ModSurface {
    /// 稳定表面 ID（`s1`、`s2`…），引用于 `mods.close`、`focus`、`overlay`。
    pub id: String,
    /// 宿主通道；`host.call` 经它到 UI 线程，事件经它回模块。
    pub host: crate::mods::host::ModHost,
    /// 模块 ID。
    pub mod_id: String,
    /// 位置形态。
    pub place: SurfacePlace,
    /// 所属代际；staging 成功后原子切换。
    pub generation: u64,
    /// 表面视图。
    pub view: AppView,
    /// 当前矩形；region 表面每帧由外框布局给出。
    pub area: Rect,
    /// 键盘焦点是否在此表面。
    pub focused: bool,
    /// 生命周期状态。
    pub state: SurfaceState,
    /// 运行期错误计数；连续 3 次进入 `Failed`。
    pub fails: u32,
    /// `window` 覆盖层的位置记忆 key。
    pub key: Option<String>,
    /// 最近一次错误摘要，写日志并标注在失败表面顶部。
    pub last_error: Option<String>,
    /// 引擎回调收集的运行期错误，绘制后由宿主取走并计数。
    pub errors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ModSurface {
    /// 引擎是否还能接收输入。
    pub fn interactive(&self) -> bool {
        matches!(self.state, SurfaceState::Running | SurfaceState::Loading)
    }

    /// 记录一次运行期错误；连续 3 次进入 `Failed`。
    pub fn record_failure(&mut self, message: String) {
        self.fails += 1;
        self.last_error = Some(message);
        if self.fails >= 3 {
            self.state = SurfaceState::Failed;
        }
    }
}

/// `run` / `request` 的动作请求记录。
#[derive(Debug, Clone)]
pub struct ModRequest {
    /// 请求 ID。
    pub id: String,
    /// 发起表面。
    pub surface: String,
    /// 动作名与参数。
    pub action: String,
    pub args: Value,
    /// accepted / confirmation / running / done / failed / conflict。
    pub status: String,
    /// 失败或冲突摘要。
    pub error: Option<String>,
    /// 记录时间，超时回收。
    pub since: Instant,
}

/// `window` 覆盖层的位置记忆。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPos {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

impl WindowPos {
    /// 从 state.json 的 `mods_windows` 条目解析。
    pub fn from_value(value: &Value) -> Option<WindowPos> {
        let number = |key: &str| value.get(key).and_then(Value::as_u64).map(|v| v as u16);
        Some(WindowPos {
            x: number("x")?,
            y: number("y")?,
            cols: number("cols")?,
            rows: number("rows")?,
        })
    }

    /// 序列化为 `mods_windows` 条目。
    pub fn to_value(self) -> Value {
        json!({"x": self.x, "y": self.y, "cols": self.cols, "rows": self.rows})
    }
}

/// 模块运行时：表面集合、代际、授予、请求与位置记忆。
#[derive(Default)]
pub struct ModRuntime {
    /// 存活表面，overlay 在 region/pane 之后（z 序从低到高）。
    pub surfaces: Vec<ModSurface>,
    /// 下一个表面 ID 序号。
    pub next_surface: u64,
    /// 每个模块的当前代际。
    pub generations: BTreeMap<String, u64>,
    /// 单个模块禁用状态；`true` 为启用。
    pub enabled: BTreeMap<String, bool>,
    /// 已批准的宿主授予：mod id → 方法与事件名集合。
    pub grants: BTreeMap<String, BTreeSet<String>>,
    /// 待确认的授予申请：mod id → 尚未批准的名集合。
    pub pending_grants: BTreeMap<String, BTreeSet<String>>,
    /// 动作请求记录。
    pub requests: BTreeMap<String, ModRequest>,
    /// `window` 位置记忆：key → 位置。
    pub windows: BTreeMap<String, WindowPos>,
    /// 请求 ID 序号。
    pub next_request: u64,
}

impl ModRuntime {
    /// 从 `hctui.toml` 的 `[grants]` 读出 `host` 与 `events` 名单。
    ///
    /// 只按行读这两个数组键；未知键与其它段落忽略。TOML 的完整解析留给
    /// 安装与分发阶段。
    pub fn manifest_grants(path: &std::path::Path) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let mut in_grants = false;
        let mut names = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                in_grants = line
                    .trim_start_matches('[')
                    .split(']')
                    .next()
                    .unwrap_or("")
                    .trim()
                    == "grants";
                continue;
            }
            if !in_grants {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if !matches!(key.trim(), "host" | "events") {
                continue;
            }
            let value = value.trim().trim_start_matches('[').trim_end_matches(']');
            for item in value.split(',') {
                let item = item.trim().trim_matches('"').trim_matches('\'');
                if !item.is_empty() {
                    names.push(item.to_string());
                }
            }
        }
        names
    }

    /// 模块是否被显式禁用。
    pub fn is_enabled(&self, mod_id: &str) -> bool {
        self.enabled.get(mod_id).copied().unwrap_or(true)
    }

    /// 下一个表面 ID。
    pub fn next_surface_id(&mut self) -> String {
        self.next_surface += 1;
        format!("s{}", self.next_surface)
    }

    /// 下一个动作请求 ID。
    pub fn next_request_id(&mut self) -> String {
        self.next_request += 1;
        format!("r{}", self.next_request)
    }

    /// 按表面 ID 查找。
    pub fn surface(&self, id: &str) -> Option<&ModSurface> {
        self.surfaces.iter().find(|s| s.id == id)
    }

    pub fn surface_mut(&mut self, id: &str) -> Option<&mut ModSurface> {
        self.surfaces.iter_mut().find(|s| s.id == id)
    }

    /// 该模块当前是否已有表面（region/pane）。
    pub fn has_docked_surface(&self, mod_id: &str) -> bool {
        self.surfaces.iter().any(|s| {
            s.mod_id == mod_id && matches!(s.place, SurfacePlace::Region | SurfacePlace::Pane)
        })
    }

    /// 关闭表面；返回是否移除了一个。
    pub fn close(&mut self, id: &str) -> bool {
        let before = self.surfaces.len();
        self.surfaces.retain(|s| s.id != id);
        self.surfaces.len() != before
    }

    /// 关闭某模块的全部 region/pane 表面（undock 语义）。
    pub fn close_docked(&mut self, mod_id: &str) -> bool {
        let before = self.surfaces.len();
        self.surfaces.retain(|s| {
            !(s.mod_id == mod_id && matches!(s.place, SurfacePlace::Region | SurfacePlace::Pane))
        });
        self.surfaces.len() != before
    }

    /// 记录一个动作请求并返回请求 ID。
    pub fn begin_request(&mut self, surface: &str, action: &str, args: Value) -> String {
        let id = self.next_request_id();
        self.requests.insert(
            id.clone(),
            ModRequest {
                id: id.clone(),
                surface: surface.to_string(),
                action: action.to_string(),
                args,
                status: "accepted".into(),
                error: None,
                since: Instant::now(),
            },
        );
        id
    }

    /// 回收 60 秒前的已完成请求。
    pub fn prune_requests(&mut self) {
        self.requests
            .retain(|_, request| request.since.elapsed() < Duration::from_secs(60));
    }

    /// 该 mod 的宿主授予是否覆盖 `name`。
    pub fn granted(&self, mod_id: &str, name: &str) -> bool {
        self.grants
            .get(mod_id)
            .is_some_and(|set| set.contains(name))
    }

    /// 批准授予；返回新增的名字。
    pub fn approve_grants(&mut self, mod_id: &str, names: &[String]) -> Vec<String> {
        let set = self.grants.entry(mod_id.to_string()).or_default();
        names
            .iter()
            .filter(|name| set.insert((*name).clone()))
            .cloned()
            .collect()
    }

    /// 撤销授予；返回移除的名字。
    pub fn revoke_grants(&mut self, mod_id: &str, names: &[String]) -> Vec<String> {
        let Some(set) = self.grants.get_mut(mod_id) else {
            return Vec::new();
        };
        names
            .iter()
            .filter(|name| set.remove(name.as_str()))
            .cloned()
            .collect()
    }

    /// 该 mod 已批准的名字，按字典序。
    pub fn grants_for(&self, mod_id: &str) -> Vec<String> {
        self.grants
            .get(mod_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 授予申请进入待确认队列。
    pub fn request_grants(&mut self, mod_id: &str, names: &[String]) {
        let granted = self.grants.get(mod_id).cloned().unwrap_or_default();
        let pending = self.pending_grants.entry(mod_id.to_string()).or_default();
        for name in names {
            if !granted.contains(name) {
                pending.insert(name.clone());
            }
        }
    }

    /// 该 mod 尚未批准的授予申请。
    pub fn pending_grants_for(&self, mod_id: &str) -> Vec<String> {
        self.pending_grants
            .get(mod_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 取出待确认名单（确认后调用 [`ModRuntime::approve_grants`]）。
    pub fn take_pending_grants(&mut self, mod_id: &str) -> Vec<String> {
        self.pending_grants
            .remove(mod_id)
            .map(|set| set.into_iter().collect())
            .unwrap_or_default()
    }

    /// 序列化为 state.json 的 `mods_grants`。
    pub fn grants_to_state(&self) -> Value {
        json!(self
            .grants
            .iter()
            .map(|(id, set)| (id.clone(), set.iter().cloned().collect::<Vec<_>>()))
            .collect::<BTreeMap<_, _>>())
    }

    /// 从 state.json 的 `mods_grants` 恢复。
    pub fn grants_from_state(&mut self, value: Option<&Value>) {
        let Some(map) = value.and_then(Value::as_object) else {
            return;
        };
        for (id, names) in map {
            let Some(names) = names.as_array() else {
                continue;
            };
            let set = self.grants.entry(id.clone()).or_default();
            for name in names {
                if let Some(name) = name.as_str() {
                    set.insert(name.to_string());
                }
            }
        }
    }

    /// 组装一条表面的 `AppOptions`；唤醒由调用方接到宿主脏标记。
    pub fn options(&self, wake: Option<nsh_platform_tui::WakeCallback>) -> AppOptions {
        let mut options = AppOptions::default();
        options.limits = embedded_limits();
        options.cache = nsh_platform_tui::CacheOptions::off();
        options.wake = wake;
        options
    }

    /// 创建一条表面并推进该模块的代际；宿主通道已按表面 ID 构建。
    pub fn open_surface(
        &mut self,
        id: String,
        mod_id: &str,
        place: SurfacePlace,
        view: AppView,
        key: Option<String>,
        host: crate::mods::host::ModHost,
    ) -> &mut ModSurface {
        let generation = {
            let gen = self.generations.entry(mod_id.to_string()).or_insert(0);
            *gen += 1;
            *gen
        };
        self.surfaces.push(ModSurface {
            id: id.clone(),
            host,
            mod_id: mod_id.to_string(),
            place,
            generation,
            view,
            area: Rect::ZERO,
            focused: false,
            state: SurfaceState::Loading,
            fails: 0,
            key,
            last_error: None,
            errors: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        self.surfaces.last_mut().expect("just pushed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_runtime_errors_mark_the_surface_failed() {
        let mut runtime = ModRuntime::default();
        let host = crate::mods::host::ModHost::new("s1", "probe");
        let view = nsh_platform_tui::AppView::new(runtime.options(None));
        let surface =
            runtime.open_surface("s1".into(), "probe", SurfacePlace::Dialog, view, None, host);
        assert_eq!(surface.generation, 1);
        surface.record_failure("boom".into());
        surface.record_failure("boom".into());
        assert_eq!(surface.state, SurfaceState::Loading);
        surface.record_failure("boom".into());
        assert_eq!(surface.state, SurfaceState::Failed);
        assert_eq!(surface.last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn grants_roundtrip_and_enumeration() {
        let mut runtime = ModRuntime::default();
        runtime.approve_grants("probe", &["state".into(), "exec".into()]);
        runtime.approve_grants("probe", &["exec".into()]);
        assert_eq!(
            runtime.grants_for("probe"),
            vec!["exec".to_string(), "state".to_string()]
        );
        runtime.request_grants("probe", &["exec".into(), "files".into()]);
        assert_eq!(
            runtime.pending_grants_for("probe"),
            vec!["files".to_string()]
        );
        let state = runtime.grants_to_state();
        let mut restored = ModRuntime::default();
        restored.grants_from_state(Some(&state));
        assert!(restored.granted("probe", "exec"));
        assert!(!restored.granted("probe", "files"));
    }

    #[test]
    fn window_positions_roundtrip_through_state() {
        let pos = WindowPos {
            x: 3,
            y: 4,
            cols: 30,
            rows: 8,
        };
        assert_eq!(WindowPos::from_value(&pos.to_value()), Some(pos));
        assert_eq!(WindowPos::from_value(&json!({"x": 1})), None);
    }

    #[test]
    fn manifest_grants_read_both_arrays() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("hctui-grants-{unique}.toml"));
        std::fs::write(
            &path,
            "[mod]\nid = \"probe\"\n\n[grants]\nhost = [\"state\", \"run\"]\nevents = [\"state.changed\"]\n",
        )
        .unwrap();
        assert_eq!(
            ModRuntime::manifest_grants(&path),
            vec![
                "state".to_string(),
                "run".to_string(),
                "state.changed".to_string()
            ]
        );
    }
}
