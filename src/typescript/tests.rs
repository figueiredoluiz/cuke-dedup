use super::*;
use crate::model::Rule;
use std::path::PathBuf;

fn file(language: SourceLanguage) -> SourceFile {
    SourceFile {
        path: PathBuf::from("features/steps/example.ts"),
        language,
    }
}

#[test]
fn extracts_cucumber_import_alias_and_regex() {
    let source = r#"
import { Given as G, Then } from '@cucumber/cucumber';
G('a user named {string}', async (name: string) => {
  await createUser(name);
});
Then(/^the user exists$/, function () { expect(this.user).toBeTruthy(); });
"#;
    let definitions = extract(source, &file(SourceLanguage::TypeScript)).unwrap();
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].registration, "Given");
    assert_eq!(definitions[0].normalized_matcher, "a user named {string}");
    assert_eq!(definitions[1].matcher_kind, MatcherKind::RegularExpression);
    assert_eq!(definitions[1].matcher, "^the user exists$");
}

#[test]
fn extracts_playwright_bdd_destructuring_alias() {
    let source = r#"
import { createBdd } from 'playwright-bdd';
const { Given: Setup, Then } = createBdd(test);
Setup(`a loaded page`, async ({ page }) => { await page.goto('/'); });
"#;
    let definitions = extract(source, &file(SourceLanguage::TypeScript)).unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].framework, Framework::PlaywrightBdd);
    assert_eq!(definitions[0].registration, "Given");
}

#[test]
fn extracts_cypress_cucumber_preprocessor_definitions() {
    let source = r#"
import { Given as Setup, Then } from '@badeball/cypress-cucumber-preprocessor';
Setup('a Cypress project', () => cy.visit('/'));
Then('the page is visible', () => cy.get('main').should('be.visible'));
"#;
    let definitions = extract(source, &file(SourceLanguage::TypeScript)).unwrap();
    assert_eq!(definitions.len(), 2);
    assert!(definitions
        .iter()
        .all(|definition| definition.framework == Framework::CypressCucumber));
}

#[test]
fn alpha_fingerprint_ignores_local_renames_but_not_opposite_assertions() {
    let first = extract(
            "Then('visible', async ({ page }) => { const item = page.locator('x'); await expect(item).toBeVisible(); });",
            &file(SourceLanguage::TypeScript),
        )
        .unwrap()
        .remove(0);
    let renamed = extract(
            "Then('shown', async ({ page }) => { const element = page.locator('x'); await expect(element).toBeVisible(); });",
            &file(SourceLanguage::TypeScript),
        )
        .unwrap()
        .remove(0);
    let opposite = extract(
            "Then('hidden', async ({ page }) => { const item = page.locator('x'); await expect(item).not.toBeVisible(); });",
            &file(SourceLanguage::TypeScript),
        )
        .unwrap()
        .remove(0);
    assert_eq!(
        first.handler.alpha_normalized,
        renamed.handler.alpha_normalized
    );
    assert_ne!(
        first.handler.alpha_normalized,
        opposite.handler.alpha_normalized
    );
}

#[test]
fn handler_source_snippets_are_normalized_and_bounded() {
    let source = format!("  {}\r\n", "界".repeat(MAX_HANDLER_SNIPPET_CHARS + 1));
    let snippet = bounded_source_snippet(&source);
    assert_eq!(snippet.chars().count(), MAX_HANDLER_SNIPPET_CHARS);
    assert!(snippet.ends_with('…'));
    assert!(!snippet.contains('\r'));
}

