//! 模块安装与分发：本地目录、链接目录与签名包，代际切换与回收。
//!
//! 安装写入 `store/<id>/<generation>/`，`current/<id>` 原子切换；运行中的
//! 实例继续使用已加载的代码，旧代际在表面关闭后由 [`Store::prune`] 回收。
//! 签名包经 [`nsh_package::TrustStore`] 校验信任根、撤销列表、构建号与最低
//! 运行时；校验通过才写入新代际。

use std::path::{Path, PathBuf};

use nsh_package::{PublicKey, Revocations, TrustStore, VerifiedPackage};

use crate::config::Config;
use crate::mods::EVENTS;
use crate::store::{copy_tree, file_sha256, now_unix, Record, Source, Store};

/// 安装预览：安装前列出 id、版本、授予与文件。
#[derive(Debug, Clone)]
pub struct Preview {
    /// 模块 ID。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 版本。
    pub version: String,
    /// 构建号。
    pub build: u64,
    /// 来源。
    pub source: Source,
    /// 声明的宿主方法。
    pub grants: Vec<String>,
    /// 文件数。
    pub files: usize,
    /// 来源路径或包。
    pub origin: PathBuf,
    /// 包整体哈希。
    pub sha256: Option<String>,
    /// 最低运行时（签名包）。
    pub min_runtime: Option<String>,
    /// 由清单推导的能力。
    pub capabilities: crate::pkg::Capabilities,
    /// 声明了模块时的模块摘要。
    pub module: Option<crate::pkg::ModuleInfo>,
    /// 不阻断安装的提示。
    pub warnings: Vec<String>,
}

/// 读包根目录清单里的包 ID：v2、v1 与旧 `[mod]` 共用同一个解析器。
pub fn manifest_id(dir: &Path) -> Result<String, String> {
    crate::pkg::load_dir(dir)
        .map(|loaded| loaded.manifest.id().to_string())
        .map_err(|error| error.to_string())
}

