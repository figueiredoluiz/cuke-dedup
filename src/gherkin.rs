//! Full-syntax, localized Gherkin feature-step extraction.
//!
//! Parsing is delegated to the maintained `gherkin` crate. Its generated parser embeds
//! Cucumber's upstream dialect data, including translated structural and step keywords.

use crate::model::{FeatureStep, SourceLocation};
use crate::resource_limits::{
    read_utf8, WorkBudget, GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE, MAX_CONCRETE_FEATURE_STEPS,
    MAX_FEATURE_STEP_PAYLOAD_BYTES, MAX_FEATURE_STEP_SCAN_BYTES, MAX_GHERKIN_MARKDOWN_CANDIDATES,
    MAX_GHERKIN_MARKDOWN_PROBES, MAX_GHERKIN_MARKDOWN_PROBE_BYTES, MAX_PROJECT_INPUT_BYTES,
};
use anyhow::{bail, Context, Result};
use gherkin::{Feature, GherkinEnv, Scenario, Step};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod table_escapes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Gherkin source syntax selected for a discovered feature file.
#[non_exhaustive]
pub enum FeatureFormat {
    /// Classic `.feature` syntax.
    Gherkin,
    /// Markdown with Gherkin syntax used by `.feature.md` files.
    GherkinMarkdown,
}

impl FeatureFormat {
    /// Infers the parser format from a path using Cucumber's `.feature.md` convention.
    pub fn from_path(path: &Path) -> Self {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".feature.md"))
        {
            Self::GherkinMarkdown
        } else {
            Self::Gherkin
        }
    }

    /// Returns a stable name suitable for diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gherkin => "gherkin",
            Self::GherkinMarkdown => "gherkin-markdown",
        }
    }
}

/// Parses a Gherkin file and returns its concrete feature steps in source order.
pub fn extract_file(path: &Path) -> Result<Vec<FeatureStep>> {
    extract_file_with_format(path, FeatureFormat::from_path(path))
}

/// Parses a feature file with an explicitly selected source format.
pub fn extract_file_with_format(path: &Path, format: FeatureFormat) -> Result<Vec<FeatureStep>> {
    extract_file_with_budget(path, format, &mut FeatureStepBudget::default())
}

/// Reads a feature against the concrete-step budget shared by the CLI corpus.
pub(crate) fn extract_file_with_budget(
    path: &Path,
    format: FeatureFormat,
    budget: &mut FeatureStepBudget,
) -> Result<Vec<FeatureStep>> {
    let source = read_utf8(path, "feature file", MAX_PROJECT_INPUT_BYTES)?;
    extract_with_budget(&source, path, format, budget)
}

/// Parses in-memory Gherkin source, using `path` for locations and diagnostics.
pub fn extract(source: &str, path: &Path) -> Result<Vec<FeatureStep>> {
    extract_with_format(source, path, FeatureFormat::Gherkin)
}

/// Parses in-memory source in the selected Gherkin format.
pub fn extract_with_format(
    source: &str,
    path: &Path,
    format: FeatureFormat,
) -> Result<Vec<FeatureStep>> {
    extract_with_budget(source, path, format, &mut FeatureStepBudget::default())
}

/// Parses and admits a whole feature before materializing any concrete steps.
fn extract_with_budget(
    source: &str,
    path: &Path,
    format: FeatureFormat,
    budget: &mut FeatureStepBudget,
) -> Result<Vec<FeatureStep>> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let parsed_source = match format {
        FeatureFormat::Gherkin => source.to_owned(),
        FeatureFormat::GherkinMarkdown => markdown_to_gherkin(source)
            .with_context(|| format!("failed to parse Gherkin feature {}", path.display()))?,
    };
    let (parsed_source, marker) = table_escapes::prepare(&parsed_source)
        .with_context(|| format!("failed to parse Gherkin feature {}", path.display()))?;
    let mut feature = Feature::parse(&parsed_source, GherkinEnv::default())
        .map_err(|error| {
            let error = anyhow::anyhow!(sort_expected_tokens(&error.to_string()));
            match table_escapes::first_unterminated_row(&parsed_source) {
                Some(line) => {
                    error.context(format!("unterminated Gherkin table row at line {line}"))
                }
                None => error,
            }
        })
        .with_context(|| format!("failed to parse Gherkin feature {}", path.display()))?;
    table_escapes::finish(&mut feature, &parsed_source, marker)
        .with_context(|| format!("failed to parse Gherkin feature {}", path.display()))?;
    let source = StepSource {
        path,
        lines: parsed_source.lines().collect(),
    };
    extract_feature(&feature, &source, budget)
        .with_context(|| format!("failed to extract Gherkin feature {}", path.display()))
}

