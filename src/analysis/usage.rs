use super::suppression::SuppressionIndex;
use super::CandidateSourceCensus;
use crate::config::Config;
use crate::model::{
    FeatureStep, Finding, FindingEvidence, MatcherKind, Rule, Severity, StepDefinition,
};
use crate::resource_limits::{
    compile_regex_with_limits, MAX_REGEX_PATTERN_BYTES, MAX_STATIC_OVERLAP_FINDINGS,
};
use cucumber_expressions::expand::IntoRegexCharIter;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::LazyLock;

static FALLBACK_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^{}]+)\}").expect("static placeholder regex"));
const MAX_OVERLAP_SUPPRESSION_WORK: u64 = 100_000_000;
const OVERLAP_PROPOSAL_WORK_MULTIPLIER: usize = 4;

/// A definition's matcher before compilation; `expression` is the regex source to index.
struct PreparedMatcher {
    expression: Option<String>,
    authoritative: bool,
}

/// A single-pass index over every prepared matcher.
///
/// `RegexSet` evaluates many patterns in one scan of the step text, replacing a per-step loop over
/// every definition. A set that exceeds the shared regex limits is split recursively, so one
/// oversized combined program cannot silently return the entire run to O(definitions x steps).
/// The index compiles a matcher only inside its set: a separate `Regex` per matcher doubled
/// compilation, which was most of the analysis allocation volume.
struct MatcherIndex {
    sets: Vec<MatcherSet>,
    /// Matchers whose singleton set failed but whose own `Regex` compiled, tested one by one.
    fallback: Vec<(usize, Regex)>,
    /// Matchers that compiled neither in a set nor alone, with whether the cause was the size limit.
    rejected: Vec<(usize, bool)>,
}

struct MatcherSet {
    set: regex::RegexSet,
    /// Definition index for each pattern in `set`, in the set's own order.
    definitions: Vec<usize>,
}

impl MatcherIndex {
    /// Indexes the matchers that kept an expression, under the shared regex size and DFA limits.
    fn build(matchers: &[PreparedMatcher]) -> Self {
        Self::build_with_limits(
            matchers,
            crate::resource_limits::REGEX_SIZE_LIMIT_BYTES,
            crate::resource_limits::REGEX_DFA_SIZE_LIMIT_BYTES,
        )
    }

    /// Indexes matchers that kept an expression into `RegexSet`s under the given limits; matchers
    /// without one are not indexed. Sets are ordered by first definition index, fallback and
    /// rejected matchers ascending.
    ///
    /// A rejected matcher splits every set on its path down to a singleton, so the index is rebuilt
    /// without it: its sets, and the scan work they charge, are those of the matchers that compile.
    /// Only the size limit rejects here; `prepare_matcher` drops invalid syntax.
    fn build_with_limits(
        matchers: &[PreparedMatcher],
        size_limit: usize,
        dfa_limit: usize,
    ) -> Self {
        let mut entries = Vec::new();
        for (index, matcher) in matchers.iter().enumerate() {
            if let Some(expression) = matcher.expression.as_deref() {
                entries.push((index, expression));
            }
        }
        let mut index = Self::from_entries(&entries, size_limit, dfa_limit);
        if !index.rejected.is_empty() {
            let rejected = std::mem::take(&mut index.rejected);
            entries.retain(|(local, _)| {
                rejected
                    .binary_search_by_key(local, |(rejected, _)| *rejected)
                    .is_err()
            });
            index = Self::from_entries(&entries, size_limit, dfa_limit);
            index.rejected.extend(rejected);
            index.rejected.sort_unstable();
        }
        index
    }

    /// Builds the sets for `entries` and orders the result.
    fn from_entries(entries: &[(usize, &str)], size_limit: usize, dfa_limit: usize) -> Self {
        let mut index = Self {
            sets: Vec::new(),
            fallback: Vec::new(),
            rejected: Vec::new(),
        };
        index.build_range(entries, size_limit, dfa_limit);
        index.sets.sort_by_key(|set| set.definitions[0]);
        index.fallback.sort_unstable_by_key(|(index, _)| *index);
        index.rejected.sort_unstable();
        index
    }

