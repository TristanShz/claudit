//! Seam 2: the settings transformation. `install` and `uninstall` are pure
//! functions from Claude Code settings JSON to settings JSON; one I/O test
//! covers the file wrapper (backup, permissions, CLAUDIT_HOME state).

mod common;

use claudit::install::{self, PriorCleanup};
use serde_json::{Value, json};

const CMD: &str = "'/Applications/claudit dir/claudit' hook";

const EVENTS: [&str; 12] = [
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

fn settings_fixture(name: &str) -> Value {
    let path = common::fixtures_dir().join("settings").join(name);
    let text = std::fs::read_to_string(&path).expect("read settings fixture");
    serde_json::from_str(&text).expect("parse settings fixture")
}

fn claudit_group() -> Value {
    json!({ "hooks": [{ "type": "command", "command": CMD, "async": true }] })
}

#[test]
fn install_adds_one_async_claudit_group_per_event() {
    let installed = install::install(&settings_fixture("minimal.json"), CMD);

    let hooks = installed.settings["hooks"]
        .as_object()
        .expect("hooks object");
    assert_eq!(hooks.len(), EVENTS.len());
    for event in EVENTS {
        assert_eq!(hooks[event], json!([claudit_group()]), "{event}");
    }
    assert_eq!(installed.settings["theme"], "dark");
}

#[test]
fn install_is_idempotent() {
    for fixture in ["minimal.json", "foreign_hooks.json", "low_retention.json"] {
        let once = install::install(&settings_fixture(fixture), CMD).settings;
        let twice = install::install(&once, CMD).settings;
        assert_eq!(twice, once, "{fixture}");
    }
}

#[test]
fn install_keeps_foreign_hooks_unmodified_and_in_order() {
    let original = settings_fixture("foreign_hooks.json");

    let installed = install::install(&original, CMD).settings;

    let hooks = &installed["hooks"];
    assert_eq!(
        hooks["PreToolUse"],
        json!([
            original["hooks"]["PreToolUse"][0],
            original["hooks"]["PreToolUse"][1],
            claudit_group(),
        ])
    );
    assert_eq!(
        hooks["Stop"],
        json!([original["hooks"]["Stop"][0], claudit_group()])
    );
    assert_eq!(hooks["PreCompact"], original["hooks"]["PreCompact"]);
    for key in ["model", "permissions", "statusLine", "someFutureSetting"] {
        assert_eq!(installed[key], original[key], "{key}");
    }
}

#[test]
fn install_replaces_claudit_hooks_from_a_moved_binary_instead_of_duplicating() {
    let installed = install::install(&settings_fixture("stale_claudit.json"), CMD).settings;

    let hooks = &installed["hooks"];
    assert_eq!(
        hooks["PostToolUse"],
        json!([
            { "matcher": "Bash", "hooks": [{ "type": "command", "command": "/Users/alice/bin/log.sh" }] },
            claudit_group(),
        ])
    );
    assert_eq!(hooks["Stop"], json!([claudit_group()]));
}

#[test]
fn claudit_commands_are_recognised_by_program_name() {
    for command in [
        "claudit hook",
        "/usr/local/bin/claudit hook",
        "'/opt/old place/claudit' hook",
        "\"/Users/alice/.cargo/bin/claudit\"  hook ",
    ] {
        assert!(install::is_claudit_command(command), "{command}");
    }
    for command in [
        "/Users/alice/bin/guard.sh",
        "claudit ingest",
        "not-claudit hook",
        "/usr/bin/claudit-wrapper hook",
        "echo claudit hook",
    ] {
        assert!(!install::is_claudit_command(command), "{command}");
    }
}

#[test]
fn the_installed_command_is_recognised_as_claudit() {
    use std::path::Path;

    assert_eq!(
        install::hook_command(Path::new("/usr/local/bin/claudit")),
        "/usr/local/bin/claudit hook"
    );
    assert_eq!(
        install::hook_command(Path::new("/Applications/claudit dir/claudit")),
        CMD
    );
    for exe in ["/usr/local/bin/claudit", "/Users/alice/my tools/claudit"] {
        let command = install::hook_command(Path::new(exe));
        assert!(install::is_claudit_command(&command), "{command}");
    }
}

#[test]
fn install_raises_retention_to_365_days_and_reports_the_previous_value() {
    let low = install::install(&settings_fixture("low_retention.json"), CMD);
    assert_eq!(low.settings["cleanupPeriodDays"], 365);
    assert_eq!(low.raised_cleanup_from, Some(PriorCleanup::Value(json!(7))));

    let absent = install::install(&settings_fixture("minimal.json"), CMD);
    assert_eq!(absent.settings["cleanupPeriodDays"], 365);
    assert_eq!(absent.raised_cleanup_from, Some(PriorCleanup::Absent));
}

#[test]
fn install_never_lowers_retention() {
    for (fixture, days) in [("high_retention.json", 1000), ("stale_claudit.json", 365)] {
        let installed = install::install(&settings_fixture(fixture), CMD);
        assert_eq!(installed.settings["cleanupPeriodDays"], days, "{fixture}");
        assert_eq!(installed.raised_cleanup_from, None, "{fixture}");
    }
}

#[test]
fn install_then_uninstall_returns_the_original_settings() {
    for fixture in [
        "minimal.json",
        "foreign_hooks.json",
        "low_retention.json",
        "high_retention.json",
    ] {
        let original = settings_fixture(fixture);
        let installed = install::install(&original, CMD);

        let restored =
            install::uninstall(&installed.settings, installed.raised_cleanup_from.as_ref());

        assert_eq!(restored, original, "{fixture}");
    }
}

#[test]
fn install_backs_up_the_settings_file_and_uninstall_restores_it() {
    use std::os::unix::fs::PermissionsExt;

    let env = common::TestEnv::new();
    let settings_file = env.paths.claude_settings_file();
    let original_text =
        std::fs::read_to_string(common::fixtures_dir().join("settings/low_retention.json"))
            .unwrap();
    std::fs::write(&settings_file, &original_text).unwrap();

    let report = install::install_settings(&env.paths, CMD, &env.clock).expect("install");

    let backup = report.backup.expect("a backup was written");
    assert_eq!(backup.parent(), Some(env.paths.claude_config_dir()));
    let name = backup.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("settings.json.claudit-backup-20260302T090000"),
        "{name}"
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), original_text);
    let mode = std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let installed: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
    assert_eq!(installed["cleanupPeriodDays"], 365);
    assert_eq!(installed["hooks"]["Stop"], json!([claudit_group()]));

    // A second install changes nothing, so it must not lose the recorded 7.
    install::install_settings(&env.paths, CMD, &env.clock).expect("reinstall");
    env.advance(chrono::Duration::seconds(5));
    install::uninstall_settings(&env.paths, &env.clock).expect("uninstall");

    let restored: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
    assert_eq!(
        restored,
        serde_json::from_str::<Value>(&original_text).unwrap()
    );
}

#[test]
fn uninstall_keeps_retention_the_user_changed_after_install() {
    let installed = install::install(&settings_fixture("low_retention.json"), CMD);
    let mut edited = installed.settings.clone();
    edited["cleanupPeriodDays"] = json!(90);

    let restored = install::uninstall(&edited, installed.raised_cleanup_from.as_ref());

    assert_eq!(restored["cleanupPeriodDays"], 90);
}