/// Sorts the expected-token set in the gherkin crate's `Error at L:C: {..}` message. The crate
/// renders a `HashSet` with `{:?}`, so its order changes between processes. Text that is not a
/// set of string literals is returned unchanged.
fn sort_expected_tokens(message: &str) -> String {
    message
        .strip_suffix('}')
        .and_then(|head| head.split_once(": {"))
        .map(|(position, set)| (position, set, debug_string_literals(set)))
        .filter(|(_, set, tokens)| tokens.join(", ") == *set)
        .map_or_else(
            || message.to_owned(),
            |(position, _, mut tokens)| {
                tokens.sort_unstable();
                format!("{position}: {{{}}}", tokens.join(", "))
            },
        )
}

/// Returns each `{:?}` string literal in `text`, quotes included; an unterminated literal is
/// dropped, which the caller detects because the literals no longer rebuild the text.
fn debug_string_literals(text: &str) -> Vec<&str> {
    let mut literals = Vec::new();
    let mut open = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if open.is_some() && character == '\\' {
            escaped = true;
        } else if character == '"' {
            match open.take() {
                Some(start) => literals.push(&text[start..=index]),
                None => open = Some(index),
            }
        }
    }
    literals
}

/// Converts Gherkin Markdown to classic Gherkin with one output line per source line, so parser
/// positions match the Markdown source. Keeps keyword headings without `#`, `# language:` lines,
/// backticked tag lines (with the backticks removed), fenced doc strings after a step, and `|`
/// rows indented 2-5 columns after a step or `Examples` heading; blanks the rest. The first
/// non-blank line becomes the `Feature:` line unless it is a keyword heading, tag or language
/// line. Unknown colon headings and list items are restored only where a parser probe accepts
/// them; in the default dialect, list items without an English step keyword are dropped first.
/// Errors when the candidate limit or probe budget is exceeded, or when table-escape preparation
/// fails.
fn markdown_to_gherkin(source: &str) -> Result<String> {
    let mut output = Vec::new();
    let mut dialect_heading_candidates = Vec::new();
    let mut step_candidates = Vec::new();
    let mut saw_feature = false;
    let mut saw_step = false;
    let mut allow_table = false;
    let mut doc_string_fence: Option<String> = None;
    let mut declared_dialect = false;
    let mut default_dialect = true;

    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];

        if let Some(fence) = &doc_string_fence {
            if trimmed.starts_with(fence) {
                output.push(format!("{indent}\"\"\""));
                doc_string_fence = None;
                saw_step = false;
                allow_table = false;
            } else {
                output.push(line.to_owned());
            }
            continue;
        }

        if saw_step && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            let fence = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            let media_type = trimmed.trim_start_matches(fence).trim();
            output.push(format!("{indent}\"\"\"{media_type}"));
            doc_string_fence = Some(fence.to_owned());
            continue;
        }

        if trimmed.starts_with("# language:") {
            declared_dialect = true;
            default_dialect = trimmed
                .strip_prefix("# language:")
                .is_some_and(|language| language.trim() == "en");
            output.push(line.to_owned());
            continue;
        }

        if let Some(heading) = markdown_heading(trimmed) {
            if heading
                .split_once(':')
                .is_some_and(|(keyword, _)| is_gherkin_heading_keyword(keyword))
            {
                saw_feature |= heading
                    .split_once(':')
                    .is_some_and(|(keyword, _)| keyword.trim().eq_ignore_ascii_case("feature"));
                let keyword = heading
                    .split_once(':')
                    .map_or(heading, |(keyword, _)| keyword);
                allow_table = keyword.trim().eq_ignore_ascii_case("examples");
                saw_step = false;
                output.push(format!("{indent}{heading}"));
            } else if heading.contains(':') {
                // The parser owns the complete dialect table but does not expose it. Keep
                // unknown colon headings as candidates and restore only the smallest set that
                // makes the complete synthesized document valid in its declared dialect.
                saw_step = false;
                if !declared_dialect
                    && !saw_feature
                    && output.iter().all(|line| line.trim().is_empty())
                {
                    saw_feature = true;
                    output.push(format!("Feature: {heading}"));
                } else {
                    ensure_markdown_candidate_capacity(
                        dialect_heading_candidates.len() + step_candidates.len(),
                    )?;
                    dialect_heading_candidates.push((output.len(), format!("{indent}{heading}")));
                    output.push(String::new());
                }
            } else if output.iter().all(|line| line.trim().is_empty()) && !saw_feature {
                saw_feature = true;
                output.push(format!("Feature: {heading}"));
            } else {
                output.push(String::new());
            }
            continue;
        }

        if is_markdown_tag_line(trimmed) {
            output.push(line.replace('`', ""));
            continue;
        }

        if !saw_feature && !trimmed.is_empty() && output.iter().all(|line| line.is_empty()) {
            saw_feature = true;
            output.push(format!("Feature: {}", trimmed.trim_matches('#').trim()));
            continue;
        }

        if let Some(step) = markdown_list_item(trimmed) {
            saw_step = true;
            allow_table = true;
            ensure_markdown_candidate_capacity(
                dialect_heading_candidates.len() + step_candidates.len(),
            )?;
            step_candidates.push((output.len(), format!("{indent}  {step}")));
            output.push(String::new());
            continue;
        }

        if is_table_separator(trimmed) {
            output.push(String::new());
            continue;
        }
        let table_indent = line.len() - trimmed.len();
        if trimmed.starts_with('|') && allow_table && (2..=5).contains(&table_indent) {
            output.push(line.to_owned());
            continue;
        }

        output.push(String::new());
    }

    // The Gherkin parser treats an unterminated final Markdown list item inconsistently.
    // Synthesized Gherkin is internal, so always terminate it without changing source locations.
    let mut probe_budget = MarkdownProbeBudget::default();
    let restored =
        restore_dialect_headings(output, &dialect_heading_candidates, true, &mut probe_budget)?;
    restore_markdown_steps(
        restored,
        &step_candidates,
        default_dialect,
        true,
        &mut probe_budget,
    )
}

