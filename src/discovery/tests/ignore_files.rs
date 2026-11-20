//! Ignore files are loaded by discovery itself: same results as the `ignore` crate's walker, and
//! a file under the root that resolves outside it is refused.

use super::*;
use crate::config::ConfigOverrides;
use crate::tests::property_runner::generated;
use proptest::prelude::*;
use std::collections::BTreeSet;

/// Directories always created under the root; index 7 in an ignore case is the root's parent.
const DIRS: [&str; 7] = ["", "a", "a/b", ".h", ".h/a", "c", "c/b"];
const PARENT: u8 = 7;
const FILES: [&str; 4] = ["x.feature", ".y.feature", "z.feature", "w.txt"];
const KINDS: [&str; 4] = [
    ".gitignore",
    ".ignore",
    ".cuke-dedupignore",
    ".git/info/exclude",
];
const LINES: [&[u8]; 18] = [
    b"x.feature",
    b"!x.feature",
    b"/z.feature",
    b"a/",
    b"!a/",
    b"b",
    b"!b",
    b"*.feature",
    b"!*.feature",
    b".h",
    b"!.h",
    b"!.y.feature",
    b"**/b/x.feature",
    b"# comment",
    b"",
    b"{a",
    b"\xEF\xBB\xBFz.feature",
    b"\xFFbad",
];

/// A file: directory index and name index.
type FileCase = (u8, u8);
/// An ignore file: directory index (or `PARENT`), kind index, line indexes and CRLF endings.
type IgnoreCase = (u8, u8, Vec<u8>, bool);

