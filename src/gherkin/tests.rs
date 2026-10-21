use super::*;

/// Checks outline table escapes preserve expanded text and source positions across plain/Markdown formats and newline styles.
#[test]
fn table_escape_matrix_preserves_outline_text_and_source_positions() {
    for (cell, expected) in [
        (r"C:\tmp\q", r"C:\tmp\q"),
        (r"north\|south", "north|south"),
        (r"north\nsouth", "north\nsouth"),
        (r"north\\south", r"north\south"),
        (r"north\\\qsouth", r"north\\qsouth"),
        (r"\日本", r"\日本"),
        ("\u{e000}\\q", "\u{e000}\\q"),
    ] {
        for markdown in [false, true] {
            for newline in ["\n", "\r\n"] {
                let source = format!(
                    "{}Feature: escapes\n{}Scenario Outline: paths\n{}Given path <value>\n{}Examples:\n  | value |\n  | {cell} |",
                    if markdown { "# " } else { "" },
                    if markdown { "## " } else { "" },
                    if markdown { "* " } else { "  " },
                    if markdown { "### " } else { "" },
                ).replace('\n', newline);
                let format = if markdown {
                    FeatureFormat::GherkinMarkdown
                } else {
                    FeatureFormat::Gherkin
                };
                let steps =
                    extract_with_format(&source, Path::new("escapes.feature"), format).unwrap();
                assert_eq!(steps.len(), 1, "{cell}");
                assert_eq!(steps[0].text, format!("path {expected}"), "{cell}");
                assert_eq!(steps[0].location.line, 3);
                assert_eq!(steps[0].location.column, 3);
            }
        }
    }
}

/// Checks table escape adaptation preserves step and attachment text and reports only actual malformed table rows.
#[test]
fn table_escapes_do_not_rewrite_step_text_or_docstrings() {
    let source = r#"Feature: escapes
 Scenario Outline: header
  Given path <val\ue>
 Examples:
  | val\ue |
  | a\q |
 Scenario: attachments
  Given literal \q
   """
   | doc\q |
   """
  Then attachment
   | data\q |
  And after \q
"#;
    let steps = extract(source, Path::new("escapes.feature")).unwrap();
    assert_eq!(
        steps.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        [r"path a\q", r"literal \q", "attachment", r"after \q"]
    );
    assert_eq!(
        steps.iter().map(|s| s.location.line).collect::<Vec<_>>(),
        [3, 8, 12, 14]
    );
    for row in [r"| bad\q", r"| bad\|", r"| a\q | b |", "| a | b |"] {
        let malformed = format!("Feature: invalid\n Scenario Outline: rows\n  Given <v>\n Examples:\n  | v |\n  {row}\n");
        let error = format!(
            "{:#}",
            extract(&malformed, Path::new("invalid.feature")).unwrap_err()
        );
        assert!(error.contains("invalid.feature"), "{row}: {error}");
        assert_eq!(
            error.contains("unterminated Gherkin table row at line 6"),
            matches!(row, r"| bad\q" | r"| bad\|"),
            "{row}: {error}"
        );
    }
    let after = "Feature: invalid\n Scenario Outline: rows\n  Given <v>\n Examples:\n  | v |\n  | bad\\q\n Scenario: later\n  Then done\n";
    let error = format!(
        "{:#}",
        extract(after, Path::new("later.feature")).unwrap_err()
    );
    assert!(
        error.contains("unterminated Gherkin table row at line 6"),
        "{error}"
    );
    let markdown = "# Feature: invalid\n## Scenario Outline: rows\n* Given <v>\n### Examples:\n  | v |\n  | bad\\q\n## Scenario: later\n* Then done\n";
    let error = format!(
        "{:#}",
        extract_with_format(
            markdown,
            Path::new("later.feature.md"),
            FeatureFormat::GherkinMarkdown
        )
        .unwrap_err()
    );
    assert!(error.contains("later.feature.md"), "{error}");
    assert!(
        error.contains("unterminated Gherkin table row at line 6"),
        "{error}"
    );
    for delimiter in ["\"\"\"", "```"] {
        let docstring = format!(
            "Feature: invalid\n Scenario: doc\n  Given text\n   {delimiter}\n   | not a table\\q\n   {delimiter}\n  Not a keyword\n"
        );
        let error = format!(
            "{:#}",
            extract(&docstring, Path::new("docstring.feature")).unwrap_err()
        );
        assert!(
            !error.contains("unterminated Gherkin table row"),
            "{delimiter}: {error}"
        );
        let table = docstring.replace("  Not a keyword\n", "  Then table\n   | bad\\q\n");
        let error = format!(
            "{:#}",
            extract(&table, Path::new("table.feature")).unwrap_err()
        );
        assert!(
            error.contains("unterminated Gherkin table row at line 8"),
            "{delimiter}: {error}"
        );
    }
    let stray_before_feature = "| prose\nFeature: invalid\n Scenario: later\n  Not a keyword\n";
    let error = format!(
        "{:#}",
        extract(stray_before_feature, Path::new("unrelated.feature")).unwrap_err()
    );
    assert!(!error.contains("unterminated Gherkin table row"), "{error}");
    for source in [
        "Feature: invalid\n Scenario: rows\n  | prose\n  Not a keyword\n",
        "Feature: invalid\n Scenario: rows\n  Given table\n   | bad\\q\n  Not a keyword\n",
    ] {
        if let Err(error) = extract(source, Path::new("unrelated.feature")) {
            let error = format!("{error:#}");
            assert!(
                !error.contains("unterminated Gherkin table row"),
                "{source:?}: {error}"
            );
        }
    }
}