/// Errors when `current` candidates already reach `MAX_GHERKIN_MARKDOWN_CANDIDATES`, so another
/// one may not be added.
fn ensure_markdown_candidate_capacity(current: usize) -> Result<()> {
    if current >= MAX_GHERKIN_MARKDOWN_CANDIDATES {
        bail!(
            "Gherkin Markdown exceeds the {}-candidate conversion limit; reduce prose-like headings or list items",
            MAX_GHERKIN_MARKDOWN_CANDIDATES
        );
    }
    Ok(())
}

#[derive(Default)]
struct MarkdownProbeBudget {
    parsed_bytes: usize,
    probes: usize,
}

impl MarkdownProbeBudget {
    /// Reports whether `source` parses and passes the table-width check, charging one probe and
    /// the prepared byte length. A parse failure is `Ok(false)`. Errors without charging when the
    /// probe count or cumulative byte budget would be exceeded, or when escape preparation fails.
    fn parse(&mut self, source: &str) -> Result<bool> {
        if self.probes >= MAX_GHERKIN_MARKDOWN_PROBES {
            bail!(
                "Gherkin Markdown conversion exceeds the {}-probe parser limit; reduce ambiguous headings or list items",
                MAX_GHERKIN_MARKDOWN_PROBES
            );
        }
        let (source, _) = table_escapes::prepare(source)?;
        let parsed_bytes = self.parsed_bytes.saturating_add(source.len());
        if parsed_bytes > MAX_GHERKIN_MARKDOWN_PROBE_BYTES {
            bail!(
                "Gherkin Markdown conversion exceeds the {}-byte parser-probe budget; reduce ambiguous headings or list items",
                MAX_GHERKIN_MARKDOWN_PROBE_BYTES
            );
        }
        self.parsed_bytes = parsed_bytes;
        self.probes += 1;
        Ok(Feature::parse(&source, GherkinEnv::default())
            .is_ok_and(|mut feature| table_escapes::finish(&mut feature, &source, None).is_ok()))
    }
}

