//! `claudit install` / `claudit uninstall`: wiring claudit's hooks into
//! Claude Code's user settings, safely and reversibly.
//!
//! The core is a pure transformation over the settings JSON ([`install`],
//! [`uninstall`]); [`install_settings`] and [`uninstall_settings`] wrap it
//! with file I/O (backup, atomic write, claudit's own state file).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::clock::Clock;
use crate::paths::Paths;
use crate::secure_fs;

/// Every hook event claudit records. Each gets one claudit hook group.
pub const EVENTS: [&str; 12] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "SubagentStart",
    "SubagentStop",
];

/// The `cleanupPeriodDays` value install guarantees at minimum.
pub const MIN_CLEANUP_PERIOD_DAYS: u64 = 365;

/// The `cleanupPeriodDays` setting as it was before install raised it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorCleanup {
    /// The key was absent (Claude Code's default applies).
    Absent,
    /// The key held this value.
    Value(Value),
}

/// The hook containers (the `hooks` object, event arrays) that claudit
/// owns: absent before install, or holding nothing but claudit's groups.
/// Uninstall drops exactly these once emptied, so a container the user
/// wrote, even an empty one, survives an install → uninstall round trip.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CreatedContainers {
    /// The `hooks` object itself.
    #[serde(default)]
    pub hooks: bool,
    /// Event arrays under `hooks`, by event name.
    #[serde(default)]
    pub events: Vec<String>,
}

/// What install changed that uninstall must undo. Kept in claudit's state
/// file between the two.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstallRecord {
    /// Set when install raised `cleanupPeriodDays`: what it was before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised_cleanup_from: Option<PriorCleanup>,
    /// The containers install created. `None` when unknown (no state file,
    /// or one written by an older claudit): uninstall then drops every
    /// container its removal empties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<CreatedContainers>,
}

/// The result of [`install`].
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// The new settings.
    pub settings: Value,
    /// What uninstall needs to restore the settings.
    pub record: InstallRecord,
}

/// Adds claudit's hook group (running `hook_command`) to every event in
/// [`EVENTS`], replacing any claudit entry already there, and raises
/// `cleanupPeriodDays` to [`MIN_CLEANUP_PERIOD_DAYS`] if it is lower.
/// Foreign hooks and unknown keys are left untouched and in order.
pub fn install(settings: &Value, hook_command: &str) -> Installed {
    let created = claudit_owned_containers(settings);
    let mut settings = remove_claudit_hooks(settings, None);
    let root = as_object(&mut settings);
    let raised_cleanup_from = raise_cleanup(root);
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = as_object(hooks);
    for event in EVENTS {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        if !groups.is_array() {
            *groups = Value::Array(Vec::new());
        }
        if let Value::Array(groups) = groups {
            groups.push(json!({
                "hooks": [{ "type": "command", "command": hook_command, "async": true }]
            }));
        }
    }
    Installed {
        settings,
        record: InstallRecord {
            raised_cleanup_from,
            created: Some(created),
        },
    }
}

/// Removes every claudit hook entry, drops the containers `record` says
/// install created once they are empty, and restores `cleanupPeriodDays` to
/// its pre-install value (unless the user has changed it since).
pub fn uninstall(settings: &Value, record: &InstallRecord) -> Value {
    let mut settings = remove_claudit_hooks(settings, record.created.as_ref());
    if let (Some(root), Some(prior)) = (
        settings.as_object_mut(),
        record.raised_cleanup_from.as_ref(),
    ) {
        restore_cleanup(root, prior);
    }
    settings
}

/// What [`install_settings`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallReport {
    /// The settings file that was (or would have been) changed.
    pub settings_file: PathBuf,
    /// The backup of the previous settings, when a file existed and changed.
    pub backup: Option<PathBuf>,
    /// False when claudit was already installed exactly this way.
    pub changed: bool,
    /// Set when this install raised `cleanupPeriodDays`.
    pub raised_cleanup_from: Option<PriorCleanup>,
}

/// What [`uninstall_settings`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct UninstallReport {
    pub settings_file: PathBuf,
    pub backup: Option<PathBuf>,
    /// False when there was nothing of claudit's to remove.
    pub changed: bool,
}

/// Applies [`install`] to Claude Code's user settings file: backs it up,
/// records the previous `cleanupPeriodDays` in claudit's state, then writes
/// the new settings atomically. Does nothing if already installed.
pub fn install_settings(
    paths: &Paths,
    hook_command: &str,
    clock: &dyn Clock,
) -> Result<InstallReport> {
    let settings_file = paths.claude_settings_file();
    let current = read_settings(&settings_file)?;
    let settings = current
        .as_ref()
        .map_or_else(|| json!({}), |c| c.value.clone());
    let installed = install(&settings, hook_command);
    let changed = current.is_none() || installed.settings != settings;
    let mut report = InstallReport {
        settings_file: settings_file.clone(),
        backup: None,
        changed,
        raised_cleanup_from: installed.record.raised_cleanup_from.clone(),
    };
    if !changed {
        return Ok(report);
    }
    let previous = read_state(paths)?;
    write_state(paths, &merge_records(previous, installed.record, &settings))?;
    if let Some(current) = &current {
        report.backup = Some(backup(&settings_file, &current.bytes, clock)?);
    }
    write_settings(&settings_file, &installed.settings)?;
    Ok(report)
}

