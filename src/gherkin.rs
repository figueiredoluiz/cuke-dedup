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
mod tests {
    use super::*;

    #[test]
    fn extracts_steps_from_full_document_structure() {
        let source = r#"
Feature: Checkout
  Background:
    Given a configured shop

  Scenario: Successful purchase
    When I pay
    Then the receipt is shown

  Rule: Declined cards are rejected
    Background:
      Given card checks are enabled

    Example: Declined purchase
      When I use a declined card

    Scenario Outline: Several invalid cards
      Then card <card> is rejected
      Examples:
        | card    |
        | stolen |
"#;
        let steps = extract(source, Path::new("checkout.feature")).unwrap();
        assert_eq!(steps.len(), 6);
        assert_eq!(steps[0].text, "a configured shop");
        assert_eq!(steps[3].text, "card checks are enabled");
        assert_eq!(steps[5].text, "card stolen is rejected");
    }

    #[test]
    fn expands_every_scenario_outline_examples_row() {
        let source = r#"
Feature: Inventory
  Scenario Outline: Counts
    Given I have <count> <item>
    Examples:
      | count | item   |
      | 2     | apples |
      | 3     | pears  |
"#;
        let steps = extract(source, Path::new("inventory.feature")).unwrap();
        assert_eq!(
            steps
                .iter()
                .map(|step| step.text.as_str())
                .collect::<Vec<_>>(),
            ["I have 2 apples", "I have 3 pears"]
        );
    }

