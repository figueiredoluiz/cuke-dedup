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
    let restored = restore_dialect_headings(base, &candidates, true, &mut probe_budget).unwrap();
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
    let restored = restore_dialect_headings(base, &candidates, true, &mut probe_budget).unwrap();
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
