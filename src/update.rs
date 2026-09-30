//! `claudit update`: replacing the running binary with the latest GitHub
//! release, the one network access claudit ever makes, and only when asked.
//!
//! The latest release is found from the redirect of `/releases/latest`
//! (no API, no token, no rate limit); its archive is downloaded with its
//! published SHA-256, checked, unpacked, and the new binary renamed over the
//! running one (atomic on the same filesystem). The download, checksum and
//! unpacking use the system's `curl`, `shasum` and `tar`, as the README's
//! install does. The version and naming rules are pure ([`Version`],
//! [`archive_name`]) and tested on their own.

use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The repository releases are published from.
pub const REPOSITORY: &str = "https://github.com/TristanShz/claudit";

/// A release version, `MAJOR.MINOR.PATCH`, as tagged (`v0.4.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parses `0.4.0` or `v0.4.0`; anything else (a pre-release suffix
    /// included) is `None`.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
        let version = Version {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
        };
        parts.next().is_none().then_some(version)
    }

    /// The version of this binary.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is MAJOR.MINOR.PATCH")
    }

    /// The release tag: `v0.4.0`.
    pub fn tag(self) -> String {
        format!("v{self}")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The version a `/releases/latest` redirect points to: the last path
/// segment of `…/releases/tag/v0.4.0`. `None` if the URL is not a tag page
/// (the repository has no release yet).
pub fn version_from_release_url(url: &str) -> Option<Version> {
    let url = url.trim().trim_end_matches('/');
    let (rest, tag) = url.rsplit_once('/')?;
    if !rest.ends_with("/releases/tag") {
        return None;
    }
    Version::parse(tag)
}

/// The release target this binary was built for, if releases ship one.
pub fn release_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// The release archive's base name, as `release.yml` packages it; the
/// archive holds `<name>/claudit`.
pub fn archive_name(version: Version, target: &str) -> String {
    format!("claudit-{}-{target}", version.tag())
}

/// What [`check`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateCheck {
    pub current: Version,
    pub latest: Version,
}

impl UpdateCheck {
    pub fn is_newer(&self) -> bool {
        self.latest > self.current
    }
}

/// Asks GitHub for the latest release.
pub fn check() -> Result<UpdateCheck> {
    let output = Command::new("curl")
        .args(["-fsSL", "-o", "/dev/null", "-w", "%{url_effective}"])
        .arg(format!("{REPOSITORY}/releases/latest"))
        .output()
        .context("run curl (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "could not reach {REPOSITORY}/releases/latest: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&output.stdout);
    let latest = version_from_release_url(&url)
        .with_context(|| format!("no release found at {REPOSITORY}/releases (got {url})"))?;
    Ok(UpdateCheck {
        current: Version::current(),
        latest,
    })
}

/// Refuses to overwrite a binary another tool manages: `cargo install`
/// keeps its own record of what it installed.
pub fn ensure_self_managed(exe: &Path) -> Result<()> {
    if exe.components().any(|c| c.as_os_str() == ".cargo") {
        bail!(
            "{} was installed with cargo; update it with:\n  cargo install --locked --force --git {REPOSITORY}",
            exe.display()
        );
    }
    Ok(())
}

/// Downloads `version` for this platform, checks its SHA-256 and renames
/// the new binary over `exe`.
pub fn install_release(exe: &Path, version: Version) -> Result<()> {
    let target = release_target()
        .context("releases only ship macOS binaries; build from source with `cargo install`")?;
    let name = archive_name(version, target);
    let dir = exe.parent().context("the binary has no parent directory")?;
    let work = WorkDir::create(dir)?;

    let archive = format!("{name}.tar.gz");
    let base = format!("{REPOSITORY}/releases/download/{}", version.tag());
    for file in [archive.clone(), format!("{archive}.sha256")] {
        run(
            Command::new("curl")
                .args(["-fsSL", "-o", &file])
                .arg(format!("{base}/{file}"))
                .current_dir(&work.0),
            &format!("download {file}"),
        )?;
    }
    run(
        Command::new("shasum")
            .args(["-a", "256", "-c", &format!("{archive}.sha256")])
            .current_dir(&work.0),
        &format!("checksum of {archive} does not match; nothing was installed"),
    )?;
    run(
        Command::new("tar")
            .args(["-xzf", &archive])
            .current_dir(&work.0),
        &format!("unpack {archive}"),
    )?;

    let new = work.0.join(&name).join("claudit");
    fs::set_permissions(&new, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("make {} executable", new.display()))?;
    fs::rename(&new, exe).with_context(|| format!("replace {}", exe.display()))?;
    Ok(())
}

fn run(command: &mut Command, what: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("{what}: could not start {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!("{what}: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// A scratch directory next to the binary, so the final rename stays on
/// one filesystem; removed on drop.
struct WorkDir(PathBuf);

impl WorkDir {
    fn create(parent: &Path) -> Result<WorkDir> {
        let path = parent.join(format!(".claudit-update-{}", std::process::id()));
        fs::create_dir(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(WorkDir(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