/// Applies [`uninstall`] to Claude Code's user settings file, restoring the
/// `cleanupPeriodDays` recorded by install, after backing the file up.
pub fn uninstall_settings(paths: &Paths, clock: &dyn Clock) -> Result<UninstallReport> {
    let settings_file = paths.claude_settings_file();
    let state = read_state(paths)?;
    let mut report = UninstallReport {
        settings_file: settings_file.clone(),
        backup: None,
        changed: false,
    };
    if let Some(current) = read_settings(&settings_file)? {
        let restored = uninstall(&current.value, &state);
        if restored != current.value {
            report.backup = Some(backup(&settings_file, &current.bytes, clock)?);
            write_settings(&settings_file, &restored)?;
            report.changed = true;
        }
    }
    match fs::remove_file(paths.install_state_file()) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            return Err(err).context("remove claudit install state");
        }
        _ => {}
    }
    Ok(report)
}

struct SettingsFile {
    bytes: Vec<u8>,
    value: Value,
}

/// Reads and parses the settings file; `None` if it does not exist. An empty
/// file counts as `{}`. Anything unparsable is an error: never overwrite it.
fn read_settings(path: &Path) -> Result<Option<SettingsFile>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let value = if bytes.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not valid JSON; left untouched", path.display()))?
    };
    if !value.is_object() {
        anyhow::bail!("{} is not a JSON object; left untouched", path.display());
    }
    Ok(Some(SettingsFile { bytes, value }))
}

