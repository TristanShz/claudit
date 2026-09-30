//! `claudit update`'s pure rules: reading the latest release from GitHub's
//! redirect, comparing versions, naming the archive `release.yml` publishes.
//! The download itself is not tested (it needs the network).

use claudit::update::{Version, archive_name, version_from_release_url};

fn v(major: u64, minor: u64, patch: u64) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

#[test]
fn parses_tags_and_plain_versions() {
    assert_eq!(Version::parse("v0.4.0"), Some(v(0, 4, 0)));
    assert_eq!(Version::parse("1.12.3"), Some(v(1, 12, 3)));
    for bad in ["", "v1", "v1.2", "v1.2.3.4", "v1.2.3-rc1", "vx.2.3"] {
        assert_eq!(Version::parse(bad), None, "{bad}");
    }
}

#[test]
fn compares_numerically_not_lexically() {
    assert!(v(0, 10, 0) > v(0, 9, 9));
    assert!(v(1, 0, 0) > v(0, 99, 99));
    assert!(v(0, 4, 1) > v(0, 4, 0));
}

#[test]
fn current_version_is_the_crate_version() {
    assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn reads_the_version_from_the_latest_release_redirect() {
    let url = "https://github.com/TristanShz/claudit/releases/tag/v0.5.0";
    assert_eq!(version_from_release_url(url), Some(v(0, 5, 0)));
    assert_eq!(
        version_from_release_url(&format!("{url}/\n")),
        Some(v(0, 5, 0))
    );
    // Without any release, GitHub redirects to the releases list.
    let none = "https://github.com/TristanShz/claudit/releases";
    assert_eq!(version_from_release_url(none), None);
}

#[test]
fn names_the_archive_as_the_release_workflow_does() {
    assert_eq!(
        archive_name(v(0, 4, 0), "aarch64-apple-darwin"),
        "claudit-v0.4.0-aarch64-apple-darwin"
    );
}
