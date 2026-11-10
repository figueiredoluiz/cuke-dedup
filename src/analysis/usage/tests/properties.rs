use super::*;
use crate::tests::property_runner;
use proptest::prelude::*;
use proptest::test_runner::TestCaseResult;

type Input = ([usize; 3], Vec<(usize, usize)>, [usize; 3], bool, usize);

/// Computes retained text/path costs from the owned finding contract, independently of production output.
fn group_fields(
    color: usize,
    count: usize,
    expansions: usize,
    suppressed: bool,
) -> (String, String, usize) {
    let message = if expansions == 1 {
        format!("Feature step `value c{color}` matches {count} definitions")
    } else {
        format!("Scenario outline step has {expansions} ambiguous expansions matching {count} definitions")
    };
    let difference = std::iter::once("`^value c[0-2]$`".to_owned())
        .chain(std::iter::repeat_n(format!("`value c{color}`"), count - 1))
        .collect::<Vec<_>>()
        .join(", ");
    let payload = message.len()
        + difference.len()
        + "More than one definition accepts at least one concrete feature step".len()
        + "Make the matchers mutually exclusive".len()
        + if suppressed { "owned 🌍".len() } else { 0 }
        + "input.feature".len()
        + count * "steps.ts".len();
    (message, difference, payload)
}

/// Renders one owned registration in the selected supported frontend.
fn declaration(matcher: &str, action: &str, ruby: bool) -> String {
    if ruby {
        format!("Given({matcher}) {{ {action}() }};")
    } else {
        format!("Given({matcher},()=>{action}());")
    }
}

/// Models logical exact-text groups, proof reuse, late expansion and final payload admission.
fn check(
    (counts, cells, limits, suppressed, window): &Input,
    language: crate::source_adapter::SourceLanguage,
) -> TestCaseResult {
    let ruby = language == crate::ruby::RUBY;
    let mut source = declaration("/^value c[0-2]$/", "shared", ruby);
    let mut members = Vec::new();
    let mut offset = 1;
    for (color, count) in counts.iter().enumerate() {
        members.push(
            std::iter::once(0)
                .chain(offset..offset + count - 1)
                .collect::<Vec<_>>(),
        );
        for index in 0..count - 1 {
            source.push_str(&declaration(
                &format!("'value c{color}'"),
                &format!("action{color}_{index}"),
                ruby,
            ));
        }
        offset += count - 1;
    }
    // This unused pair must produce overlap only when ambiguity analysis stayed complete.
    source.push_str(&declaration("'solo'", "solo", ruby));
    source.push_str(&declaration("'never red'", "quietA", ruby));
    source.push_str(&declaration("/^never (red|blue)$/", "quietB", ruby));
    let file = SourceFile {
        path: PathBuf::from(if ruby { "steps.rb" } else { "steps.ts" }),
        language,
    };
    let definitions = crate::source_adapter::adapter_for_language(language)
        .unwrap()
        .extract(&source, &file)
        .unwrap()
        .definitions;
    prop_assert_eq!(definitions.len(), offset + 3);
    prop_assert_eq!(definitions[0].matcher.as_str(), "^value c[0-2]$");
    for (color, indices) in members.iter().enumerate() {
        let matcher = format!("value c{color}");
        for index in indices.iter().skip(1) {
            prop_assert_eq!(&definitions[*index].matcher, &matcher);
        }
    }
    let cells: Vec<_> = [(0, 0), (1, 1), (2, 2)]
        .into_iter()
        .chain(cells.iter().copied())
        .collect();
    let mut steps: Vec<_> = cells
        .iter()
        .map(|(origin, color)| FeatureStep {
            keyword: "Given".into(),
            text: format!("value c{color}"),
            location: crate::model::SourceLocation::new(
                "input.feature",
                origin + 3,
                3,
                origin + 3,
                17,
            ),
        })
        .collect();
    for (row, text) in [(20, "solo"), (21, "no match")] {
        steps.push(FeatureStep {
            keyword: "Given".into(),
            text: text.into(),
            location: crate::model::SourceLocation::new("input.feature", row, 3, row, 20),
        });
    }
    let mut remaining = limits.map(|n| n as u128);
    let mut groups = BTreeMap::<(usize, usize), usize>::new();
    let mut proofs = BTreeSet::new();
    let mut cut = false;
    for &(origin, color) in &cells {
        if let Some(expansions) = groups.get_mut(&(origin, color)) {
            *expansions += 1;
            continue;
        }
        if cut {
            continue;
        }
        let count = counts[color] as u128;
        let cost = 2 * count * if proofs.contains(&color) { 1 } else { 2 };
        if cost > remaining[0] || remaining[1] == 0 {
            cut = true;
            continue;
        }
        remaining[0] -= cost;
        remaining[1] -= 1;
        proofs.insert(color);
        groups.insert((origin, color), 1);
    }
    let mut expected = Vec::new();
    for ((origin, color), expansions) in groups {
        let (message, difference, payload) =
            group_fields(color, counts[color], expansions, *suppressed);
        if payload as u128 > remaining[2] {
            cut = true;
            break;
        }
        remaining[2] -= payload as u128;
        expected.push((origin, color, message, difference));
    }
    let (_root, mut config) = config();
    config
        .rules
        .insert(Rule::OverlappingMatcher, Severity::Warning);
    if *suppressed {
        config.suppressions.push(SuppressionConfig {
            rule: Rule::AmbiguousStep,
            reason: "owned 🌍".into(),
            path: None,
            matcher: None,
        });
    }
    let (findings, outcome) = limited_ambiguities(&definitions, &steps, &config, *limits, *window);
    let admitted: Vec<_> = findings
        .iter()
        .filter(|f| f.rule == Rule::AmbiguousStep)
        .collect();
    prop_assert_eq!(admitted.len(), expected.len());
    for (finding, (origin, color, message, difference)) in admitted.iter().zip(&expected) {
        prop_assert_eq!(&finding.message, message);
        prop_assert_eq!(&finding.evidence.matcher_difference, difference);
        prop_assert_eq!(
            finding.evidence.handler_evidence.as_str(),
            "More than one definition accepts at least one concrete feature step"
        );
        prop_assert_eq!(
            finding.suggested_action.as_str(),
            "Make the matchers mutually exclusive"
        );
        prop_assert_eq!(finding.severity, config.severity(Rule::AmbiguousStep));

        prop_assert_eq!(
            &finding.primary,
            &steps
                .iter()
                .find(|s| s.location.line == origin + 3)
                .unwrap()
                .location
        );
        let related: Vec<_> = members[*color]
            .iter()
            .map(|i| definitions[*i].location.clone())
            .collect();
        prop_assert_eq!(&finding.related, &related);
        prop_assert_eq!(
            finding.suppression.as_ref().map(|s| s.reason.as_str()),
            suppressed.then_some("owned 🌍")
        );
    }
    prop_assert_eq!(outcome.used, (0..=offset).collect::<BTreeSet<_>>());
    prop_assert_eq!(outcome.ambiguity_truncated, cut);
    prop_assert_eq!(!outcome.incomplete.is_empty(), cut);
    let overlaps: Vec<_> = findings
        .iter()
        .filter(|f| f.rule == Rule::OverlappingMatcher)
        .collect();
    prop_assert_eq!(overlaps.len(), usize::from(!cut));
    if cut {
        prop_assert_eq!(outcome.overlap_census, CandidateSourceCensus::default());
    } else {
        prop_assert!(
            overlaps[0].primary == definitions[offset + 1].location
                || overlaps[0]
                    .related
                    .contains(&definitions[offset + 1].location)
        );
    }
    Ok(())
}