/// Whether parsing a `probe_bytes` document once per candidate would fit the probe byte budget;
/// gates the speculative whole-document parse.
fn whole_document_retry_is_bounded(probe_bytes: usize, candidates: usize) -> bool {
    probe_bytes.saturating_mul(candidates) <= MAX_GHERKIN_MARKDOWN_PROBE_BYTES
}

/// Applies the group's replacements and keeps them when the rendered document parses; otherwise
/// reverts and recurses into each half until single candidates are rejected. Candidates whose
/// line is out of range are ignored, and a group with none in range costs no probe.
fn restore_candidate_group<F>(
    lines: &mut [String],
    candidates: &[(usize, String)],
    render: &F,
    probe_budget: &mut MarkdownProbeBudget,
) -> Result<()>
where
    F: Fn(&[String]) -> String,
{
    let mut previous = Vec::with_capacity(candidates.len());
    for (line, replacement) in candidates {
        if *line < lines.len() {
            previous.push((
                *line,
                std::mem::replace(&mut lines[*line], replacement.clone()),
            ));
        }
    }
    if previous.is_empty() {
        return Ok(());
    }
    if probe_budget.parse(&render(lines))? {
        return Ok(());
    }
    for (line, original) in previous {
        lines[line] = original;
    }
    if candidates.len() == 1 {
        return Ok(());
    }
    let midpoint = candidates.len() / 2;
    restore_candidate_group(lines, &candidates[..midpoint], render, probe_budget)?;
    restore_candidate_group(lines, &candidates[midpoint..], render, probe_budget)
}

/// Runs `restore_candidate_group` over consecutive `GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE` chunks,
/// so each chunk is probed with the replacements accepted from earlier chunks.
fn restore_candidate_batches<F>(
    lines: &mut [String],
    candidates: &[(usize, String)],
    render: &F,
    probe_budget: &mut MarkdownProbeBudget,
) -> Result<()>
where
    F: Fn(&[String]) -> String,
{
    for batch in candidates.chunks(GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE) {
        restore_candidate_group(lines, batch, render, probe_budget)?;
    }
    Ok(())
}

/// Restores list-item step candidates into `source`. In the default dialect, candidates without
/// an English step keyword are dropped first; `source` is returned unchanged when none remain.
/// Tries all candidates in one parse (for other dialects only when its projected cost fits the
/// byte budget), then falls back to batched group probes.
fn restore_markdown_steps(
    source: String,
    candidates: &[(usize, String)],
    default_dialect: bool,
    trailing_newline: bool,
    probe_budget: &mut MarkdownProbeBudget,
) -> Result<String> {
    if candidates.is_empty() {
        return Ok(source);
    }
    let default_candidates;
    let candidates = if default_dialect {
        default_candidates = candidates
            .iter()
            .filter(|(_, step)| is_default_dialect_step(step.trim_start()))
            .cloned()
            .collect::<Vec<_>>();
        default_candidates.as_slice()
    } else {
        candidates
    };
    if candidates.is_empty() {
        return Ok(source);
    }
    let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();
    let render = |lines: &[String]| {
        let joined = lines.join("\n");
        if trailing_newline {
            format!("{joined}\n")
        } else {
            joined
        }
    };
    let mut all = lines.clone();
    for (line, step) in candidates {
        if *line < all.len() {
            all[*line] = step.clone();
        }
    }
    let all_source = render(&all);
    // Default-English candidates were already restricted to exact Gherkin step keywords, so a
    // single whole-document parse is both authoritative and charged by its actual byte size.
    // Keep the conservative projected-work guard for dialect-dependent candidates: malformed
    // localized input can otherwise make this speculative parse path disproportionately costly.
    if (default_dialect || whole_document_retry_is_bounded(all_source.len(), candidates.len()))
        && probe_budget.parse(&all_source)?
    {
        return Ok(all_source);
    }

    // Probe bounded groups, splitting only groups that make the synthesized document invalid.
    // This keeps the parser authoritative for every dialect without reparsing once per bullet
    // when most candidates are valid steps.
    restore_candidate_batches(&mut lines, candidates, &render, probe_budget)?;
    Ok(render(&lines))
}