/// 读 `nsh.json` 里的显示名与版本；缺失时给默认值。
fn app_metadata(dir: &Path) -> (String, String, u64) {
    let Ok(text) = std::fs::read_to_string(dir.join("nsh.json")) else {
        return (String::new(), "0.0.0".to_string(), 1);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (String::new(), "0.0.0".to_string(), 1);
    };
    (
        value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("0.0.0")
            .to_string(),
        value
            .get("build")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1),
    )
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                count_files(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

/// 本地目录的安装预览。
pub fn preview_local(dir: &Path) -> Result<Preview, String> {
    let loaded = crate::pkg::install_ready(dir)?;
    let id = loaded.manifest.id().to_string();
    let (name, version, build) = app_metadata(dir);
    let grants = loaded.manifest.grants();
    Ok(Preview {
        id,
        name: if name.is_empty() {
            loaded.manifest.name().to_string()
        } else {
            name
        },
        version,
        build,
        source: Source::Local,
        grants,
        files: count_files(dir),
        origin: dir.to_path_buf(),
        sha256: None,
        min_runtime: None,
        capabilities: loaded.manifest.capabilities(),
        module: loaded.manifest.module_info(),
        warnings: Vec::new(),
    })
}

/// 构建信任根与撤销列表；签名包安装前调用。
pub fn trust_store(config: &Config) -> Result<TrustStore, String> {
    let state = crate::store::Store::open(config.mods_state_dir());
    std::fs::create_dir_all(state.trust_dir()).map_err(|error| error.to_string())?;
    let mut trust =
        TrustStore::open(state.trust_dir()).map_err(|error| format!("trust store: {error}"))?;
    for entry in &config.mods_trust_roots {
        trust.add_root(read_root(entry)?);
    }
    let path = config.mods_revocations_path();
    if path.exists() {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let list = Revocations::from_json(&text)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        match trust.update_revocations(list) {
            Ok(_) => {}
            Err(nsh_package::Error::StaleRevocations { got, have }) => {
                eprintln!("revocations: keeping sequence {have} (file has {got})");
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    Ok(trust)
}

fn read_root(entry: &str) -> Result<PublicKey, String> {
    let path = Path::new(entry);
    let text = if path.is_file() {
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?
    } else {
        entry.to_string()
    };
    let key = text.lines().next().unwrap_or("").trim();
    PublicKey::from_base64(key).map_err(|error| format!("trust root {entry}: {error}"))
}

/// 签名包的预览与校验；返回已验证包与信任存储（调用方安装后 `accept`）。
pub fn verify_package(
    config: &Config,
    path: &Path,
) -> Result<(Preview, VerifiedPackage, TrustStore), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let trust = trust_store(config)?;
    if trust.roots().is_empty() {
        return Err("no trust roots: add [mods] trust_roots to config.toml".to_string());
    }
    let package = trust
        .verify(&bytes, nsh_package::RUNTIME_VERSION)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let id = package.app.id.clone();
    let grants = package_grants(&package);
    let files = 1 + usize::from(package.source_map.is_some()) + package.assets.len();
    let preview = Preview {
        id,
        name: package.app.name.clone(),
        version: package.app.version.clone(),
        build: package.app.build,
        source: Source::Packaged,
        grants,
        files,
        origin: path.to_path_buf(),
        sha256: Some(package.sha256.clone()),
        min_runtime: None,
        capabilities: packaged_capabilities(),
        module: Some(packaged_module(&package.app.name)),
        warnings: package.ignored.clone(),
    };
    Ok((preview, package, trust))
}

/// 签名包的清单由 [`package_manifest`] 生成：只有模块，没有动作与终端入口。
fn packaged_capabilities() -> crate::pkg::Capabilities {
    crate::pkg::Capabilities {
        actions: false,
        terminals: false,
        module: true,
    }
}

fn packaged_module(name: &str) -> crate::pkg::ModuleInfo {
    crate::pkg::ModuleInfo {
        title: name.to_string(),
        bundle: crate::pkg::DEFAULT_BUNDLE.to_string(),
        place: "none".to_string(),
        size: None,
        focusable: true,
        region: None,
    }
}

fn package_host_grants(package: &VerifiedPackage) -> Vec<String> {
    package
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.get("host"))
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 签名包生成的清单授予：声明的宿主方法加上全部已知事件。
fn package_grants(package: &VerifiedPackage) -> Vec<String> {
    let mut grants = package_host_grants(package);
    grants.extend(EVENTS.iter().map(|name| (*name).to_string()));
    grants
}

fn package_record(package: &VerifiedPackage) -> (Record, Preview) {
    let grants = package_grants(package);
    let preview = Preview {
        id: package.app.id.clone(),
        name: package.app.name.clone(),
        version: package.app.version.clone(),
        build: package.app.build,
        source: Source::Packaged,
        grants,
        files: 1 + usize::from(package.source_map.is_some()) + package.assets.len(),
        origin: PathBuf::new(),
        sha256: Some(package.sha256.clone()),
        min_runtime: None,
        capabilities: packaged_capabilities(),
        module: Some(packaged_module(&package.app.name)),
        warnings: package.ignored.clone(),
    };
    let record = Record {
        id: package.app.id.clone(),
        source: Source::Packaged,
        generation: 0,
        installed_at: now_unix(),
        enabled: true,
        manifest_hash: None,
        origin: None,
        sha256: Some(package.sha256.clone()),
        key_id: Some(package.key_id.clone()),
        app: Some(serde_json::json!({
            "id": package.app.id,
            "name": package.app.name,
            "version": package.app.version,
            "build": package.app.build,
        })),
        min_runtime: None,
        capabilities: Some(preview.capabilities),
        module: preview.module.clone(),
        grants: Some(preview.grants.clone()),
    };
    (record, preview)
}

/// 本地目录安装（拷贝到新代际）。发布顺序与插件安装一致：写记录 → 切指针。
pub fn install_local(config: &Config, dir: &Path) -> Result<Record, String> {
    install_local_from(config, dir, None)
}

/// 同 [`install_local`]，但记录里的来源可以指定成安装来源（git 地址或归档路径），
/// 而不是暂存目录。
pub fn install_local_from(
    config: &Config,
    dir: &Path,
    origin: Option<&str>,
) -> Result<Record, String> {
    let preview = preview_local(dir)?;
    let store = Store::open(config.mods_state_dir());
    let (generation, target) = store
        .prepare_generation(&preview.id)
        .map_err(|error| error.to_string())?;
    copy_tree(dir, &target).map_err(|error| format!("{}: {error}", target.display()))?;
    let record = Record {
        id: preview.id.clone(),
        source: Source::Local,
        generation,
        installed_at: now_unix(),
        enabled: true,
        manifest_hash: file_sha256(&target.join("hctui.toml")),
        origin: Some(origin.unwrap_or(&dir.display().to_string()).to_string()),
        sha256: None,
        key_id: None,
        app: Some(serde_json::json!({
            "name": preview.name,
            "version": preview.version,
            "build": preview.build,
        })),
        min_runtime: None,
        capabilities: Some(preview.capabilities),
        module: preview.module.clone(),
        grants: Some(preview.grants.clone()),
    };
    store
        .save_record(&record)
        .map_err(|error| error.to_string())?;
    store
        .set_current(&preview.id, &target)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

/// 签名包安装；先校验再写入新代际。
pub fn install_package(config: &Config, path: &Path) -> Result<Record, String> {
    let (_preview, package, mut trust) = verify_package(config, path)?;
    let (mut record, _) = package_record(&package);
    let store = Store::open(config.mods_state_dir());
    let (generation, target) = store
        .prepare_generation(&record.id)
        .map_err(|error| error.to_string())?;
    write_package(&target, &package).map_err(|error| error.to_string())?;
    trust
        .accept(&package)
        .map_err(|error| format!("accept package: {error}"))?;
    record.generation = generation;
    record.installed_at = now_unix();
    store
        .save_record(&record)
        .map_err(|error| error.to_string())?;
    store
        .set_current(&record.id, &target)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

fn write_package(target: &Path, package: &VerifiedPackage) -> std::io::Result<()> {
    let dist = target.join("dist");
    std::fs::create_dir_all(&dist)?;
    std::fs::write(dist.join("bundle.js"), &package.source)?;
    if let Some(map) = &package.source_map {
        std::fs::write(dist.join("bundle.js.map"), map)?;
    }
    let capabilities = package
        .capabilities
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(std::io::Error::other)?
        .unwrap_or_else(|| "{}".to_string());
    std::fs::write(dist.join("capabilities.json"), capabilities)?;
    std::fs::write(
        dist.join("app.json"),
        serde_json::to_string(&serde_json::json!({
            "id": package.app.id,
            "name": package.app.name,
            "version": package.app.version,
            "build": package.app.build,
        }))
        .map_err(std::io::Error::other)?,
    )?;
    for (path, bytes) in &package.assets {
        let path = dist.join("assets").join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    std::fs::write(target.join("hctui.toml"), package_manifest(package))?;
    Ok(())
}

/// 签名包不带 `hctui.toml`：用包内能力生成最小清单，宿主方法照声明授予，
/// 事件按已知名授予（事件只读，方法仍逐项校验）。
fn package_manifest(package: &VerifiedPackage) -> String {
    let host = package_host_grants(package)
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let events = EVENTS
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[mod]\nid = \"{}\"\ntitle = \"{}\"\nplace = \"none\"\nhost_api = 1\n\n[grants]\nhost = [{host}]\nevents = [{events}]\n",
        package.app.id, package.app.name
    )
}

/// 安装或更新：目录走本地拷贝，`.nshpkg` 走签名校验；`id` 给出时校验一致。
pub fn update(config: &Config, id: Option<&str>, target: &Path) -> Result<Record, String> {
    let record = if target.is_dir() {
        if let Some(id) = id {
            let found = manifest_id(target)?;
            if found != id {
                return Err(format!(
                    "{} declares mod id {found}, not {id}",
                    target.display()
                ));
            }
        }
        install_local(config, target)?
    } else {
        let (_preview, package, mut trust) = verify_package(config, target)?;
        if let Some(id) = id {
            if package.app.id != id {
                return Err(format!(
                    "{} is app {}, not {id}",
                    target.display(),
                    package.app.id
                ));
            }
        }
        let (mut record, _) = package_record(&package);
        let store = Store::open(config.mods_state_dir());
        let (generation, dir) = store
            .prepare_generation(&record.id)
            .map_err(|error| error.to_string())?;
        write_package(&dir, &package).map_err(|error| error.to_string())?;
        trust
            .accept(&package)
            .map_err(|error| format!("accept package: {error}"))?;
        record.generation = generation;
        record.installed_at = now_unix();
        store
            .save_record(&record)
            .map_err(|error| error.to_string())?;
        store
            .set_current(&record.id, &dir)
            .map_err(|error| error.to_string())?;
        record
    };
    Ok(record)
}

/// 链接本地目录：不拷贝，`current` 直接指向源目录，改动后重载生效。
pub fn link(config: &Config, dir: &Path) -> Result<Record, String> {
    let loaded = crate::pkg::install_ready(dir)?;
    let id = loaded.manifest.id().to_string();
    let capabilities = loaded.manifest.capabilities();
    let module = loaded.manifest.module_info();
    let grants = loaded.manifest.grants();
    let store = Store::open(config.mods_state_dir());
    let target = std::path::absolute(dir).map_err(|error| error.to_string())?;
    let record = Record {
        id: id.clone(),
        source: Source::Linked,
        generation: 1,
        installed_at: now_unix(),
        enabled: true,
        manifest_hash: file_sha256(&target.join("hctui.toml")),
        origin: Some(target.display().to_string()),
        sha256: None,
        key_id: None,
        app: None,
        min_runtime: None,
        capabilities: Some(capabilities),
        module,
        grants: Some(grants),
    };
    store
        .save_record(&record)
        .map_err(|error| error.to_string())?;
    store
        .set_current(&id, &target)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

/// 取消链接；只对 linked 来源生效。
pub fn unlink(config: &Config, id: &str) -> Result<(), String> {
    let store = Store::open(config.mods_state_dir());
    let record = store.record(id).ok_or_else(|| format!("no mod {id}"))?;
    if record.source != Source::Linked {
        return Err(format!("{id} is {} , not linked", record.source.as_str()));
    }
    store.remove(id).map_err(|error| error.to_string())
}

/// 卸载：删除记录、指针、代际与日志。
pub fn uninstall(config: &Config, id: &str) -> Result<(), String> {
    let store = Store::open(config.mods_state_dir());
    if store.record(id).is_none() {
        return Err(format!("no mod {id}"));
    }
    store.remove(id).map_err(|error| error.to_string())
}

/// 模块的根目录：按包记录解析（两个 store 与两个开发目录由 [`crate::pkg::resolve`] 统一判断）。
pub fn module_root(config: &Config, id: &str) -> Option<PathBuf> {
    crate::pkg::module_root(config, id)
}

/// 记录里的启用状态；损坏记录按启用处理。
pub fn enabled(config: &Config, id: &str) -> bool {
    Store::open(config.mods_state_dir())
        .record(id)
        .map(|record| record.enabled)
        .unwrap_or(true)
}

/// 持久化启用状态；无记录时忽略。
pub fn set_enabled(config: &Config, id: &str, enabled: bool) -> Result<(), String> {
    let store = Store::open(config.mods_state_dir());
    let Some(mut record) = store.record(id) else {
        return Ok(());
    };
    record.enabled = enabled;
    store
        .save_record(&record)
        .map_err(|error| error.to_string())
}

/// 启动时把记录里的启用状态载入运行时。
pub fn apply_enabled(config: &Config, runtime: &mut crate::mods::ModRuntime) {
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        for record in Store::open(state).list() {
            if !record.enabled {
                runtime.enabled.insert(record.id, false);
            }
        }
    }
}

fn grants_path(config: &Config, id: &str) -> PathBuf {
    config
        .mods_state_dir()
        .join("grants")
        .join(format!("{id}.json"))
}

/// 读持久化的授予（CLI 在无窗口时写入）。
pub fn load_grants(config: &Config, id: &str) -> Vec<String> {
    std::fs::read_to_string(grants_path(config, id))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

/// 写持久化的授予。
pub fn save_grants(config: &Config, id: &str, grants: &[String]) -> Result<(), String> {
    let path = grants_path(config, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(grants).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn unique_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("hctui-mods-store-{tag}-{unique}"))
    }

    fn config_in(root: &Path) -> Config {
        Config {
            config_path: root.join("config").join("config.toml"),
            state_path: root.join("state").join("state.json"),
            ..Config::default()
        }
    }

    fn make_module(root: &Path, id: &str) -> PathBuf {
        let dir = root.join(format!("src-{id}"));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(
            dir.join("hctui.toml"),
            format!("[mod]\nid = \"{id}\"\nhost_api = 1\n\n[grants]\nhost = [\"state\"]\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("nsh.json"),
            format!("{{\"name\":\"{id}\",\"version\":\"1.0.0\",\"build\":1}}"),
        )
        .unwrap();
        std::fs::write(dir.join("dist").join("bundle.js"), "// bundle").unwrap();
        dir
    }

    #[test]
    fn install_update_and_uninstall_switch_generations() {
        let root = unique_dir("install");
        let config = config_in(&root);
        let source = make_module(&root, "probe");
        let first = install_local(&config, &source).unwrap();
        assert_eq!(first.generation, 1);
        let store = Store::open(config.mods_state_dir());
        let dir = store.current_dir("probe").expect("current");
        assert!(dir.join("dist").join("bundle.js").exists());

        std::fs::write(source.join("dist").join("bundle.js"), "// updated").unwrap();
        let second = update(&config, Some("probe"), &source).unwrap();
        assert_eq!(second.generation, 2);
        assert!(store.generation_dir("probe", 1).exists());
        // 旧代际仍被表面引用时不回收；引用消失后回收。
        assert!(store.prune("probe", &[1]).is_empty());
        assert_eq!(store.prune("probe", &[]), vec![1]);

        uninstall(&config, "probe").unwrap();
        assert!(store.record("probe").is_none());
        assert!(!store.generation_dir("probe", 2).exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn link_points_current_at_the_source_and_unlinks() {
        let root = unique_dir("link");
        let config = config_in(&root);
        let source = make_module(&root, "probe");
        link(&config, &source).unwrap();
        let store = Store::open(config.mods_state_dir());
        assert_eq!(
            store.current_dir("probe"),
            Some(std::path::absolute(&source).unwrap())
        );
        assert_eq!(store.record("probe").unwrap().source, Source::Linked);
        assert!(unlink(&config, "probe").is_ok());
        assert!(store.record("probe").is_none());
        std::fs::remove_dir_all(root).ok();
    }

    fn sign_package(
        root: &nsh_package::SecretKey,
        release: &nsh_package::SecretKey,
        build: u64,
    ) -> Vec<u8> {
        let now = crate::store::now_unix();
        let certificate = nsh_package::certify(
            root,
            &release.public(),
            &["dev.hctui.*".to_string()],
            now.saturating_sub(10),
            now + 3600,
        );
        let contents = nsh_package::Contents {
            app: nsh_package::App {
                id: "dev.hctui.probe".into(),
                name: "Probe".into(),
                version: "1.0.0".into(),
                build,
            },
            capabilities: Some(serde_json::json!({"host": ["state"]})),
            source: "// bundle".into(),
            bytecode: None,
            source_map: None,
            assets: Vec::new(),
            min_runtime: None,
        };
        nsh_package::build(&contents, release, &certificate).unwrap()
    }

    #[test]
    fn trusted_package_installs_and_an_older_build_is_refused() {
        let root_dir = unique_dir("package");
        let root = nsh_package::SecretKey::generate().unwrap();
        let release = nsh_package::SecretKey::generate().unwrap();
        let mut config = config_in(&root_dir);
        config.mods_trust_roots = vec![root.public().to_base64()];
        let newer = root_dir.join("newer.nshpkg");
        std::fs::create_dir_all(&root_dir).unwrap();
        std::fs::write(&newer, sign_package(&root, &release, 2)).unwrap();
        let record = install_package(&config, &newer).unwrap();
        assert_eq!(record.id, "dev.hctui.probe");
        assert_eq!(record.source, Source::Packaged);
        assert_eq!(record.generation, 1);
        let store = Store::open(config.mods_state_dir());
        let dir = store.current_dir(&record.id).unwrap();
        assert!(dir.join("dist").join("bundle.js").exists());
        let manifest = std::fs::read_to_string(dir.join("hctui.toml")).unwrap();
        assert!(manifest.contains("\"state\""));

        let older = root_dir.join("older.nshpkg");
        std::fs::write(&older, sign_package(&root, &release, 1)).unwrap();
        let error = install_package(&config, &older).unwrap_err();
        assert!(error.contains("older"), "{error}");
        std::fs::remove_dir_all(root_dir).ok();
    }

    #[test]
    fn package_needs_a_trusted_root_and_a_clean_revocation_list() {
        let root_dir = unique_dir("revoked");
        let root = nsh_package::SecretKey::generate().unwrap();
        let release = nsh_package::SecretKey::generate().unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        let package = root_dir.join("probe.nshpkg");
        std::fs::write(&package, sign_package(&root, &release, 1)).unwrap();

        let mut config = config_in(&root_dir);
        let error = install_package(&config, &package).unwrap_err();
        assert!(error.contains("trust roots"), "{error}");

        let wrong = nsh_package::SecretKey::generate().unwrap();
        config.mods_trust_roots = vec![wrong.public().to_base64()];
        let error = install_package(&config, &package).unwrap_err();
        assert!(error.contains("not trusted"), "{error}");

        config.mods_trust_roots = vec![root.public().to_base64()];
        let list = nsh_package::revoke(&root, 1, &[release.public().id()], &[]);
        let path = config.mods_revocations_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, list.to_json()).unwrap();
        let error = install_package(&config, &package).unwrap_err();
        assert!(error.contains("revoked"), "{error}");
        std::fs::remove_dir_all(root_dir).ok();
    }

    #[test]
    fn grants_persist_between_processes() {
        let root = unique_dir("grants");
        let config = config_in(&root);
        save_grants(&config, "probe", &["state".into(), "exec".into()]).unwrap();
        assert_eq!(
            load_grants(&config, "probe"),
            vec!["state".to_string(), "exec".to_string()]
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// 指针切换失败时记录已经落盘：下一次打开按记录修好指针，模块仍可加载。
    #[cfg(unix)]
    #[test]
    fn a_failed_pointer_switch_keeps_the_new_record() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = unique_dir("current-fail");
        let config = config_in(&root);
        let source = make_module(&root, "probe");
        let first = install_local(&config, &source).unwrap();
        let store = Store::open(config.mods_state_dir());
        let old_dir = store.current_dir("probe").expect("current");

        let current = store.root().join("current");
        let mode = std::fs::metadata(&current).unwrap().permissions().mode();
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(
            std::fs::write(current.join(".write-probe"), b"x").is_err(),
            "0o555 still accepts writes"
        );
        let failed = install_local(&config, &source).unwrap_err();
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(failed.to_lowercase().contains("denied"), "{failed}");

        // 记录先行：新记录已落盘，指针还停在旧代际。
        assert_eq!(
            store.record("probe").unwrap().generation,
            first.generation + 1,
            "the record must be published before the pointer switches"
        );
        assert_eq!(
            store.current_dir("probe").as_deref(),
            Some(old_dir.as_path())
        );

        let reopened = Store::open(config.mods_state_dir());
        let repaired = reopened.generation_dir("probe", first.generation + 1);
        assert_eq!(reopened.current_dir("probe"), Some(repaired.clone()));
        assert!(repaired.join("dist").join("bundle.js").is_file());
        assert_eq!(module_root(&config, "probe"), Some(repaired));
        std::fs::remove_dir_all(root).ok();
    }
}
