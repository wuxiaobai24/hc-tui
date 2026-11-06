//! Verified release archives and atomic replacement; no build toolchain at runtime.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPOSITORY: &str = "wuxiaobai24/hc-tui";

pub fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => bail!("no release archive for {os}/{arch}"),
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
        bail!("expected a release version such as 0.4.0");
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
        .context("update needs curl on PATH")?;
    if !status.success() {
        bail!("download failed: {url}");
    }
    Ok(())
}

fn checksum(archive: &Path, expected: &Path) -> Result<()> {
    let text = fs::read_to_string(expected).context("archive needs its .sha256 file")?;
    let expected = text
        .split_whitespace()
        .next()
        .context("empty SHA-256 file")?;
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid SHA-256 file");
    }
    let output = if cfg!(target_os = "macos") {
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(archive)
            .output()
    } else {
        Command::new("sha256sum").arg(archive).output()
    }
    .context("cannot calculate SHA-256")?;
    let actual = String::from_utf8(output.stdout)?;
    if !output.status.success() || actual.split_whitespace().next() != Some(expected) {
        bail!("SHA-256 mismatch; installed executable was not changed");
    }
    Ok(())
}

fn unpack(archive: &Path, directory: &Path) -> Result<PathBuf> {
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .context("update needs tar on PATH")?;
    if !listing.status.success() {
        bail!("cannot list release archive");
    }
    let listing = String::from_utf8(listing.stdout)?;
    let name = listing
        .lines()
        .find(|line| *line == "./hc-tui" || *line == "hc-tui")
        .context("archive has no hc-tui executable at its root")?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .arg(name)
        .status()?;
    let candidate = directory.join("hc-tui");
    if !status.success() || !fs::symlink_metadata(&candidate)?.file_type().is_file() {
        bail!("archive executable is not a regular file");
    }
    Ok(candidate)
}

fn install(candidate: &Path, executable: &Path) -> Result<()> {
    let parent = executable
        .parent()
        .context("executable has no parent directory")?;
    let staged = parent.join(crate::shared::identity(".hc-tui-update-"));
    let result = (|| -> Result<()> {
        let mut input = fs::File::open(candidate)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .context("cannot stage update beside executable; check directory permissions")?;
        std::io::copy(&mut input, &mut output)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        output.flush()?;
        output.sync_all()?;
        fs::rename(&staged, executable).context("cannot atomically replace executable")?;
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
                    Some(version(args.next().context("--version needs a value")?)?.to_string())
            }
            "--from" => {
                from = Some(PathBuf::from(
                    args.next().context("--from needs a .tar.gz archive")?,
                ))
            }
            "--check" => check = true,
            "--help" | "-h" => {
                println!("hc-tui update [--version X.Y.Z] [--check] [--from archive.tar.gz]\nArchives require a matching archive.tar.gz.sha256. Updates replace this executable atomically.");
                return Ok(());
            }
            arg => bail!("unknown update option: {arg}"),
        }
    }
    if check && from.is_some() {
        bail!("--check cannot be combined with --from");
    }
    let temporary = Temporary::new()?;
    let (archive, expected_version) = if let Some(archive) = from {
        (fs::canonicalize(archive)?, requested)
    } else {
        let endpoint = match &requested {
            Some(version) => {
                format!("https://api.github.com/repos/{REPOSITORY}/releases/tags/v{version}")
            }
            None => format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
        };
        let metadata = temporary.0.join("release.json");
        download(&endpoint, &metadata)?;
        let release: Value = serde_json::from_slice(&fs::read(metadata)?)?;
        let tag = release["tag_name"].as_str().context("release has no tag")?;
        let latest = version(tag)?.to_string();
        println!("installed {} · release {latest}", env!("CARGO_PKG_VERSION"));
        if check
            || (requested.is_none()
                && numeric_version(&latest)? <= numeric_version(env!("CARGO_PKG_VERSION"))?)
        {
            return Ok(());
        }
        let asset = format!("hc-tui-{latest}-{}.tar.gz", target()?);
        let assets = release["assets"]
            .as_array()
            .context("release has no assets")?;
        for name in [&asset, &format!("{asset}.sha256")] {
            let found = assets
                .iter()
                .find(|a| a["name"].as_str() == Some(name.as_str()))
                .with_context(|| format!("release is missing {name}; no update performed"))?;
            let url = found["browser_download_url"]
                .as_str()
                .context("asset has no download URL")?;
            let expected =
                format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}");
            if url != expected {
                bail!("unexpected release asset URL");
            }
            download(url, &temporary.0.join(name))?;
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
        .context("downloaded executable cannot run on this machine")?;
    let output_version = String::from_utf8(output.stdout)?;
    let value = version(
        output_version
            .trim()
            .strip_prefix("hc-tui ")
            .context("invalid executable version output")?,
    )?;
    if !output.status.success()
        || expected_version
            .as_deref()
            .is_some_and(|expected| expected != value)
    {
        bail!("executable version does not match release");
    }
    install(&candidate, &std::env::current_exe()?)?;
    println!("updated hc-tui to {value}; reopen running clients to use it");
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
