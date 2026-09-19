use super::suppression::SuppressionIndex;
use super::CandidateSourceCensus;
use crate::config::Config;
use crate::model::{
    FeatureStep, Finding, FindingEvidence, MatcherKind, Rule, Severity, StepDefinition,
};
use crate::resource_limits::{compile_regex, MAX_REGEX_PATTERN_BYTES, MAX_STATIC_OVERLAP_FINDINGS};
use cucumber_expressions::expand::IntoRegexCharIter;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

static FALLBACK_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^{}]+)\}").expect("static placeholder regex"));
const MAX_OVERLAP_SUPPRESSION_WORK: u64 = 100_000_000;
const OVERLAP_PROPOSAL_WORK_MULTIPLIER: usize = 4;

struct CompiledMatcher {
    regex: Option<Regex>,
    expression: Option<String>,
    authoritative: bool,
}

/// A single-pass index over every compiled matcher.
///
/// `RegexSet` evaluates many patterns in one scan of the step text, replacing a per-step loop over
/// every definition. A set that exceeds the shared regex limits is split recursively, so one
/// oversized combined program cannot silently return the entire run to O(definitions x steps).
struct MatcherIndex {
    sets: Vec<MatcherSet>,
    /// Individual matchers whose combined program could not be built even as a singleton.
    fallback_definitions: Vec<usize>,
}

struct MatcherSet {
    set: regex::RegexSet,
    /// Definition index for each pattern in `set`, in the set's own order.
    definitions: Vec<usize>,
}

impl MatcherIndex {
    fn build(compiled: &[CompiledMatcher]) -> Self {
        Self::build_with_limits(
            compiled,
            crate::resource_limits::REGEX_SIZE_LIMIT_BYTES,
            crate::resource_limits::REGEX_DFA_SIZE_LIMIT_BYTES,
        )
    }

    fn build_with_limits(
        compiled: &[CompiledMatcher],
        size_limit: usize,
        dfa_limit: usize,
    ) -> Self {
        let mut entries = Vec::new();
        for (index, matcher) in compiled.iter().enumerate() {
            if let Some(expression) = matcher.expression.as_deref() {
                entries.push((index, expression));
            }
        }
        let mut index = Self {
            sets: Vec::new(),
            fallback_definitions: Vec::new(),
        };
        index.build_range(&entries, size_limit, dfa_limit);
        index.sets.sort_by_key(|set| set.definitions[0]);
        index.fallback_definitions.sort_unstable();
        index
    }

    fn build_range(&mut self, entries: &[(usize, &str)], size_limit: usize, dfa_limit: usize) {
        if entries.is_empty() {
            return;
        }
        let patterns = entries
            .iter()
            .map(|(_, expression)| *expression)
            .collect::<Vec<_>>();
        match regex::RegexSetBuilder::new(&patterns)
            .size_limit(size_limit)
            .dfa_size_limit(dfa_limit)
            .build()
        {
            Ok(set) => self.sets.push(MatcherSet {
                set,
                definitions: entries.iter().map(|(index, _)| *index).collect(),
            }),
            Err(_) if entries.len() > 1 => {
                let middle = entries.len() / 2;
                self.build_range(&entries[..middle], size_limit, dfa_limit);
                self.build_range(&entries[middle..], size_limit, dfa_limit);
            }
            Err(_) => self.fallback_definitions.push(entries[0].0),
        }
    }

    fn matches(&self, compiled: &[CompiledMatcher], text: &str, output: &mut Vec<usize>) {
        output.clear();
        for indexed in &self.sets {
            output.extend(
                indexed
                    .set
                    .matches(text)
                    .into_iter()
                    .map(|pattern| indexed.definitions[pattern]),
            );
        }
        output.extend(self.fallback_definitions.iter().copied().filter(|index| {
            compiled[*index]
                .regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(text))
        }));
        output.sort_unstable();
    }

    /// Work units required before a witness can be matched against this index.
    ///
    /// Each RegexSet performs one automaton scan; definitions that could not join a set require
    /// one individual regex scan. Charging this before matching closes the no-candidate case,
    /// where prior pair budgets never advanced despite repeatedly scanning the complete index.
    fn scan_work(&self) -> usize {
        self.sets
            .len()
            .saturating_add(self.fallback_definitions.len())
    }
}