/// Whether `step` starts with a case-sensitive English step keyword or `*` followed by a space.
fn is_default_dialect_step(step: &str) -> bool {
    ["Given ", "When ", "Then ", "And ", "But ", "* "]
        .into_iter()
        .any(|keyword| step.starts_with(keyword))
}

/// Renders `base` with the unknown colon headings the parser accepts. Returns `base` without
/// trying any candidate when it already parses; otherwise tries all candidates at once when the
/// projected cost fits the byte budget, then batched group probes, and falls back to `base` when
/// the batched result still does not parse.
fn restore_dialect_headings(
    base: Vec<String>,
    candidates: &[(usize, String)],
    trailing_newline: bool,
    probe_budget: &mut MarkdownProbeBudget,
) -> Result<String> {
    let render = |lines: &[String]| {
        let joined = lines.join("\n");
        if trailing_newline {
            format!("{joined}\n")
        } else {
            joined
        }
    };
    let base_source = render(&base);
    if candidates.is_empty() {
        return Ok(base_source);
    }
    if probe_budget.parse(&base_source)? {
        return Ok(base_source);
    }
    let mut all = base.clone();
    for (line, heading) in candidates {
        all[*line] = heading.clone();
    }
    let all_source = render(&all);
    if whole_document_retry_is_bounded(all_source.len(), candidates.len())
        && probe_budget.parse(&all_source)?
    {
        return Ok(all_source);
    }

    // Restore only bounded groups that preserve a valid document. Invalid groups are split so
    // unrelated localized headings can still be recovered without exhaustive subset search.
    let mut accepted = base;
    restore_candidate_batches(&mut accepted, candidates, &render, probe_budget)?;
    let accepted_source = render(&accepted);
    if probe_budget.parse(&accepted_source)? {
        return Ok(accepted_source);
    }
    Ok(base_source)
}

/// Returns the text after one or more leading `#` and one space, right-trimmed; `None` when the
/// line does not start with `#` or the hashes are not followed by a space.
fn markdown_heading(line: &str) -> Option<&str> {
    let heading = line.strip_prefix('#')?;
    let heading = heading.trim_start_matches('#');
    heading.strip_prefix(' ').map(str::trim_end)
}

/// Whether `keyword` is an English Gherkin structural keyword, ignoring surrounding whitespace
/// and ASCII case.
fn is_gherkin_heading_keyword(keyword: &str) -> bool {
    matches!(
        keyword.trim().to_ascii_lowercase().as_str(),
        "feature"
            | "business need"
            | "ability"
            | "background"
            | "rule"
            | "scenario"
            | "example"
            | "scenario outline"
            | "scenario template"
            | "examples"
            | "scenarios"
    )
}

/// Returns the right-trimmed text of a `*`, `-`, `+` or `N.` list item, unwrapping a leading
/// `**bold**` keyword and joining it to the rest with one space. `None` for other lines and for
/// an unclosed `**`.
fn markdown_list_item(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("* ")
        .or_else(|| line.strip_prefix("- "))
        .or_else(|| line.strip_prefix("+ "))
        .or_else(|| {
            let (number, rest) = line.split_once(". ")?;
            (!number.is_empty() && number.chars().all(|character| character.is_ascii_digit()))
                .then_some(rest)
        })?;
    let rest = rest.trim_end();
    if let Some(bold) = rest.strip_prefix("**") {
        let (keyword, tail) = bold.split_once("**")?;
        let tail = tail.trim_start();
        return Some(if tail.is_empty() {
            keyword.to_owned()
        } else {
            format!("{keyword} {tail}")
        });
    }
    Some(rest.to_owned())
}

/// Whether the line has at least one token and every whitespace-separated token is a backticked
/// tag such as `` `@smoke` ``.
fn is_markdown_tag_line(line: &str) -> bool {
    let mut found = false;
    for token in line.split_whitespace() {
        if !(token.starts_with("`@") && token.ends_with('`')) {
            return false;
        }
        found = true;
    }
    found
}