/// Checks complete final members, usage, suppression and honest cutoff across matcher windows.
#[test]
fn generated_ambiguity_outcomes_preserve_members_and_completeness() {
    property_runner::generated(
        (
            proptest::array::uniform3(2usize..=4),
            proptest::collection::vec((0usize..=2, 0usize..=2), 0..=12),
            (0usize..=128, 0usize..=16, 0usize..=4096).prop_map(|(a, b, c)| [a, b, c]),
            any::<bool>(),
            1usize..=4,
        ),
        |input| {
            check(input, TYPESCRIPT)?;
            check(input, crate::ruby::RUBY)
        },
    );
}

/// Covers exact/one-over costs, shared proof copies and late expansions after admission refusal.
#[test]
fn ambiguity_reference_boundaries_preserve_complete_admitted_groups() {
    let bytes: usize = (0..3).map(|color| group_fields(color, 2, 1, false).2).sum();
    for language in [TYPESCRIPT, crate::ruby::RUBY] {
        for limits in [
            [24, 3, bytes],
            [23, 3, bytes],
            [24, 2, bytes],
            [24, 3, bytes - 1],
            [0, 0, 0],
        ] {
            check(&([2; 3], vec![], limits, false, 1), language).unwrap();
        }
        check(
            &([2; 3], vec![(0, 1), (0, 0)], [28, 3, usize::MAX], false, 1),
            language,
        )
        .unwrap();
        check(
            &([2; 3], vec![(0, 1), (0, 1)], [28, 4, usize::MAX], true, 2),
            language,
        )
        .unwrap();
    }
}