/// Definitions compiled per window instead of all at once.
///
/// Holding every compiled matcher and one monolithic `RegexSet` alive was ~90% of peak memory on a
/// large corpus, because a compiled program is orders of magnitude larger than the matcher text it
/// came from. Compiling a window, scanning it, and dropping it bounds peak at window size rather
/// than corpus size. Output is unaffected: every definition is still compiled and still scanned
/// against every step text and every witness, only not all at the same time.
/// Measured on a 10k-definition corpus with rules on: 392 MB unwindowed, 146 MB at 1,000, and
/// 133 MB at 250, with 250 also the fastest. Smaller windows keep each automaton's lazy-DFA cache
/// warm instead of thrashing one monolithic program.
const DEFAULT_MATCHER_WINDOW: usize = 250;

/// Window size override, used by the corpus gate to force cross-window behaviour.
///
/// The corpus fixtures are far smaller than the default window, so without this every fixture would
/// sit in a single window and the cross-window merge paths would never be exercised.
fn matcher_window_size() -> usize {
    std::env::var("CUKE_DEDUP_MATCHER_WINDOW")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MATCHER_WINDOW)
}

/// The part of a compiled matcher that outlives its window.
///
/// Two bits per definition, against kilobytes for the compiled program they summarize.
#[derive(Clone, Copy)]
struct MatcherSummary {
    authoritative: bool,
    has_regex: bool,
}

/// Compiles each window in index order and hands it to `visit` before dropping it.
///
/// `visit` receives the window's start offset so local indices can be mapped back to definition
/// indices. Windows are visited in ascending order and `MatcherIndex::matches` returns sorted local
/// indices, so anything accumulated across windows is already in ascending definition order.
fn scan_windows(
    definitions: &[StepDefinition],
    parameter_types: &BTreeMap<String, String>,
    window: usize,
    mut visit: impl FnMut(usize, &[CompiledMatcher], &MatcherIndex, Vec<Option<String>>),
) -> usize {
    let mut scan_work = 0_usize;
    for start in (0..definitions.len()).step_by(window) {
        let end = (start + window).min(definitions.len());
        let (compiled, diagnostics): (Vec<_>, Vec<_>) = definitions[start..end]
            .iter()
            .map(|definition| compile_matcher(definition, parameter_types))
            .unzip();
        let index = MatcherIndex::build(&compiled);
        scan_work = scan_work.saturating_add(index.scan_work());
        visit(start, &compiled, &index, diagnostics);
    }
    scan_work
}

enum CucumberRegexExpression {
    Compiled(String),
    Unsupported,
    ResourceLimit,
}

pub(super) struct FeatureUsageOutcome {
    pub(super) used: BTreeSet<usize>,
    pub(super) incomplete: Vec<String>,
    pub(super) overlap_census: CandidateSourceCensus,
}

/// Compactly records which ambiguity groups contain each definition.
///
/// A broad feature step matching N definitions needs O(N) memberships. Expanding that group into
/// all N*(N-1)/2 definition pairs was both unnecessary and an adversarial memory hazard.
struct ProvenAmbiguities {
    groups: HashMap<Vec<usize>, usize>,
    memberships: Vec<Vec<usize>>,
}

impl ProvenAmbiguities {
    fn new(definition_count: usize) -> Self {
        Self {
            groups: HashMap::new(),
            memberships: vec![Vec::new(); definition_count],
        }
    }

    fn record(&mut self, matched: &BTreeSet<usize>) {
        let group = matched.iter().copied().collect::<Vec<_>>();
        let identifier = self.groups.len();
        match self.groups.entry(group) {
            std::collections::hash_map::Entry::Occupied(_) => {}
            std::collections::hash_map::Entry::Vacant(entry) => {
                for definition in entry.key() {
                    self.memberships[*definition].push(identifier);
                }
                entry.insert(identifier);
            }
        }
    }

    fn contains(&self, left: usize, right: usize) -> bool {
        let left = &self.memberships[left];
        let right = &self.memberships[right];
        let (mut left_index, mut right_index) = (0, 0);
        while left_index < left.len() && right_index < right.len() {
            match left[left_index].cmp(&right[right_index]) {
                std::cmp::Ordering::Less => left_index += 1,
                std::cmp::Ordering::Greater => right_index += 1,
                std::cmp::Ordering::Equal => return true,
            }
        }
        false
    }
}