#[test]
fn extracts_rule_scoped_inline_suppressions_and_rejects_malformed_directives() {
    let extracted = extract_detailed(
        r#"
// cuke-dedup:ignore duplicate-matcher -- wording fixed by an external contract
// cuke-dedup:ignore unused-definition -- exercised by a remote suite
Given('external wording', () => work());
// cuke-dedup:ignore duplicate-handler
Then('broken directive', () => other());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert_eq!(extracted.definitions[0].inline_suppressions.len(), 2);
    assert_eq!(
        extracted.definitions[0].inline_suppressions[0].rule,
        Rule::DuplicateMatcher
    );
    assert_eq!(extracted.diagnostics.len(), 1);
    assert_eq!(
        extracted.diagnostics[0].level,
        ExtractionDiagnosticLevel::Error
    );
    assert!(extracted.diagnostics[0]
        .message
        .contains("cuke-dedup:ignore RULE -- REASON"));
}

#[test]
fn structural_fingerprint_masks_string_literal_contents() {
    let first = extract(
        "Then('save', async ({ page }) => { await page.locator('#save').click(); });",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap()
    .remove(0);
    let second = extract(
        "Then('saves', async ({ page }) => { await page.locator('#saves').click(); });",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap()
    .remove(0);
    assert_ne!(
        first.handler.alpha_normalized,
        second.handler.alpha_normalized
    );
    assert_eq!(first.handler.structural, second.handler.structural);
}

#[test]
fn ignores_unrelated_member_calls_and_destructuring_aliases() {
    let source = r#"
somePromise.then('not a step', () => {});
const { Given: G } = require('unrelated-lib');
G('also not a step', () => {});
"#;
    assert!(extract(source, &file(SourceLanguage::JavaScript))
        .unwrap()
        .is_empty());
}

#[test]
fn retains_regex_flags_and_decodes_javascript_hex_escapes() {
    let definitions = extract(
        r#"Then(/^THE USER$/i, namedHandler); Given("letter \x41 and \u{1F600}", namedHandler);"#,
        &file(SourceLanguage::JavaScript),
    )
    .unwrap();
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].matcher_flags, "i");
    assert_eq!(definitions[1].matcher, "letter A and 😀");
}

#[test]
fn preserves_all_flags_but_normalizes_only_semantic_regex_flags() {
    let definition = extract(
        r"Then(/^VALUE$/ygusm, () => work());",
        &file(SourceLanguage::JavaScript),
    )
    .unwrap()
    .remove(0);
    assert_eq!(definition.matcher_flags, "ygusm");
    assert_eq!(definition.normalized_matcher, "[regex-flags:msu] ^VALUE$");
}

#[test]
fn resolves_named_handlers_to_bodies_without_cross_file_name_collisions() {
    let first = extract(
        "function handler() { doAlpha(); } Given('alpha step', handler);",
        &SourceFile {
            path: PathBuf::from("steps/a.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap()
    .remove(0);
    let second = extract(
        "function handler() { doBetaDifferently(); } Given('bravo step', handler);",
        &SourceFile {
            path: PathBuf::from("steps/b.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap()
    .remove(0);

    assert!(first.handler.comparable);
    assert!(second.handler.comparable);
    assert!(first.handler.source_snippet.contains("doAlpha"));
    assert_ne!(
        first.handler.alpha_normalized,
        second.handler.alpha_normalized
    );
}

#[test]
fn resolves_bound_named_handlers_without_dropping_the_definition() {
    let definition = extract(
        "function handler() { doWork(); } Given('bound step', handler.bind(world));",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap()
    .remove(0);
    assert!(definition.handler.comparable);
    assert!(definition.handler.source_snippet.contains("doWork"));
}

#[test]
fn bound_arguments_are_part_of_handler_identity() {
    let definitions = extract(
        r#"
function handler(value) { doWork(value); }
Given('one', handler.bind(null, 1));
Given('two', handler.bind(null, 2));
Given('another one', handler.bind(null, 1));
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert_ne!(
        definitions[0].handler.alpha_normalized,
        definitions[1].handler.alpha_normalized
    );
    assert_eq!(
        definitions[0].handler.alpha_normalized,
        definitions[2].handler.alpha_normalized
    );
    assert!(definitions[0]
        .handler
        .source_snippet
        .contains("bound with (null, 1)"));
}

#[test]
fn unresolved_handler_references_are_not_comparable() {
    let definition = extract(
        "import { handler } from './shared'; Given('a step', handler);",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap()
    .remove(0);
    assert!(!definition.handler.comparable);
    assert!(!definition.handler.trivial);
}

#[test]
fn recognizes_resolved_and_inline_stub_handler_shapes() {
    let definitions = extract(
        r#"
function notImplemented() { throw new Error('pending'); }
Given('named stub', notImplemented);
Given('expression stub', () => pending());
Given('return stub', function () { return 'pending'; });
Given('await stub', async () => await pending());
Given('block await stub', async () => { await this.pending(); });
Given('return await stub', async function () { return await pending(); });
Given('promise stub', () => Promise.resolve());
Given('void stub', () => void 0);
Given('real handler', () => doWork());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();
    assert_eq!(definitions.len(), 9);
    assert!(
        definitions[0].handler.trivial,
        "resolved named stub: {:?}",
        definitions[0].handler
    );
    assert!(definitions[1].handler.trivial);
    assert!(definitions[2].handler.trivial);
    assert!(definitions[3].handler.trivial);
    assert!(definitions[4].handler.trivial);
    assert!(definitions[5].handler.trivial);
    assert!(definitions[6].handler.trivial);
    assert!(definitions[7].handler.trivial);
    assert!(!definitions[8].handler.trivial);
}

#[test]
fn real_handlers_with_stub_like_text_remain_comparable() {
    let definitions = extract(
        r#"
Given('append', () => appending(row));
Given('timer', () => world.suspendingTimer());
Given('return value', () => { return 'the confirmed order id'; });
Given('domain error', () => { throw new DomainError(order.id); });
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();
    assert_eq!(definitions.len(), 4);
    assert!(definitions
        .iter()
        .all(|definition| !definition.handler.trivial));
}

#[test]
fn structural_templates_preserve_substitution_behavior() {
    let definitions = extract(
        r#"
Given('user name', () => render(`${user.firstName} ${user.lastName}`));
Given('order total', () => render(`${order.subtotal + order.tax}`));
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();
    assert_ne!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
}

#[test]
fn lowercase_registrations_require_supported_import_evidence() {
    let unrelated = extract(
            "function fn() { work(); } then('not a step', fn); const alias = then; alias('also not a step', fn);",
            &file(SourceLanguage::JavaScript),
        )
        .unwrap();
    assert!(unrelated.is_empty());

    let supported = extract(
        r#"
import { createBdd } from 'playwright-bdd';
const { given } = createBdd(test);
given('a real step', () => work());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();
    assert_eq!(supported.len(), 1);

    for source in [
        "// import { given } from '@cucumber/cucumber';\ngiven('phantom', () => work());",
        "const documentation = \"import { given } from 'playwright-bdd'\";\ngiven('phantom', () => work());",
    ] {
        assert!(extract(source, &file(SourceLanguage::TypeScript))
            .unwrap()
            .is_empty());
    }
}

#[test]
fn regular_expression_normalization_preserves_matching_semantics() {
    assert_eq!(
        normalize_regular_expression(r"^I have (\d{2}) (foo|bar)$"),
        r"^I have (\d{2}) (foo|bar)$"
    );
    assert_eq!(
        normalize_regular_expression(r"^value (?=ahead)(\w+)$"),
        r"^value (?=ahead)(\w+)$"
    );
    assert_eq!(
        normalize_regular_expression(r"^(?<name>\w+) wins$"),
        normalize_regular_expression(r"^(\w+) wins$")
    );
    assert_ne!(
        normalize_regular_expression(r"^I wait (\d+) seconds$"),
        normalize_regular_expression(r"^I wait (a few|several) seconds$")
    );
    assert_ne!(
        normalize_regular_expression(r"^(a)\1$"),
        normalize_regular_expression(r"^(b)\1$")
    );
    assert_eq!(
        normalize_regular_expression(r"^a (?:optional )?step$"),
        "^a (?:optional )?step$"
    );
    assert_ne!(
        normalize_regular_expression(r"^a step$"),
        normalize_regular_expression(r"a step")
    );
    assert_eq!(normalize_regular_expression(r"[\s+]"), r"[\s+]");
}

#[test]
fn regex_flag_encoding_cannot_collide_with_literal_matcher_text() {
    let definitions = extract(
        r#"
Given(/^a step here$/i, () => first());
Given('a step here /i', () => second());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();
    assert_ne!(
        definitions[0].normalized_matcher,
        definitions[1].normalized_matcher
    );
}

#[test]
fn resolves_supported_registration_import_shapes_and_alias_chains() {
    for source in [
        "export { given } from '@cucumber/cucumber'; given('step', () => work());",
        "import * as c from '@cucumber/cucumber'; const { given } = c; given('step', () => work());",
        "const G = Given; const H = G; H('step', () => work());",
        "const { given: g } = require('@cucumber/cucumber'); const h = g; h('step', () => work());",
    ] {
        let definitions = extract(source, &file(SourceLanguage::TypeScript)).unwrap();
        assert_eq!(definitions.len(), 1, "source: {source}");
    }
}

#[test]
fn runtime_absent_and_shadowed_registrations_are_ignored() {
    for source in [
        "import type { given } from '@cucumber/cucumber'; given('phantom', () => work());",
        "import  type  { Given } from './support/world'; Given('phantom', () => work());",
        "function Given(name, handler) { return handler; } Given('phantom', () => work());",
        "const Given = (name, handler) => handler; Given('phantom', () => work());",
        "let Given; Given = wrap; Given('phantom', () => work());",
        "const Given = wrap; Given('phantom', () => work());",
        "class Given {} Given('phantom', () => work());",
        "type Given = (name: string) => void; export type { Given }; Given('phantom', () => work());",
    ] {
        let definitions = extract(source, &file(SourceLanguage::TypeScript)).unwrap();
        assert!(definitions.is_empty(), "source: {source}");
    }
}

#[test]
fn reports_unsupported_regexes_and_malformed_string_matchers() {
    let extracted = extract_detailed(
        r#"
Given(/value (?=ahead)/, () => work());
Given('broken \xZZ', () => broken());
Given('valid step', () => valid());
"#,
        &file(SourceLanguage::JavaScript),
    )
    .unwrap();
    assert_eq!(extracted.definitions.len(), 2);
    assert_eq!(extracted.diagnostics.len(), 3);
    assert!(extracted.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == ExtractionDiagnosticLevel::Error
            && diagnostic.message.contains("syntax errors")
    }));
    assert!(extracted.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == ExtractionDiagnosticLevel::Warning
            && diagnostic
                .message
                .contains("unsupported by static usage analysis")
    }));
    assert!(extracted.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == ExtractionDiagnosticLevel::Error
            && diagnostic.message.contains("invalid JavaScript escape")
    }));
}

#[test]
fn supported_regexes_remain_authoritative_under_resource_limits() {
    for (matcher, flags) in [
        (r"^the user exists$", ""),
        (r"^the USER exists$", "i"),
        (r"^(?:one|two|three) users$", "u"),
    ] {
        assert_eq!(
            rust_regex_support(matcher, flags),
            RegexSupport::Supported,
            "{matcher}/{flags}"
        );
    }
}

#[test]
fn deeply_nested_handlers_do_not_use_the_process_stack() {
    let depth = 8_000;
    let source = format!(
        "Given('deep step', () => {}value{});",
        "[".repeat(depth),
        "]".repeat(depth)
    );
    let extracted = extract_detailed(&source, &file(SourceLanguage::TypeScript)).unwrap();
    assert_eq!(extracted.definitions.len(), 1);
}

#[test]
fn source_locations_count_unicode_code_points() {
    let definition = extract(
        "const prefix = '日本語'; Given('unicode column', () => work());",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap()
    .remove(0);
    assert_eq!(definition.location.column, 23);
}

#[test]
fn generator_handlers_are_extracted_and_comparable() {
    let definitions = extract(
        "Given('generator', function* () { yield performWork(); });",
        &file(SourceLanguage::JavaScript),
    )
    .unwrap();
    assert_eq!(definitions.len(), 1);
    assert!(definitions[0].handler.comparable);
}

#[test]
fn dynamic_matchers_and_handlers_emit_diagnostics_instead_of_disappearing() {
    let extracted = extract_detailed(
        "Given(dynamicMatcher, () => work()); Given('wrapped', wrap(handler));",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();
    assert_eq!(extracted.definitions.len(), 1);
    assert!(!extracted.definitions[0].handler.comparable);
    assert!(extracted
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("step matcher")));
    assert!(extracted
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("step handler")));
}