/// Copies the previous settings bytes next to the file, owner-only, under a
/// timestamped name that is never overwritten.
fn backup(settings_file: &Path, bytes: &[u8], clock: &dyn Clock) -> Result<PathBuf> {
    let stamp = clock.now().format("%Y%m%dT%H%M%S%.6fZ");
    let name = format!("settings.json.claudit-backup-{stamp}");
    let path = settings_file.with_file_name(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(secure_fs::FILE_MODE)
        .open(&path)
        .with_context(|| format!("create backup {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(path)
}

/// Writes `settings` atomically (temp file + rename), keeping the existing
/// file's permissions, or owner-only for a new file.
fn write_settings(path: &Path, settings: &Value) -> Result<()> {
    let dir = path
        .parent()
        .context("settings file has no parent directory")?;
    secure_fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mode = fs::metadata(path).map_or(secure_fs::FILE_MODE, |m| m.permissions().mode() & 0o777);
    let tmp = path.with_file_name(format!("settings.json.claudit-{}.tmp", std::process::id()));
    let mut text = serde_json::to_string_pretty(settings)?;
    text.push('\n');
    let written = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(secure_fs::FILE_MODE)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written.with_context(|| format!("write {}", path.display()))
}

/// Combines the record of an earlier install (still in the state file) with
/// this one's. The earlier record is closer to the user's original settings:
/// its prior `cleanupPeriodDays` wins, and its created containers stand, plus
/// any container that was absent from `settings` and so is new this time.
fn merge_records(
    previous: InstallRecord,
    current: InstallRecord,
    settings: &Value,
) -> InstallRecord {
    let created = match (previous.created, current.created) {
        (Some(mut previous), Some(current)) => {
            let hooks = settings.get("hooks");
            previous.hooks |= current.hooks && hooks.is_none();
            for event in current.events {
                let absent = hooks.and_then(|h| h.get(&event)).is_none();
                if absent && !previous.events.contains(&event) {
                    previous.events.push(event);
                }
            }
            Some(previous)
        }
        (previous, current) => previous.or(current),
    };
    InstallRecord {
        raised_cleanup_from: previous.raised_cleanup_from.or(current.raised_cleanup_from),
        created,
    }
}

fn read_state(paths: &Paths) -> Result<InstallRecord> {
    let path = paths.install_state_file();
    match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(InstallRecord::default()),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

fn write_state(paths: &Paths, state: &InstallRecord) -> Result<()> {
    let path = paths.install_state_file();
    secure_fs::create_dir_all(paths.home())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(secure_fs::FILE_MODE)
        .open(&path)
        .with_context(|| format!("write {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, state)?;
    file.sync_all()?;
    Ok(())
}

/// The hook command for the claudit binary at `exe`: `<exe> hook`, with the
/// path single-quoted for the shell when it needs to be.
pub fn hook_command(exe: &Path) -> String {
    let exe = exe.to_string_lossy();
    let plain = exe
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+@%:,".contains(c));
    if plain {
        format!("{exe} hook")
    } else {
        format!("'{}' hook", exe.replace('\'', r"'\''"))
    }
}

/// Whether a hook `command` runs claudit's hook: its program's file name is
/// `claudit` (wherever it lives, quoted or not) and its only argument `hook`.
/// Matching on the file name lets a moved or upgraded binary be recognised.
pub fn is_claudit_command(command: &str) -> bool {
    let command = command.trim();
    let (program, rest) = match command.strip_prefix(['\'', '"']) {
        Some(quoted) => {
            let quote = command.chars().next().unwrap_or('\'');
            match quoted.split_once(quote) {
                Some(split) => split,
                None => return false,
            }
        }
        None => command
            .split_once(char::is_whitespace)
            .unwrap_or((command, "")),
    };
    let file_name = program.rsplit('/').next().unwrap_or(program);
    file_name == "claudit" && rest.trim() == "hook"
}

/// The hook containers claudit owns in `settings`, before install touches
/// them: those absent, and those holding only claudit groups (left by an
/// earlier install). An empty container the user wrote is not claudit's.
fn claudit_owned_containers(settings: &Value) -> CreatedContainers {
    let hooks = settings.get("hooks").and_then(Value::as_object);
    let owned_event = |event: &str| match hooks.and_then(|h| h.get(event)) {
        Some(Value::Array(groups)) => !groups.is_empty() && groups.iter().all(is_claudit_group),
        Some(_) => false,
        None => true,
    };
    let events: Vec<String> = EVENTS
        .into_iter()
        .filter(|event| owned_event(event))
        .map(String::from)
        .collect();
    let hooks_owned = match hooks {
        None => settings.get("hooks").is_none(),
        Some(hooks) => {
            !hooks.is_empty()
                && hooks
                    .iter()
                    .all(|(event, groups)| groups.is_array() && owned_event(event))
        }
    };
    CreatedContainers {
        hooks: hooks_owned,
        events,
    }
}

/// Removes claudit's hook entries from every event, and the groups they
/// were the only handlers of. An event array or `hooks` object left empty
/// is dropped when `created` lists it as claudit's; with `created` unknown,
/// when this removal emptied it. Everything else is untouched.
fn remove_claudit_hooks(settings: &Value, created: Option<&CreatedContainers>) -> Value {
    let mut settings = settings.clone();
    let Some(root) = settings.as_object_mut() else {
        return settings;
    };
    let Some(Value::Object(hooks)) = root.get_mut("hooks") else {
        return settings;
    };
    let events_before = hooks.len();
    hooks.retain(|event, groups| {
        let Value::Array(groups) = groups else {
            return true;
        };
        let groups_before = groups.len();
        groups.retain_mut(|group| {
            let Some(Value::Array(handlers)) = group.get_mut("hooks") else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| !is_claudit_handler(handler));
            handlers.len() == before || !handlers.is_empty()
        });
        let emptied_by_removal = groups.is_empty() && groups_before > 0;
        let drop = match created {
            Some(created) => groups.is_empty() && created.events.iter().any(|e| e == event),
            None => emptied_by_removal,
        };
        !drop
    });
    let drop_hooks = match created {
        Some(created) => hooks.is_empty() && created.hooks,
        None => hooks.is_empty() && events_before > 0,
    };
    if drop_hooks {
        root.remove("hooks");
    }
    settings
}

/// Raises `cleanupPeriodDays` to the minimum if lower (absent counts as
/// lower: Claude Code defaults to 30). A non-numeric value is left alone.
fn raise_cleanup(root: &mut Map<String, Value>) -> Option<PriorCleanup> {
    let prior = match root.get("cleanupPeriodDays") {
        None => PriorCleanup::Absent,
        Some(value) if value.as_f64()? < MIN_CLEANUP_PERIOD_DAYS as f64 => {
            PriorCleanup::Value(value.clone())
        }
        Some(_) => return None,
    };
    root.insert("cleanupPeriodDays".into(), json!(MIN_CLEANUP_PERIOD_DAYS));
    Some(prior)
}

/// Restores `cleanupPeriodDays` to `prior`, unless the user has changed it
/// since install (it no longer holds the value install wrote).
fn restore_cleanup(root: &mut Map<String, Value>, prior: &PriorCleanup) {
    if root.get("cleanupPeriodDays") != Some(&json!(MIN_CLEANUP_PERIOD_DAYS)) {
        return;
    }
    match prior {
        PriorCleanup::Absent => {
            root.remove("cleanupPeriodDays");
        }
        PriorCleanup::Value(value) => {
            root.insert("cleanupPeriodDays".into(), value.clone());
        }
    }
}

/// A hook group whose every handler runs claudit.
fn is_claudit_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|handlers| !handlers.is_empty() && handlers.iter().all(is_claudit_handler))
}

fn is_claudit_handler(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_claudit_command)
}

/// The settings root as an object; a non-object root is replaced by `{}`.
fn as_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    match value {
        Value::Object(map) => map,
        _ => unreachable!("just made an object"),
    }
}
