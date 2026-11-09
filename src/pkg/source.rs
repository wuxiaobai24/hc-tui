//! 包的安装来源：本地目录、git 仓库与 tarball。
//!
//! 三种来源统一先落成一个暂存目录 [`Staged`]：git 用 `git clone --depth 1`，
//! tarball 用 `tar` 解包，本地目录直接用原路径。暂存目录由 [`Staged`] 持有，
//! 离开作用域时删除，安装失败不留残余。
//!
//! 安装过程只复制文件、读清单，不执行包里的任何脚本；git 地址只来自用户输入，
//! 不从清单里读地址再递归拉取。[`remote_version`] 读来源端的版本：git 来源用
//! `git ls-remote` 找到默认分支，再浅取一份提交只读清单，不 clone 整仓。

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{load_dir, parse_file, MANIFEST_FILE, PLUGIN_FILE};

/// 清单所在的最深层级：包放在仓库或归档里的唯一子目录时逐层下行。
const MAX_DESCENT: usize = 3;

/// 来源形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// 本地目录，直接安装，不落暂存目录。
    Dir(PathBuf),
    /// git 仓库地址（`https://`、`ssh://`、`file://`、`git@` 或 `.git` 结尾）。
    Git(String),
    /// `.tar.gz` / `.tgz` 归档文件。
    Archive(PathBuf),
    /// 其他路径：签名包或其他文件，由调用方按旧规则处理。
    Other,
}

/// 按用户输入判断来源形态。
pub fn kind(src: &str) -> Kind {
    let path = Path::new(src);
    if path.is_dir() {
        return Kind::Dir(path.to_path_buf());
    }
    if path.is_file() {
        return if archive_name(src) {
            Kind::Archive(path.to_path_buf())
        } else {
            Kind::Other
        };
    }
    if git_address(src) {
        Kind::Git(src.to_string())
    } else {
        Kind::Other
    }
}

fn archive_name(src: &str) -> bool {
    let name = src.to_ascii_lowercase();
    name.ends_with(".tar.gz") || name.ends_with(".tgz")
}

fn git_address(src: &str) -> bool {
    src.starts_with("https://")
        || src.starts_with("http://")
        || src.starts_with("ssh://")
        || src.starts_with("file://")
        || src.starts_with("git://")
        || src.starts_with("git@")
        || src.ends_with(".git")
}

/// 一个已落地的安装来源。
#[derive(Debug)]
pub struct Staged {
    path: PathBuf,
    origin: String,
    owned: Option<PathBuf>,
}

impl Staged {
    /// 包根目录：清单所在的那一层。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 记录里保存的来源：git 地址原样，归档是路径。
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if let Some(root) = &self.owned {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// 把来源落成一个暂存目录；本地目录直接返回原路径，不复制。
pub fn stage(src: &str) -> Result<Staged, String> {
    match kind(src) {
        Kind::Dir(path) => Ok(Staged {
            path,
            origin: src.to_string(),
            owned: None,
        }),
        Kind::Git(url) => clone(&url),
        Kind::Archive(path) => unpack(&path),
        Kind::Other => Err(format!(
            "{src}: not a directory, a git repository or a .tar.gz/.tgz archive"
        )),
    }
}

/// 来源端的清单版本。
pub fn remote_version(src: &str) -> Result<String, String> {
    match kind(src) {
        Kind::Git(url) => git_version(&url),
        Kind::Dir(dir) => manifest_version(&dir),
        Kind::Archive(_) => {
            let staged = stage(src)?;
            manifest_version(staged.path())
        }
        Kind::Other => Err(format!(
            "{src}: only a directory, a git repository or a .tar.gz/.tgz has a manifest to compare"
        )),
    }
}

fn manifest_version(dir: &Path) -> Result<String, String> {
    let loaded = load_dir(dir).map_err(|error| error.to_string())?;
    Ok(loaded.manifest.version().to_string())
}

/// 临时目录的唯一根：`<tmp>/hc-tui-source-<tag>-<pid>-<nano>`。
fn temp_root(tag: &str) -> Result<PathBuf, String> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!(
        "hc-tui-source-{tag}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    Ok(root)
}

fn clone(url: &str) -> Result<Staged, String> {
    let root = temp_root("git")?;
    let package = root.join("package");
    if let Err(error) = git(&[
        "clone",
        "--depth",
        "1",
        "--quiet",
        "--",
        url,
        &package.to_string_lossy(),
    ]) {
        let _ = std::fs::remove_dir_all(&root);
        return Err(format!("clone failed: {error}"));
    }
    Ok(Staged {
        path: package_root(&package),
        origin: url.to_string(),
        owned: Some(root),
    })
}

fn unpack(archive: &Path) -> Result<Staged, String> {
    let root = temp_root("tar")?;
    let package = root.join("package");
    let unpacked = std::fs::create_dir_all(&package).and_then(|()| {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&package)
            .output()
    });
    match unpacked {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let _ = std::fs::remove_dir_all(&root);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!("unpack failed: {}", reason(&stderr)));
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!("{}: {error}", archive.display()));
        }
    }
    Ok(Staged {
        path: package_root(&package),
        origin: archive.display().to_string(),
        owned: Some(root),
    })
}