pub(super) fn analyze_feature_usage(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
    overlap_budget: usize,
) -> FeatureUsageOutcome {
    analyze_feature_usage_in_windows(
        definitions,
        steps,
        config,
        suppressions,
        findings,
        overlap_budget,
        matcher_window_size(),
    )
}

/// Window size is a parameter rather than an environment read so tests can vary it without
/// mutating process state, which would make them order-dependent under the default test harness.
fn analyze_feature_usage_in_windows(
    definitions: &[StepDefinition],
    steps: &[FeatureStep],
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
    overlap_budget: usize,
    window: usize,
) -> FeatureUsageOutcome {
    let mut summaries: Vec<MatcherSummary> = Vec::with_capacity(definitions.len());
    let mut incomplete: Vec<String> = Vec::new();

    // Scenario Outline expansion repeats identical step text, and repeated text always produces
    // the same match set, so each distinct text is matched once per window.
    let mut matched_texts: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for step in steps {
        matched_texts.entry(step.text.as_str()).or_default();
    }

    let mut local = Vec::new();
    let scan_work = scan_windows(
        definitions,
        &config.parameter_types,
        window,
        |start, compiled, index, diagnostics| {
            summaries.extend(compiled.iter().map(|matcher| MatcherSummary {
                authoritative: matcher.authoritative,
                has_regex: matcher.regex.is_some(),
            }));
            incomplete.extend(diagnostics.into_iter().flatten());
            for (text, accumulated) in matched_texts.iter_mut() {
                index.matches(compiled, text, &mut local);
                accumulated.extend(local.iter().map(|index| index + start));
            }
        },
    );

    // Definitions whose matcher cannot be modeled authoritatively cannot be proven unused.
    // A permissive fallback may still fail to match the available feature corpus, so both
    // uncompiled and non-authoritative matchers remain indeterminate.
    let mut used: BTreeSet<_> = summaries
        .iter()
        .enumerate()
        .filter_map(|(index, matcher)| {
            (!matcher.has_regex || !matcher.authoritative).then_some(index)
        })
        .collect();
    let mut ambiguities: BTreeMap<_, (&FeatureStep, BTreeSet<usize>, usize)> = BTreeMap::new();
    let mut proven = ProvenAmbiguities::new(definitions.len());
    let ambiguity_is_reported = config.severity(Rule::AmbiguousStep) != Severity::Off;
    for step in steps {
        // Merged across windows before anything reads it. Recording `proven` from a single
        // window's matches would miss a pair split across windows, and `overlapping-matcher`
        // would then report an overlap that `ambiguous-step` already owns.
        let matches = &matched_texts[step.text.as_str()];
        used.extend(matches.iter().copied());
        let authoritative_matches: BTreeSet<_> = matches
            .iter()
            .copied()
            .filter(|index| summaries[*index].authoritative)
            .collect();
        if authoritative_matches.len() > 1 && ambiguity_is_reported {
            proven.record(&authoritative_matches);
            let ambiguity_key = (
                step.location.path.clone(),
                step.location.line,
                step.location.column,
                authoritative_matches.clone(),
            );
            let entry = ambiguities
                .entry(ambiguity_key)
                .or_insert_with(|| (step, BTreeSet::new(), 0));
            entry.1.extend(authoritative_matches);
            entry.2 += 1;
        }
    }
    // Ambiguity proven by a real step is strictly better evidence than a synthesized witness.
    // Membership intersection answers that question without materializing every pair in a broad
    // ambiguity group.
    let overlap = analyze_matcher_overlap(
        definitions,
        &summaries,
        &proven,
        config,
        suppressions,
        findings,
        overlap_budget,
        window,
        scan_work,
    );
    if let Some(diagnostic) = overlap.incomplete {
        incomplete.push(diagnostic);
    }
    let severity = config.severity(Rule::AmbiguousStep);
    if ambiguity_is_reported {
        for (_, (step, matches, expansion_count)) in ambiguities {
            let matched_indices = matches.iter().copied().collect::<Vec<_>>();
            let matched_definitions: Vec<_> = matched_indices
                .iter()
                .map(|index| &definitions[*index])
                .collect();
            let matcher_list = matched_definitions
                .iter()
                .map(|definition| format!("`{}`", definition.matcher))
                .collect::<Vec<_>>()
                .join(", ");
            let message = if expansion_count == 1 {
                format!(
                    "Feature step `{}` matches {} definitions",
                    step.text,
                    matches.len()
                )
            } else {
                format!(
                    "Scenario outline step has {expansion_count} ambiguous expansions matching {} definitions",
                    matches.len()
                )
            };
            findings.push(Finding {
                rule: Rule::AmbiguousStep,
                severity,
                message,
                primary: step.location.clone(),
                related: matched_definitions
                    .iter()
                    .map(|definition| definition.location.clone())
                    .collect(),
                evidence: FindingEvidence {
                    matcher_similarity: None,
                    handler_similarity: None,
                    matcher_difference: matcher_list,
                    handler_evidence:
                        "More than one definition accepts at least one concrete feature step"
                            .to_owned(),
                    comparison: None,
                    cluster: None,
                },
                suggested_action: "Make the matchers mutually exclusive".to_owned(),
                suppression: suppressions
                    .find_reason(Rule::AmbiguousStep, &matched_indices)
                    .map(|reason| crate::model::Suppression {
                        reason: reason.to_owned(),
                    }),
            });
        }
    }
    FeatureUsageOutcome {
        used,
        incomplete,
        overlap_census: overlap.census,
    }
}

