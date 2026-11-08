//! 主题来源：来源链、来源标记与热重载。
//!
//! `theme.name` 是显式的内置主题名，或者是 [`AUTO`] / [`GHOSTTY`] / [`OMARCHY`] 之一；
//! 名字是内置 `default` 时由 `[theme] source` 决定跟随谁。来源链按下面的顺序尝试，
//! 解析失败继续下一个，最终落到内置 `default` 时给一条警告。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use super::{ghostty, omarchy, Theme};

/// 跟随终端主题（Ghostty 优先，其次 Omarchy）。
pub const AUTO: &str = "auto";
/// 只跟随 Ghostty。
pub const GHOSTTY: &str = "ghostty";
/// 只跟随 Omarchy。
pub const OMARCHY: &str = "omarchy";
/// 关闭跟随，用内置主题。
pub const BUILTIN: &str = "builtin";

/// 热重载节流：最多每秒查一次文件指纹。
const THROTTLE: Duration = Duration::from_secs(1);

/// 来源种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ghostty,
    Omarchy,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ghostty => GHOSTTY,
            Kind::Omarchy => OMARCHY,
        }
    }

    fn load(self) -> Result<Loaded, Error> {
        match self {
            Kind::Ghostty => ghostty::load(),
            Kind::Omarchy => omarchy::load(),
        }
    }
}

/// 读来源的失败原因。`Absent` 表示这台机器上就没有这个来源，静默跳过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Absent(String),
    Broken(String),
}

impl Error {
    pub fn reason(&self) -> &str {
        match self {
            Error::Absent(reason) | Error::Broken(reason) => reason,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason())
    }
}

/// 一份来源解析出来的配色，以及影响它的文件。
pub struct Loaded {
    pub theme: Theme,
    /// 来源侧的名字，例如 Ghostty 的 `theme = <Name>` 或 Omarchy 的主题目录名。
    pub label: String,
    /// 所有被读过的文件；热重载按它们的时间戳判断是否要重读。
    pub files: Vec<PathBuf>,
}

/// 一个文件的时间戳与长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime_ns: u128,
    len: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime_ns = meta
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            mtime_ns,
            len: meta.len(),
        })
    }
}

/// 被读文件的指纹：规范化路径加时间戳。规范化让软链切换必然改变指纹。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint(Vec<(PathBuf, Option<Stamp>)>);

impl Fingerprint {
    fn of(files: &[PathBuf]) -> Self {
        Self(
            files
                .iter()
                .map(|path| {
                    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                    (canonical, Stamp::of(path))
                })
                .collect(),
        )
    }
}

/// 主题来自哪里。`theme show` 用它说明本次用的是哪个来源、哪个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    kind: Kind,
    label: String,
    files: Vec<PathBuf>,
    fingerprint: Fingerprint,
}

impl Source {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// 例如 `ghostty: catppuccin (/home/u/.config/ghostty/config, ...)`。
    pub fn describe(&self) -> String {
        let files = self
            .files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}: {} ({files})", self.kind.name(), self.label)
    }
}

/// 内置 `default` 的 token，名字换成配置值，让配置原样写回。
pub fn fallback(name: &str) -> Theme {
    let mut theme = super::base();
    theme.name = name.to_string();
    theme
}

/// 主题来源的优先顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefer {
    Auto,
    Ghostty,
    Omarchy,
    Builtin,
}

impl Prefer {
    fn from_source(value: &str) -> Self {
        match value {
            GHOSTTY => Self::Ghostty,
            OMARCHY => Self::Omarchy,
            BUILTIN => Self::Builtin,
            _ => Self::Auto,
        }
    }

    fn chain(self) -> &'static [(Kind, bool)] {
        // 第二项为真时，来源缺失也算失败并进入警告。
        match self {
            Self::Auto => &[(Kind::Ghostty, false), (Kind::Omarchy, false)],
            Self::Ghostty => &[(Kind::Ghostty, true), (Kind::Omarchy, false)],
            Self::Omarchy => &[(Kind::Omarchy, true)],
            Self::Builtin => &[],
        }
    }
}

/// 按配置的名字与 `[theme] source` 决定优先顺序。
pub fn prefer(name: &str, configured: &str) -> Prefer {
    match name {
        OMARCHY => Prefer::Omarchy,
        GHOSTTY => Prefer::Ghostty,
        AUTO => Prefer::Auto,
        // 内置 `default` 是「跟随来源，回退内置默认」；其它名字是显式内置主题。
        "default" => Prefer::from_source(configured),
        _ => Prefer::Builtin,
    }
}

/// 解析主题：显式内置名直接取用，跟随名与内置 `default` 走来源链。
pub fn apply(name: &str, configured: &str, custom: &[(String, String)]) -> (Theme, Vec<String>) {
    let prefer = prefer(name, configured);
    let mut reasons = Vec::new();
    let mut found = None;
    for (kind, warn_absent) in prefer.chain() {
        match kind.load() {
            Ok(loaded) => {
                found = Some(following(*kind, loaded));
                break;
            }
            Err(error) if *warn_absent || matches!(&error, Error::Broken(_)) => {
                reasons.push(error.reason().to_string());
            }
            Err(_) => {}
        }
    }
    let mut warnings = Vec::new();
    let mut theme = match found {
        Some(theme) => theme,
        None => {
            if !reasons.is_empty() {
                warnings.push(format!(
                    "主题来源不可用（{}），使用内置 default",
                    reasons.join("；")
                ));
            }
            match super::builtin(name) {
                Some(theme) => theme,
                None => {
                    if prefer == Prefer::Builtin {
                        warnings.push(format!("未知主题 {name}，使用 default"));
                    }
                    super::builtin("default").expect("default theme exists")
                }
            }
        }
    };
    if prefer != Prefer::Builtin {
        theme.name = name.to_string();
    }
    for (token, value) in custom {
        match super::parse_color(value) {
            Some(color) => theme.set(token, color),
            None => warnings.push(format!("[theme.custom] {token}: 无法解析颜色 {value}")),
        }
    }
    (theme, warnings)
}