    #[test]
    fn header_only_examples_preserve_verbatim_scenario_steps() {
        let source = r#"
Feature: Empty examples
  Scenario Outline: No data yet
    Given a known step
    Examples:
      | value |
"#;
        let steps = extract(source, Path::new("empty.feature")).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "a known step");
    }

    #[test]
    fn examples_values_are_substituted_once() {
        let source = r#"
Feature: Placeholder values
  Scenario Outline: Preserve value text
    Given <a> and <b>
    Examples:
      | a   | b |
      | <b> | z |
"#;
        let steps = extract(source, Path::new("values.feature")).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "<b> and z");
    }

    #[test]
    fn multiple_examples_blocks_keep_only_available_rows() {
        let source = r#"
Feature: Several examples
  Scenario Outline: Mixed blocks
    Given value <value>
    Examples: Populated
      | value |
      | first |
    Examples: Header only
      | value |
"#;
        let steps = extract(source, Path::new("mixed.feature")).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "value first");
    }

    #[test]
    fn uses_upstream_localized_dialect_keywords() {
        let source = r#"# language: pt
Funcionalidade: Pagamento
  Contexto:
    Dado uma loja configurada

  Regra: Cartões recusados são rejeitados
    Cenário: Compra recusada
      Quando eu uso um cartão recusado
      Então a compra falha
      E uma mensagem aparece
      Mas o pedido não é criado
"#;
        let steps = extract(source, Path::new("pagamento.feature")).unwrap();
        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0].keyword, "Dado");
        assert_eq!(steps[2].keyword, "Então");
        assert_eq!(steps[4].keyword, "Mas");
    }

    #[test]
    fn structural_keywords_are_not_mistaken_for_steps() {
        let source = "Feature: Empty\n  Rule: Structure only\n    Example: No steps\n";
        assert!(extract(source, Path::new("empty.feature"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn extracts_official_gherkin_markdown_structure_and_preserves_lines() {
        let source = r#"# Feature: Staying alive

This is living documentation with [a link](https://example.com).

## Rule: If you don't eat you die

`@important` `@essential`
### Scenario Outline: eating

* Given there are <start> cucumbers
* When I eat <eat> cucumbers
* Then I should have <left> cucumbers

#### Examples:

  | start | eat | left |
  | ----- | --- | ---- |
  |    12 |   5 |    7 |
  |    20 |   5 |   15 |
"#;
        let steps = extract_with_format(
            source,
            Path::new("staying-alive.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 6);
        assert_eq!(steps[0].text, "there are 12 cucumbers");
        assert_eq!(steps[1].text, "there are 20 cucumbers");
        assert_eq!(steps[0].location.line, 10);
        assert_eq!(steps[4].location.line, 12);
    }

    #[test]
    fn gherkin_markdown_supports_doc_strings_and_an_implicit_feature_heading() {
        let source = r#"Checkout documentation

## Scenario: payload

- Given this payload

  ```json
  {"ok": true}
  ```
"#;
        let steps = extract_with_format(
            source,
            Path::new("checkout.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "this payload");
        assert_eq!(steps[0].location.line, 5);
    }

    #[test]
    fn gherkin_markdown_ignores_colon_prose_headings() {
        let source = r#"# Feature: Documentation

## Implementation: details

## Scenario: behavior

* Given a documented step
"#;
        let steps = extract_with_format(
            source,
            Path::new("documentation.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "a documented step");
    }

    #[test]
    fn gherkin_markdown_requires_gherkin_table_indentation() {
        let source = r#"# Feature: Documentation

## Scenario: behavior

* Given a documented step

| prose | table |
| one |
"#;
        let steps = extract_with_format(
            source,
            Path::new("documentation.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
    }

    #[test]
    fn gherkin_markdown_restores_structural_headings_from_the_active_dialect() {
        let source = r#"# language: pt

# Funcionalidade: Pagamento

## Implementação: detalhes

## Cenário: Compra

* Dado uma loja configurada
"#;
        let steps = extract_with_format(
            source,
            Path::new("pagamento.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "uma loja configurada");
    }

    #[test]
    fn gherkin_markdown_separates_steps_from_prose_lists_and_fenced_examples() {
        let source = r#"# Checkout: overview

## Scenario: behavior

* Given a documented step

### Notes

* first note

```text
not a doc string
```
"#;
        let steps = extract_with_format(
            source,
            Path::new("documentation.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "a documented step");
    }

    #[test]
    fn gherkin_markdown_accepts_all_common_list_markers_and_bold_keywords() {
        let source = r#"# Feature: Lists

## Scenario: behavior

+ Given a plus step
1. When an ordered step runs
* **Then** a bold step works
"#;
        let steps = extract_with_format(
            source,
            Path::new("lists.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(
            steps
                .iter()
                .map(|step| step.text.as_str())
                .collect::<Vec<_>>(),
            ["a plus step", "an ordered step runs", "a bold step works"]
        );
    }

    #[test]
    fn gherkin_markdown_preserves_a_final_step_without_a_trailing_newline() {
        let source = "# Feature: Final line\n\n## Scenario: behavior\n\n* Given the final step";
        let steps = extract_with_format(
            source,
            Path::new("final.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "the final step");
        assert_eq!(steps[0].location.line, 5);
    }

    #[test]
    fn gherkin_markdown_accepts_single_dash_gfm_table_separators() {
        let source = r#"# Feature: Examples

## Scenario Outline: values

* Given value <value>

### Examples:

  | value |
  | - |
  | one |
"#;
        let steps = extract_with_format(
            source,
            Path::new("examples.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text, "value one");
    }

    #[test]
    fn gherkin_markdown_rejects_candidate_storms_before_repeated_parsing() {
        let source = format!(
            "# Feature: Limits\n\n## Scenario: prose\n\n{}",
            "- prose item\n".repeat(MAX_GHERKIN_MARKDOWN_CANDIDATES + 1)
        );
        let error = extract_with_format(
            &source,
            Path::new("candidate-storm.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap_err();
        assert!(error.to_string().contains("candidate conversion limit"));
    }

    #[test]
    fn gherkin_markdown_counts_heading_and_step_candidates_together() {
        let source = format!(
            "# Feature: Limits\n\n## Scenario: prose\n\n{}\n## Note: one\n## Note: two\n",
            "- prose item\n".repeat(MAX_GHERKIN_MARKDOWN_CANDIDATES - 1)
        );
        let error = extract_with_format(
            &source,
            Path::new("mixed-candidate-storm.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap_err();
        assert!(error.to_string().contains("candidate conversion limit"));
    }

    #[test]
    fn a_tag_before_an_unknown_colon_heading_does_not_create_an_implicit_feature() {
        let source = "`@smoke`\n\n# Documentation: overview\n";
        let error = extract_with_format(
            source,
            Path::new("tagged-prose.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("failed to parse Gherkin feature"));
    }

    #[test]
    fn valid_base_documents_do_not_restore_speculative_dialect_headings() {
        let base = vec!["Feature: base".to_owned(), String::new()];
        let candidates = vec![(1, "Scenario: speculative".to_owned())];
        let mut probe_budget = MarkdownProbeBudget::default();
        let restored =
            restore_dialect_headings(base, &candidates, true, &mut probe_budget).unwrap();
        assert_eq!(restored, "Feature: base\n\n");
    }

    #[test]
    fn parser_probe_and_retry_budgets_include_the_exact_ceiling() {
        let mut probe_budget = MarkdownProbeBudget {
            parsed_bytes: MAX_GHERKIN_MARKDOWN_PROBE_BYTES,
            probes: 0,
        };
        assert!(probe_budget.parse("").is_ok());
        assert!(probe_budget.parse("xx").is_err());

        assert!(whole_document_retry_is_bounded(
            MAX_GHERKIN_MARKDOWN_PROBE_BYTES,
            1
        ));
        assert!(!whole_document_retry_is_bounded(
            MAX_GHERKIN_MARKDOWN_PROBE_BYTES / 2 + 1,
            2
        ));

        let mut probe_budget = MarkdownProbeBudget {
            parsed_bytes: 0,
            probes: MAX_GHERKIN_MARKDOWN_PROBES,
        };
        assert!(probe_budget.parse("").is_err());
    }

    #[test]
    fn out_of_range_step_candidates_are_ignored_defensively() {
        let source = "Feature: base\n".to_owned();
        let candidates = vec![(1, "  Given unreachable".to_owned())];
        let mut probe_budget = MarkdownProbeBudget::default();
        let restored =
            restore_markdown_steps(source, &candidates, true, true, &mut probe_budget).unwrap();
        assert_eq!(restored, "Feature: base\n");
    }

    #[test]
    fn dialect_restoration_retains_only_candidates_that_make_the_document_valid() {
        let base = vec![
            String::new(),
            String::new(),
            "  Scenario: behavior".to_owned(),
        ];
        let candidates = vec![
            (0, "# language: not-a-dialect".to_owned()),
            (1, "Feature: first".to_owned()),
        ];
        let mut probe_budget = MarkdownProbeBudget::default();
        let restored =
            restore_dialect_headings(base, &candidates, true, &mut probe_budget).unwrap();
        assert!(restored.contains("Feature: first"));
        assert!(!restored.contains("not-a-dialect"));
    }

    #[test]
    fn prose_size_does_not_consume_budget_for_synthesized_parser_probes() {
        let prose = format!("{}\n", "x".repeat(100)).repeat(5_000);
        let source = format!(
            "# Feature: Limits\n\n{prose}\n## Scenario: behavior\n\n{}",
            "- Given a valid step\n".repeat(150)
        );
        let steps = extract_with_format(
            &source,
            Path::new("prose-heavy.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 150);
    }

    #[test]
    fn realistic_large_markdown_with_mixed_lists_stays_within_the_probe_budget() {
        let prose = format!("{}\n", "documentation ".repeat(8)).repeat(2_000);
        let items = (0..400)
            .map(|index| {
                if index % 20 == 0 {
                    format!("- implementation note {index}\n")
                } else {
                    format!("- Given documented behavior {index}\n")
                }
            })
            .collect::<String>();
        let source = format!(
            "# Feature: Large living documentation\n\n{prose}\n## Scenario: behavior\n\n{items}"
        );

        assert!(source.len() > 200 * 1024);
        let steps = extract_with_format(
            &source,
            Path::new("large-documentation.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 380);
    }

    #[test]
    fn thousands_of_default_dialect_steps_use_one_bounded_whole_document_probe() {
        let items = (0..4_200)
            .map(|index| {
                format!(
                    "- Given documented behavior {index} has supporting context {}\n",
                    "x".repeat(24)
                )
            })
            .collect::<String>();
        let source =
            format!("# Feature: Large default-English corpus\n\n## Scenario: behavior\n\n{items}");

        assert!(source.len() > 200 * 1024);
        let steps = extract_with_format(
            &source,
            Path::new("many-default-steps.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap();
        assert_eq!(steps.len(), 4_200);
    }

    #[test]
    fn default_dialect_whole_document_probe_is_charged_to_the_budget() {
        let source = "Feature: accounting\n  Scenario: behavior\n\n".to_owned();
        let candidates = vec![(2, "    Given a charged probe".to_owned())];
        let mut probe_budget = MarkdownProbeBudget::default();

        let restored =
            restore_markdown_steps(source, &candidates, true, true, &mut probe_budget).unwrap();

        assert!(restored.contains("Given a charged probe"));
        assert_eq!(probe_budget.probes, 1);
        assert_eq!(probe_budget.parsed_bytes, restored.len());
    }

    #[test]
    fn gherkin_markdown_bounds_ambiguous_parser_probe_work() {
        let items = (0..800)
            .map(|index| format!("- Dado passo válido {index}\n- prosa {}\n", "x".repeat(96)))
            .collect::<String>();
        let source = format!(
            "# language: pt\n\n# Funcionalidade: Limites\n\n## Cenário: prosa\n\n{}",
            items
        );
        let error = extract_with_format(
            &source,
            Path::new("probe-budget.feature.md"),
            FeatureFormat::GherkinMarkdown,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Gherkin Markdown conversion exceeds"));
    }
}