struct OverlapOutcome {
    census: CandidateSourceCensus,
    incomplete: Option<String>,
}

/// Reports matcher pairs that can both accept one step text, without needing a feature corpus.
///
/// `ambiguous-step` can only fire for overlaps a discovered feature actually exercises. A suite
/// with no features, a generated corpus, or simply an unused overlap therefore hides a defect
/// that fails at runtime the first time someone writes the step. This pass synthesizes one
/// witness per Cucumber Expression and matches it against every definition, so the overlap is
/// reported from the definitions alone.
///
/// Witnesses are only synthesized for Cucumber Expressions, whose parameter types have known
/// sample values. Regular expressions are never reversed into a sample — that would be unsound —
/// but they are still matched against, which is what catches a regex duplicating an expression.
#[allow(clippy::too_many_arguments)]
fn analyze_matcher_overlap(
    definitions: &[StepDefinition],
    summaries: &[MatcherSummary],
    proven: &ProvenAmbiguities,
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
    work_budget: usize,
    window: usize,
    // Automaton scans one witness costs across the whole corpus, measured by the step-matching
    // walk and spent here to bound the witness walk. Sound only because both walks build the same
    // windows from the same definitions under the same limits, so `MatcherIndex` splits the same
    // way and the count is identical. Changing the window size between the two walks would break
    // that.
    scan_work: usize,
) -> OverlapOutcome {
    let severity = config.severity(Rule::OverlappingMatcher);
    if severity == Severity::Off {
        return OverlapOutcome {
            census: CandidateSourceCensus::default(),
            incomplete: None,
        };
    }
    let proposal_budget = work_budget.saturating_mul(OVERLAP_PROPOSAL_WORK_MULTIPLIER);

    // Representatives, chosen exactly as a single pass would: first authoritative member of each
    // equivalent-matcher group, in definition order. Equivalent matchers are already connected by
    // the duplicate rules, so one representative witness preserves every cross-group overlap
    // without evaluating an identical witness once per group member.
    //
    // This is why witnesses cannot be built before the first walk and folded into it, which would
    // avoid compiling every matcher twice. `witness_for` needs only the definition, but *choosing*
    // the representative needs `authoritative`, which exists only after compiling. Dropping that
    // filter would let a non-authoritative member claim its group's slot and yield no witness,
    // silently losing the overlap coverage of every authoritative member behind it.
    let mut witnessed_groups = HashSet::new();
    let mut witnesses: Vec<(usize, String)> = Vec::new();
    let witness_scan_budget = proposal_budget;
    for (left, definition) in definitions.iter().enumerate() {
        if !summaries[left].authoritative {
            continue;
        }
        if !witnessed_groups.insert((definition.matcher_kind, &definition.normalized_matcher)) {
            continue;
        }
        if let Some(witness) = witness_for(definition, &config.parameter_types) {
            witnesses.push((left, witness));
        }
    }

    // A witness has to be matched against every definition, so the windows are walked again and
    // each witness accumulates its matches across all of them. Only proposable matches are kept —
    // the same three filters a single pass applies — so this holds the pairs the replay below
    // would consider and nothing more, capped by the proposal budget.
    // Every witness costs the same full-index scan, so the number the budget affords is exact and
    // the cut is a uniform prefix: a witness is scanned against every window or none, which keeps
    // each accumulated match set complete. Charging this before scanning restores the bound a
    // single pass had — otherwise witnesses that accept nothing would sweep every window while
    // advancing no budget at all.
    let affordable = witness_scan_budget
        .checked_div(scan_work)
        .unwrap_or(witnesses.len());
    let scan_truncated = witnesses.len() > affordable;
    witnesses.truncate(affordable);

    let mut proposals: Vec<Vec<usize>> = vec![Vec::new(); witnesses.len()];
    let mut accumulated = 0_usize;
    let mut local = Vec::new();
    // Summaries and compilation diagnostics were already collected by the step-matching pass;
    // recompiling here must not duplicate them.
    scan_windows(
        definitions,
        &config.parameter_types,
        window,
        |start, compiled, index, _diagnostics| {
            for ((left, witness), accepted) in witnesses.iter().zip(proposals.iter_mut()) {
                if accumulated >= proposal_budget {
                    break;
                }
                index.matches(compiled, witness, &mut local);
                let definition = &definitions[*left];
                for right in local.iter().map(|index| index + start) {
                    if right == *left || !summaries[right].authoritative {
                        continue;
                    }
                    // Equivalent matchers are already reported as duplicates; overlap adds nothing.
                    if definition.matcher_kind == definitions[right].matcher_kind
                        && definition.normalized_matcher == definitions[right].normalized_matcher
                    {
                        continue;
                    }
                    if accumulated >= proposal_budget {
                        break;
                    }
                    accepted.push(right);
                    accumulated += 1;
                }
            }
        },
    );

    let mut considered = HashSet::new();
    let mut evaluated = 0_usize;
    let mut proposal_work = 0_usize;
    let mut retained = 0_usize;
    let mut truncated = scan_truncated;
    let mut suppression_work = 0_u64;
    'definitions: for ((left, witness), accepted) in witnesses.iter().zip(proposals.iter()) {
        for right in accepted.iter().copied() {
            if proposal_work == proposal_budget {
                truncated = true;
                break 'definitions;
            }
            proposal_work += 1;
            let pair = if *left < right {
                (*left, right)
            } else {
                (right, *left)
            };
            if considered.contains(&pair) {
                continue;
            }
            if evaluated == work_budget {
                truncated = true;
                break 'definitions;
            }
            considered.insert(pair);
            evaluated += 1;
            if proven.contains(pair.0, pair.1) {
                continue;
            }
            if retained == MAX_STATIC_OVERLAP_FINDINGS {
                truncated = true;
                break 'definitions;
            }
            let lookup_work = suppressions.lookup_work(Rule::OverlappingMatcher, &[pair.0, pair.1]);
            let Some(next_suppression_work) = suppression_work.checked_add(lookup_work) else {
                truncated = true;
                break 'definitions;
            };
            if next_suppression_work > MAX_OVERLAP_SUPPRESSION_WORK {
                truncated = true;
                break 'definitions;
            }
            suppression_work = next_suppression_work;
            let suppression = suppressions
                .find_reason(Rule::OverlappingMatcher, &[pair.0, pair.1])
                .map(|reason| crate::model::Suppression {
                    reason: reason.to_owned(),
                });
            findings.push(Finding {
                rule: Rule::OverlappingMatcher,
                severity,
                message: format!(
                    "Matchers `{}` and `{}` both accept the step `{witness}`",
                    definitions[pair.0].matcher, definitions[pair.1].matcher
                ),
                primary: definitions[pair.0].location.clone(),
                related: vec![definitions[pair.1].location.clone()],
                evidence: FindingEvidence {
                    matcher_similarity: None,
                    handler_similarity: None,
                    matcher_difference: format!(
                        "`{}` ↔ `{}`",
                        definitions[pair.0].matcher, definitions[pair.1].matcher
                    ),
                    handler_evidence: format!(
                        "No feature step exercises this overlap yet; `{witness}` would be ambiguous at runtime"
                    ),
                    comparison: None,
                    cluster: None,
                },
                suggested_action: "Make the matchers mutually exclusive before a feature reaches both"
                    .to_owned(),
                suppression,
            });
            retained += 1;
        }
    }
    OverlapOutcome {
        census: CandidateSourceCensus {
            evaluated,
            skipped: u64::from(truncated),
        },
        incomplete: truncated.then(|| {
            format!(
                "static matcher-overlap analysis is incomplete: evaluated {evaluated} definition comparisons and skipped at least 1 after safety limits; partial findings are available"
            )
        }),
    }
}

