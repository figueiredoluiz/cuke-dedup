use super::*;
use crate::resource_limits::MAX_PROJECT_INPUT_BYTES;
use crate::tests::property_runner::generated;
use proptest::prelude::*;

impl DiscoveryBudget {
    /// Test-local walk-entry, selected-file, source-byte and glob-evaluation limits.
    pub(crate) fn testing(limits: [usize; 4]) -> Self {
        Self::with_limits(limits)
    }
}

const UNITS: [&str; 4] = [
    "walk entries",
    "selected files",
    "selected source bytes",
    "glob evaluations",
];

/// A file below a top-level directory: kind (feature, TypeScript, opt-in Ruby, other) and size.
type FileCase = (u8, usize);

/// A top-level directory: kind (plain, excluded, gitignored, hidden) and its files.
type DirCase = (u8, Vec<FileCase>);

/// Independent totals from the tree model, plus the visible regular files the walk examines.
#[derive(Debug, Default)]
struct Expected {
    entries: usize,
    selected: usize,
    bytes: usize,
    walked_files: usize,
    features: usize,
}

/// Writes the modelled tree and returns its expected discovery totals.
fn materialize(root: &Path, dirs: &[DirCase]) -> Expected {
    let mut expected = Expected::default();
    let mut ignored = String::new();
    for (index, (kind, files)) in dirs.iter().enumerate() {
        let name = match kind {
            0 => format!("plain{index}"),
            1 => format!("plain{index}/node_modules"),
            2 => {
                ignored.push_str(&format!("ignored{index}/\n"));
                format!("ignored{index}")
            }
            _ => format!(".hidden{index}"),
        };
        let directory = root.join(&name);
        fs::create_dir_all(&directory).unwrap();
        match kind {
            // The plain parent and the excluded directory itself; nothing below it.
            1 => expected.entries += 2,
            0 => expected.entries += 1 + files.len(),
            _ => {}
        }
        for (position, (file_kind, size)) in files.iter().enumerate() {
            let extension = ["feature", "ts", "rb", "txt"][usize::from(*file_kind)];
            fs::write(
                directory.join(format!("f{position}.{extension}")),
                "x".repeat(*size),
            )
            .unwrap();
            if *kind != 0 {
                continue;
            }
            expected.walked_files += 1;
            match file_kind {
                0 => {
                    expected.features += 1;
                    expected.selected += 1;
                    expected.bytes += size;
                }
                1 => {
                    expected.selected += 1;
                    expected.bytes += size;
                }
                2 => expected.selected += 1,
                _ => {}
            }
        }
    }
    // Hidden, so the walk never yields or counts it.
    fs::write(root.join(".gitignore"), ignored).unwrap();
    expected
}

/// Small generated trees mixing every directory and file kind.
fn trees() -> impl Strategy<Value = Vec<DirCase>> {
    prop::collection::vec(
        (0_u8..4, prop::collection::vec((0_u8..4, 0_usize..48), 0..5)),
        0..6,
    )
}

/// Every cap at the model total admits the unbounded result; each cap one below rejects it.
#[test]
fn generated_discovery_admits_exact_totals_and_rejects_one_less() {
    generated(trees(), |dirs| {
        let directory = tempfile::tempdir().unwrap();
        let expected = materialize(directory.path(), dirs);
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        let unbounded = discover(&config).unwrap();
        let measured = DiscoveryBudget::default();
        prop_assert_eq!(&discover_within(&config, &measured).unwrap(), &unbounded);
        let spent = measured.spent();
        prop_assert_eq!(
            &spent[..3],
            &[expected.entries, expected.selected, expected.bytes][..]
        );
        let patterns = config.features.len();
        let lower = 2 * expected.entries;
        let upper = lower + expected.walked_files * (1 + patterns) + patterns * expected.features;
        prop_assert!(
            (lower..=upper).contains(&spent[3]),
            "glob evaluations {} outside {lower}..={upper}",
            spent[3]
        );

        let exact = DiscoveryBudget::testing(spent);
        prop_assert_eq!(&discover_within(&config, &exact).unwrap(), &unbounded);
        for unit in 0..4 {
            if spent[unit] == 0 {
                continue;
            }
            let mut limits = spent;
            limits[unit] -= 1;
            let error = discover_within(&config, &DiscoveryBudget::testing(limits))
                .unwrap_err()
                .to_string();
            prop_assert_eq!(
                error,
                format!(
                    "discovery exceeds {} {}; exclude generated or vendored directories or narrow \
                     the feature and definition globs",
                    limits[unit], UNITS[unit]
                )
            );
        }
        Ok(())
    });
}

