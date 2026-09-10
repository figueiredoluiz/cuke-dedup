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
    let (compiled, incomplete): (Vec<_>, Vec<_>) = definitions
        .iter()
        .map(|definition| compile_matcher(definition, &config.parameter_types))
        .unzip();
    let mut incomplete: Vec<String> = incomplete.into_iter().flatten().collect();
    // Definitions with syntax unsupported by Rust's regex engine cannot be proven unused.
    // Treat them as indeterminate instead of emitting a guaranteed false positive.
    let mut used: BTreeSet<_> = compiled
        .iter()
        .enumerate()
        .filter_map(|(index, matcher)| matcher.regex.is_none().then_some(index))
        .collect();
    let index = MatcherIndex::build(&compiled);
    let mut ambiguities: BTreeMap<_, (&FeatureStep, BTreeSet<usize>, usize)> = BTreeMap::new();
    let mut proven = ProvenAmbiguities::new(definitions.len());
    let ambiguity_is_reported = config.severity(Rule::AmbiguousStep) != Severity::Off;
    let mut matches = Vec::new();
    // Scenario Outline expansion repeats identical step text, and repeated text always produces
    // the same match set, so each distinct text is matched once.
    let mut matched_texts: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for step in steps {
        let matches: &Vec<usize> = match matched_texts.get(step.text.as_str()) {
            Some(cached) => cached,
            None => {
                index.matches(&compiled, &step.text, &mut matches);
                matched_texts
                    .entry(step.text.as_str())
                    .or_insert_with(|| matches.clone())
            }
        };
        used.extend(matches.iter().copied());
        let authoritative_matches: BTreeSet<_> = matches
            .iter()
            .copied()
            .filter(|index| compiled[*index].authoritative)
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
        &compiled,
        &index,
        &proven,
        config,
        suppressions,
        findings,
        overlap_budget,
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
    compiled: &[CompiledMatcher],
    index: &MatcherIndex,
    proven: &ProvenAmbiguities,
    config: &Config,
    suppressions: &SuppressionIndex<'_>,
    findings: &mut Vec<Finding>,
    work_budget: usize,
) -> OverlapOutcome {
    let severity = config.severity(Rule::OverlappingMatcher);
    if severity == Severity::Off {
        return OverlapOutcome {
            census: CandidateSourceCensus::default(),
            incomplete: None,
        };
    }
    let mut considered = HashSet::new();
    let mut matches = Vec::new();
    let mut evaluated = 0_usize;
    let mut proposal_work = 0_usize;
    let proposal_budget = work_budget.saturating_mul(OVERLAP_PROPOSAL_WORK_MULTIPLIER);
    let mut retained = 0_usize;
    let mut truncated = false;
    let mut suppression_work = 0_u64;
    let mut witnessed_groups = HashSet::new();
    'definitions: for (left, definition) in definitions.iter().enumerate() {
        if !compiled[left].authoritative {
            continue;
        }
        // Equivalent matchers are already connected by the duplicate rules. One representative
        // witness preserves every cross-group overlap without evaluating an identical witness N
        // times for a large duplicate group.
        if !witnessed_groups.insert((definition.matcher_kind, &definition.normalized_matcher)) {
            continue;
        }
        let Some(witness) = witness_for(definition, &config.parameter_types) else {
            continue;
        };
        index.matches(compiled, &witness, &mut matches);
        for right in matches.iter().copied() {
            if right == left || !compiled[right].authoritative {
                continue;
            }
            // Equivalent matchers are already reported as duplicates; overlap adds nothing.
            if definition.matcher_kind == definitions[right].matcher_kind
                && definition.normalized_matcher == definitions[right].normalized_matcher
            {
                continue;
            }
            if proposal_work == proposal_budget {
                truncated = true;
                break 'definitions;
            }
            proposal_work += 1;
            let pair = if left < right {
                (left, right)
            } else {
                (right, left)
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
            let expression = crate::typescript::rust_regex_expression(
                &definition.matcher,
                &definition.matcher_flags,
            );
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
mod tests {
    use super::*;
    use crate::config::ConfigOverrides;
    use crate::discovery::{SourceFile, SourceLanguage};
    use std::path::PathBuf;

    fn definitions(source: &str) -> Vec<StepDefinition> {
        crate::typescript::extract(
            source,
            &SourceFile {
                path: PathBuf::from("steps.ts"),
                language: SourceLanguage::TypeScript,
            },
        )
        .unwrap()
    }

    fn config() -> (tempfile::TempDir, Config) {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        (directory, config)
    }

    #[test]
    fn cucumber_expression_expansion_stops_at_the_regex_pattern_limit() {
        let matcher = ".".repeat(600_000);
        assert!(matches!(
            cucumber_regex_expression(&matcher),
            CucumberRegexExpression::ResourceLimit
        ));
    }

    #[test]
    fn regex_pattern_limit_is_inclusive() {
        let mut definition = crate::typescript::extract(
            "Given(/x/, () => work());",
            &SourceFile {
                path: PathBuf::from("steps.ts"),
                language: SourceLanguage::TypeScript,
            },
        )
        .unwrap()
        .remove(0);
        definition.matcher = format!(
            "(?x){}",
            " ".repeat(MAX_REGEX_PATTERN_BYTES.saturating_sub(4))
        );

        let (compiled, error) = compile_matcher(&definition, &BTreeMap::new());
        assert!(error.is_none());
        assert!(compiled.regex.is_some());
    }

    #[test]
    fn broad_ambiguity_groups_use_linear_membership_storage() {
        let matched = (0..10_000).collect::<BTreeSet<_>>();
        let mut proven = ProvenAmbiguities::new(matched.len());

        proven.record(&matched);
        proven.record(&matched);

        assert_eq!(proven.groups.len(), 1);
        assert_eq!(
            proven.memberships.iter().map(Vec::len).sum::<usize>(),
            10_000
        );
        assert!(proven.contains(0, 9_999));
    }

    #[test]
    fn matcher_overlap_respects_the_shared_candidate_budget() {
        let definitions = definitions(
            "Given('value {first}', () => first());\n\
             Given('value {second}', () => second());\n\
             Given('value {third}', () => third());",
        );
        let (_directory, mut config) = config();
        for name in ["first", "second", "third"] {
            config
                .parameter_types
                .insert(name.to_owned(), "red|green".to_owned());
        }
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();

        let outcome =
            analyze_feature_usage(&definitions, &[], &config, &suppressions, &mut findings, 1);

        assert_eq!(outcome.overlap_census.evaluated, 1);
        assert_eq!(outcome.overlap_census.skipped, 1);
        assert!(outcome
            .incomplete
            .iter()
            .any(|diagnostic| diagnostic.contains("matcher-overlap analysis is incomplete")));
    }

    #[test]
    fn reverse_witnesses_do_not_consume_the_unique_pair_budget_twice() {
        let definitions = definitions(
            "Given('value {first}', () => first());\n\
             Given('value {second}', () => second());",
        );
        let (_directory, mut config) = config();
        for name in ["first", "second"] {
            config
                .parameter_types
                .insert(name.to_owned(), "red|green".to_owned());
        }
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();

        let outcome =
            analyze_feature_usage(&definitions, &[], &config, &suppressions, &mut findings, 1);

        assert_eq!(outcome.overlap_census.evaluated, 1);
        assert_eq!(outcome.overlap_census.skipped, 0);
        assert!(outcome.incomplete.is_empty());
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn matcher_overlap_caps_finding_storms_and_marks_the_result_incomplete() {
        let mut source = String::new();
        let mut parameter_types = BTreeMap::new();
        for index in 0..150 {
            source.push_str(&format!(
                "Given('value {{type_{index}}}', () => action_{index}());\n"
            ));
            parameter_types.insert(format!("type_{index}"), "red|green".to_owned());
        }
        let definitions = definitions(&source);
        let (_directory, mut config) = config();
        config.parameter_types = parameter_types;
        let suppressions = SuppressionIndex::new(&config, &definitions);
        let mut findings = Vec::new();

        let outcome = analyze_feature_usage(
            &definitions,
            &[],
            &config,
            &suppressions,
            &mut findings,
            30_000,
        );

        assert_eq!(findings.len(), MAX_STATIC_OVERLAP_FINDINGS);
        assert_eq!(outcome.overlap_census.skipped, 1);
        assert!(outcome
            .incomplete
            .iter()
            .any(|diagnostic| diagnostic.contains("matcher-overlap analysis is incomplete")));
        assert!(findings
            .iter()
            .all(|finding| finding.rule == Rule::OverlappingMatcher));
    }

    #[test]
    fn oversized_combined_matcher_programs_split_without_changing_matches() {
        let compiled = (0..200)
            .map(|index| {
                let expression = format!("^distinct matcher value {index:04}$");
                CompiledMatcher {
                    regex: Some(Regex::new(&expression).unwrap()),
                    expression: Some(expression),
                    authoritative: true,
                }
            })
            .collect::<Vec<_>>();

        let index = MatcherIndex::build_with_limits(&compiled, 4 * 1024, 64 * 1024);
        assert!(index.sets.len() > 1);
        assert!(index.fallback_definitions.is_empty());

        let mut matches = Vec::new();
        index.matches(&compiled, "distinct matcher value 0123", &mut matches);
        assert_eq!(matches, vec![123]);
    }
}