/// Writes the tree under `outer/root` and returns the canonical root.
fn materialize(outer: &Path, files: &[FileCase], ignores: &[IgnoreCase]) -> PathBuf {
    let root = outer.join("root");
    for dir in DIRS {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for (dir, name) in files {
        let path = root
            .join(DIRS[usize::from(*dir)])
            .join(FILES[usize::from(*name)]);
        fs::write(path, "Feature: x\n").unwrap();
    }
    for (dir, kind, lines, crlf) in ignores {
        let base = if *dir == PARENT {
            outer.to_path_buf()
        } else {
            root.join(DIRS[usize::from(*dir)])
        };
        let path = base.join(KINDS[usize::from(*kind)]);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ending: &[u8] = if *crlf { b"\r\n" } else { b"\n" };
        let text = lines
            .iter()
            .map(|line| LINES[usize::from(*line)])
            .collect::<Vec<_>>()
            .join(ending);
        fs::write(path, text).unwrap();
    }
    crate::config::canonical_platform_path(&root).unwrap()
}

/// The `ignore` crate's walker configured as discovery configured it before owning ignore files:
/// selected feature paths and the number of walk errors.
fn reference(root: &Path, include_hidden: bool) -> (BTreeSet<String>, usize) {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .add_custom_ignore_filename(".cuke-dedupignore")
        .require_git(false)
        .hidden(!include_hidden)
        .follow_links(false);
    let (mut selected, mut errors) = (BTreeSet::new(), 0);
    for entry in builder.build() {
        let Ok(entry) = entry else {
            errors += 1;
            continue;
        };
        let name = entry.file_name().to_string_lossy();
        if entry.file_type().is_some_and(|kind| kind.is_file()) && name.ends_with(".feature") {
            selected.insert(relative(root, entry.path()));
        }
    }
    (selected, errors)
}

/// `path` under `root` with `/` separators.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

/// Discovery's selected feature paths and errors for `root`.
fn discovered(root: &Path, include_hidden: bool) -> Result<(BTreeSet<String>, usize)> {
    let overrides = ConfigOverrides {
        features: Some(vec!["**/*.feature".to_owned()]),
        include_hidden: Some(include_hidden),
        ..ConfigOverrides::default()
    };
    let config = Config::load(root, overrides)?;
    let files = discover(&config)?;
    let selected = files
        .features
        .iter()
        .map(|file| relative(root, &file.path))
        .collect();
    Ok((selected, files.errors.len()))
}

/// Generated trees select exactly the files the `ignore` crate's walker selects, with as many
/// walk errors: per-level precedence of the three ignore kinds and git exclude, ancestor files,
/// whitelists overriding parents and the hidden filter, BOMs, CRLF, bad globs and non-UTF-8 lines.
#[test]
fn ignore_files_match_the_crate_walker() {
    let file = (0u8..7, 0u8..4);
    let ignore_file = (
        prop_oneof![0u8..7, Just(PARENT)],
        0u8..4,
        prop::collection::vec(0u8..18, 0..5),
        any::<bool>(),
    );
    let strategy = (
        any::<bool>(),
        prop::collection::vec(file, 0..12),
        prop::collection::vec(ignore_file, 0..6),
    );
    generated(strategy, |(include_hidden, files, ignores)| {
        let outer = tempfile::tempdir().unwrap();
        let root = materialize(outer.path(), files, ignores);
        let expected = reference(&root, *include_hidden);
        let actual = discovered(&root, *include_hidden)
            .map_err(|e| TestCaseError::fail(format!("{e:#}")))?;
        prop_assert_eq!(actual, expected);
        Ok(())
    });
}

/// Fixed controls the generator might reach rarely: a nested whitelist reopening a file a parent
/// level ignores, a deeper `.gitignore` losing to a shallower `.cuke-dedupignore`, and an
/// ancestor `.gitignore`.
#[test]
fn ignore_precedence_controls_match_the_crate_walker() {
    let cases: [(&[FileCase], &[IgnoreCase]); 5] = [
        (
            &[(1, 0), (2, 0)],
            &[(0, 0, vec![7], false), (1, 0, vec![1], false)],
        ),
        (&[(2, 0)], &[(0, 2, vec![0], false), (2, 0, vec![1], false)]),
        (&[(0, 0), (0, 2)], &[(PARENT, 0, vec![0], false)]),
        // An invalid glob above the root is one walk error; under the root it is dropped.
        (&[(0, 0), (0, 2)], &[(PARENT, 0, vec![15, 0], false)]),
        (&[(0, 0), (0, 2)], &[(0, 0, vec![15, 0], false)]),
    ];
    for (files, ignores) in cases {
        let outer = tempfile::tempdir().unwrap();
        let root = materialize(outer.path(), files, ignores);
        for include_hidden in [false, true] {
            assert_eq!(
                discovered(&root, include_hidden).unwrap(),
                reference(&root, include_hidden),
                "{files:?} {ignores:?}"
            );
        }
    }
}

/// `.git` file forms resolve git exclude as the `ignore` crate's walker does: unreadable, without a
/// `gitdir:` line, without `commondir`, with an absolute or relative `commondir`, no final newline.
#[test]
fn git_file_forms_match_the_crate_walker() {
    let outer = tempfile::tempdir().unwrap();
    let admin = crate::config::canonical_platform_path(outer.path())
        .unwrap()
        .join("admin");
    let pointer = format!("gitdir: {}", admin.display());
    let absolute = format!("{}\n", admin.display());
    let forms: [(&[u8], Option<&[u8]>); 6] = [
        (b"\xFF\n", Some(b".\n")),
        (b"not a pointer\n", Some(b".\n")),
        (pointer.as_bytes(), None),
        (pointer.as_bytes(), Some(absolute.as_bytes())),
        (pointer.as_bytes(), Some(b".")),
        (pointer.as_bytes(), Some(b"\xFF")),
    ];
    for (dot_git, commondir) in forms {
        let _ = fs::remove_dir_all(outer.path().join("root"));
        let _ = fs::remove_dir_all(&admin);
        let root = materialize(outer.path(), &[(1, 0), (1, 2)], &[]);
        fs::create_dir_all(admin.join("info")).unwrap();
        fs::write(admin.join("info/exclude"), "x.feature\n").unwrap();
        if let Some(commondir) = commondir {
            fs::write(admin.join("commondir"), commondir).unwrap();
        }
        fs::write(root.join("a/.git"), dot_git).unwrap();
        assert_eq!(
            discovered(&root, false).unwrap(),
            reference(&root, false),
            "{dot_git:?} {commondir:?}"
        );
    }
}

/// An ignore file above the root past the input limit fails discovery.
#[test]
fn an_oversized_ancestor_ignore_file_is_fatal() {
    let outer = tempfile::tempdir().unwrap();
    let root = materialize(outer.path(), &[(0, 0)], &[]);
    let size = usize::try_from(crate::resource_limits::MAX_CONFIG_INPUT_BYTES).unwrap() + 1;
    fs::write(outer.path().join(".ignore"), "#".repeat(size)).unwrap();
    let error = discovered(&root, false).unwrap_err();
    assert!(
        crate::resource_limits::is_fatal_input_error(&error),
        "{error:#}"
    );
}

/// Levels are retained only along the walk's current path: a wide tree of sibling directories,
/// each with its own ignore file, never holds more levels than the deepest path.
#[test]
fn retained_ignore_levels_follow_the_walk_path() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    for index in 0..64 {
        let dir = root.join(format!("d{index}/e"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(root.join(format!("d{index}/.gitignore")), "z.feature\n").unwrap();
        fs::write(dir.join("x.feature"), "Feature: x\n").unwrap();
    }
    let root = crate::config::canonical_platform_path(&root).unwrap();
    let (rules, _) = super::super::ignore_rules::IgnoreRules::new(&root, false).unwrap();
    let mut builder = ignore::WalkBuilder::new(&root);
    builder.standard_filters(false).follow_links(false);
    let mut peak = 0;
    for entry in builder.build() {
        let entry = entry.unwrap();
        if entry.depth() > 0 {
            assert!(rules.admits(&entry));
            peak = peak.max(rules.retained());
        }
    }
    assert_eq!(peak, 3, "root, d<n> and d<n>/e");
}

#[cfg(unix)]
mod links {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Where the ignore file content lives.
    #[derive(Clone, Copy, Debug)]
    enum Form {
        Copy,
        LinkInside,
        LinkOutside,
    }

    /// Each ignore kind at the root and nested: copies and inside links apply, outside links
    /// fail discovery before the content is used.
    #[test]
    fn ignore_files_resolving_outside_are_refused() {
        for kind in KINDS {
            for dir in ["", "a"] {
                for form in [Form::Copy, Form::LinkInside, Form::LinkOutside] {
                    let outer = tempfile::tempdir().unwrap();
                    let outside = tempfile::tempdir().unwrap();
                    let root = materialize(outer.path(), &[(0, 0), (1, 0)], &[]);
                    let path = root.join(dir).join(kind);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let target = match form {
                        Form::Copy => path.clone(),
                        Form::LinkInside => root.join("c/ignore-source"),
                        Form::LinkOutside => outside.path().join("ignore-source"),
                    };
                    fs::write(&target, "x.feature\n").unwrap();
                    if !matches!(form, Form::Copy) {
                        symlink(&target, &path).unwrap();
                    }
                    let label = format!("{kind} in {dir:?} {form:?}");
                    match form {
                        Form::Copy | Form::LinkInside => {
                            let (selected, _) = discovered(&root, false).unwrap();
                            let expected = if dir.is_empty() {
                                &[][..]
                            } else {
                                &["x.feature"][..]
                            };
                            assert_eq!(
                                selected.iter().map(String::as_str).collect::<Vec<_>>(),
                                expected,
                                "{label}"
                            );
                        }
                        Form::LinkOutside => {
                            let error = discovered(&root, false).unwrap_err();
                            assert!(
                                format!("{error:#}").contains("resolves outside the analysis root"),
                                "{label}: {error:#}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A refused ignore file is fatal even when exclusions drop every entry after it (the
    /// directory itself is walked; `a/*` would also match `a/` and never enter it).
    #[test]
    fn a_refusal_before_excluded_entries_is_fatal() {
        for kind in &KINDS[..3] {
            for exclude in ["a/?*", "**/a/?*"] {
                let outer = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let root = outer.path().join("root");
                fs::create_dir_all(root.join("a")).unwrap();
                fs::write(root.join("a/x.feature"), "Feature: x\n").unwrap();
                let target = outside.path().join("ignore-source");
                fs::write(&target, "z.feature\n").unwrap();
                symlink(&target, root.join("a").join(kind)).unwrap();
                let overrides = ConfigOverrides {
                    features: Some(vec!["**/*.feature".to_owned()]),
                    exclude: Some(vec![exclude.to_owned()]),
                    ..ConfigOverrides::default()
                };
                let config = Config::load(&root, overrides).unwrap();
                let error = discover(&config).expect_err(&format!("{kind} {exclude}"));
                assert!(
                    format!("{error:#}").contains("resolves outside the analysis root"),
                    "{kind} {exclude}: {error:#}"
                );
            }
        }
    }

    /// Git metadata a `.git` file points at inside the root is contained: an `info/exclude` or a
    /// `commondir` there linked outside is refused; as regular files they apply.
    #[test]
    fn git_metadata_inside_the_root_is_contained() {
        let cases = [None, Some("info/exclude"), Some("commondir")];
        for (linked, relative) in cases.into_iter().flat_map(|l| [(l, false), (l, true)]) {
            let outer = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::create_dir_all(outer.path().join("root/a")).unwrap();
            let root = crate::config::canonical_platform_path(&outer.path().join("root")).unwrap();
            let admin = root.join("admin");
            fs::create_dir_all(admin.join("info")).unwrap();
            fs::write(root.join("a/x.feature"), "Feature: x\n").unwrap();
            fs::write(root.join("a/z.feature"), "Feature: z\n").unwrap();
            // `ignore` reads a relative `gitdir:` against the working directory.
            let pointer = if relative {
                let cwd = std::env::current_dir().unwrap();
                let up = "../".repeat(cwd.components().count() - 1);
                PathBuf::from(up).join(admin.strip_prefix("/").unwrap())
            } else {
                admin.clone()
            };
            fs::write(
                root.join("a/.git"),
                format!("gitdir: {}\n", pointer.display()),
            )
            .unwrap();
            for (name, content) in [("info/exclude", "x.feature\n"), ("commondir", ".\n")] {
                let path = admin.join(name);
                if linked == Some(name) {
                    let target = outside.path().join("source");
                    fs::write(&target, content).unwrap();
                    symlink(&target, &path).unwrap();
                } else {
                    fs::write(&path, content).unwrap();
                }
            }
            match linked {
                None => {
                    let (selected, _) = discovered(&root, false).unwrap();
                    assert_eq!(
                        selected.iter().map(String::as_str).collect::<Vec<_>>(),
                        ["a/z.feature"]
                    );
                }
                Some(name) => {
                    let error = discovered(&root, false).expect_err(&format!("{name} {relative}"));
                    assert!(
                        format!("{error:#}").contains("resolves outside the analysis root"),
                        "{name} {relative}: {error:#}"
                    );
                }
            }
        }
    }
}
