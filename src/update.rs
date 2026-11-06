//! Verified release archives and atomic replacement; no build toolchain at runtime.
use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::i18n::{t, t1, t2};

const REPOSITORY: &str = "wuxiaobai24/hc-tui";

pub fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => bail!(t2("no release archive for {}/{}", os, arch)),
    }
}

fn version(value: &str) -> Result<&str> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        bail!(t("expected a release version such as 0.4.0"));
    }
    Ok(value)
}

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(crate::shared::identity("hc-tui-update-"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn download(url: &str, path: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "10",
            "--max-time",
            "180",
            "--output",
        ])
        .arg(path)
        .arg(url)
        .status()
        .context(t("update needs curl on PATH"))?;
    if !status.success() {
        bail!(t1("download failed: {}", url));
    }
    Ok(())
}

fn version_from_release_url(url: &str) -> Result<String> {
    let prefix = format!("https://github.com/{REPOSITORY}/releases/tag/");
    Ok(version(
        url.trim()
            .strip_prefix(&prefix)
            .context(t("latest release redirected to an unexpected URL"))?,
    )?
    .to_string())
}

fn latest_version() -> Result<String> {
    // The web redirect is public and avoids the anonymous GitHub API quota.
    let output = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--head",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "--output",
            "/dev/null",
            "--write-out",
            "%{url_effective}",
        ])
        .arg(format!("https://github.com/{REPOSITORY}/releases/latest"))
        .output()
        .context(t("update needs curl on PATH"))?;
    if !output.status.success() {
        bail!(
            "{}",
            t1(
                "cannot resolve latest release: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        );
    }
    version_from_release_url(&String::from_utf8(output.stdout)?)
}

fn checksum(archive: &Path, expected: &Path) -> Result<()> {
    let text = fs::read_to_string(expected).context(t("archive needs its .sha256 file"))?;
    let expected = text
        .split_whitespace()
        .next()
        .context(t("empty SHA-256 file"))?;
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!(t("invalid SHA-256 file"));
    }
    let output = if cfg!(target_os = "macos") {
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(archive)
            .output()
    } else {
        Command::new("sha256sum").arg(archive).output()
    }
    .context(t("cannot calculate SHA-256"))?;
    let actual = String::from_utf8(output.stdout)?;
    if !output.status.success() || actual.split_whitespace().next() != Some(expected) {
        bail!(t("SHA-256 mismatch; installed executable was not changed"));
    }
    Ok(())
}

fn unpack(archive: &Path, directory: &Path) -> Result<PathBuf> {
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .context(t("update needs tar on PATH"))?;
    if !listing.status.success() {
        bail!(t("cannot list release archive"));
    }
    let listing = String::from_utf8(listing.stdout)?;
    let name = listing
        .lines()
        .find(|line| *line == "./hc-tui" || *line == "hc-tui")
        .context(t("archive has no hc-tui executable at its root"))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .arg(name)
        .status()?;
    let candidate = directory.join("hc-tui");
    if !status.success() || !fs::symlink_metadata(&candidate)?.file_type().is_file() {
        bail!(t("archive executable is not a regular file"));
    }
    Ok(candidate)
}