/// Checks escape preparation leaves fenced docstrings intact and enforces reserved-character and parser-probe budgets.
#[test]
fn table_escape_adaptation_is_bounded_and_does_not_change_docstrings() {
    let text = "Feature: fences\n Scenario: example\n  Given text\n   ```\n   | literal\\q |\n   ```\n  Then table\n   | table\\q |\n";
    let (prepared, marker) = table_escapes::prepare(text).unwrap();
    assert!(marker.is_some());
    assert!(prepared.contains("| literal\\q |"));
    assert!(!prepared.contains("| table\\q |"));
    let alphabet: String = ('\u{e000}'..='\u{f8ff}').collect();
    let exhausted = format!("Feature: {alphabet}\n Scenario: rows\n  Given table\n   | a\\q |\n");
    let error = format!(
        "{:#}",
        extract(&exhausted, Path::new("exhausted.feature")).unwrap_err()
    );
    assert!(error.contains("failed to parse Gherkin feature exhausted.feature"));
    assert!(error.contains("reserved-character budget"));
    let mut budget = MarkdownProbeBudget {
        probes: 0,
        parsed_bytes: MAX_GHERKIN_MARKDOWN_PROBE_BYTES - text.len(),
    };
    assert!(budget
        .parse(text)
        .unwrap_err()
        .to_string()
        .contains("parser-probe budget"));
}

/// Checks malformed tables in backgrounds and rule outlines fail rather than producing truncated extraction.
#[test]
fn malformed_tables_under_rules_and_backgrounds_are_not_truncated() {
    for source in [
        "Feature: rows\n Background:\n  Given table\n   | one |\n   | a | b |\n Scenario: next\n  Then done\n",
        "Feature: rows\n Rule: scoped\n  Scenario Outline: example\n   Given <v>\n  Examples:\n   | v |\n   | a | b |\n",
    ] {
        assert!(extract(source, Path::new("invalid.feature")).is_err());
    }
}

/// Checks backgrounds, scenarios, rules, and outlines all contribute extracted steps.
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

/// Checks each examples row produces its own substituted outline step.
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

/// Checks an outline with header-only examples retains its original step text.
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

/// Checks placeholder text introduced by an examples value is not substituted again.
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

/// Checks empty examples blocks do not add steps beside populated blocks.
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

/// Checks Portuguese dialect keywords are extracted with their original spelling.
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