/// Builds one concrete step text a Cucumber Expression would accept, or `None` when unknown.
fn witness_for(
    definition: &StepDefinition,
    parameter_types: &BTreeMap<String, String>,
) -> Option<String> {
    if definition.matcher_kind != MatcherKind::CucumberExpression {
        return None;
    }
    let mut witness = String::with_capacity(definition.matcher.len());
    let mut offset = 0;
    for capture in FALLBACK_PLACEHOLDER.captures_iter(&definition.matcher) {
        let whole = capture.get(0).expect("whole match");
        witness.push_str(&witness_literal(&definition.matcher[offset..whole.start()]));
        let sample = match &capture[1] {
            "int" => "1".to_owned(),
            "float" => "1.5".to_owned(),
            "word" => "sample".to_owned(),
            "string" => "\"sample\"".to_owned(),
            // A declared type contributes a sample only when its pattern is plainly enumerable.
            // An undeclared type is unknown outright, and a pattern with real regex syntax has no
            // sample that can be derived soundly, so both leave the matcher without a witness.
            name => literal_alternative(parameter_types.get(name)?)?,
        };
        witness.push_str(&sample);
        offset = whole.end();
    }
    witness.push_str(&witness_literal(&definition.matcher[offset..]));
    Some(witness)
}

/// Returns one string a declared parameter-type pattern accepts, when that is unambiguous.
///
/// Only patterns built from literal alternatives — `red`, `red|green`, `(red|green)` — yield a
/// sample. Anything using real regular-expression syntax could accept infinitely many strings
/// with no canonical representative, and guessing one would produce false overlap reports.
fn literal_alternative(pattern: &str) -> Option<String> {
    let trimmed = pattern.trim();
    let trimmed = match (trimmed.strip_prefix("(?:"), trimmed.strip_prefix('(')) {
        (Some(inner), _) => inner.strip_suffix(')')?,
        (None, Some(inner)) => inner.strip_suffix(')')?,
        _ => trimmed,
    };
    let first = trimmed.split('|').next()?;
    if first.is_empty()
        || first
            .chars()
            .any(|character| !character.is_alphanumeric() && !matches!(character, ' ' | '_' | '-'))
    {
        return None;
    }
    Some(first.to_owned())
}

