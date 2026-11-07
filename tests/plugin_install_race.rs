//! 真跨进程安装：两个独立的 `hc-tui plugins install` 进程共用同一个状态目录。
//!
//! 同进程用例（`plugin::registry::tests::two_installers_racing_leave_one_generation`）
//! 碰不到 `stale_lock` 的同 PID 例外，这里用两个真进程覆盖包锁的跨进程语义：
//! 一个装完另一个才拿到锁，被 SIGKILL 的持有者留下的锁由下一个进程接管。

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");
const ID: &str = "dev.example.race";
/// 复制负载足够大，让两个安装者的复制窗口真的重叠。
const PAYLOAD: usize = 48 * 1024 * 1024;

struct Workspace {
    root: PathBuf,
}

impl Workspace {
    fn new(tag: &str) -> Workspace {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("hctui-install-race-{tag}-{unique}"));
        let workspace = Workspace { root };
        std::fs::create_dir_all(workspace.config().join("hc-tui")).unwrap();
        std::fs::write(workspace.config().join("hc-tui").join("config.toml"), "").unwrap();
        std::fs::create_dir_all(workspace.source()).unwrap();
        std::fs::write(
            workspace.source().join("plugin.toml"),
            format!(
                "manifest_version = 1\nid = \"{ID}\"\nname = \"Race\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n"
            ),
        )
        .unwrap();
        std::fs::write(workspace.source().join("payload.bin"), vec![7u8; PAYLOAD]).unwrap();
        workspace
    }

    fn source(&self) -> PathBuf {
        self.root.join("source")
    }

    fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// 插件的代际存储根：`<state>/hc-tui/plugins/`。
    fn plugin_state(&self) -> PathBuf {
        self.state().join("hc-tui").join("plugins")
    }

    fn package_dir(&self) -> PathBuf {
        self.plugin_state().join("store").join(ID)
    }

    /// 隔离 XDG 目录后的一次 `plugins install`。
    fn install(&self) -> Command {
        let mut command = Command::new(BIN);
        command
            .arg("plugins")
            .arg("install")
            .arg(self.source())
            .arg("--yes")
            .env("XDG_STATE_HOME", self.state())
            .env("XDG_CONFIG_HOME", self.config())
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env_remove("HC_SESSION")
            .env_remove("HCTUI_SOCKET");
        command
    }

    /// 安装后该有的全部不变量：一条记录、指针等于记录代际、无锁无暂存。
    fn assert_published(&self) -> u64 {
        let records = self.plugin_state().join("records");
        let record_files: Vec<String> = std::fs::read_dir(&records)
            .unwrap_or_else(|error| panic!("cannot list {}: {error}", records.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(record_files, vec![format!("{ID}.json")], "one record only");

        let record: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(records.join(format!("{ID}.json"))).unwrap(),
        )
        .unwrap();
        let generation = record["generation"].as_u64().unwrap();
        let published = self.package_dir().join(generation.to_string());
        assert!(
            published.join("plugin.toml").is_file(),
            "record points at a missing generation {}",
            published.display()
        );
        assert_eq!(
            std::fs::read_link(self.plugin_state().join("current").join(ID)).unwrap(),
            published,
            "current does not follow the record"
        );

        let leftovers: Vec<String> = std::fs::read_dir(self.package_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".tmp-") || name == ".lock")
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        generation
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 输出里的 `installed <id> generation <n> (...)` 一行。
fn reported_generation(output: &Output) -> u64 {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains(" generation "))
        .unwrap_or_else(|| panic!("no generation line in {stdout:?}"));
    line.split_whitespace()
        .skip_while(|word| *word != "generation")
        .nth(1)
        .and_then(|word| word.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("cannot read the generation from {line:?}"))
}

fn killed_status(output: &Output) -> String {
    format!(
        "{:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn two_processes_installing_the_same_plugin_serialize() {
    let workspace = Workspace::new("race");
    // 两个进程同时起跑，各自都是完整的 `hc-tui plugins install`。
    let first = workspace
        .install()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("first installer starts");
    let second = workspace
        .install()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("second installer starts");
    let mut first = first;
    assert!(
        first.try_wait().unwrap().is_none(),
        "the first installer already exited before the second one started"
    );
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();

    // 本用例写死的判定：两个安装者都必须退出码 0。锁超时（30s）不算通过，
    // 那条分支由 `a_killed_installer_leaves_a_lock_the_next_process_takes_over` 覆盖。
    assert!(
        first.status.success(),
        "first installer: {}",
        killed_status(&first)
    );
    assert!(
        second.status.success(),
        "second installer: {}",
        killed_status(&second)
    );

    let mut generations = vec![reported_generation(&first), reported_generation(&second)];
    generations.sort_unstable();
    assert_ne!(
        generations[0], generations[1],
        "installers shared one generation: {generations:?}"
    );

    // 每次安装都是新代际，两代都完整落盘。
    for generation in &generations {
        let dir = workspace.package_dir().join(generation.to_string());
        assert!(
            dir.join("plugin.toml").is_file(),
            "{} missing",
            dir.display()
        );
        let size = std::fs::metadata(dir.join("payload.bin")).unwrap().len();
        assert_eq!(
            size, PAYLOAD as u64,
            "generation {generation} is incomplete"
        );
    }

    let published = workspace.assert_published();
    assert_eq!(
        published, generations[1],
        "the record is not the last generation"
    );
}

#[test]
fn a_killed_installer_leaves_a_lock_the_next_process_takes_over() {
    let workspace = Workspace::new("killed");
    let mut child = workspace
        .install()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("installer starts");
    let lock = workspace.package_dir().join(".lock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !lock.exists() {
        assert!(
            Instant::now() < deadline,
            "installer never took the package lock"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    child.kill().expect("installer is killed");
    let status = child.wait().unwrap();
    // SIGKILL 不执行 `PackageLock` 的 Drop：锁文件带着死掉的持有者 PID 留下。
    assert!(
        status.code().is_none(),
        "installer finished before it could be killed: {status:?}"
    );
    assert!(lock.exists(), "lock file vanished after SIGKILL");
    if let Ok(text) = std::fs::read_to_string(&lock) {
        if let Ok(holder) = text.trim().parse::<u32>() {
            assert_ne!(holder, std::process::id(), "the lock is held by this test");
        }
    }

    let output = workspace
        .install()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("second installer runs");
    assert!(
        output.status.success(),
        "the next process did not take over the dead lock: {}",
        killed_status(&output)
    );
    let published = workspace.assert_published();
    assert_eq!(published, reported_generation(&output));
    assert_eq!(
        std::fs::metadata(
            workspace
                .package_dir()
                .join(published.to_string())
                .join("payload.bin")
        )
        .unwrap()
        .len(),
        PAYLOAD as u64
    );
}
