//! `hc-tui mods check`：清单校验与 80×24 无窗口试渲染。
//!
//! 校验顺序：解析 `hctui.toml` 的 `[mod]` 与 `[grants]`、检查 `host_api` 与
//! `min_hc_tui_version`、定位 `dist/bundle.js`、在 `TestBackend` 上加载并渲染
//! 首帧。存在 `nsh` 时再跑一次 `nsh check`；缺失时记为警告，不阻断宿主侧检查。

use std::path::{Path, PathBuf};

use ratatui_core::backend::TestBackend;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Terminal;

use crate::config::Config;
use crate::mods::host;
use crate::mods::ModRuntime;

/// 检查结果。
#[derive(Debug)]
pub struct Report {
    /// 模块目录。
    pub path: PathBuf,
    /// 清单里的模块 ID。
    pub id: String,
    /// 声明的宿主方法与事件名。
    pub grants: Vec<String>,
    /// 不阻断加载的提示。
    pub warnings: Vec<String>,
}

/// 把 CLI 参数解析成模块目录：存在的路径优先，其次安装存储，最后配置目录。
pub fn resolve_dir(config: &Config, target: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(target);
    if path.is_dir() {
        return Ok(path);
    }
    if let Some(dir) = crate::mods::store::module_root(config, target) {
        return Ok(dir);
    }
    Err(format!(
        "no mod at {target}, {}, or {}",
        config.mods_dir().join(target).display(),
        config.mods_state_dir().join("store").join(target).display()
    ))
}

/// 校验并试渲染一个模块。
pub fn check(config: &Config, target: &str) -> Result<Report, String> {
    let path = resolve_dir(config, target)?;
    let loaded = crate::pkg::load_dir(&path).map_err(|error| error.to_string())?;
    let manifest = &loaded.manifest;
    let id = manifest.id().to_string();
    if *manifest.host_api.get_ref() != crate::plugin::manifest::HOST_API {
        return Err(format!(
            "hctui.toml: host_api {} is not supported (host api {})",
            manifest.host_api.get_ref(),
            crate::plugin::manifest::HOST_API
        ));
    }
    let mut warnings = Vec::new();
    if let Some(minimum) = manifest
        .module
        .as_ref()
        .and_then(crate::pkg::Module::min_hc_tui_version)
    {
        let current = env!("CARGO_PKG_VERSION");
        if compare_versions(minimum, current).is_gt() {
            return Err(format!(
                "hctui.toml: needs hc-tui {minimum} or newer (running {current})"
            ));
        }
    }
    let grants = manifest.grants();
    let bundle_path = path.join(manifest.bundle().unwrap_or("dist/bundle.js"));
    if !bundle_path.exists() {
        return Err(format!(
            "{}: no dist/bundle.js; run `nsh build -o dist` in {}",
            bundle_path.display(),
            path.display()
        ));
    }
    let bundle = nsh_platform_tui::Bundle::read(&bundle_path).map_err(|error| error.to_string())?;
    let runtime = ModRuntime::default();
    let host = host::ModHost::new("s1", &id);
    host.set_params(host::build_params(&host::ParamsContext {
        mod_id: id.clone(),
        surface: "s1".to_string(),
        generation: 1,
        place: "region",
        props: serde_json::json!({}),
        space: serde_json::Value::Null,
        tab: serde_json::Value::Null,
        pane: serde_json::Value::Null,
        client: "check".to_string(),
        core: "check".to_string(),
        workspace: "check".to_string(),
        cell: [8, 16],
        locale: if crate::i18n::zh() { "zh" } else { "en" },
        theme: serde_json::json!({"name": "check", "tokens": {}, "palette": []}),
        hc_tui: env!("CARGO_PKG_VERSION").to_string(),
    }));
    let mut options = runtime.options(None);
    options.host = host.channel.clone();
    let mut view = nsh_platform_tui::AppView::new(options);
    view.load(bundle).map_err(|error| error.to_string())?;
    let area = Rect::new(0, 0, 80, 24);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
        .map_err(|error| error.to_string())?;
    terminal
        .draw(|frame| view.render(frame, area))
        .map_err(|error| error.to_string())?;
    if view.exit_requested() {
        return Err("the mod requested exit during its first frame".to_string());
    }
    drop(view);
    // 安装后的代际只有 dist；源码检查只在开发目录上有意义。
    if path.join("App.tsx").exists() {
        if let Err(error) = run_nsh_check(&path) {
            warnings.push(error);
        }
    }
    Ok(Report {
        path,
        id,
        grants,
        warnings,
    })
}