/// Checks feature, rule, and example headings do not become steps.
#[test]
fn structural_keywords_are_not_mistaken_for_steps() {
    let source = "Feature: Empty\n  Rule: Structure only\n    Example: No steps\n";
    assert!(extract(source, Path::new("empty.feature"))
        .unwrap()
        .is_empty());
}

/// Checks Markdown outlines expand examples while retaining original step line numbers.
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

/// Checks a Markdown scenario with a fenced docstring works under an implicit feature heading.
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

/// Checks colon-bearing prose headings do not interfere with Markdown step extraction.
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

/// Checks an unindented prose table is ignored beside a valid Markdown step.
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

/// Checks Markdown conversion restores Portuguese structural headings while ignoring prose headings.
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

/// Checks notes and fenced prose examples do not become Markdown steps or attachments.
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

/// Checks plus and ordered list markers and bold step keywords are accepted.
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

/// Checks the final Markdown step retains its text and line without a trailing newline.
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

/// Checks single-dash Markdown table separators are excluded from outline example values.
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

/// Checks excessive Markdown list candidates fail with a candidate-conversion limit.
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
    let error = format!("{error:#}");
    assert!(error.contains("failed to parse Gherkin feature candidate-storm.feature.md"));
    assert!(error.contains("candidate conversion limit"));
}

/// Checks heading and list candidates share the same conversion limit.
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
    let error = format!("{error:#}");
    assert!(error.contains("failed to parse Gherkin feature mixed-candidate-storm.feature.md"));
    assert!(error.contains("candidate conversion limit"));
}

/// Checks a tag and unknown colon heading cannot synthesize a valid feature.
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

/// Checks a valid base document remains unchanged when speculative dialect headings are offered.
#[test]
fn valid_base_documents_do_not_restore_speculative_dialect_headings() {
    let base = vec!["Feature: base".to_owned(), String::new()];
    let candidates = vec![(1, "Scenario: speculative".to_owned())];
    let mut probe_budget = MarkdownProbeBudget::default();
    let restored = restore_dialect_headings(base, &candidates, true, &mut probe_budget).unwrap();
    assert_eq!(restored, "Feature: base\n\n");
}

/// Checks byte and retry boundaries are inclusive while exhausted probe counts block further parsing.
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

/// Checks a step candidate beyond the document leaves the restored source unchanged.
#[test]
fn out_of_range_step_candidates_are_ignored_defensively() {
    let source = "Feature: base\n".to_owned();
    let candidates = vec![(1, "  Given unreachable".to_owned())];
    let mut probe_budget = MarkdownProbeBudget::default();
    let restored =
        restore_markdown_steps(source, &candidates, true, true, &mut probe_budget).unwrap();
    assert_eq!(restored, "Feature: base\n");
}

/// Checks dialect restoration retains a valid feature heading and rejects an invalid language directive.
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
    let restored = restore_dialect_headings(base, &candidates, true, &mut probe_budget).unwrap();
    assert!(restored.contains("Feature: first"));
    assert!(!restored.contains("not-a-dialect"));
}

/// Checks a prose-heavy Markdown document still extracts 150 valid steps within the probe budget.
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

/// Checks a large mixed Markdown document extracts 380 steps while ignoring implementation notes.
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

/// Checks a large default-dialect Markdown document extracts all 4,200 steps within conversion limits.
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

/// Checks a successful whole-document probe charges its count and restored byte length.
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

/// Checks ambiguous localized lists stop with an explicit Markdown conversion-budget error.
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
    let error = format!("{error:#}");
    assert!(error.contains("failed to parse Gherkin feature probe-budget.feature.md"));
    assert!(error.contains("Gherkin Markdown conversion exceeds"));
}