/// Whether the line is a Markdown table delimiter row: it starts and ends with `|` and every cell
/// is one or more `-`, optionally wrapped in `:` alignment markers.
fn is_table_separator(line: &str) -> bool {
    if !line.starts_with('|') || !line.ends_with('|') {
        return false;
    }
    let cells = line.trim_matches('|').split('|');
    let mut found = false;
    for cell in cells {
        let cell = cell.trim().trim_matches(':');
        if cell.is_empty() || !cell.chars().all(|character| character == '-') {
            return false;
        }
        found = true;
    }
    found
}

/// The parsed document's path and lines, which step locations are measured against.
struct StepSource<'a> {
    path: &'a Path,
    lines: Vec<&'a str>,
}

/// Transactional retained-step admission and persistent consumed template-scan work.
#[derive(Clone, Copy)]
pub(crate) struct FeatureStepBudget {
    steps: WorkBudget,
    bytes: WorkBudget,
    scan: WorkBudget,
}

impl Default for FeatureStepBudget {
    /// Uses the per-call limits, shared across files by the CLI.
    fn default() -> Self {
        Self {
            steps: WorkBudget::new(MAX_CONCRETE_FEATURE_STEPS),
            bytes: WorkBudget::new(MAX_FEATURE_STEP_PAYLOAD_BYTES),
            scan: WorkBudget::new(MAX_FEATURE_STEP_SCAN_BYTES),
        }
    }
}

impl FeatureStepBudget {
    /// Charges retained text, keyword and path bytes before allocating them.
    fn admit_bytes(&mut self, bytes: usize) -> Result<()> {
        if !self.bytes.admit(bytes) {
            bail!(
                "feature expansion exceeds {} payload bytes",
                self.bytes.limit()
            );
        }
        Ok(())
    }
}

/// Preflights a whole feature, then materializes it in stable source/Examples-row order.
fn extract_feature(
    feature: &Feature,
    source: &StepSource,
    budget: &mut FeatureStepBudget,
) -> Result<Vec<FeatureStep>> {
    let mut admitted = *budget;
    let mut lengths = Vec::new();
    visit_feature_steps(feature, &mut |step, values| {
        if !admitted.steps.admit(1) {
            bail!(
                "feature expansion exceeds {} concrete steps",
                admitted.steps.limit()
            );
        }
        admitted.admit_bytes(source.path.as_os_str().as_encoded_bytes().len())?;
        admitted.admit_bytes(step.keyword.trim().len())?;
        // Scanning work is spent even if this feature later fails retained-step admission.
        if values.is_some() && !budget.scan.admit(step.value.len()) {
            bail!(
                "feature expansion exceeds {} scanned template bytes",
                budget.scan.limit()
            );
        }
        let mut length = 0;
        visit_step_text(&step.value, values, |part| {
            admitted.admit_bytes(part.len())?;
            length += part.len();
            Ok(())
        })?;
        lengths.push(length);
        Ok(())
    })?;
    let mut steps = Vec::with_capacity(lengths.len());
    let mut lengths = lengths.into_iter();
    let mut end_columns = BTreeMap::new();
    visit_feature_steps(feature, &mut |step, values| {
        // Reserve the checked expanded size, including when substitutions shrink the template.
        let mut text = String::with_capacity(lengths.next().expect("each step was admitted"));
        visit_step_text(&step.value, values, |part| {
            text.push_str(part);
            Ok(())
        })?;
        let row = step.position.line;
        let column = step.position.col;
        let end = *end_columns
            .entry((row, column))
            .or_insert_with(|| step_end_column(step, source));
        steps.push(FeatureStep {
            keyword: step.keyword.trim().to_owned(),
            text,
            location: SourceLocation::new(source.path, row, column, row, end),
        });
        Ok(())
    })?;
    steps.sort_by(|left, right| {
        left.location
            .line
            .cmp(&right.location.line)
            .then(left.location.column.cmp(&right.location.column))
    });
    admitted.scan = budget.scan;
    *budget = admitted;
    Ok(steps)
}

