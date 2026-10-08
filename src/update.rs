//! `claudit update`: replacing the running binary with the latest GitHub
//! release, the one network access claudit ever makes, and only when asked.
//!
//! The latest release is found from the redirect of `/releases/latest`
//! (no API, no token, no rate limit); its archive is downloaded with its
//! published SHA-256, checked, unpacked, and the new binary renamed over the
//! running one (atomic on the same filesystem; Windows, which cannot
//! overwrite a running binary, first moves it aside). The download and
//! unpacking use the system's `curl` and `tar` (both shipped with Windows
//! 10 and later), as the README's install does. The version and naming rules
//! are pure ([`Version`], [`archive_name`], [`archive_extension`]) and
//! tested on their own.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

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
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// The release archive's base name, as `release.yml` packages it; the
/// archive holds `<name>/claudit` (`claudit.exe` on Windows).
pub fn archive_name(version: Version, target: &str) -> String {
    format!("claudit-{}-{target}", version.tag())
}

/// The release archive's extension for `target`: `zip` on Windows, `tar.gz`
/// elsewhere.
pub fn archive_extension(target: &str) -> &'static str {
    if target.contains("-windows-") {
        "zip"
    } else {
        "tar.gz"
    }
}

/// Where curl discards the page it was redirected to.
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

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
        .args(["-fsSL", "-o", NULL_DEVICE, "-w", "%{url_effective}"])
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
    let target = release_target().with_context(|| {
        format!(
            "no release is published for {} {}; build from source with `cargo install`",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let name = archive_name(version, target);
    let dir = exe.parent().context("the binary has no parent directory")?;
    let work = WorkDir::create(dir)?;

    let archive = format!("{name}.{}", archive_extension(target));
    let checksum = format!("{archive}.sha256");
    let base = format!("{REPOSITORY}/releases/download/{}", version.tag());
    for file in [&archive, &checksum] {
        run(
            Command::new("curl")
                .args(["-fsSL", "-o", file])
                .arg(format!("{base}/{file}"))
                .current_dir(&work.0),
            &format!("download {file}"),
        )?;
    }
    verify_checksum(&work.0.join(&archive), &work.0.join(&checksum))
        .with_context(|| format!("check {archive}; nothing was installed"))?;
    run(
        Command::new("tar")
            .args(["-xf", &archive])
            .current_dir(&work.0),
        &format!("unpack {archive}"),
    )?;

    let new = work
        .0
        .join(&name)
        .join(format!("claudit{}", std::env::consts::EXE_SUFFIX));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("make {} executable", new.display()))?;
    }
    replace_binary(&new, exe)
}

/// Checks `archive` against the SHA-256 in `checksum` (`<hex>  <name>`, as
/// `shasum -a 256` writes it).
fn verify_checksum(archive: &Path, checksum: &Path) -> Result<()> {
    let text =
        fs::read_to_string(checksum).with_context(|| format!("read {}", checksum.display()))?;
    let expected = text
        .split_whitespace()
        .next()
        .with_context(|| format!("{} is empty", checksum.display()))?;
    let mut hasher = Sha256::new();
    let mut file =
        fs::File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    io::copy(&mut file, &mut hasher).with_context(|| format!("read {}", archive.display()))?;
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("SHA-256 mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

/// Renames `new` over `exe`. Windows cannot overwrite a running binary but
/// can rename it: the running one is moved to `<exe>.old` first (removed by
/// the next update), and moved back if the new one cannot take its place.
fn replace_binary(new: &Path, exe: &Path) -> Result<()> {
    if cfg!(windows) {
        let mut old = exe.as_os_str().to_owned();
        old.push(".old");
        let old = PathBuf::from(old);
        let _ = fs::remove_file(&old);
        fs::rename(exe, &old).with_context(|| format!("move {} aside", exe.display()))?;
        if let Err(err) = fs::rename(new, exe) {
            let _ = fs::rename(&old, exe);
            return Err(err).with_context(|| format!("replace {}", exe.display()));
        }
        return Ok(());
    }
    fs::rename(new, exe).with_context(|| format!("replace {}", exe.display()))
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