/// 解出来的包根：有清单就在这一层，只有唯一子目录时逐层下行。
fn package_root(dir: &Path) -> PathBuf {
    let mut current = dir.to_path_buf();
    for _ in 0..MAX_DESCENT {
        if has_manifest(&current) {
            break;
        }
        let mut children: Vec<PathBuf> = std::fs::read_dir(&current)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default();
        if children.len() != 1 {
            break;
        }
        current = children.remove(0);
    }
    current
}

fn has_manifest(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file() || dir.join(PLUGIN_FILE).is_file()
}

/// 执行一条 git 命令；失败时把 stderr 原样带回去。
fn git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        // 不交互：没有凭据就立刻失败，不在安装过程中停下来等输入。
        .env("GIT_TERMINAL_PROMPT", "0")
        // 不跑 LFS 过滤器：安装不执行包里的任何东西。
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args(args)
        .output()
        .map_err(|error| format!("git is not available: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(reason(&stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn reason(stderr: &str) -> String {
    if stderr.is_empty() {
        "command failed without a message".to_string()
    } else {
        stderr.lines().collect::<Vec<_>>().join(" ")
    }
}

/// 远端默认分支的名字；`git ls-remote --symref <url> HEAD` 的 `ref: ` 行给出。
fn default_branch(url: &str) -> Result<String, String> {
    let listing = git(&["ls-remote", "--symref", "--", url, "HEAD"])
        .map_err(|error| format!("clone failed: cannot reach {url}: {error}"))?;
    for line in listing.lines() {
        let Some(rest) = line.strip_prefix("ref: ") else {
            continue;
        };
        let Some((name, target)) = rest.split_once('\t') else {
            continue;
        };
        if target.trim() == "HEAD" {
            return Ok(name.trim().to_string());
        }
    }
    Err(format!("clone failed: {url} has no HEAD branch"))
}

fn git_version(url: &str) -> Result<String, String> {
    let branch = default_branch(url)?;
    let root = temp_root("check")?;
    let result = shallow_version(url, &branch, &root);
    let _ = std::fs::remove_dir_all(&root);
    result
}

/// 浅取一份提交，只把清单读出来；不 clone 整仓。
fn shallow_version(url: &str, branch: &str, root: &Path) -> Result<String, String> {
    let repo = root.join("repo");
    let repo = repo.to_string_lossy().to_string();
    git(&["init", "--quiet", &repo])?;
    git(&[
        "-C",
        &repo,
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "--no-tags",
        "--",
        url,
        branch,
    ])
    .map_err(|error| format!("clone failed: {url}: {error}"))?;
    let mut prefix = String::new();
    for _ in 0..MAX_DESCENT {
        for name in [MANIFEST_FILE, PLUGIN_FILE] {
            let path = format!("{prefix}{name}");
            let Ok(text) = git(&["-C", &repo, "show", &format!("FETCH_HEAD:{path}")]) else {
                continue;
            };
            let loaded = parse_file(Path::new(&path), &text).map_err(|error| error.to_string())?;
            return Ok(loaded.manifest.version().to_string());
        }
        let Some(child) = single_tree(&repo)? else {
            break;
        };
        prefix.push_str(&child);
        prefix.push('/');
    }
    Err(format!(
        "{url}: no {MANIFEST_FILE} or {PLUGIN_FILE} at the head of {branch}"
    ))
}

/// 提交里唯一的目录名；根目录不止一项或那一项不是目录时返回 `None`。
fn single_tree(repo: &str) -> Result<Option<String>, String> {
    let listing = git(&["-C", repo, "ls-tree", "FETCH_HEAD"])?;
    let mut names: Vec<(bool, String)> = Vec::new();
    for line in listing.lines() {
        let Some((meta, name)) = line.split_once('\t') else {
            continue;
        };
        names.push((meta.contains(" tree "), name.to_string()));
    }
    match names.as_slice() {
        [(true, name)] => Ok(Some(name.clone())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::PLUGIN_FILE;

    fn unique_root(tag: &str) -> PathBuf {
        let root = crate::testutil::unique_dir("hctui-source", tag);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_package(dir: &Path, version: &str) {
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!("manifest_version = 2\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"{version}\"\nhost_api = 1\n\n[module]\nbundle = \"dist/bundle.js\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("dist").join("bundle.js"), "// bundle").unwrap();
    }

    /// 暂存根的临时目录：路径里第一个 `hc-tui-source-` 层。
    fn staging_root(path: &Path) -> PathBuf {
        path.ancestors()
            .find(|step| {
                step.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("hc-tui-source-"))
            })
            .unwrap()
            .to_path_buf()
    }

    /// 建一个本地仓库并提交；返回仓库路径。
    fn git_repo(tag: &str, version: &str) -> PathBuf {
        let root = unique_root(tag);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        write_package(&repo, version);
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["add", "-A"],
            vec![
                "-c",
                "user.email=m11c@example.com",
                "-c",
                "user.name=m11c",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ] {
            let mut command = Command::new("git");
            command.arg("-C").arg(&repo).args(args);
            assert!(
                command.output().unwrap().status.success(),
                "git fixture failed"
            );
        }
        repo
    }

    #[test]
    fn the_kind_follows_the_input_shape() {
        let root = unique_root("kind");
        assert_eq!(kind(&root.display().to_string()), Kind::Dir(root.clone()));
        assert!(matches!(kind("https://example.com/repo.git"), Kind::Git(_)));
        assert!(matches!(kind("git@example.com:repo.git"), Kind::Git(_)));
        assert!(matches!(kind("file:///tmp/repo.git"), Kind::Git(_)));
        let archive = root.join("probe.tar.gz");
        std::fs::write(&archive, "").unwrap();
        assert_eq!(kind(&archive.display().to_string()), Kind::Archive(archive));
        let signed = root.join("probe.nshpkg");
        std::fs::write(&signed, "").unwrap();
        assert_eq!(kind(&signed.display().to_string()), Kind::Other);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_git_repository_is_staged_from_a_shallow_clone() {
        let repo = git_repo("clone", "0.2.0");
        let url = format!("file://{}", repo.display());
        let staged = stage(&url).unwrap();
        assert_eq!(staged.origin(), url);
        let loaded = super::super::install_ready(staged.path()).unwrap();
        assert_eq!(loaded.manifest.version(), "0.2.0");
        let workspace = staging_root(staged.path());
        drop(staged);
        assert!(!workspace.exists(), "staging directory survived the drop");
        let parent = repo.parent().unwrap().to_path_buf();
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn a_tarball_is_unpacked_through_the_single_top_directory() {
        let root = unique_root("tar");
        let source = root.join("probe-0.2.0");
        write_package(&source, "0.2.0");
        let archive = root.join("probe.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&root)
            .arg("probe-0.2.0")
            .status()
            .unwrap();
        assert!(status.success());
        let staged = stage(&archive.display().to_string()).unwrap();
        assert!(staged.path().join(MANIFEST_FILE).is_file());
        assert_eq!(
            remote_version(&archive.display().to_string()).unwrap(),
            "0.2.0"
        );
        let temp = staging_root(staged.path());
        drop(staged);
        assert!(!temp.exists(), "staging directory survived the drop");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_remote_version_of_a_git_repository_comes_from_the_default_branch() {
        let repo = git_repo("version", "0.2.0");
        let url = format!("file://{}", repo.display());
        assert_eq!(remote_version(&url).unwrap(), "0.2.0");
        // 目录来源与工作区一致。
        assert_eq!(
            remote_version(&repo.display().to_string()).unwrap(),
            "0.2.0"
        );
        let parent = repo.parent().unwrap().to_path_buf();
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn a_repository_without_a_manifest_is_reported_not_staged() {
        let root = unique_root("nomanifest");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("README.md"), "nothing").unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["add", "-A"],
            vec![
                "-c",
                "user.email=m11c@example.com",
                "-c",
                "user.name=m11c",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ] {
            let mut command = Command::new("git");
            command.arg("-C").arg(&repo).args(args);
            assert!(command.output().unwrap().status.success());
        }
        let error = remote_version(&format!("file://{}", repo.display())).unwrap_err();
        assert!(error.contains(MANIFEST_FILE), "{error}");
        let stage_error = stage(&format!("file://{}", repo.display()));
        // 暂存成功但包根没有清单：安装前的校验会拒绝，这里只确认没有清单可读。
        if let Ok(staged) = stage_error {
            assert!(super::super::install_ready(staged.path()).is_err());
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_plugin_manifest_is_still_a_version_source() {
        let root = unique_root("v1");
        std::fs::write(
            root.join(PLUGIN_FILE),
            "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.3.0\"\nhost_api = 1\n",
        )
        .unwrap();
        assert_eq!(
            remote_version(&root.display().to_string()).unwrap(),
            "0.3.0"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