/// Resolves Cucumber Expression literal syntax into one concrete rendering.
///
/// `(s)` optionals are taken, `a/b` alternations take the first branch, and `\` escapes are
/// unwrapped, matching how the equivalent regex would accept the produced text.
fn witness_literal(literal: &str) -> String {
    let mut output = String::with_capacity(literal.len());
    let characters: Vec<_> = literal.chars().collect();
    let mut offset = 0;
    while offset < characters.len() {
        match characters[offset] {
            '\\' => {
                offset += 1;
                if let Some(character) = characters.get(offset) {
                    output.push(*character);
                    offset += 1;
                }
            }
            '(' => {
                let end = characters[offset + 1..]
                    .iter()
                    .position(|character| *character == ')')
                    .map(|position| offset + position + 1);
                match end {
                    Some(end) => {
                        output.extend(&characters[offset + 1..end]);
                        offset = end + 1;
                    }
                    None => {
                        output.push('(');
                        offset += 1;
                    }
                }
            }
            '/' => {
                // Keep the first alternative: drop everything up to the next separator.
                while offset < characters.len()
                    && !characters[offset].is_whitespace()
                    && (characters[offset] == '/' || offset == 0 || characters[offset] != ' ')
                {
                    offset += 1;
                    if offset < characters.len() && characters[offset].is_whitespace() {
                        break;
                    }
                }
            }
            character => {
                output.push(character);
                offset += 1;
            }
        }
    }
    output
}