/// 单个来源的即时解析：成功挂来源标记，失败退回内置默认 token。
pub fn borrow(kind: Kind, result: Result<Loaded, Error>, name: &str) -> Theme {
    match result {
        Ok(loaded) => following(kind, loaded),
        Err(_) => fallback(name),
    }
}

/// 把解析结果挂上来源标记。
fn following(kind: Kind, loaded: Loaded) -> Theme {
    let mut theme = loaded.theme;
    let fingerprint = Fingerprint::of(&loaded.files);
    theme.source = Some(Source {
        kind,
        label: loaded.label,
        files: loaded.files,
        fingerprint,
    });
    theme
}

/// 上次查询指纹的时间，节流用。
static LAST_POLL: Mutex<Option<Instant>> = Mutex::new(None);

/// 事件循环里调用：来源文件变了就重载并广播 `theme.changed`。
pub fn poll(app: &mut crate::app::App) {
    let Some(source) = app.config.theme.source.clone() else {
        return;
    };
    let now = Instant::now();
    {
        let mut last = LAST_POLL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.is_some_and(|at| now.duration_since(at) < THROTTLE) {
            return;
        }
        *last = Some(now);
    }
    let current = Fingerprint::of(&source.files);
    if current == source.fingerprint {
        return;
    }
    match source.kind.load() {
        Ok(loaded) => {
            let mut theme = following(source.kind, loaded);
            // 配置里的 `[theme.custom]` 在来源之上仍然生效。
            for (token, value) in &app.config.theme_custom {
                if let Some(color) = super::parse_color(value) {
                    theme.set(token, color);
                }
            }
            tracing::info!(
                name = %theme.name,
                source = %theme.source.as_ref().map_or("builtin", |source| source.label()),
                "theme reloaded"
            );
            app.note = format!(
                "theme: {}",
                theme
                    .source
                    .as_ref()
                    .map_or("builtin", |source| source.label())
            );
            app.config.theme = theme;
            app.notify_mods_theme();
            app.bump_mod_revision_domains(&["theme"]);
            app.mark_dirty();
        }
        Err(error) => {
            // 记下这次的指纹，避免文件写到一半时每秒重试。
            if let Some(source) = app.config.theme.source.as_mut() {
                source.fingerprint = current;
            }
            app.note = format!("{}: {}", source.kind.name(), error.reason());
            app.mark_dirty();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_follows_symlink_switch() {
        let root = std::env::temp_dir().join("hc-tui-source-fingerprint");
        let _ = std::fs::remove_dir_all(&root);
        for (name, accent) in [("aether", "#89b4fa"), ("other", "#ff8800")] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("colors.toml"), format!("accent = \"{accent}\"\n")).unwrap();
        }
        let current = root.join("current");
        std::os::unix::fs::symlink(root.join("aether"), &current).unwrap();
        let files = vec![current.join("colors.toml")];
        let before = Fingerprint::of(&files);
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(root.join("other"), &current).unwrap();
        let after = Fingerprint::of(&files);
        assert_ne!(before, after);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preference_follows_the_configured_source() {
        assert_eq!(prefer("default", "auto"), Prefer::Auto);
        assert_eq!(prefer("default", "ghostty"), Prefer::Ghostty);
        assert_eq!(prefer("default", "omarchy"), Prefer::Omarchy);
        assert_eq!(prefer("default", "builtin"), Prefer::Builtin);
        assert_eq!(prefer("auto", "builtin"), Prefer::Auto);
        assert_eq!(prefer("omarchy", "auto"), Prefer::Omarchy);
        assert_eq!(prefer("ghostty", "builtin"), Prefer::Ghostty);
        assert_eq!(prefer("nord", "auto"), Prefer::Builtin);
    }

    #[test]
    fn chains_put_ghostty_first() {
        assert_eq!(Prefer::Auto.chain()[0].0, Kind::Ghostty);
        assert_eq!(Prefer::Ghostty.chain()[0].0, Kind::Ghostty);
        assert_eq!(Prefer::Omarchy.chain()[0].0, Kind::Omarchy);
        assert!(Prefer::Builtin.chain().is_empty());
    }

    #[test]
    fn headless_and_explicit_sources_fall_back_to_builtin() {
        // 用必然不存在的路径，验证回退不 panic、名字保持配置值、只给一条警告。
        // 环境变量是进程级的，两条断言放在同一个测试里避免并行互踩。
        std::env::set_var("HC_GHOSTTY_CONFIG", "/nonexistent/hc-tui/ghostty/config");
        std::env::set_var("OMARCHY_THEME_DIR", "/nonexistent/hc-tui/omarchy/theme");
        for (name, configured, warned) in [
            ("auto", "auto", false),
            ("default", "auto", false),
            ("nord", "auto", false),
            ("omarchy", "auto", true),
            ("ghostty", "auto", true),
        ] {
            let (theme, warnings) = apply(name, configured, &[]);
            assert_eq!(theme.name, name);
            assert_eq!(warnings.len(), usize::from(warned), "{name}: {warnings:?}");
            if warned {
                assert!(warnings[0].contains("内置 default"), "{warnings:?}");
            }
        }
        std::env::remove_var("HC_GHOSTTY_CONFIG");
        std::env::remove_var("OMARCHY_THEME_DIR");
    }
}