/// Excluded and gitignored subtrees cost their own entry at most, however many files they hold.
#[test]
fn pruned_subtrees_are_not_charged_for_their_contents() {
    let directory = tempfile::tempdir().unwrap();
    for parent in ["node_modules", "ignored"] {
        fs::create_dir_all(directory.path().join(parent)).unwrap();
        for index in 0..50 {
            fs::write(directory.path().join(format!("{parent}/s{index}.ts")), "").unwrap();
        }
    }
    fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let budget = DiscoveryBudget::default();
    discover_within(&config, &budget).unwrap();
    assert_eq!(budget.spent()[..3], [1, 0, 0]);
}

/// A file over the per-file read cap charges only what the bounded reader can read.
#[test]
fn oversized_files_charge_the_bounded_read() {
    let directory = tempfile::tempdir().unwrap();
    let file = fs::File::create(directory.path().join("large.feature")).unwrap();
    file.set_len(MAX_PROJECT_INPUT_BYTES * 2).unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let budget = DiscoveryBudget::default();
    discover_within(&config, &budget).unwrap();
    let bounded = usize::try_from(MAX_PROJECT_INPUT_BYTES).unwrap() + 1;
    assert_eq!(budget.spent()[..3], [1, 1, bounded]);
}

/// Configured definition globs, suffix probes and unmatched-pattern passes are charged too.
#[test]
fn definition_pattern_work_is_charged() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("steps")).unwrap();
    fs::write(directory.path().join("steps/a.rb"), "").unwrap();
    fs::write(directory.path().join("steps/b.ts"), "").unwrap();
    let overrides = || ConfigOverrides {
        definitions: Some(vec!["steps/**".to_owned(), "missing/**".to_owned()]),
        ..ConfigOverrides::default()
    };
    let config = Config::load(directory.path(), overrides()).unwrap();
    let budget = DiscoveryBudget::default();
    let files = discover_within(&config, &budget).unwrap();
    assert_eq!(files.unmatched_definition_patterns, ["missing/**"]);
    let without = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let baseline = DiscoveryBudget::default();
    discover_within(&without, &baseline).unwrap();
    // `b.ts` stops at its first matching glob (1). The opt-in `a.rb` matches `steps/**`, probes it
    // without naming its suffix, then tries `missing/**` (3). The unmatched pass stops `steps/**`
    // at the first file and tries `missing/**` on both (3).
    assert_eq!(budget.spent()[3], baseline.spent()[3] + 1 + 3 + 3);
    // The unmatched definition pass runs last, so one evaluation less fails inside it.
    let mut limits = budget.spent();
    limits[3] -= 1;
    let error = discover_within(&config, &DiscoveryBudget::testing(limits)).unwrap_err();
    assert!(error.to_string().contains("glob evaluations"), "{error}");
}

/// Overlapping definition globs stop at the first match, so repeating one costs nothing per file.
#[test]
fn overlapping_definition_globs_stop_at_the_first_match() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..20 {
        fs::write(directory.path().join(format!("s{index}.ts")), "").unwrap();
    }
    let spent = |patterns: usize| {
        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                definitions: Some(vec!["**/*.ts".to_owned(); patterns]),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        let budget = DiscoveryBudget::default();
        assert_eq!(
            discover_within(&config, &budget).unwrap().definitions.len(),
            20
        );
        budget.spent()[3]
    };
    // Each extra pattern costs one evaluation in the unmatched pass, none per file.
    assert_eq!(spent(50) - spent(1), 49);
}

/// One budget spans every discovery of a run, so a second identical tree overflows it.
#[test]
fn a_shared_budget_accumulates_across_discoveries() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.feature"), "Feature: a").unwrap();
    fs::write(directory.path().join("b.ts"), "").unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let single = DiscoveryBudget::default();
    discover_within(&config, &single).unwrap();
    let once = single.spent();
    assert!(once.iter().all(|&total| total > 0), "{once:?}");
    let shared = DiscoveryBudget::default();
    discover_within(&config, &shared).unwrap();
    discover_within(&config, &shared).unwrap();
    assert_eq!(shared.spent(), once.map(|value| value * 2));

    let tight = DiscoveryBudget::testing(once);
    discover_within(&config, &tight).unwrap();
    let error = discover_within(&config, &tight).unwrap_err().to_string();
    assert!(error.starts_with("discovery exceeds "), "{error}");
}

/// Exhausting the entry budget stops the walk: no later charge or selection happens.
#[test]
fn entry_exhaustion_stops_before_selecting_more_files() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..20 {
        fs::write(directory.path().join(format!("s{index}.feature")), "").unwrap();
    }
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let budget = DiscoveryBudget::testing([3, usize::MAX, usize::MAX, usize::MAX]);
    let error = discover_within(&config, &budget).unwrap_err().to_string();
    assert!(
        error.starts_with("discovery exceeds 3 walk entries"),
        "{error}"
    );
    let spent = budget.spent();
    assert_eq!(spent[0], 4);
    assert!(spent[1] <= 3, "selected after exhaustion: {spent:?}");
}
