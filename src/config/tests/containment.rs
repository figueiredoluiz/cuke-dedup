//! Automatically found configuration is read only when it resolves inside the analysis root.

use super::*;
use std::fs;

/// One automatic source: its path under the root, a payload and the setting the payload changes.
struct Site {
    name: &'static str,
    payload: &'static str,
    applied: fn(&Config) -> bool,
}

/// Every automatic configuration source read through a file path under the root.
fn sites() -> [Site; 6] {
    [
        Site {
            name: "package.json",
            payload: r#"{"cukeDedup":{"threshold":37}}"#,
            applied: |config| config.threshold == 37.0,
        },
        Site {
            name: ".cuke-dedup.json",
            payload: r#"{"threshold":42}"#,
            applied: |config| config.threshold == 42.0,
        },
        Site {
            name: ".config/cuke-dedup.json",
            payload: r#"{"threshold":42}"#,
            applied: |config| config.threshold == 42.0,
        },
        Site {
            name: ".config/.cuke-dedup.json",
            payload: r#"{"threshold":42}"#,
            applied: |config| config.threshold == 42.0,
        },
        Site {
            name: "cuke-dedup.config.json",
            payload: r#"{"threshold":42}"#,
            applied: |config| config.threshold == 42.0,
        },
        Site {
            name: "cucumber.json",
            payload: r#"{"default":{"paths":["marker.feature"]}}"#,
            applied: |config| config.features == ["marker.feature"],
        },
    ]
}

/// How the source reaches its content.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// A regular file at the path.
    Copy,
    /// A link to a file elsewhere under the root.
    LinkInside,
    /// A link to a file outside the root.
    LinkOutside,
    /// The path's parent directory is a link to a directory outside the root.
    ParentOutside,
}

/// Links `path` to `target`.
#[cfg(unix)]
fn link(target: &Path, path: &Path) {
    std::os::unix::fs::symlink(target, path).unwrap();
}

/// Writes `site` under `root` in `form`, with outside content under `outside`.
#[cfg(unix)]
fn materialize(root: &Path, outside: &Path, site: &Site, form: Form) {
    let path = root.join(site.name);
    let parent = Path::new(site.name)
        .parent()
        .filter(|p| !p.as_os_str().is_empty());
    match form {
        Form::Copy => {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, site.payload).unwrap();
        }
        Form::LinkInside => {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::create_dir_all(root.join("shared")).unwrap();
            let target = root.join("shared/config-source");
            fs::write(&target, site.payload).unwrap();
            link(&target, &path);
        }
        Form::LinkOutside => {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let target = outside.join("config-source");
            fs::write(&target, site.payload).unwrap();
            link(&target, &path);
        }
        Form::ParentOutside => {
            let parent = parent.expect("a nested site");
            let target = outside.join("linked-dir");
            fs::create_dir_all(&target).unwrap();
            fs::write(
                target.join(Path::new(site.name).file_name().unwrap()),
                site.payload,
            )
            .unwrap();
            link(&target, &root.join(parent));
        }
    }
}

/// Copies and inside links load; outside links and outside parents fail before applying content.
#[cfg(unix)]
#[test]
fn automatic_sources_load_only_inside_the_root() {
    for site in sites() {
        let nested = Path::new(site.name).components().count() > 1;
        for form in [
            Form::Copy,
            Form::LinkInside,
            Form::LinkOutside,
            Form::ParentOutside,
        ] {
            if matches!(form, Form::ParentOutside) && !nested {
                continue;
            }
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            materialize(root.path(), outside.path(), &site, form);
            let loaded = Config::load(root.path(), ConfigOverrides::default());
            match form {
                Form::Copy | Form::LinkInside => {
                    let config = loaded.unwrap_or_else(|e| panic!("{} {form:?}: {e:#}", site.name));
                    assert!((site.applied)(&config), "{} {form:?}", site.name);
                }
                Form::LinkOutside | Form::ParentOutside => {
                    let error = loaded.expect_err(&format!("{} {form:?} loaded", site.name));
                    let message = format!("{error:#}");
                    // `--config` replaces only the standalone candidates.
                    let remedy = match site.name {
                        "package.json" | "cucumber.json" => {
                            "replace the link with a file inside the root"
                        }
                        _ => "pass it with --config to use it explicitly",
                    };
                    assert!(
                        message.contains("resolves outside the analysis root")
                            && message.ends_with(remedy),
                        "{} {form:?}: {message}",
                        site.name
                    );
                    assert!(is_fatal_input_error(&error), "{} {form:?}", site.name);
                }
            }
        }
    }
}

/// An explicit `--config` outside the root stays allowed.
#[test]
fn explicit_config_outside_the_root_loads() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("external.json");
    fs::write(&path, r#"{"threshold":42}"#).unwrap();
    let overrides = ConfigOverrides {
        config_file: Some(path),
        ..ConfigOverrides::default()
    };
    let config = Config::load(root.path(), overrides).unwrap();
    assert_eq!(config.threshold, 42.0);
}

/// A refused standalone candidate stops loading instead of falling through to the next one.
#[cfg(unix)]
#[test]
fn a_refused_candidate_does_not_fall_through() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("config-source");
    fs::write(&target, r#"{"threshold":42}"#).unwrap();
    link(&target, &root.path().join(".cuke-dedup.json"));
    fs::write(
        root.path().join("cuke-dedup.config.json"),
        r#"{"threshold":7}"#,
    )
    .unwrap();
    let error = Config::load(root.path(), ConfigOverrides::default()).unwrap_err();
    assert!(
        format!("{error:#}").contains(".cuke-dedup.json"),
        "{error:#}"
    );
}