/// Checks repeated parses of multi-token parse errors render one sorted expected-token set.
#[test]
fn parse_errors_render_expected_tokens_in_one_order() {
    // Each parse builds a fresh `HashSet` with new hash keys, so 64 parses of a multi-token set
    // agree by chance with probability at most 2^-63 when the set is unsorted.
    for (source, expected) in [
        (
            include_str!("../../fixtures/ruby-parity/gherkin/malformed-table-neighbor/features/missing-final-pipe.feature"),
            r#"unterminated Gherkin table row at line 7: Error at 8:1: {"_", "unknown keyword"}"#,
        ),
        (
            "Feature: a\n  Scenario: b\n    Given c\n  @tag\n",
            r#"Error at 5:1: {"\"@\"", "_", "no match", "tag character", "unknown keyword"}"#,
        ),
        (
            "Feature: a\n  Scenario: b\n    Given c\n    \"\"\"\n    open\n",
            r#"Error at 6:1: {"\"\\\"\\\"\\\"\"", "_"}"#,
        ),
        (
            "Scenario: no feature\n",
            r#"Error at 1:14: {"unknown keyword"}"#,
        ),
    ] {
        // A CRLF checkout (Windows CI) of the fixture parses to a different expected-token set.
        let source = source.replace("\r\n", "\n");
        for _ in 0..64 {
            let error = extract(&source, Path::new("bad.feature")).unwrap_err();
            assert_eq!(
                format!("{error:#}"),
                format!("failed to parse Gherkin feature bad.feature: {expected}")
            );
        }
    }
}