/// 存在 `nsh` 时运行 `nsh check`；缺失时返回一条警告文本。
fn run_nsh_check(path: &Path) -> Result<(), String> {
    let output = std::process::Command::new("nsh")
        .arg("check")
        .arg(path)
        .output();
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "nsh check failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(format!("nsh check skipped: {error}")),
    }
}

/// 旧 `[mod]` 段里一个标量的文本形态；旧用例用它固定字段归属。
#[cfg(test)]
fn manifest_value(text: &str, key: &str) -> Option<String> {
    let loaded = crate::pkg::parse_file(Path::new("hctui.toml"), text).ok()?;
    let manifest = &loaded.manifest;
    match key {
        "id" => Some(manifest.id().to_string()),
        "name" => Some(manifest.name().to_string()),
        "host_api" => Some(manifest.host_api.get_ref().to_string()),
        "min_hc_tui_version" => manifest
            .module
            .as_ref()
            .and_then(crate::pkg::Module::min_hc_tui_version)
            .map(str::to_string),
        _ => None,
    }
}

/// `a` 与 `b` 的版本比较；非数字段按字典序。
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |value: &str| -> Vec<String> {
        value
            .split(['.', '-', '+'])
            .map(|part| part.to_string())
            .collect()
    };
    let a = parts(a);
    let b = parts(b);
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).map(String::as_str).unwrap_or("0");
        let right = b.get(index).map(String::as_str).unwrap_or("0");
        let ordering = match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(left), Ok(right)) => left.cmp(&right),
            _ => left.cmp(right),
        };
        if ordering.is_ne() {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_values_are_scoped_to_the_mod_section() {
        let text = r#"
[mod]
id = "agents"
host_api = 1
min_hc_tui_version = "0.4.0"

[grants]
host = ["state", "run"]
events = ["state.changed"]
"#;
        assert_eq!(manifest_value(text, "id").as_deref(), Some("agents"));
        assert_eq!(manifest_value(text, "host_api").as_deref(), Some("1"));
        assert_eq!(
            manifest_value(text, "min_hc_tui_version").as_deref(),
            Some("0.4.0")
        );
        assert_eq!(manifest_value(text, "events"), None);
    }

    #[test]
    fn check_renders_a_bundle_and_reads_the_manifest() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("hctui-check-{unique}"));
        let dist = dir.join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(
            dir.join("hctui.toml"),
            "[mod]\nid = \"probe\"\nhost_api = 1\n\n[grants]\nhost = [\"state\", \"run\"]\nevents = [\"state.changed\"]\n",
        )
        .unwrap();
        let code = nsh_bundler::compile_module(
            "App.tsx",
            r#"import { mount, Text, View } from "nsh";
mount(() => <View><Text>check</Text></View>);"#,
            &[],
        )
        .unwrap();
        std::fs::write(dist.join("bundle.js"), code).unwrap();
        let report = check(&Config::default(), dir.to_str().unwrap()).unwrap();
        assert_eq!(report.id, "probe");
        assert_eq!(
            report.grants,
            vec![
                "state".to_string(),
                "run".to_string(),
                "state.changed".to_string()
            ]
        );
    }

    #[test]
    fn check_rejects_a_missing_bundle() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("hctui-check-missing-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hctui.toml"), "[mod]\nid = \"probe\"\n").unwrap();
        let error = check(&Config::default(), dir.to_str().unwrap()).unwrap_err();
        assert!(error.contains("bundle.js"), "{error}");
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(compare_versions("0.10.0", "0.9.0").is_gt());
        assert!(compare_versions("0.4.0", "0.4.0").is_eq());
        assert!(compare_versions("0.4.0", "0.4.1").is_lt());
    }
}