fn install(candidate: &Path, executable: &Path) -> Result<()> {
    let parent = executable
        .parent()
        .context(t("executable has no parent directory"))?;
    let staged = parent.join(crate::shared::identity(".hc-tui-update-"));
    let result = (|| -> Result<()> {
        let mut input = fs::File::open(candidate)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .context(t(
                "cannot stage update beside executable; check directory permissions",
            ))?;
        std::io::copy(&mut input, &mut output)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        output.flush()?;
        output.sync_all()?;
        fs::rename(&staged, executable).context(t("cannot atomically replace executable"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

pub fn run(args: &[String]) -> Result<()> {
    let mut requested = None;
    let mut from = None;
    let mut check = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => {
                requested =
                    Some(version(args.next().context(t("--version needs a value"))?)?.to_string())
            }
            "--from" => {
                from = Some(PathBuf::from(
                    args.next().context(t("--from needs a .tar.gz archive"))?,
                ))
            }
            "--check" => check = true,
            "--help" | "-h" => {
                println!(
                    "{}",
                    t("hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]\nArchives require a matching archive.tar.gz.sha256. Updates replace this executable atomically.")
                );
                return Ok(());
            }
            arg => bail!(t1("unknown update option: {}", arg)),
        }
    }
    if check && from.is_some() {
        bail!(t("--check cannot be combined with --from"));
    }
    let temporary = Temporary::new()?;
    let (archive, expected_version) = if let Some(archive) = from {
        (fs::canonicalize(archive)?, requested)
    } else {
        let latest = match &requested {
            Some(version) => version.clone(),
            None => latest_version()?,
        };
        println!(
            "{}",
            t2(
                "installed {} · release {}",
                env!("CARGO_PKG_VERSION"),
                &latest
            )
        );
        if check
            || (requested.is_none()
                && numeric_version(&latest)? <= numeric_version(env!("CARGO_PKG_VERSION"))?)
        {
            return Ok(());
        }
        let asset = format!("hc-tui-{latest}-{}.tar.gz", target()?);
        for name in [&asset, &format!("{asset}.sha256")] {
            let url = format!("https://github.com/{REPOSITORY}/releases/download/v{latest}/{name}");
            download(&url, &temporary.0.join(name))?;
        }
        (temporary.0.join(asset), Some(latest))
    };
    let hash = PathBuf::from(format!("{}.sha256", archive.display()));
    checksum(&archive, &hash)?;
    let unpacked = temporary.0.join("unpacked");
    fs::create_dir(&unpacked)?;
    let candidate = unpack(&archive, &unpacked)?;
    let output = Command::new(&candidate)
        .arg("--version")
        .output()
        .context(t("downloaded executable cannot run on this machine"))?;
    let output_version = String::from_utf8(output.stdout)?;
    let value = version(
        output_version
            .trim()
            .strip_prefix("hc-tui ")
            .context(t("invalid executable version output"))?,
    )?;
    if !output.status.success()
        || expected_version
            .as_deref()
            .is_some_and(|expected| expected != value)
    {
        bail!(t("executable version does not match release"));
    }
    install(&candidate, &std::env::current_exe()?)?;
    println!(
        "{}",
        t1(
            "updated hc-tui to {}; reopen running clients to use it",
            value
        )
    );
    Ok(())
}

fn numeric_version(value: &str) -> Result<(u64, u64, u64)> {
    let parts: Vec<_> = version(value)?
        .split('.')
        .map(str::parse::<u64>)
        .collect::<std::result::Result<_, _>>()?;
    Ok((parts[0], parts[1], parts[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_cannot_escape_release_paths() {
        for invalid in ["../main", "v1.2", "1.2.3/../../bad", "1.2.3-rc1", "1..3"] {
            assert!(version(invalid).is_err());
        }
        assert_eq!(version("v0.4.0").unwrap(), "0.4.0");
        assert_eq!(
            version_from_release_url("https://github.com/wuxiaobai24/hc-tui/releases/tag/v0.4.1")
                .unwrap(),
            "0.4.1"
        );
        assert!(version_from_release_url("https://example.com/releases/tag/v0.4.1").is_err());
        assert!(numeric_version("0.10.0").unwrap() > numeric_version("0.9.9").unwrap());
    }
    #[test]
    fn checksum_failure_leaves_destination_unchanged_and_install_replaces_atomically() {
        let dir = Temporary::new().unwrap();
        let candidate = dir.0.join("candidate");
        let destination = dir.0.join("installed");
        let digest = dir.0.join("candidate.sha256");
        fs::write(&candidate, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        fs::write(&digest, "0".repeat(64)).unwrap();
        assert!(checksum(&candidate, &digest).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        install(&candidate, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 3);
    }
}