pub(super) fn analyze_unused(
    definitions: &[StepDefinition],
    used: &BTreeSet<usize>,
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
) {
    let severity = config.severity(Rule::UnusedDefinition);
    if severity == Severity::Off {
        return;
    }
    for (index, definition) in definitions.iter().enumerate() {
        if used.contains(&index) {
            continue;
        }
        findings.push(Finding {
            rule: Rule::UnusedDefinition,
            severity,
            message: format!(
                "Step definition `{}` is not used by the feature corpus",
                definition.matcher
            ),
            primary: definition.location.clone(),
            related: Vec::new(),
            evidence: FindingEvidence {
                matcher_similarity: None,
                handler_similarity: None,
                matcher_difference: "No discovered feature step matched this definition".to_owned(),
                handler_evidence: String::new(),
                comparison: None,
                cluster: None,
            },
            suggested_action: "Remove the definition or add the missing feature usage".to_owned(),
            suppression: suppressions
                .find_reason(Rule::UnusedDefinition, &[index])
                .map(|reason| crate::model::Suppression {
                    reason: reason.to_owned(),
                }),
        });
    }
}

fn compile_matcher(
    definition: &StepDefinition,
    parameter_types: &BTreeMap<String, String>,
) -> (CompiledMatcher, Option<String>) {
    match definition.matcher_kind {
        MatcherKind::RegularExpression => {
            if definition.matcher.len() > MAX_REGEX_PATTERN_BYTES {
                return (
                    CompiledMatcher {
                        regex: None,
                        expression: None,
                        authoritative: true,
                    },
                    Some(regex_limit_message(definition)),
                );
            }
            let Some(expression) = crate::typescript::rust_regex_expression(
                &definition.matcher,
                &definition.matcher_flags,
            ) else {
                return (
                    CompiledMatcher {
                        regex: None,
                        expression: None,
                        authoritative: false,
                    },
                    None,
                );
            };
            let (regex, error) = compile_definition_regex(&expression, definition);
            (
                CompiledMatcher {
                    expression: regex.is_some().then(|| expression.clone()),
                    regex,
                    authoritative: true,
                },
                error,
            )
        }
        MatcherKind::CucumberExpression => {
            match cucumber_regex_expression(&definition.matcher) {
                CucumberRegexExpression::Compiled(expression) => {
                    let (regex, error) = compile_definition_regex(&expression, definition);
                    (
                        CompiledMatcher {
                            expression: regex.is_some().then(|| expression.clone()),
                            regex,
                            authoritative: true,
                        },
                        error,
                    )
                }
                CucumberRegexExpression::ResourceLimit => (
                    CompiledMatcher {
                        regex: None,
                        expression: None,
                        authoritative: true,
                    },
                    Some(regex_limit_message(definition)),
                ),
                CucumberRegexExpression::Unsupported => {
                    // Project-defined parameter types cannot be resolved without loading runtime
                    // code. Declared ones are substituted exactly; anything still unknown falls
                    // back to a permissive pattern that avoids false unused reports but cannot
                    // prove runtime ambiguity. Oversized input already returned ResourceLimit.
                    let fallback =
                        fallback_cucumber_expression_regex(&definition.matcher, parameter_types);
                    let (regex, error) = compile_definition_regex(&fallback.expression, definition);
                    (
                        CompiledMatcher {
                            expression: regex.is_some().then(|| fallback.expression.clone()),
                            regex,
                            authoritative: fallback.exact,
                        },
                        error,
                    )
                }
            }
        }
    }
}

fn cucumber_regex_expression(matcher: &str) -> CucumberRegexExpression {
    if matcher.len() > MAX_REGEX_PATTERN_BYTES {
        return CucumberRegexExpression::ResourceLimit;
    }
    let Ok(expression) = cucumber_expressions::Expression::parse(matcher) else {
        return CucumberRegexExpression::Unsupported;
    };
    let mut expanded = String::with_capacity(matcher.len().min(MAX_REGEX_PATTERN_BYTES));
    for character in expression.into_regex_char_iter() {
        let Ok(character) = character else {
            return CucumberRegexExpression::Unsupported;
        };
        if expanded.len().saturating_add(character.len_utf8()) > MAX_REGEX_PATTERN_BYTES {
            return CucumberRegexExpression::ResourceLimit;
        }
        expanded.push(character);
    }
    CucumberRegexExpression::Compiled(expanded)
}

