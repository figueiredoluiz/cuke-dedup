//! Full-syntax, localized Gherkin feature-step extraction.
//!
//! Parsing is delegated to the maintained `gherkin` crate. Its generated parser embeds
//! Cucumber's upstream dialect data, including translated structural and step keywords.

use crate::model::{FeatureStep, SourceLocation};
use crate::resource_limits::{
    read_utf8, GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE, MAX_GHERKIN_MARKDOWN_CANDIDATES,
    MAX_GHERKIN_MARKDOWN_PROBES, MAX_GHERKIN_MARKDOWN_PROBE_BYTES, MAX_PROJECT_INPUT_BYTES,
};
use anyhow::{bail, Context, Result};
use gherkin::{Background, Feature, GherkinEnv, Scenario, Step};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

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
    let source = read_utf8(path, "feature file", MAX_PROJECT_INPUT_BYTES)?;
    extract_with_format(&source, path, format)
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
    let parsed_source = match format {
        FeatureFormat::Gherkin => source.to_owned(),
        FeatureFormat::GherkinMarkdown => markdown_to_gherkin(source)?,
    };
    let feature = Feature::parse(&parsed_source, GherkinEnv::default())
        .with_context(|| format!("failed to parse Gherkin feature {}", path.display()))?;
    Ok(extract_feature(&feature, path))
}

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
    fn parse(&mut self, source: &str) -> Result<bool> {
        if self.probes >= MAX_GHERKIN_MARKDOWN_PROBES {
            bail!(
                "Gherkin Markdown conversion exceeds the {}-probe parser limit; reduce ambiguous headings or list items",
                MAX_GHERKIN_MARKDOWN_PROBES
            );
        }
        let parsed_bytes = self.parsed_bytes.saturating_add(source.len());
        if parsed_bytes > MAX_GHERKIN_MARKDOWN_PROBE_BYTES {
            bail!(
                "Gherkin Markdown conversion exceeds the {}-byte parser-probe budget; reduce ambiguous headings or list items",
                MAX_GHERKIN_MARKDOWN_PROBE_BYTES
            );
        }
        self.parsed_bytes = parsed_bytes;
        self.probes += 1;
        Ok(Feature::parse(source, GherkinEnv::default()).is_ok())
    }
}

fn whole_document_retry_is_bounded(probe_bytes: usize, candidates: usize) -> bool {
    probe_bytes.saturating_mul(candidates) <= MAX_GHERKIN_MARKDOWN_PROBE_BYTES
}

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

fn is_default_dialect_step(step: &str) -> bool {
    ["Given ", "When ", "Then ", "And ", "But ", "* "]
        .into_iter()
        .any(|keyword| step.starts_with(keyword))
}

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

fn markdown_heading(line: &str) -> Option<&str> {
    let heading = line.strip_prefix('#')?;
    let heading = heading.trim_start_matches('#');
    heading.strip_prefix(' ').map(str::trim_end)
}

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

fn extract_feature(feature: &Feature, path: &Path) -> Vec<FeatureStep> {
    let mut steps = Vec::new();
    if let Some(background) = &feature.background {
        collect_background(background, path, &mut steps);
    }
    for scenario in &feature.scenarios {
        collect_scenario(scenario, path, &mut steps);
    }
    for rule in &feature.rules {
        if let Some(background) = &rule.background {
            collect_background(background, path, &mut steps);
        }
        for scenario in &rule.scenarios {
            collect_scenario(scenario, path, &mut steps);
        }
    }
    steps.sort_by(|left, right| {
        left.location
            .line
            .cmp(&right.location.line)
            .then(left.location.column.cmp(&right.location.column))
    });
    steps
}

fn collect_background(background: &Background, path: &Path, output: &mut Vec<FeatureStep>) {
    output.extend(background.steps.iter().map(|step| convert_step(step, path)));
}

fn collect_scenario(scenario: &Scenario, path: &Path, output: &mut Vec<FeatureStep>) {
    if scenario.examples.is_empty() {
        output.extend(scenario.steps.iter().map(|step| convert_step(step, path)));
        return;
    }

    let output_start = output.len();
    for examples in &scenario.examples {
        let Some(table) = &examples.table else {
            continue;
        };
        let Some(headers) = table.rows.first() else {
            continue;
        };
        for row in table.rows.iter().skip(1) {
            let values: BTreeMap<_, _> = headers
                .iter()
                .zip(row)
                .map(|(header, value)| (header.as_str(), value.as_str()))
                .collect();
            output.extend(scenario.steps.iter().map(|step| {
                let text = substitute_outline_values(&step.value, &values);
                convert_step_with_text(step, path, text)
            }));
        }
    }
    if output.len() == output_start {
        output.extend(scenario.steps.iter().map(|step| convert_step(step, path)));
    }
}

fn substitute_outline_values(text: &str, values: &BTreeMap<&str, &str>) -> String {
    let mut output = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find('<') {
        output.push_str(&remaining[..start]);
        let placeholder = &remaining[start..];
        let Some(relative_end) = placeholder.find('>') else {
            output.push_str(placeholder);
            return output;
        };
        let end = start + relative_end;
        let name = &remaining[start + 1..end];
        if let Some(value) = values.get(name) {
            output.push_str(value);
        } else {
            output.push_str(&remaining[start..=end]);
        }
        remaining = &remaining[end + 1..];
    }
    output.push_str(remaining);
    output
}

fn convert_step(step: &Step, path: &Path) -> FeatureStep {
    convert_step_with_text(step, path, step.value.clone())
}

fn convert_step_with_text(step: &Step, path: &Path, text: String) -> FeatureStep {
    let keyword = step.keyword.trim().to_owned();
    let end_column = step.position.col + keyword.chars().count() + 1 + step.value.chars().count();
    FeatureStep {
        keyword,
        text,
        location: SourceLocation::new(
            path,
            step.position.line,
            step.position.col,
            step.position.line,
            end_column,
        ),
    }
}

#[cfg(test)]
mod tests;