/// Checks expected-token sorting handles escaped literals and leaves unmodeled text unchanged.
#[test]
fn sort_expected_tokens_sorts_literals_and_fails_closed() {
    for (message, expected) in [
        (
            r#"Error at 8:1: {"unknown keyword", "_"}"#,
            r#"Error at 8:1: {"_", "unknown keyword"}"#,
        ),
        (
            r#"Error at 2:3: {"\"|\"", "\", \"", "[_]", "\"\\\\\""}"#,
            r#"Error at 2:3: {"[_]", "\", \"", "\"\\\\\"", "\"|\""}"#,
        ),
        (r#"Error at 1:1: {"x"}"#, r#"Error at 1:1: {"x"}"#),
        ("Error at 1:1: {}", "Error at 1:1: {}"),
        // Unmodeled shapes keep the upstream text rather than dropping tokens.
        ("Error at 1:1: {b, a}", "Error at 1:1: {b, a}"),
        (r#"Error at 1:1: {"b", "a}"#, r#"Error at 1:1: {"b", "a}"#),
        ("plain message", "plain message"),
    ] {
        assert_eq!(sort_expected_tokens(message), expected, "{message}");
    }
}

/// Extracts the single outline step in `step` with one `Examples` row and returns its text.
fn outline_text(step: &str, headers: &[&str], values: &[&str]) -> String {
    let source = format!(
        "Feature: f\n  Scenario Outline: s\n    Given {step}\n    Examples:\n      | {} |\n      | {} |\n",
        headers.join(" | "),
        values.join(" | ")
    );
    let steps = extract(&source, Path::new("outline.feature")).unwrap();
    assert_eq!(steps.len(), 1, "{step}");
    steps[0].text.clone()
}

/// Pins `<name>` substitution across stray, nested, adjacent, repeated and unknown placeholder
/// shapes. Rows marked "reference" match `@cucumber/gherkin` 42 pickles; every verbatim claim
/// sits beside a replaced `<b>` so a blanket "never substitute" cannot pass.
#[test]
fn outline_placeholders_replace_each_header_wherever_it_appears() {
    // (step text, headers, values, expected)
    let cases: [(&str, &[&str], &[&str], &str); 20] = [
        // Stray `<` before a placeholder (reference).
        ("the <a<b> value", &["b"], &["7"], "the <a7 value"),
        ("<<b>>", &["b"], &["7"], "<7>"),
        ("<<<b>", &["b"], &["7"], "<<7"),
        // Plain, adjacent and repeated placeholders (reference).
        ("<a> <b>", &["a", "b"], &["1", "2"], "1 2"),
        ("<b><b>", &["b"], &["7"], "77"),
        ("<b>x<b>", &["b"], &["7"], "7x7"),
        // `<` and `>` at the ends and out of order (reference).
        ("<b> <", &["b"], &["7"], "7 <"),
        ("< <b>", &["b"], &["7"], "< 7"),
        ("> <b>", &["b"], &["7"], "> 7"),
        ("a> b> <b>", &["b"], &["7"], "a> b> 7"),
        // Unknown names and unclosed `<` stay verbatim (reference).
        ("<z> <b>", &["b"], &["7"], "<z> 7"),
        ("<b> <z", &["b"], &["7"], "7 <z"),
        ("<b> <b", &["b"], &["7"], "7 <b"),
        ("<B> <b>", &["b"], &["7"], "<B> 7"),
        // Headers with spaces and an empty header (reference).
        ("<my col> <b>", &["my col", "b"], &["V", "7"], "V 7"),
        ("<> <b>", &["", "b"], &["E", "7"], "E 7"),
        // The first of equal headers supplies the value (reference).
        ("<a> <b>", &["a", "a", "b"], &["1", "2", "7"], "1 7"),
        // A value is not scanned again, whatever the header order.
        ("<a> <b>", &["a", "b"], &["<b>", "7"], "<b> 7"),
        ("<b> <a>", &["b", "a"], &["<a>", "7"], "<a> 7"),
        // Unmodeled: a header name containing `>` ends at the first `>`, so it stays verbatim
        // while the ordinary header still resolves.
        ("<a>b> <b>", &["a>b", "b"], &["V", "7"], "<a>b> 7"),
    ];
    for (step, headers, values, expected) in cases {
        assert_eq!(outline_text(step, headers, values), expected, "{step}");
    }
}

/// A run of stray `<` ahead of one placeholder is scanned once, not once per `<`.
#[test]
fn outline_placeholder_scan_stays_linear_on_stray_angle_brackets() {
    let values = BTreeMap::from([("b", "7")]);
    let text = format!("{}b>", "<".repeat(200_000));
    assert_eq!(
        substitute_outline_values(&text, &values),
        format!("{}7", "<".repeat(199_999))
    );
    let unclosed = "<".repeat(200_000);
    assert_eq!(substitute_outline_values(&unclosed, &values), unclosed);
    // A header that shares a long prefix with every candidate name is compared once per length
    // match, not once per `<`: per-`<` comparison takes seconds at this size, this takes
    // milliseconds.
    let started = std::time::Instant::now();
    let long_header = format!("{}b", "<".repeat(800_000));
    let long_values = BTreeMap::from([(long_header.as_str(), "7")]);
    let miss = format!("{}a>", "<".repeat(800_001));
    assert_eq!(substitute_outline_values(&miss, &long_values), miss);
    let hit = format!("{}b>", "<".repeat(800_001));
    assert_eq!(substitute_outline_values(&hit, &long_values), "7");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// Substitution reaches only the step line: a doc string or data table under an outline step
/// keeps the step's own text and is not part of the reported step.
#[test]
fn outline_step_arguments_do_not_change_step_text() {
    let source = "Feature: f\n  Scenario Outline: s\n    Given the <a<b> value\n      \"\"\"\n      <b>\n      \"\"\"\n    And the <b> table\n      | <b> |\n    Examples:\n      | b |\n      | 7 |\n";
    let steps = extract(source, Path::new("args.feature")).unwrap();
    let texts: Vec<_> = steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(texts, ["the <a7 value", "the 7 table"]);
}

/// Extracts `source` and returns each step's `(keyword, line, column, end column)`.
fn step_columns(source: &str, format: FeatureFormat) -> Vec<(String, usize, usize, usize)> {
    extract_with_format(source, Path::new("columns.feature"), format)
        .unwrap()
        .into_iter()
        .map(|step| {
            assert_eq!(step.location.line, step.location.end_line);
            (
                step.keyword,
                step.location.line,
                step.location.column,
                step.location.end_column,
            )
        })
        .collect()
}

/// The end column is one past the last character of the step text as written, for any spaces or
/// tabs between keyword and text, any keyword form, trailing whitespace, multi-byte text and
/// CRLF. The one-space row pins the unchanged common path; the wider rows fail on the old
/// `keyword + 1 + text` arithmetic.
#[test]
fn step_end_column_follows_the_source_line() {
    // (step line, keyword, column, end column)
    let cases = [
        ("    Given the panel opens", "Given", 5, 26),
        ("    Given   the panel opens", "Given", 5, 28),
        ("    Given \tthe panel opens", "Given", 5, 27),
        ("    Given \t  the panel opens", "Given", 5, 29),
        ("    Given   the panel opens   ", "Given", 5, 28),
        ("\tGiven   x", "Given", 2, 11),
        ("    *   star step", "*", 5, 18),
        ("    And    more", "And", 5, 16),
        ("    But  less", "But", 5, 14),
        ("    Given   crème brûlée 日本", "Given", 5, 28),
        ("    Given  🙂 ok", "Given", 5, 16),
    ];
    for (line, keyword, column, end_column) in cases {
        for newline in ["\n", "\r\n"] {
            let source = format!("Feature: f\n  Scenario: s\n{line}\n").replace('\n', newline);
            assert_eq!(
                step_columns(&source, FeatureFormat::Gherkin),
                [(keyword.to_owned(), 3, column, end_column)],
                "{line:?}"
            );
        }
    }
    // Localized keyword: `Dado` is four characters.
    let source = "# language: pt\nFuncionalidade: f\n  Cenário: s\n    Dado   algo\n";
    assert_eq!(
        step_columns(source, FeatureFormat::Gherkin),
        [("Dado".to_owned(), 4, 5, 16)]
    );
    // Unmodeled: a tab straight after the keyword is not a step in the parser, so it reports no
    // step instead of a wrong column; the space-and-tab row above is the control.
    let source = "Feature: f\n  Scenario: s\n    Given\tthe panel opens\n";
    assert!(step_columns(source, FeatureFormat::Gherkin).is_empty());
    // Indentation other than spaces and tabs does not start a step either, which keeps the
    // keyword's byte offset equal to its character offset.
    for indent in ["\u{3000}", "\u{a0}", "  \u{2003}"] {
        let source = format!("Feature: f\n  Scenario: s\n{indent}Given   the panel\n");
        assert!(
            step_columns(&source, FeatureFormat::Gherkin).is_empty(),
            "{indent:?}"
        );
    }
    // A keyword with no text never yields a step with an empty span: bare, the line is not a
    // step; with a trailing space, the parser rejects the file.
    let bare = "Feature: f\n  Scenario: s\n    Given\n";
    assert!(step_columns(bare, FeatureFormat::Gherkin).is_empty());
    let spaced = "Feature: f\n  Scenario: s\n    Given \n";
    assert!(extract(spaced, Path::new("empty.feature")).is_err());
}

/// Outline expansions and backgrounds report the source line's end column, not the expanded
/// text's: the value is shorter than the placeholder in one row and longer in the other.
#[test]
fn outline_and_background_end_columns_describe_the_source_line() {
    let source = "Feature: f\n  Background:\n    Given   a  background\n  Scenario Outline: s\n    Given   the <n> panel\n    Examples:\n      | n |\n      | 1 |\n      | 12345678901234 |\n";
    assert_eq!(
        step_columns(source, FeatureFormat::Gherkin),
        [
            ("Given".to_owned(), 3, 5, 26),
            ("Given".to_owned(), 5, 5, 26),
            ("Given".to_owned(), 5, 5, 26),
        ]
    );
}

/// Markdown list items keep their column and report the end column of the converted line.
#[test]
fn markdown_step_end_columns_follow_the_converted_line() {
    let source = "# Feature: f\n\n## Scenario: s\n\n* Given   the panel\n- When \t the tab\n";
    assert_eq!(
        step_columns(source, FeatureFormat::GherkinMarkdown),
        [
            ("Given".to_owned(), 5, 3, 20),
            ("When".to_owned(), 6, 3, 17),
        ]
    );
}