fn compile_definition_regex(
    expression: &str,
    definition: &StepDefinition,
) -> (Option<Regex>, Option<String>) {
    if expression.len() > MAX_REGEX_PATTERN_BYTES {
        return (None, Some(regex_limit_message(definition)));
    }
    match compile_regex(expression) {
        Ok(regex) => (Some(regex), None),
        Err(regex::Error::CompiledTooBig(_)) => (None, Some(regex_limit_message(definition))),
        Err(_) => (None, None),
    }
}

fn regex_limit_message(definition: &StepDefinition) -> String {
    format!(
        "step matcher at {}:{}:{} exceeds the {}-byte regex resource limit; simplify the matcher or remove its source from definition discovery",
        definition.location.path.display(),
        definition.location.line,
        definition.location.column,
        MAX_REGEX_PATTERN_BYTES
    )
}

/// A fallback pattern plus whether every placeholder in it resolved to a known type.
struct FallbackExpression {
    expression: String,
    exact: bool,
}

fn fallback_cucumber_expression_regex(
    expression: &str,
    parameter_types: &BTreeMap<String, String>,
) -> FallbackExpression {
    let mut output = String::from("^");
    let mut offset = 0;
    let mut exact = true;
    for capture in FALLBACK_PLACEHOLDER.captures_iter(expression) {
        let whole = capture.get(0).expect("whole match");
        output.push_str(&fallback_cucumber_literal_regex(
            &expression[offset..whole.start()],
        ));
        let name = &capture[1];
        match name {
            "int" => output.push_str(r"-?\d+"),
            "float" => output.push_str(r"-?(?:\d+\.)?\d+"),
            "word" => output.push_str(r"\S+"),
            "string" => output.push_str(r#"(?:"[^"]*"|'[^']*')"#),
            _ => match parameter_types.get(name) {
                // A declared type is exact, so usage and ambiguity stay authoritative.
                Some(pattern) => {
                    output.push_str("(?:");
                    output.push_str(pattern);
                    output.push(')');
                }
                None => {
                    output.push_str(r".+");
                    exact = false;
                }
            },
        }
        offset = whole.end();
    }
    output.push_str(&fallback_cucumber_literal_regex(&expression[offset..]));
    output.push('$');
    FallbackExpression {
        expression: output,
        exact,
    }
}

fn fallback_cucumber_literal_regex(literal: &str) -> String {
    let mut output = String::new();
    let characters: Vec<_> = literal.chars().collect();
    let mut offset = 0;
    while offset < characters.len() {
        if characters[offset].is_whitespace() {
            let start = offset;
            while offset < characters.len() && characters[offset].is_whitespace() {
                offset += 1;
            }
            output.push_str(&regex::escape(
                &characters[start..offset].iter().collect::<String>(),
            ));
            continue;
        }
        let start = offset;
        while offset < characters.len() && !characters[offset].is_whitespace() {
            offset += 1;
        }
        output.push_str(&fallback_cucumber_token_regex(
            &characters[start..offset].iter().collect::<String>(),
        ));
    }
    output
}

fn fallback_cucumber_token_regex(token: &str) -> String {
    let alternatives: Vec<_> = token.split('/').collect();
    if alternatives.len() > 1 {
        return format!(
            "(?:{})",
            alternatives
                .iter()
                .map(|alternative| fallback_cucumber_token_regex(alternative))
                .collect::<Vec<_>>()
                .join("|")
        );
    }

    let mut output = String::new();
    let characters: Vec<_> = token.chars().collect();
    let mut offset = 0;
    while offset < characters.len() {
        if characters[offset] == '(' {
            if let Some(end) = characters[offset + 1..]
                .iter()
                .position(|character| *character == ')')
                .map(|index| offset + index + 1)
            {
                let optional: String = characters[offset + 1..end].iter().collect();
                output.push_str("(?:");
                output.push_str(&regex::escape(&optional));
                output.push_str(")?");
                offset = end + 1;
                continue;
            }
            output.push_str(r"\(");
            offset += 1;
            continue;
        }
        let start = offset;
        while offset < characters.len() && characters[offset] != '(' {
            offset += 1;
        }
        output.push_str(&regex::escape(
            &characters[start..offset].iter().collect::<String>(),
        ));
    }
    output
}

#[cfg(test)]
mod tests;