/// Visits backgrounds once and scenarios in document order, with the same walk for both passes.
fn visit_feature_steps(
    feature: &Feature,
    visit: &mut impl FnMut(&Step, Option<&OutlineValues<'_>>) -> Result<()>,
) -> Result<()> {
    for background in feature.background.iter() {
        for step in &background.steps {
            visit(step, None)?;
        }
    }
    for scenario in &feature.scenarios {
        visit_scenario_steps(scenario, visit)?;
    }
    for rule in &feature.rules {
        for background in rule.background.iter() {
            for step in &background.steps {
                visit(step, None)?;
            }
        }
        for scenario in &rule.scenarios {
            visit_scenario_steps(scenario, visit)?;
        }
    }
    Ok(())
}

/// Immutable substitutions and header metadata shared by every step in one Examples row.
struct OutlineValues<'a> {
    values: BTreeMap<&'a str, &'a str>,
    header_lengths: BTreeSet<usize>,
}

impl<'a> OutlineValues<'a> {
    /// Derives header lengths once from the same first-value-wins map used for substitution.
    fn new(values: BTreeMap<&'a str, &'a str>) -> Self {
        let header_lengths = values.keys().map(|name| name.len()).collect();
        Self {
            values,
            header_lengths,
        }
    }
}

/// Visits every example row, falling back to unsubstituted steps only when no data row exists.
fn visit_scenario_steps(
    scenario: &Scenario,
    visit: &mut impl FnMut(&Step, Option<&OutlineValues<'_>>) -> Result<()>,
) -> Result<()> {
    let mut expanded = false;
    for examples in &scenario.examples {
        let Some((headers, rows)) = examples
            .table
            .as_ref()
            .and_then(|table| table.rows.split_first())
        else {
            continue;
        };
        for row in rows {
            expanded = true;
            let mut values = BTreeMap::new();
            for (header, value) in headers.iter().zip(row) {
                values.entry(header.as_str()).or_insert(value.as_str());
            }
            let values = OutlineValues::new(values);
            for step in &scenario.steps {
                visit(step, Some(&values))?;
            }
        }
    }
    if !expanded {
        for step in &scenario.steps {
            visit(step, None)?;
        }
    }
    Ok(())
}

/// Replaces each `<name>` with its example value, where `name` is a header and runs from a `<`
/// to the next `>`. A `<` that does not open a header name stays verbatim and scanning resumes
/// right after it, so a stray `<` before a placeholder does not hide it. Unknown names, an
/// unclosed `<` and a header name containing `>` stay verbatim; substituted values are not
/// rescanned. The first of several equal headers supplies the value.
fn visit_step_text(
    text: &str,
    values: Option<&OutlineValues<'_>>,
    mut emit: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let Some(values) = values else {
        return emit(text);
    };
    let mut position = 0;
    // Only a name as long as some header is compared, so overlapping candidates that share a long
    // prefix with a header cost one comparison each instead of one per start.
    // The next `>` is looked up once per stretch of `<`, so a run of stray `<` stays linear.
    let mut close = text.find('>');
    while let Some(start) = text[position..].find('<').map(|offset| position + offset) {
        if close.is_some_and(|end| end < start) {
            close = text[start..].find('>').map(|offset| start + offset);
        }
        let Some(end) = close else {
            break;
        };
        emit(&text[position..start])?;
        let name = &text[start + 1..end];
        if let Some(value) = values
            .header_lengths
            .contains(&name.len())
            .then(|| values.values.get(name))
            .flatten()
        {
            emit(value)?;
            position = end + 1;
        } else {
            emit("<")?;
            position = start + 1;
        }
    }
    emit(&text[position..])
}

/// Returns the step's end column, one past the last character of the
/// source `step.value` on its line, whatever whitespace separates it from the keyword. The parser
/// yields no step with empty text and only spaces and tabs may precede the keyword, so the value
/// is always present on its line and the keyword's byte offset equals its character offset.
fn step_end_column(step: &Step, source: &StepSource) -> usize {
    let keyword = step.keyword.trim();
    let after_keyword = source
        .lines
        .get(step.position.line - 1)
        .and_then(|line| line.get(step.position.col - 1 + keyword.len()..))
        .unwrap_or_default();
    let separator = after_keyword
        .split(step.value.as_str())
        .next()
        .unwrap_or_default()
        .chars()
        .count();
    step.position.col + keyword.chars().count() + separator + step.value.chars().count()
}

#[cfg(test)]
mod tests;