    /// Builds one set for `entries`; on a build error the range is halved and each half retried.
    /// A single entry that still fails is compiled alone: it goes to `fallback` when that succeeds,
    /// else to `rejected`. Empty input is a no-op.
    ///
    /// Neither form implies the other: a set compiles without capture groups, so a capture-heavy
    /// pattern can fit only as a set, while a large literal alternation can fit only alone.
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
            Err(_) => {
                let (index, expression) = entries[0];
                match compile_regex_with_limits(expression, size_limit, dfa_limit) {
                    Ok(regex) => self.fallback.push((index, regex)),
                    Err(error) => self
                        .rejected
                        .push((index, matches!(error, regex::Error::CompiledTooBig(_)))),
                }
            }
        }
    }

    /// Replaces `output` with the sorted local indices of every matcher accepting `text`; fallback
    /// matchers are tested one by one with their own regex.
    fn matches(&self, text: &str, output: &mut Vec<usize>) {
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
        output.extend(
            self.fallback
                .iter()
                .filter(|(_, regex)| regex.is_match(text))
                .map(|(index, _)| *index),
        );
        output.sort_unstable();
    }

    /// Work units required before a witness can be matched against this index.
    ///
    /// Each RegexSet performs one automaton scan; definitions that could not join a set require
    /// one individual regex scan. Charging this before matching closes the no-candidate case,
    /// where prior pair budgets never advanced despite repeatedly scanning the complete index.
    fn scan_work(&self) -> usize {
        self.sets.len().saturating_add(self.fallback.len())
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

impl MatcherSummary {
    /// Summarizes a matcher after its window's index cleared the expressions it rejected.
    fn of(matcher: &PreparedMatcher) -> Self {
        Self {
            authoritative: matcher.authoritative,
            has_regex: matcher.expression.is_some(),
        }
    }
}

/// Compiles each window in index order and hands it to `visit` before dropping it; a `Break`
/// from `visit` stops the walk, so no later window is compiled. Returns the scan work of the
/// windows visited, which is the whole corpus only when `visit` never breaks.
///
/// `visit` receives the window's start offset so local indices can be mapped back to definition
/// indices. Windows are visited in ascending order and `MatcherIndex::matches` returns sorted local
/// indices, so anything accumulated across windows is already in ascending definition order.
fn scan_windows(
    definitions: &[StepDefinition],
    parameter_types: &BTreeMap<String, String>,
    window: usize,
    mut visit: impl FnMut(
        usize,
        &[PreparedMatcher],
        &MatcherIndex,
        Vec<Option<String>>,
    ) -> ControlFlow<()>,
) -> usize {
    let mut scan_work = 0_usize;
    for start in (0..definitions.len()).step_by(window) {
        let end = (start + window).min(definitions.len());
        let (mut matchers, mut diagnostics): (Vec<_>, Vec<_>) = definitions[start..end]
            .iter()
            .map(|definition| prepare_matcher(definition, parameter_types))
            .unzip();
        let index = MatcherIndex::build(&matchers);
        // A rejected matcher has no program; only the size limit is reported.
        for &(local, too_big) in &index.rejected {
            matchers[local].expression = None;
            diagnostics[local] = too_big.then(|| regex_limit_message(&definitions[start + local]));
        }
        scan_work = scan_work.saturating_add(index.scan_work());
        if visit(start, &matchers, &index, diagnostics).is_break() {
            break;
        }
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
    /// Creates an index with no groups and an empty membership list per definition.
    fn new(definition_count: usize) -> Self {
        Self {
            groups: HashMap::new(),
            memberships: vec![Vec::new(); definition_count],
        }
    }

    /// Registers the matched definitions as one group unless the same set is already recorded;
    /// group identifiers only increase, so every membership list stays sorted.
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

    /// Reports whether two definitions share a recorded group by merging their sorted memberships.
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

/// Matches each distinct feature step text against every definition, reports `ambiguous-step` for
/// steps with more than one authoritative match, and runs static overlap within `overlap_budget`;
/// each rule is skipped when its severity is off. Definitions without an authoritative compiled
/// matcher count as used. The window size comes from `CUKE_DEDUP_MATCHER_WINDOW`, default 250.
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
        |start, matchers, index, diagnostics| {
            summaries.extend(matchers.iter().map(MatcherSummary::of));
            incomplete.extend(diagnostics.into_iter().flatten());
            for (text, accumulated) in matched_texts.iter_mut() {
                index.matches(text, &mut local);
                accumulated.extend(local.iter().map(|index| index + start));
            }
            ControlFlow::Continue(())
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
        &matched_texts,
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
    matched_texts: &BTreeMap<&str, Vec<usize>>,
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
    work_budget: usize,
    window: usize,
    // Automaton scans one witness costs across the whole corpus, measured by the step-matching
    // walk and spent here to bound the witness walk. Sound only because both walks build the same
    // windows from the same definitions under the same limits, so `MatcherIndex` splits the same
    // way and the count is identical, and because the step-matching visitor never breaks.
    // Changing the window size between the two walks, or breaking that walk early, would break it.
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
    // Scanning a witness evaluates no comparison, so it is bounded by the configured limit rather
    // than by what the pair stage left: at an exact limit, witnesses that accept nothing complete.
    let witness_scan_budget = config
        .max_candidate_comparisons
        .saturating_mul(OVERLAP_PROPOSAL_WORK_MULTIPLIER);
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
    // the cut is a uniform prefix of witnesses. Charging this before scanning restores the bound a
    // single pass had — otherwise witnesses that accept nothing would sweep every window while
    // advancing no budget at all.
    let affordable = witness_scan_budget
        .checked_div(scan_work)
        .unwrap_or(witnesses.len());
    let scan_truncated = witnesses.len() > affordable;
    witnesses.truncate(affordable);

    let (proposals, overflow, _source) = collect_overlap_proposals(
        definitions,
        summaries,
        &config.parameter_types,
        window,
        &witnesses,
        proposal_budget,
        Some(matched_texts),
    );

    let mut considered = HashSet::new();
    let mut evaluated = 0_usize;
    let mut retained = 0_usize;
    let mut truncated = scan_truncated || overflow;
    let mut suppression_work = 0_u64;
    'definitions: for ((left, witness), accepted) in witnesses.iter().zip(proposals.iter()) {
        for right in accepted.iter().copied() {
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

/// Each witness's proposable matches across every window, storing at most `budget` of them; the
/// flag reports a proposable match that did not fit. Proposable means the three filters a single
/// pass applies: another authoritative definition whose matcher is not equivalent.
fn collect_overlap_proposals(
    definitions: &[StepDefinition],
    summaries: &[MatcherSummary],
    parameter_types: &BTreeMap<String, String>,
    window: usize,
    witnesses: &[(usize, String)],
    budget: usize,
    matched_texts: Option<&BTreeMap<&str, Vec<usize>>>,
) -> (Vec<Vec<usize>>, bool, OverlapProposalSource) {
    if witnesses.is_empty() {
        return (Vec::new(), false, OverlapProposalSource::FeatureCache);
    }
    if let Some(proposals) = matched_texts.and_then(|matched_texts| {
        cached_overlap_proposals(definitions, summaries, witnesses, budget, matched_texts)
    }) {
        return (proposals, false, OverlapProposalSource::FeatureCache);
    }
    let mut proposals: Vec<Vec<usize>> = vec![Vec::new(); witnesses.len()];
    let mut stored = 0_usize;
    let mut overflow = false;
    let mut local = Vec::new();
    // **Equivalence with a single pass holds only until the budget is exhausted.** Windows are
    // walked outermost, so the stored stream is window-major while a single pass was witness-major;
    // when the cap cuts it, which pairs survive depends on the window size. That is accepted:
    // restoring witness-major order would hold every window alive at once or need a third walk, and
    // a run that sets `incomplete` already promises only a subset ("the absence of a finding proves
    // nothing", see `analyze_with_diagnostics`). A run that does not truncate is byte-identical at
    // any window size. The cap is checked per proposable match, so a witness cut mid-list keeps a
    // partial match set; that is sound because the cut marks the pass incomplete, and no later
    // window is compiled.
    scan_windows(
        definitions,
        parameter_types,
        window,
        // Summaries and compilation diagnostics were already collected by the step-matching pass;
        // recompiling here must not duplicate them.
        |start, _, index, _diagnostics| {
            for ((left, witness), accepted) in witnesses.iter().zip(proposals.iter_mut()) {
                index.matches(witness, &mut local);
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
                    if stored == budget {
                        overflow = true;
                        return ControlFlow::Break(());
                    }
                    accepted.push(right);
                    stored += 1;
                }
            }
            ControlFlow::Continue(())
        },
    );
    (proposals, overflow, OverlapProposalSource::CompiledWindows)
}

#[derive(Debug, PartialEq, Eq)]
enum OverlapProposalSource {
    FeatureCache,
    CompiledWindows,
}

/// Reuses complete first-pass matches when every selected witness is an observed feature text.
///
/// Falling back on a missing text or the first proposal beyond `budget` preserves the window-major
/// partial subset and truncation behavior of the compiled scan. A complete cached proposal stream
/// is order-equivalent after each witness's definition indices are accumulated across all windows.
fn cached_overlap_proposals(
    definitions: &[StepDefinition],
    summaries: &[MatcherSummary],
    witnesses: &[(usize, String)],
    budget: usize,
    matched_texts: &BTreeMap<&str, Vec<usize>>,
) -> Option<Vec<Vec<usize>>> {
    let mut proposals = Vec::with_capacity(witnesses.len());
    let mut stored = 0_usize;
    for (left, witness) in witnesses {
        let matches = matched_texts.get(witness.as_str())?;
        let definition = &definitions[*left];
        let mut accepted = Vec::new();
        for right in matches.iter().copied() {
            if right == *left || !summaries[right].authoritative {
                continue;
            }
            if definition.matcher_kind == definitions[right].matcher_kind
                && definition.normalized_matcher == definitions[right].normalized_matcher
            {
                continue;
            }
            if stored == budget {
                return None;
            }
            accepted.push(right);
            stored += 1;
        }
        proposals.push(accepted);
    }
    Some(proposals)
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
            name => definition
                .analysis_profile()
                .dialect()
                .parameter_sample(parameter_types.get(name)?)?,
        };
        witness.push_str(&sample);
        offset = whole.end();
    }
    witness.push_str(&witness_literal(&definition.matcher[offset..]));
    Some(witness)
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

/// Marks definitions reached by statically known indirect step texts (`step 'x'`) as used.
pub(super) fn apply_indirect_usage(
    definitions: &[StepDefinition],
    usage: Option<&crate::source_adapter::IndirectStepUsage>,
    config: &Config,
    outcome: &mut FeatureUsageOutcome,
) {
    if !definitions
        .iter()
        .any(|d| d.analysis_profile().step_invocation)
    {
        return;
    }
    let Some(usage) = usage.filter(|usage| !usage.unknown) else {
        outcome.used.extend(0..definitions.len());
        outcome.incomplete.push(
            "Ruby indirect step usage is unresolved; unused-definition findings are disabled"
                .to_owned(),
        );
        return;
    };
    if usage.texts.is_empty() {
        return;
    }
    let mut matched = Vec::new();
    scan_windows(
        definitions,
        &config.parameter_types,
        matcher_window_size(),
        |start, _, index, _| {
            for text in &usage.texts {
                index.matches(text, &mut matched);
                outcome
                    .used
                    .extend(matched.iter().map(|index| index + start));
            }
            ControlFlow::Continue(())
        },
    );
}

/// Reports `unused-definition` for each definition not in `used`; does nothing when the rule is
/// off, and skips definitions whose matchers are not global (Jest-Cucumber).
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
        if used.contains(&index) || !definition.analysis_profile().matches_by_text() {
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

/// Prepares a definition's matcher for its window's index, returning a diagnostic only for
/// resource limits. Non-global and literal matchers get no expression and are non-authoritative,
/// as is a regex the dialect cannot translate. Oversized matchers are authoritative with no
/// expression and a diagnostic; `MatcherIndex` rejects the ones that do not compile.
/// A Cucumber Expression the library cannot expand uses the permissive fallback, authoritative
/// only when every placeholder resolved.
fn prepare_matcher(
    definition: &StepDefinition,
    parameter_types: &BTreeMap<String, String>,
) -> (PreparedMatcher, Option<String>) {
    match (
        definition.analysis_profile().matches_by_text(),
        definition.matcher_kind,
    ) {
        (false, _) | (_, MatcherKind::Literal) => (
            PreparedMatcher {
                expression: None,
                authoritative: false,
            },
            None,
        ),
        (_, MatcherKind::RegularExpression) => {
            if definition.matcher.len() > MAX_REGEX_PATTERN_BYTES {
                return (
                    PreparedMatcher {
                        expression: None,
                        authoritative: true,
                    },
                    Some(regex_limit_message(definition)),
                );
            }
            let expression = definition
                .analysis_profile()
                .dialect()
                .translate(&definition.matcher, &definition.matcher_flags)
                .map(|translated| translated.expression());
            let Some(expression) = expression else {
                return (
                    PreparedMatcher {
                        expression: None,
                        authoritative: false,
                    },
                    None,
                );
            };
            let (expression, error) = checked_expression(expression, definition);
            (
                PreparedMatcher {
                    expression,
                    authoritative: true,
                },
                error,
            )
        }
        (_, MatcherKind::CucumberExpression) => {
            match cucumber_regex_expression(&definition.matcher) {
                CucumberRegexExpression::Compiled(expression) => {
                    let (expression, error) = checked_expression(expression, definition);
                    (
                        PreparedMatcher {
                            expression,
                            authoritative: true,
                        },
                        error,
                    )
                }
                CucumberRegexExpression::ResourceLimit => (
                    PreparedMatcher {
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
                    let (expression, error) = checked_expression(fallback.expression, definition);
                    (
                        PreparedMatcher {
                            expression,
                            authoritative: fallback.exact,
                        },
                        error,
                    )
                }
            }
        }
    }
}

/// Expands a Cucumber Expression to regex source: `Unsupported` when parsing or expansion fails,
/// `ResourceLimit` when the matcher or the expansion exceeds `MAX_REGEX_PATTERN_BYTES`.
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

/// Keeps `expression` for the index unless it exceeds `MAX_REGEX_PATTERN_BYTES`, which yields
/// the resource-limit diagnostic, or does not parse, which drops it with no diagnostic.
///
/// The parser has the `regex` builder's defaults, so it rejects exactly the patterns that fail
/// with a syntax error. Unparsed, such a pattern would reach `MatcherIndex` and rebuild its window.
fn checked_expression(
    expression: String,
    definition: &StepDefinition,
) -> (Option<String>, Option<String>) {
    if expression.len() > MAX_REGEX_PATTERN_BYTES {
        (None, Some(regex_limit_message(definition)))
    } else if regex_syntax::Parser::new().parse(&expression).is_err() {
        (None, None)
    } else {
        (Some(expression), None)
    }
}

/// Formats the resource-limit diagnostic with the definition's path, line and column.
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

/// Builds an anchored approximation of a Cucumber Expression: `{int}`, `{float}`, `{word}` and
/// `{string}` map to fixed patterns, other declared names to their configured pattern, and the rest
/// to `.+`, which clears `exact`. `{}` is not a placeholder here and stays literal text.
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

/// Translates literal text between placeholders: whitespace runs are escaped verbatim and other
/// tokens go through `fallback_cucumber_token_regex`. Backslash escapes are not interpreted.
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

/// Translates one whitespace-free token: `/` splits alternatives before anything else, `(x)`
/// becomes an optional group, an unclosed `(` is literal, and other text is escaped.
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
