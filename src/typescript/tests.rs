use super::*;
use crate::model::Rule;
use std::fs;
use std::path::PathBuf;

fn extract_ts(source: &str) -> Vec<crate::model::StepDefinition> {
    extract(source, &file(SourceLanguage::TypeScript)).unwrap()
}

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
fn extracts_supported_legacy_cucumber_registration_modules_in_esm_and_cjs() {
    for (module, expected_framework) in [
        ("cucumber", Framework::CucumberJs),
        (
            "cypress-cucumber-preprocessor/steps",
            Framework::CypressCucumber,
        ),
    ] {
        for (syntax, matcher) in [
            (
                format!(
                    "import {{ Given as Setup }} from '{module}';\nSetup('esm {module}', () => work());"
                ),
                format!("esm {module}"),
            ),
            (
                format!(
                    "const legacy = require('{module}');\nlegacy.Given('cjs {module}', () => work());"
                ),
                format!("cjs {module}"),
            ),
        ] {
            let definitions = extract_ts(&syntax);
            assert_eq!(definitions.len(), 1, "{module}: {syntax}");
            assert_eq!(definitions[0].matcher, matcher);
            assert_eq!(definitions[0].registration, "Given");
            assert_eq!(definitions[0].framework, expected_framework);
        }
    }
}

#[test]
fn legacy_cypress_preprocessor_root_and_similar_prefixes_are_not_registration_modules() {
    for module in [
        "cypress-cucumber-preprocessor",
        "cypress-cucumber-preprocessor/steps-extra",
    ] {
        let source = format!(
            "import {{ Given as Setup }} from '{module}';\nSetup('not a registration export', () => work());"
        );
        assert!(extract_ts(&source).is_empty(), "{module}");
    }
}

#[test]
fn playwright_step_decorator_name_is_not_invented_for_other_framework_modules() {
    for source in [
        "import { Step } from '@cucumber/cucumber';\nStep('not exported', () => work());",
        "const { Step } = require('@badeball/cypress-cucumber-preprocessor');\nStep('not exported', () => work());",
    ] {
        assert!(extract_ts(source).is_empty(), "{source}");
    }

    assert!(extract_ts(
        "import { Fixture as Given } from 'playwright-bdd/decorators';\nGiven('not a step registration', () => work());"
    )
    .is_empty());
}

#[test]
fn extracts_playwright_bdd_method_decorators_with_aliases_and_options() {
    let source = r#"
import { Fixture, Given as Setup, Step } from 'playwright-bdd/decorators';

export class TodoSteps {
  @Fixture('ignored fixture metadata')
  page = {};

  @Setup('a prepared todo', { timeout: 1000 })
  async prepare({ page: localPage }) { await localPage.goto('/todos'); }

  @Step(`the todo is visible`)
  async verify({ page }) { await page.goto('/todos'); }
}
"#;

    let definitions = extract_ts(source);
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].matcher, "a prepared todo");
    assert_eq!(definitions[0].registration, "Given");
    assert_eq!(definitions[1].matcher, "the todo is visible");
    assert_eq!(definitions[1].registration, "Step");
    assert!(definitions
        .iter()
        .all(|definition| definition.framework == Framework::PlaywrightBdd));
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.comparable));
    assert_eq!(
        definitions[0].handler.alpha_normalized,
        definitions[1].handler.alpha_normalized
    );
    assert!(definitions[0]
        .handler
        .source_snippet
        .contains("localPage.goto"));
    assert!(!definitions[0].handler.source_snippet.contains("prepare"));
}

#[test]
fn extracts_playwright_bdd_method_decorators_from_cjs_namespaces() {
    let source = r#"
const decorators = require('playwright-bdd/decorators');
class WorkspaceSteps {
  @decorators.Given('a CJS decorator')
  async prepare() { await work(); }
}
"#;

    let definitions = extract_ts(source);
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].matcher, "a CJS decorator");
    assert_eq!(definitions[0].registration, "Given");
    assert_eq!(definitions[0].framework, Framework::PlaywrightBdd);
}

#[test]
fn extracts_playwright_bdd_method_decorators_with_javascript_grammar() {
    let source = r#"
import { Given } from 'playwright-bdd/decorators';
class JavaScriptSteps {
  @Given('a JavaScript decorator')
  async prepare() { await work(); }
}
"#;
    for extension in ["js", "jsx"] {
        let source_file = SourceFile {
            path: PathBuf::from(format!("features/steps/example.{extension}")),
            language: SourceLanguage::JavaScript,
        };
        let extracted = extract_detailed(source, &source_file).unwrap();
        assert!(extracted.diagnostics.is_empty(), "{extension}");
        assert_eq!(extracted.definitions.len(), 1, "{extension}");
        assert_eq!(extracted.definitions[0].matcher, "a JavaScript decorator");
        assert_eq!(extracted.definitions[0].framework, Framework::PlaywrightBdd);
    }
}

#[test]
fn unsupported_cjs_destructured_exports_shadow_ambient_registration_names() {
    for source in [
        r#"
const { Fixture: Given } = require('playwright-bdd/decorators');
Given('not a decorator registration', () => work());
"#,
        r#"
const { unsupported: Given } = require('cypress-cucumber-preprocessor/steps');
Given('not a legacy Cypress registration', () => work());
"#,
    ] {
        assert!(extract_ts(source).is_empty(), "{source}");
    }
}

#[test]
fn call_and_decorator_registration_apis_are_not_interchangeable() {
    let decorator_as_call = r#"
import { Given } from 'playwright-bdd/decorators';
Given('not an ordinary registration', () => work());
"#;
    assert!(extract_ts(decorator_as_call).is_empty());

    let call_as_decorator = r#"
import { createBdd } from 'playwright-bdd';
const { Given } = createBdd();
class InvalidSteps {
  @Given('not a decorator registration')
  async prepare() { await work(); }
}
"#;
    assert!(extract_ts(call_as_decorator).is_empty());

    let cucumber_call_as_decorator = r#"
import { Given } from '@cucumber/cucumber';
class InvalidCucumberSteps {
  @Given('not a Cucumber decorator')
  prepare() { work(); }
}
"#;
    assert!(extract_ts(cucumber_call_as_decorator).is_empty());

    let unsupported_create_bdd_export = r#"
import { createBdd } from 'playwright-bdd';
const { Step } = createBdd();
Step('not exported by createBdd', () => work());
"#;
    assert!(extract_ts(unsupported_create_bdd_export).is_empty());
}

#[test]
fn project_reexports_preserve_playwright_decorator_registration_provenance() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("decorators.ts"),
        "export { Given as Setup } from 'playwright-bdd/decorators';\n",
    )
    .unwrap();
    let path = directory.path().join("steps.ts");
    let source = r#"
import { Setup } from './decorators';
class WorkspaceSteps {
  @Setup('a re-exported decorator')
  async prepare() { await work(); }
}
"#;
    fs::write(&path, source).unwrap();
    let source_file = SourceFile {
        path,
        language: SourceLanguage::TypeScript,
    };
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let extracted = extract_detailed_impl(source, &source_file, &mut session).unwrap();
    assert!(extracted.diagnostics.is_empty());
    assert_eq!(extracted.definitions.len(), 1);
    assert_eq!(extracted.definitions[0].registration, "Given");
    assert_eq!(extracted.definitions[0].framework, Framework::PlaywrightBdd);
}

#[test]
fn dynamic_playwright_bdd_decorator_matchers_warn_without_inventing_definitions() {
    let source = r#"
import { Given } from 'playwright-bdd/decorators';
class DynamicSteps {
  @Given(computeMatcher())
  async prepare() { await work(); }
}
"#;
    let file = file(SourceLanguage::TypeScript);
    let extracted = extract_detailed(source, &file).unwrap();

    assert!(extracted.definitions.is_empty());
    assert!(extracted.diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("dynamic or unsupported step matcher")));
}

#[test]
fn decorated_stub_methods_keep_the_existing_trivial_handler_guard() {
    let source = r#"
import { Given, Then } from 'playwright-bdd/decorators';
class PendingSteps {
  @Given('a pending setup')
  async prepare() { await pending(); }

  @Then('a pending result')
  verify() { throw new Error('not implemented'); }
}
"#;
    let definitions = extract_ts(source);
    assert_eq!(definitions.len(), 2);
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.trivial));
}

#[test]
fn decorated_method_fingerprints_include_runtime_declaration_semantics() {
    let source = r#"
import { Given, When, Then, Step } from 'playwright-bdd/decorators';
class RuntimeSemantics {
  @Given('async handler')
  async asyncHandler(value) { return work(value); }

  @When('sync handler')
  syncHandler(value) { return work(value); }

  @Then('first default')
  withFirstDefault(value = first()) { return work(value); }

  @Step('second default')
  withSecondDefault(value = second()) { return work(value); }

  @Given('renamed default one')
  renamedDefaultOne(value = seed()) { return work(value); }

  @When('renamed default two')
  renamedDefaultTwo(renamed = seed()) { return work(renamed); }

  @Then('static handler')
  static staticHandler(value) { return work(value); }

  @Step('instance handler')
  instanceHandler(value) { return work(value); }

  @Given('generator handler')
  *generatorHandler(value) { return work(value); }
}
"#;
    let definitions = extract_ts(source);
    assert_eq!(definitions.len(), 9);
    assert_ne!(
        definitions[0].handler.alpha_normalized, definitions[1].handler.alpha_normalized,
        "async and sync methods are behaviorally distinct"
    );
    assert_ne!(
        definitions[2].handler.alpha_normalized, definitions[3].handler.alpha_normalized,
        "different default initializers must remain visible"
    );
    assert!(definitions[2]
        .handler
        .behavior_signature
        .iter()
        .any(|event| event == "call:first"));
    assert!(!definitions[2].handler.trivial);
    assert_eq!(
        definitions[4].handler.alpha_normalized, definitions[5].handler.alpha_normalized,
        "renaming a parameter must not change an equivalent defaulted handler"
    );
    assert_ne!(
        definitions[6].handler.alpha_normalized, definitions[7].handler.alpha_normalized,
        "static and instance methods are behaviorally distinct"
    );
    assert_ne!(
        definitions[7].handler.alpha_normalized, definitions[8].handler.alpha_normalized,
        "generator and ordinary methods are behaviorally distinct"
    );
}

#[test]
fn decorated_method_modifiers_are_detected_across_comments() {
    let source = r#"
import { Given, When, Then, Step } from 'playwright-bdd/decorators';
class CommentedModifiers {
  @Given('commented async')
  async/**/commentedAsync(value) { return work(value); }

  @When('ordinary sync')
  ordinarySync(value) { return work(value); }

  @Then('commented static')
  static/* note */commentedStatic(value) { return work(value); }

  @Step('ordinary instance')
  ordinaryInstance(value) { return work(value); }
}
"#;
    let definitions = extract_ts(source);
    assert_eq!(definitions.len(), 4);
    assert_ne!(
        definitions[0].handler.alpha_normalized,
        definitions[1].handler.alpha_normalized
    );
    assert_ne!(
        definitions[2].handler.alpha_normalized,
        definitions[3].handler.alpha_normalized
    );
}

#[test]
fn malformed_decorator_registrations_warn_without_crossing_class_members() {
    let source = r#"
import { Given } from 'playwright-bdd/decorators';
class InvalidSteps {
  @Given()
  emptyMatcher() { work(); }

  @Given('not attached to a method')
  value = 1;
}
"#;
    let extracted = extract_detailed(source, &file(SourceLanguage::TypeScript)).unwrap();
    assert!(extracted.definitions.is_empty());
    assert!(extracted.diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("dynamic or unsupported step matcher")));
    assert!(extracted.diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("dynamic or unsupported step handler")));
}

#[test]
fn project_resolved_registrations_propagate_framework_metadata() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("support")).unwrap();
    fs::create_dir_all(directory.path().join("packages/bdd/src")).unwrap();
    fs::write(
        directory.path().join("package.json"),
        r##"{"imports":{"#bdd":"./support/world.ts"},"workspaces":["packages/*"]}"##,
    )
    .unwrap();
    fs::write(
        directory.path().join("support/world.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("packages/bdd/package.json"),
        r#"{"name":"@example/bdd","exports":"./src/index.ts"}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("packages/bdd/src/index.ts"),
        "export { Given } from 'playwright-bdd';\n",
    )
    .unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    for (name, module, expected) in [
        ("imports.ts", "#bdd", Framework::CucumberJs),
        ("workspace.ts", "@example/bdd", Framework::PlaywrightBdd),
    ] {
        let path = directory.path().join(name);
        fs::write(
            &path,
            format!("import {{ Given }} from '{module}';\nGiven('resolved', () => work());\n"),
        )
        .unwrap();
        let file = SourceFile {
            path,
            language: SourceLanguage::TypeScript,
        };
        let source = fs::read_to_string(&file.path).unwrap();
        let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();
        assert_eq!(extracted.definitions.len(), 1, "{module}");
        assert_eq!(extracted.definitions[0].framework, expected, "{module}");
    }
}

#[test]
fn mixed_framework_detection_is_scoped_to_each_source_file() {
    let directory = tempfile::tempdir().unwrap();
    let cases = [
        (
            "playwright.steps.ts",
            "import { createBdd } from 'playwright-bdd';\nconst { Given } = createBdd();\nGiven('playwright step', () => work());\n",
            Framework::PlaywrightBdd,
        ),
        (
            "cucumber.steps.ts",
            "import { Given } from '@cucumber/cucumber';\nGiven('cucumber step', () => work());\n",
            Framework::CucumberJs,
        ),
        (
            "cypress.steps.ts",
            "import { Given } from '@badeball/cypress-cucumber-preprocessor';\nGiven('cypress step', () => work());\n",
            Framework::CypressCucumber,
        ),
    ];
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    for (name, source, expected) in cases {
        let path = directory.path().join(name);
        fs::write(&path, source).unwrap();
        let source_file = SourceFile {
            path,
            language: SourceLanguage::TypeScript,
        };
        let extracted = extract_detailed_impl(source, &source_file, &mut session).unwrap();
        assert_eq!(extracted.definitions.len(), 1, "{name}");
        assert_eq!(extracted.definitions[0].framework, expected, "{name}");
    }
}

#[test]
fn playwright_bdd_factory_requires_import_evidence_and_supports_aliases() {
    for source in [
        "import { createBdd as makeBdd } from 'playwright-bdd';\n\
         const { Given } = makeBdd();\nGiven('aliased esm', () => work());",
        "const { createBdd: makeBdd } = require('playwright-bdd');\n\
         const { Given } = makeBdd();\nGiven('aliased cjs', () => work());",
        "const { createBdd } = require('playwright-bdd');\n\
         const { Given } = createBdd();\nGiven('shorthand cjs', () => work());",
    ] {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert_eq!(definitions[0].framework, Framework::PlaywrightBdd);
    }

    let unrelated = extract_ts(
        "const createBdd = () => ({ Given: () => {} });\n\
         const { Given } = createBdd();\nGiven('not a step', () => work());",
    );
    assert!(unrelated.is_empty());
}

#[test]
fn mixed_framework_bindings_keep_per_registration_provenance_in_any_import_order() {
    for imports in [
        r#"
import { Given as CypressGiven } from '@badeball/cypress-cucumber-preprocessor';
import { Given as DecoratorGiven } from 'playwright-bdd/decorators';
import { Then as CucumberThen } from '@cucumber/cucumber';
"#,
        r#"
import { Then as CucumberThen } from '@cucumber/cucumber';
import { Given as DecoratorGiven } from 'playwright-bdd/decorators';
import { Given as CypressGiven } from '@badeball/cypress-cucumber-preprocessor';
"#,
    ] {
        let source = format!(
            r#"{imports}
CypressGiven('a Cypress step', () => cy.visit('/'));
CucumberThen('a Cucumber step', () => work());
class MixedSteps {{
  @DecoratorGiven('a Playwright decorator')
  async prepare() {{ await work(); }}
}}
"#
        );
        let definitions = extract_ts(&source);
        assert_eq!(definitions.len(), 3, "{source}");
        let frameworks = definitions
            .iter()
            .map(|definition| (definition.matcher.as_str(), definition.framework))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            frameworks.get("a Cypress step"),
            Some(&Framework::CypressCucumber)
        );
        assert_eq!(
            frameworks.get("a Cucumber step"),
            Some(&Framework::CucumberJs)
        );
        assert_eq!(
            frameworks.get("a Playwright decorator"),
            Some(&Framework::PlaywrightBdd)
        );
    }
}

#[test]
fn project_resolves_playwright_bdd_registrations_created_and_exported_by_local_fixture() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("fixtures")).unwrap();
    fs::create_dir_all(directory.path().join("steps")).unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"paths":{"~/*":["./*"]}}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("fixtures/test.ts"),
        r#"
import { createBdd as makeBdd } from 'playwright-bdd';
const test = {};
export const { Given, When: Action, Then } = makeBdd(test);
"#,
    )
    .unwrap();
    let path = directory.path().join("steps/example.steps.ts");
    fs::write(
        &path,
        r#"
import { Given, Action, Then } from '~/fixtures/test';
Given('a local fixture', () => prepare());
Action('an aliased registration', () => act());
Then('the result is visible', () => verify());
"#,
    )
    .unwrap();
    let file = SourceFile {
        path,
        language: SourceLanguage::TypeScript,
    };
    let source = fs::read_to_string(&file.path).unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();

    assert!(extracted.diagnostics.is_empty());
    assert_eq!(extracted.definitions.len(), 3);
    assert_eq!(extracted.definitions[0].registration, "Given");
    assert_eq!(extracted.definitions[1].registration, "When");
    assert_eq!(extracted.definitions[2].registration, "Then");
    assert!(extracted
        .definitions
        .iter()
        .all(|definition| definition.framework == Framework::PlaywrightBdd));
}

#[test]
fn resolved_framework_facades_provide_expect_import_provenance() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("fixtures")).unwrap();
    fs::create_dir_all(directory.path().join("steps")).unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"paths":{"~/*":["./*"]}}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("fixtures/test.ts"),
        r#"
import { createBdd } from 'playwright-bdd';
export const { Then } = createBdd({});
export const expect = createAssertionFactory();
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("fixtures/lookalike.ts"),
        r#"
export const Then = makeLocalCallback();
export const expect = makeLocalAssertionFactory();
"#,
    )
    .unwrap();
    let path = directory.path().join("steps/example.steps.ts");
    let source = r#"
import { Then } from '~/fixtures/test';
import { expect } from '~/fixtures/test';
import * as api from '~/fixtures/test';
import { Given } from '@cucumber/cucumber';
import { Then as helper, expect as localCheck } from '~/fixtures/lookalike';
Then('a facade assertion', ({ state }) => expect(state).toBe('ready'));
api.Then('a facade namespace is not assertion provenance', ({ state }) => api.expect(state).toBe('ready'));
Given('an unrelated module is not assertion provenance', ({ state }) => localCheck(state).toBe('ready'));
"#;
    fs::write(&path, source).unwrap();
    let file = SourceFile {
        path,
        language: SourceLanguage::TypeScript,
    };
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let extracted = extract_detailed_impl(source, &file, &mut session).unwrap();

    assert!(extracted.diagnostics.is_empty());
    assert_eq!(extracted.definitions.len(), 3);
    assert_eq!(extracted.definitions[0].handler.behavior_signature.len(), 1);
    assert!(
        extracted.definitions[0].handler.behavior_signature[0].starts_with("assert:expect#toBe:")
    );
    assert!(extracted.definitions[1]
        .handler
        .behavior_signature
        .iter()
        .all(|event| !event.starts_with("assert:")));
    assert!(extracted.definitions[2]
        .handler
        .behavior_signature
        .iter()
        .all(|event| !event.starts_with("assert:")));
}

#[test]
fn project_resolves_commonjs_playwright_bdd_factory_aliases() {
    for (name, fixture) in [
        (
            "alias",
            "const { createBdd: makeBdd } = require('playwright-bdd');\n\
             export const { Given } = makeBdd(test);\n",
        ),
        (
            "shorthand",
            "const { createBdd } = require('playwright-bdd');\n\
             export const { Given } = createBdd(test);\n",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        fs::write(directory.path().join("fixture.ts"), fixture).unwrap();
        let path = directory.path().join(format!("{name}.steps.ts"));
        fs::write(
            &path,
            "import { Given } from './fixture';\nGiven('from cjs fixture', () => work());\n",
        )
        .unwrap();
        let file = SourceFile {
            path,
            language: SourceLanguage::TypeScript,
        };
        let source = fs::read_to_string(&file.path).unwrap();
        let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

        let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();

        assert!(extracted.diagnostics.is_empty(), "{name}");
        assert_eq!(extracted.definitions.len(), 1, "{name}");
        assert_eq!(extracted.definitions[0].framework, Framework::PlaywrightBdd);
    }
}

#[test]
fn project_resolves_reexported_runtime_registration_imports() {
    for source in [
        "import { Given as Setup } from '@cucumber/cucumber';\nexport { Setup as Given };\n",
        "const { Given: Setup } = require('@cucumber/cucumber');\nexport { Setup as Given };\n",
        "const { Given } = require('@cucumber/cucumber');\nexport { Given };\n",
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        fs::write(directory.path().join("barrel.ts"), source).unwrap();
        let path = directory.path().join("example.steps.ts");
        fs::write(
            &path,
            "import { Given } from './barrel';\nGiven('through import barrel', () => work());\n",
        )
        .unwrap();
        let file = SourceFile {
            path,
            language: SourceLanguage::TypeScript,
        };
        let source = fs::read_to_string(&file.path).unwrap();
        let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

        let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();

        assert!(extracted.diagnostics.is_empty(), "{source}");
        assert_eq!(extracted.definitions.len(), 1, "{source}");
        assert_eq!(extracted.definitions[0].framework, Framework::CucumberJs);
    }
}

#[test]
fn project_resolves_chained_import_then_export_provenance() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("registration-source.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("registration-barrel.ts"),
        "import { Given as Setup } from './registration-source';\nexport { Setup as Given };\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("factory-source.ts"),
        "export { createBdd } from 'playwright-bdd';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("factory-barrel.ts"),
        "import { createBdd as makeBdd } from './factory-source';\nexport { makeBdd };\n",
    )
    .unwrap();
    let path = directory.path().join("example.steps.ts");
    fs::write(
        &path,
        r#"
import { Given as CucumberGiven } from './registration-barrel';
import { makeBdd as buildBdd } from './factory-barrel';
const { Given: PlaywrightGiven } = buildBdd(test);
CucumberGiven('through a registration chain', () => prepare());
PlaywrightGiven('through a factory chain', () => act());
"#,
    )
    .unwrap();
    let file = SourceFile {
        path,
        language: SourceLanguage::TypeScript,
    };
    let source = fs::read_to_string(&file.path).unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();

    assert!(extracted.diagnostics.is_empty());
    assert_eq!(extracted.definitions.len(), 2);
    assert_eq!(extracted.definitions[0].framework, Framework::CucumberJs);
    assert_eq!(extracted.definitions[1].framework, Framework::PlaywrightBdd);
}

#[test]
fn module_resolution_clears_active_paths_after_recoverable_errors() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let module = directory.path().join("fixture.ts");
    fs::write(&module, "export { Given from '@cucumber/cucumber';\n").unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let first_path = directory.path().join("first.steps.ts");
    fs::write(
        &first_path,
        "import { Given } from './fixture';\nGiven('first attempt', () => work());\n",
    )
    .unwrap();
    let first_file = SourceFile {
        path: first_path,
        language: SourceLanguage::TypeScript,
    };
    let first_source = fs::read_to_string(&first_file.path).unwrap();
    let first = extract_detailed_impl(&first_source, &first_file, &mut session).unwrap();
    assert!(first.definitions.is_empty());
    assert!(!first.diagnostics.is_empty());

    fs::write(&module, "export { Given } from '@cucumber/cucumber';\n").unwrap();
    let second_path = directory.path().join("second.steps.ts");
    fs::write(
        &second_path,
        "import { Given } from './fixture';\nGiven('second attempt', () => work());\n",
    )
    .unwrap();
    let second_file = SourceFile {
        path: second_path,
        language: SourceLanguage::TypeScript,
    };
    let second_source = fs::read_to_string(&second_file.path).unwrap();
    let second = extract_detailed_impl(&second_source, &second_file, &mut session).unwrap();

    assert!(second.diagnostics.is_empty());
    assert_eq!(second.definitions.len(), 1);
    assert_eq!(second.definitions[0].framework, Framework::CucumberJs);
}

#[test]
fn project_resolves_explicit_exports_but_rejects_untrusted_create_bdd_lookalikes() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("fixture.ts"),
        r#"
import { createBdd } from 'playwright-bdd';
const { Given: Setup } = createBdd(test);
export { Setup as Given };
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("lookalike.ts"),
        r#"
const createBdd = () => ({ Given: () => {} });
export const { Given } = createBdd();
"#,
    )
    .unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    for (module, expected_definitions) in [("./fixture", 1), ("./lookalike", 0)] {
        let path = directory.path().join(format!("{}.steps.ts", &module[2..]));
        fs::write(
            &path,
            format!(
                "import {{ Given }} from '{module}';\nGiven('resolved safely', () => work());\n"
            ),
        )
        .unwrap();
        let file = SourceFile {
            path,
            language: SourceLanguage::TypeScript,
        };
        let source = fs::read_to_string(&file.path).unwrap();
        let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();
        assert_eq!(
            extracted.definitions.len(),
            expected_definitions,
            "{module}"
        );
    }
}

#[test]
fn project_resolves_wrapped_create_bdd_results_through_star_exports() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("fixture.ts"),
        r#"
import { createBdd } from 'playwright-bdd';
export const { Given } = (createBdd(test) as ReturnType<typeof createBdd>)!;
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("barrel.ts"),
        "export * from './fixture';\n",
    )
    .unwrap();
    let path = directory.path().join("example.steps.ts");
    fs::write(
        &path,
        "import { Given } from './barrel';\nGiven('through a barrel', () => work());\n",
    )
    .unwrap();
    let file = SourceFile {
        path,
        language: SourceLanguage::TypeScript,
    };
    let source = fs::read_to_string(&file.path).unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();

    assert!(extracted.diagnostics.is_empty());
    assert_eq!(extracted.definitions.len(), 1);
    assert_eq!(extracted.definitions[0].registration, "Given");
    assert_eq!(extracted.definitions[0].framework, Framework::PlaywrightBdd);
}

#[test]
fn project_does_not_trust_type_only_create_bdd_imports() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("fixture.ts"),
        r#"
import { type createBdd as Factory } from 'playwright-bdd';
export const { Given } = Factory(test);
"#,
    )
    .unwrap();
    let path = directory.path().join("example.steps.ts");
    fs::write(
        &path,
        "import { Given } from './fixture';\nGiven('not trusted', () => work());\n",
    )
    .unwrap();
    let file = SourceFile {
        path,
        language: SourceLanguage::TypeScript,
    };
    let source = fs::read_to_string(&file.path).unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);

    let extracted = extract_detailed_impl(&source, &file, &mut session).unwrap();

    assert!(extracted.definitions.is_empty());
    assert_eq!(extracted.diagnostics.len(), 1);
    assert!(extracted.diagnostics[0]
        .message
        .contains("could not be resolved"));
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
// cuke-dedup:ignore unknown-rule -- not a real rule
When('unknown rule', () => another());
// cuke-dedup:ignore unused-definition --
When('empty reason', () => finalAction());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert_eq!(extracted.definitions[0].inline_suppressions.len(), 2);
    assert_eq!(
        extracted.definitions[0].inline_suppressions[0].rule,
        Rule::DuplicateMatcher
    );
    assert_eq!(extracted.diagnostics.len(), 3);
    assert_eq!(
        extracted.diagnostics[0].level,
        ExtractionDiagnosticLevel::Error
    );
    assert!(extracted.diagnostics[0]
        .message
        .contains("cuke-dedup:ignore RULE -- REASON"));
    assert!(extracted.diagnostics[1].message.contains("unknown rule"));
    assert!(extracted.diagnostics[2]
        .message
        .contains("reason must not be empty"));
}

#[test]
fn rejects_oversized_inline_suppression_reasons() {
    let reason = "x".repeat(crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS + 1);
    let extracted = extract_detailed(
        &format!(
            "// cuke-dedup:ignore duplicate-handler -- {reason}\nGiven('step', () => work());"
        ),
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert!(extracted.definitions[0].inline_suppressions.is_empty());
    assert!(extracted.diagnostics[0]
        .message
        .contains("512-character limit"));
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
fn behavior_signatures_canonicalize_calls_without_treating_await_as_behavior() {
    struct Case {
        name: &'static str,
        source: &'static str,
        expected: &'static [&'static str],
    }

    for case in [
        Case {
            name: "direct member call",
            source: "Given('step', async function () { await this.page.click('#save'); });",
            expected: &["call:this.page#click"],
        },
        Case {
            name: "fluent member call",
            source:
                "Given('step', async function () { await this.page.locator('#save').click(); });",
            expected: &["call:this.page#click", "call:this.page#locator"],
        },
        Case {
            name: "parenthesized awaited receiver",
            source:
                "Given('step', async function () { await (await this.page.locator('#save')).click(); });",
            expected: &["call:this.page#click", "call:this.page#locator"],
        },
        Case {
            name: "factory call receiver",
            source: "Given('step', async function () { await browser().click('#save'); });",
            expected: &["call:browser#click", "call:browser"],
        },
        Case {
            name: "free local callback",
            source: "Given('step', async (perform) => { await perform(); });",
            expected: &["call:v0"],
        },
        Case {
            name: "receiver identity",
            source: "Given('step', async ({ audit }) => { await audit.click(); });",
            expected: &["call:v0#click"],
        },
    ] {
        let definition = extract(case.source, &file(SourceLanguage::TypeScript))
            .unwrap()
            .remove(0);
        assert_eq!(
            definition.handler.behavior_signature, case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn behavior_signatures_alpha_normalize_declared_receivers() {
    let definitions = extract_ts(
        "Given('first', async (page) => { await page.goto('/'); });\n\
         Given('second', async (browser) => { await browser.goto('/'); });\n\
         Given('third', page => page.goto('/'));\n\
         Given('fourth', browser => browser.goto('/'));",
    );
    assert_eq!(definitions.len(), 4);
    assert_eq!(
        definitions[0].handler.alpha_normalized,
        definitions[1].handler.alpha_normalized
    );
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    assert_eq!(definitions[0].handler.behavior_signature, ["call:v0#goto"]);
    assert_eq!(
        definitions[2].handler.alpha_normalized,
        definitions[3].handler.alpha_normalized
    );
    assert_eq!(definitions[2].handler.behavior_signature, ["call:v0#goto"]);
}

#[test]
fn behavior_signatures_preserve_assertion_subjects_and_polarity() {
    let definitions = extract_ts(
        r#"
Then('primary value', async ({ page }, expected) => {
  await expect(new Form(page).primaryInput).toHaveValue(expected);
});
Then('secondary value', async ({ page }, expected) => {
  await expect(new Form(page).secondaryInput).toHaveValue(expected);
});
Then('panel active', async ({ page }) => {
  await expect(new Navigation(page).panel).toHaveClass(/active/);
});
Then('panel inactive', async ({ page }) => {
  await expect(new Navigation(page).panel).not.toHaveClass(/active/);
});
Then('status active', async ({ page }) => {
  await expect(new Navigation(page).status).toBe('active');
});
Then('status inactive', async ({ page }) => {
  await expect(new Navigation(page).status).toBe('inactive');
});
"#,
    );

    assert_eq!(definitions.len(), 6);
    assert_ne!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    assert_ne!(
        definitions[2].handler.behavior_signature,
        definitions[3].handler.behavior_signature
    );
    assert_ne!(
        definitions[4].handler.behavior_signature,
        definitions[5].handler.behavior_signature
    );
    assert!(definitions.iter().all(|definition| {
        definition.handler.behavior_signature.len() == 1
            && definition.handler.behavior_signature[0].starts_with("assert:expect")
    }));
    assert!(definitions[3].handler.behavior_signature[0].contains(".not#toHaveClass"));
}

#[test]
fn behavior_signatures_still_alpha_normalize_assertion_subject_locals() {
    let definitions = extract_ts(
        r#"
Then('first assertion', async ({ page }) => {
  const item = page.locator('#status');
  await expect(item).toBeVisible();
});
Then('second assertion', async ({ page }) => {
  const element = page.locator('#status');
  await expect(element).toBeVisible();
});
"#,
    );

    assert_eq!(definitions.len(), 2);
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
}

#[test]
fn assertion_values_resolve_safe_local_constant_initializers() {
    let definitions = extract_ts(
        r#"
Then('first subject', ({ page }) => {
  const target = new Form(page).primaryInput;
  expect(target).toHaveValue('ready');
});
Then('second subject', ({ page }) => {
  const target = new Form(page).secondaryInput;
  expect(target).toHaveValue('ready');
});
Then('first expected value', ({ state }) => {
  const expected = 'ready';
  expect(state).toBe(expected);
});
Then('second expected value', ({ state }) => {
  const expected = 'idle';
  expect(state).toBe(expected);
});
Then('renamed equivalent constant', ({ state }) => {
  const wanted = 'ready';
  expect(state).toBe(wanted);
});
Then('chained ready constant', ({ state }) => {
  const base = 'ready';
  const expected = base;
  expect(state).toBe(expected);
});
Then('chained idle constant', ({ state }) => {
  const base = 'idle';
  const expected = base;
  expect(state).toBe(expected);
});
"#,
    );

    assert_eq!(definitions.len(), 7);
    assert_ne!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    assert_ne!(
        definitions[2].handler.behavior_signature,
        definitions[3].handler.behavior_signature
    );
    assert_eq!(
        definitions[2].handler.behavior_signature,
        definitions[4].handler.behavior_signature
    );
    assert_ne!(
        definitions[5].handler.behavior_signature,
        definitions[6].handler.behavior_signature
    );
}

#[test]
fn assertion_chains_unwrap_typescript_expression_wrappers() {
    for source in [
        "Then('as wrapper', ({ state }) => (expect(state) as any).toBe('ready'));",
        "Then('satisfies wrapper', ({ state }) => (expect(state) satisfies Assertion).toBe('ready'));",
        "Then('non-null wrapper', ({ state }) => expect(state)!.toBe('ready'));",
        "Then('type assertion wrapper', ({ state }) => (<Assertion>expect(state)).toBe('ready'));",
    ] {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert!(
            definitions[0].handler.behavior_signature[0].starts_with("assert:expect#toBe:"),
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
    }
}

#[test]
fn behavior_signatures_cover_supported_expect_variants_and_incomplete_chains() {
    let definitions = extract_ts(
        r#"
Then('soft assertion', async ({ page }) => {
  await expect.soft(page.locator('#status')).toBeVisible();
});
Then('polled assertion', async ({ state }) => {
  await expect.poll(() => state.value).toBe('ready');
});
Then('standalone expect call', ({ value }) => {
  expect(value);
});
Then('missing assertion subject', () => {
  expect().toBeVisible();
});
"#,
    );

    assert_eq!(definitions.len(), 4);
    assert_eq!(definitions[0].handler.behavior_signature.len(), 1);
    assert!(definitions[0].handler.behavior_signature[0].starts_with("assert:expect.soft#"));
    assert_eq!(definitions[1].handler.behavior_signature.len(), 1);
    assert!(definitions[1].handler.behavior_signature[0].starts_with("assert:expect.poll#"));
    assert_eq!(definitions[2].handler.behavior_signature, ["call:expect"]);
    assert_eq!(
        definitions[3].handler.behavior_signature,
        ["call:expect#toBeVisible", "call:expect"]
    );

    let deep_chain = format!(
        "Then('bounded chain', ({{ value }}) => expect(value){}.toBeVisible());",
        ".not".repeat(20)
    );
    let definition = extract_ts(&deep_chain).remove(0);
    assert!(!definition.handler.behavior_signature.is_empty());
}

#[test]
fn behavior_signatures_resolve_trusted_expect_imports_and_requires() {
    let cases = [
        r#"
import { expect as check } from '@playwright/test';
Then('aliased ESM assertion', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
import * as testApi from '@playwright/test';
Then('namespaced ESM assertion', ({ state }) => testApi.expect(state).toBe('ready'));
"#,
        r#"
const { expect: verify } = require('@jest/globals');
Then('aliased CJS assertion', ({ state }) => verify(state).toBe('ready'));
"#,
        r#"
const testApi = require('@playwright/test');
Then('namespaced CJS assertion', ({ state }) => testApi.expect(state).toBe('ready'));
"#,
        r#"
import check from 'expect';
Then('default assertion import', ({ state }) => check(state).toBe('ready'));
"#,
    ];

    for source in cases {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert_eq!(
            definitions[0].handler.behavior_signature.len(),
            1,
            "{source}"
        );
        assert!(
            definitions[0].handler.behavior_signature[0].starts_with("assert:expect#toBe:"),
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
    }
}

#[test]
fn behavior_signatures_do_not_trust_expect_aliases_from_unrelated_modules() {
    let definition = extract_ts(
        r#"
import { expect as check } from 'unrelated-assertion-library';
Then('unrelated assertion alias', ({ state }) => check(state).toBe('ready'));
"#,
    )
    .remove(0);

    assert_eq!(
        definition.handler.behavior_signature,
        ["call:check#toBe", "call:check"]
    );
}

#[test]
fn runtime_expect_bindings_shadow_the_ambient_assertion_factory() {
    let cases = [
        r#"
import { expect } from 'unrelated-assertion-library';
Then('shadowed by ESM import', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
const expect = require('unrelated-assertion-library');
Then('shadowed by CJS require', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
const expect = makeAssertionFactory();
Then('shadowed by variable', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
if (enabled) { var expect = makeAssertionFactory(); }
Then('shadowed by module var hoisting', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
for (var expect of factories) { consume(expect); }
Then('shadowed by module loop var hoisting', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
if (enabled) { var require = makeLoader(); }
const { expect: check } = require('@playwright/test');
Then('shadowed module require var has no provenance', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
function expect(value) { return makeAssertion(value); }
Then('shadowed by declaration', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
Then('shadowed by parameter', (expect, state) => expect(state).toBe('ready'));
"#,
        r#"
expect = makeAssertionFactory();
Then('shadowed by assignment', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
import { expect as check } from '@playwright/test';
Then('trusted alias shadowed by parameter', (check, state) => check(state).toBe('ready'));
"#,
        r#"
import { expect as check } from '@playwright/test';
Then('trusted alias shadowed by one parameter', check => check(state).toBe('ready'));
"#,
        r#"
function helper() {
  const { expect: check } = require('@playwright/test');
  return check;
}
Then('nested trusted binding does not leak', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
function require() { return unrelatedModule(); }
const { expect: check } = require('@playwright/test');
Then('shadowed require has no provenance', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
Then('named function expression shadows ambient', function expect(state) {
  expect(state).toBe('ready');
});
"#,
        r#"
Then('shadowed by catch binding', async ({ state }) => {
  try { await work(); } catch (expect) { expect(state).toBe('ready'); }
});
"#,
        r#"
Then('shadowed by loop binding', ({ state, factories }) => {
  for (const expect of factories) { expect(state).toBe('ready'); }
});
"#,
        r#"
Then('shadowed by function var binding', ({ state, enabled }) => {
  if (enabled) { var expect = makeAssertionFactory(); }
  expect(state).toBe('ready');
});
"#,
        r#"
Then('shadowed by function loop var binding', ({ state, factories }) => {
  for (var expect of factories) { consume(expect); }
  expect(state).toBe('ready');
});
"#,
        r#"
Then('shadowed across switch cases', ({ state }) => {
  switch (state.kind) {
    case 'local': let expect = makeAssertionFactory(); break;
    case 'assert': expect(state).toBe('ready'); break;
  }
});
"#,
        r#"
({ expect } = assertionFactories);
Then('shadowed by destructuring assignment', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
enum expect { Ready }
Then('shadowed by runtime enum', ({ state }) => expect(state).toBe('ready'));
"#,
    ];

    for source in cases {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert!(
            definitions[0]
                .handler
                .behavior_signature
                .iter()
                .all(|event| !event.starts_with("assert:")),
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
    }
}

#[test]
fn typescript_runtime_and_erased_declarations_have_distinct_assertion_shadowing() {
    let cases = [
        (
            r#"
namespace expect { export const custom = true; }
Then('runtime namespace shadows ambient', ({ state }) => expect(state).toBe('ready'));
"#,
            false,
        ),
        (
            r#"
module expect { export const custom = true; }
Then('runtime module shadows ambient', ({ state }) => expect(state).toBe('ready'));
"#,
            false,
        ),
        (
            r#"
declare namespace expect { const custom: boolean; }
Then('declared namespace is erased', ({ state }) => expect(state).toBe('ready'));
"#,
            true,
        ),
        (
            r#"
declare const expect: AssertionFactory;
Then('declared constant is erased', ({ state }) => expect(state).toBe('ready'));
"#,
            true,
        ),
    ];

    for (source, expected_assertion) in cases {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert_eq!(
            definitions[0]
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.starts_with("assert:")),
            expected_assertion,
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
    }
}

#[test]
fn assertion_namespaces_named_expect_do_not_restore_the_callable_ambient() {
    for source in [
        r#"
import * as expect from '@playwright/test';
Then('ESM namespace occupies expect', ({ state }) => expect(state).toBe('ready'));
Then('ESM namespace property remains trusted', ({ state }) => expect.expect(state).toBe('ready'));
"#,
        r#"
const expect = require('@playwright/test');
Then('CJS namespace occupies expect', ({ state }) => expect(state).toBe('ready'));
Then('CJS namespace property remains trusted', ({ state }) => expect.expect(state).toBe('ready'));
"#,
    ] {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 2, "{source}");
        assert!(
            definitions[0]
                .handler
                .behavior_signature
                .iter()
                .all(|event| !event.starts_with("assert:")),
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
        assert!(
            definitions[1]
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.starts_with("assert:")),
            "{source}: {:?}",
            definitions[1].handler.behavior_signature
        );
    }
}

#[test]
fn handler_local_shadows_do_not_disable_assertions_in_other_handlers() {
    let cases = [
        r#"
const { expect: check } = require('@playwright/test');
function helper(require) { return require('local-helper'); }
Then('module CJS assertion remains trusted', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
import { expect as check } from '@playwright/test';
const helper = check => check;
Then('module ESM assertion remains trusted', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
Then('local function name is shadowed', function expect(state) {
  expect(state).toBe('ready');
});
Then('ambient assertion remains trusted elsewhere', ({ state }) => expect(state).toBe('ready'));
"#,
    ];

    for source in cases {
        let definitions = extract_ts(source);
        let trusted = definitions.last().expect("assertion step");
        assert!(
            trusted
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.starts_with("assert:")),
            "{source}: {:?}",
            trusted.handler.behavior_signature
        );
        if definitions.len() == 2 {
            assert!(definitions[0]
                .handler
                .behavior_signature
                .iter()
                .all(|event| !event.starts_with("assert:")));
        }
    }
}

#[test]
fn nested_scope_shadows_do_not_disable_outer_assertions_in_the_same_handler() {
    let definition = extract_ts(
        r#"
Then('outer assertion remains trusted', ({ state }) => {
  expect(state).toBe('ready');
  function helper(expect) { return expect(state).toBe('local'); }
});
"#,
    )
    .remove(0);

    assert_eq!(
        definition
            .handler
            .behavior_signature
            .iter()
            .filter(|event| event.starts_with("assert:"))
            .count(),
        1
    );
}

#[test]
fn property_and_lexically_resolved_assignments_preserve_assertion_provenance() {
    let cases = [
        r#"
import * as pw from '@playwright/test';
Then('namespace property assignment', ({ state }) => {
  pw.someConfig = true;
  pw.expect(state).toBe('ready');
});
"#,
        r#"
Then('bare assertion property assignment', ({ state }) => {
  expect.custom = true;
  expect(state).toBe('ready');
});
"#,
        r#"
Then('destructured assertion property assignment', ({ state, source }) => {
  ({ value: expect.custom } = source);
  expect(state).toBe('ready');
});
"#,
        r#"
import * as pw from '@playwright/test';
Then('array namespace property assignment', ({ state, source }) => {
  [pw.value] = source;
  pw.expect(state).toBe('ready');
});
"#,
        r#"
{ let expect; expect = makeAssertionFactory(); }
Then('block assignment stays local', ({ state }) => expect(state).toBe('ready'));
"#,
    ];

    for source in cases {
        let definition = extract_ts(source).remove(0);
        assert!(
            definition
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.starts_with("assert:")),
            "{source}: {:?}",
            definition.handler.behavior_signature
        );
    }

    let definition = extract_ts(
        r#"
function mutate() { expect = makeAssertionFactory(); }
Then('unresolved outer mutation is conservative', ({ state }) => expect(state).toBe('ready'));
"#,
    )
    .remove(0);
    assert!(definition
        .handler
        .behavior_signature
        .iter()
        .all(|event| !event.starts_with("assert:")));
}

#[test]
fn javascript_using_loop_bindings_shadow_ambient_expect() {
    let source = r#"
Then('using loop binding', ({ state, resources }) => {
  for (using expect of resources) { expect(state).toBe('ready'); }
});
"#;
    let definitions = extract(source, &file(SourceLanguage::JavaScript)).unwrap();
    assert_eq!(definitions.len(), 1);
    assert!(definitions[0]
        .handler
        .behavior_signature
        .iter()
        .all(|event| !event.starts_with("assert:")));
}

#[test]
fn runtime_class_names_shadow_ambient_expect_only_inside_the_class() {
    for class in [
        "class expect",
        "const Wrapper = class expect",
        "abstract class expect",
    ] {
        let source = format!(
            r#"
import {{ Then }} from 'playwright-bdd/decorators';
import {{ Given }} from '@cucumber/cucumber';
{class} {{
  @Then('class name is runtime binding')
  verify(state) {{ expect(state).toBe('ready'); }}
}}
Given('ambient remains trusted outside the class', ({{ state }}) => expect(state).toBe('ready'));
"#
        );
        let definitions = extract_ts(&source);
        assert_eq!(definitions.len(), 2, "{source}");
        assert!(
            definitions[0]
                .handler
                .behavior_signature
                .iter()
                .all(|event| !event.starts_with("assert:")),
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
        assert_eq!(
            definitions[1]
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.starts_with("assert:")),
            class.starts_with("const "),
            "{source}: {:?}",
            definitions[1].handler.behavior_signature
        );
    }
}

#[test]
fn block_and_class_static_bindings_stay_within_their_lexical_scopes() {
    let cases = [
        r#"
{ class expect {} }
Then('block class does not shadow ambient assertion', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
{ function expect(value) { return value; } }
Then('block function does not shadow ambient assertion', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
Then('class static var stays local', ({ state }) => {
  class Helper { static { var expect = makeAssertionFactory(); consume(expect); } }
  expect(state).toBe('ready');
});
"#,
    ];

    for source in cases {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert_eq!(
            definitions[0]
                .handler
                .behavior_signature
                .iter()
                .filter(|event| event.starts_with("assert:"))
                .count(),
            1,
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
    }
}

#[test]
fn method_names_do_not_shadow_assertion_bindings_inside_their_bodies() {
    let definitions = extract_ts(
        r#"
import { Then } from 'playwright-bdd/decorators';
class Assertions {
  @Then('method name is not a lexical binding')
  expect({ state }) { expect(state).toBe('ready'); }
}
"#,
    );

    assert_eq!(definitions.len(), 1);
    assert!(definitions[0]
        .handler
        .behavior_signature
        .iter()
        .any(|event| event.starts_with("assert:")));
}

#[test]
fn shadowed_expect_calls_retain_generic_behavior_comparison() {
    let definitions = extract_ts(
        r#"
import { expect } from 'unrelated-assertion-library';
Then('the local state shows the first condition', ({ state }) => expect(state).toBe('ready'));
Then('the local state shows the final condition', ({ state }) => expect(state).toBe('idle'));
"#,
    );

    assert_eq!(definitions.len(), 2);
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    assert!(definitions[0]
        .handler
        .behavior_signature
        .iter()
        .all(|event| !event.starts_with("assert:")));
}

#[test]
fn trusted_and_type_only_expect_imports_preserve_assertion_semantics() {
    for source in [
        r#"
import { expect } from '@playwright/test';
Then('trusted runtime binding', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
import type { expect } from 'unrelated-types';
Then('ambient binding remains', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
const check = require('expect');
Then('standalone CJS binding', ({ state }) => check(state).toBe('ready'));
"#,
        r#"
const { expect } = require('@playwright/test');
Then('shorthand CJS binding', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
import check = require('expect');
Then('TypeScript import require binding', ({ state }) => check(state).toBe('ready'));
"#,
    ] {
        let definitions = extract_ts(source);
        assert_eq!(definitions.len(), 1, "{source}");
        assert_eq!(
            definitions[0].handler.behavior_signature.len(),
            1,
            "{source}"
        );
        assert!(
            definitions[0].handler.behavior_signature[0].starts_with("assert:expect#toBe:"),
            "{source}: {:?}",
            definitions[0].handler.behavior_signature
        );
    }
}

#[test]
fn ambient_variable_declarations_do_not_shadow_runtime_assertion_provenance() {
    for source in [
        r#"
declare var expect: unknown;
Then('ambient expect remains available', ({ state }) => expect(state).toBe('ready'));
"#,
        r#"
declare var require: unknown;
const { expect } = require('@playwright/test');
Then('CommonJS expect remains available', ({ state }) => expect(state).toBe('ready'));
"#,
    ] {
        let definitions = extract_ts(source);

        assert_eq!(definitions.len(), 1, "{source}");
        assert!(
            definitions[0].handler.behavior_signature[0].starts_with("assert:expect#toBe:"),
            "{source}"
        );
    }
}

#[test]
fn decorated_method_metadata_does_not_inflate_executable_behavior_similarity() {
    let definitions = extract_ts(
        r#"
import { Given } from 'playwright-bdd/decorators';
import { expect as check } from '@playwright/test';
class StatusSteps {
  @Given('the account status shows the first condition')
  first({ page }) { check(page.status).toBe('ready'); }

  @Given('the account status shows the final condition')
  second({ page }) { check(page.status).toBe('idle'); }
}
"#,
    );

    assert_eq!(definitions.len(), 2);
    assert!(definitions.iter().all(|definition| {
        definition.handler.behavior_signature.len() == 2
            && definition.handler.behavior_signature[0] == "method:instance sync"
            && definition.handler.behavior_signature[1].starts_with("assert:expect#toBe:")
    }));
    assert_ne!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
}

#[test]
fn alpha_fingerprints_assign_identifiers_by_declaration_order() {
    let definitions = extract_ts(
        "Given('first', (z, a) => z.goto(a));\n\
         Given('second', (b, y) => b.goto(y));",
    );

    assert_eq!(definitions.len(), 2);
    assert_eq!(
        definitions[0].handler.alpha_normalized,
        definitions[1].handler.alpha_normalized
    );
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
}

#[test]
fn defaulted_destructured_parameters_preserve_initializer_behavior() {
    let definitions = extract_ts(
        "Given('first', ({ page: browser = makePage() } = {}) => browser.goto('/'));\n\
         Given('second', ({ page: tab = makePage() } = {}) => tab.goto('/'));",
    );
    assert_eq!(definitions.len(), 2);
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    assert_eq!(
        definitions[0].handler.behavior_signature,
        ["call:makePage", "call:v0#goto"]
    );
    assert!(definitions
        .iter()
        .all(|definition| !definition.handler.trivial));
}

#[test]
fn computed_callees_keep_the_structural_fallback() {
    let definition = extract(
        "Given('step', ({ page, method }) => page[method]());",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap()
    .remove(0);
    assert_eq!(definition.handler.behavior_signature.len(), 1);
    assert!(definition.handler.behavior_signature[0].starts_with("call:(subscript_expression"));
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
fn type_only_imports_neither_resolve_modules_nor_shadow_runtime_globals() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("broken.ts"),
        "export { Given } from '@cucumber/cucumber'; const broken = ;\n",
    )
    .unwrap();
    let path = directory.path().join("steps.ts");
    let source = r#"
import /* comment */ type { Given } from './broken';
import { type Then } from './missing';
Given('ambient runtime registration', () => work());
"#;
    fs::write(&path, source).unwrap();
    let mut session = TypeScriptExtractionSession::for_root(directory.path(), &[]);
    let extracted = extract_detailed_impl(
        source,
        &SourceFile {
            path,
            language: SourceLanguage::TypeScript,
        },
        &mut session,
    )
    .unwrap();

    assert!(extracted.diagnostics.is_empty());
    assert_eq!(extracted.definitions.len(), 1);
    assert_eq!(
        extracted.definitions[0].matcher,
        "ambient runtime registration"
    );
    assert_eq!(extracted.definitions[0].framework, Framework::Unknown);
}

#[test]
fn side_effect_framework_imports_preserve_runtime_framework_metadata() {
    for (module, expected) in [
        ("@cucumber/cucumber", Framework::CucumberJs),
        (
            "@badeball/cypress-cucumber-preprocessor",
            Framework::CypressCucumber,
        ),
    ] {
        let definitions = extract_ts(&format!(
            "import '{module}';\nGiven('side effect framework', () => work());\n"
        ));
        assert_eq!(definitions.len(), 1, "{module}");
        assert_eq!(definitions[0].framework, expected, "{module}");
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
        "Given(dynamicMatcher, () => work()); Given('wrapped', wrap(handler)); Given('object handler', { timeout: 1 }); Given('missing handler');",
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

#[test]
fn unresolved_registration_shaped_calls_are_visible_without_flagging_helpers() {
    let cases = [
        (
            "import { Given } from '@company/bdd';\nGiven('missing', () => work());",
            1,
        ),
        ("const bdd = {}; bdd.Then('missing', () => work());", 1),
        (
            "Step('missing', () => work()); And('also missing', () => work());",
            2,
        ),
        ("helper('ordinary call', () => work());", 0),
        ("load().then(() => work());", 0),
        (
            "import * as utilities from '@company/helpers'; utilities.log('ordinary call');",
            0,
        ),
    ];

    for (source, expected_calls) in cases {
        let extracted = extract_detailed(source, &file(SourceLanguage::TypeScript)).unwrap();
        assert!(extracted.definitions.is_empty(), "{source}");
        let completeness = extracted
            .diagnostics
            .iter()
            .filter(|diagnostic| crate::source_adapter::is_completeness_diagnostic(diagnostic))
            .collect::<Vec<_>>();
        if expected_calls == 0 {
            assert!(completeness.is_empty(), "{source}");
        } else {
            assert_eq!(completeness.len(), 1, "{source}");
            assert!(
                completeness[0]
                    .message
                    .contains(&format!("{expected_calls} call(s)")),
                "{source}"
            );
        }
    }
}

#[test]
fn unresolved_registration_imports_name_the_module_in_diagnostics() {
    for source in [
        "import { Given } from '@company/bdd'; Given('missing', () => work());",
        "import * as bdd from '@company/bdd'; bdd.Given('missing', () => work());",
    ] {
        let extracted = extract_detailed(source, &file(SourceLanguage::TypeScript)).unwrap();
        let diagnostics = extracted
            .diagnostics
            .iter()
            .filter(|diagnostic| crate::source_adapter::is_completeness_diagnostic(diagnostic))
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1, "{source}");
        assert!(
            diagnostics[0]
                .message
                .contains("unresolved module `@company/bdd`"),
            "{source}: {}",
            diagnostics[0].message
        );
    }
}

#[test]
fn resolved_registration_with_an_unsupported_matcher_is_not_called_unresolved() {
    let extracted = extract_detailed(
        "Given(dynamicMatcher, () => work());",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert!(extracted.definitions.is_empty());
    assert!(extracted
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("step matcher")));
    assert!(!extracted
        .diagnostics
        .iter()
        .any(crate::source_adapter::is_completeness_diagnostic));
}

#[test]
fn unresolved_calls_remain_visible_when_the_same_file_yields_definitions() {
    let extracted = extract_detailed(
        r#"
import { Given } from '@cucumber/cucumber';
import { Then } from '@company/bdd';
Given('visible step', () => visible());
Then('invisible step', () => invisible());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert_eq!(extracted.definitions.len(), 1);
    let completeness = extracted
        .diagnostics
        .iter()
        .filter(|diagnostic| crate::source_adapter::is_completeness_diagnostic(diagnostic))
        .collect::<Vec<_>>();
    assert_eq!(completeness.len(), 1);
    assert_eq!(completeness[0].location.line, 5);
    assert!(completeness[0].message.contains("1 call(s)"));
}

#[test]
fn extracts_registrations_through_transparent_wrappers_and_static_subscripts() {
    let extracted = extract_detailed(
        r#"
import { Given } from '@cucumber/cucumber';
import * as bdd from '@cucumber/cucumber';
(Given)('parenthesized', () => first());
(Given as typeof Given)('asserted', () => second());
(Given satisfies typeof Given)('satisfied', () => third());
Given!('non-null', () => fourth());
(<typeof Given>Given)('type-asserted', () => fifth());
(Given<string>)('instantiated', () => sixth());
bdd['Then']('subscripted', () => seventh());
bdd['Th\u0065n']('escaped subscript', () => eighth());
(bdd as typeof bdd).When('wrapped namespace', () => ninth());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert_eq!(extracted.definitions.len(), 9);
    assert_eq!(extracted.definitions[0].matcher, "parenthesized");
    assert_eq!(extracted.definitions[1].matcher, "asserted");
    assert_eq!(extracted.definitions[2].matcher, "satisfied");
    assert_eq!(extracted.definitions[3].matcher, "non-null");
    assert_eq!(extracted.definitions[4].matcher, "type-asserted");
    assert_eq!(extracted.definitions[5].matcher, "instantiated");
    assert_eq!(extracted.definitions[6].registration, "Then");
    assert_eq!(extracted.definitions[7].matcher, "escaped subscript");
    assert_eq!(extracted.definitions[8].matcher, "wrapped namespace");
    assert!(!extracted
        .diagnostics
        .iter()
        .any(crate::source_adapter::is_completeness_diagnostic));
}

#[test]
fn static_subscript_registrations_are_included_in_completeness_diagnostics() {
    let extracted = extract_detailed(
        r#"
import { Given } from '@cucumber/cucumber';
import * as custom from '@company/bdd';
Given('visible step', () => visible());
custom['Then']('invisible step', () => invisible());
"#,
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert_eq!(extracted.definitions.len(), 1);
    let completeness = extracted
        .diagnostics
        .iter()
        .filter(|diagnostic| crate::source_adapter::is_completeness_diagnostic(diagnostic))
        .collect::<Vec<_>>();
    assert_eq!(completeness.len(), 1);
    assert_eq!(completeness[0].location.line, 5);
}

#[test]
fn computed_subscripts_and_lowercase_then_calls_are_not_registration_evidence() {
    let extracted = extract_detailed(
        "const property = 'Then'; custom[property]('dynamic', handler); promise['then'](handler);",
        &file(SourceLanguage::TypeScript),
    )
    .unwrap();

    assert!(extracted.definitions.is_empty());
    assert!(!extracted
        .diagnostics
        .iter()
        .any(crate::source_adapter::is_completeness_diagnostic));
}

#[test]
fn deeply_parenthesized_registration_callees_are_unwrapped_iteratively() {
    let depth = 1_000;
    let source = format!(
        "{}Given{}('deep', () => work());",
        "(".repeat(depth),
        ")".repeat(depth)
    );
    let extracted = extract_detailed(&source, &file(SourceLanguage::TypeScript)).unwrap();

    assert_eq!(extracted.definitions.len(), 1);
    assert_eq!(extracted.definitions[0].matcher, "deep");
}

#[test]
fn positional_registration_wrappers_are_inferred_but_reordering_ones_are_not() {
    let inferred = extract_ts(
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { Given(text, handler); }
step("wrapped step", () => work());"#,
    );
    assert_eq!(inferred.len(), 1);
    assert_eq!(inferred[0].matcher, "wrapped step");
    // A wrapper resolves to the registration it forwards to, so the step is a Given.
    assert_eq!(inferred[0].registration, "Given");

    let returned = extract_ts(
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { return Given(text, handler); }
step("returned step", () => work());"#,
    );
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].matcher, "returned step");

    // Every shape below breaks the positional correspondence the rule depends on, so extracting
    // argument 0 as the matcher would attribute the wrong text to the wrong handler.
    for source in [
        // Reordered forward.
        r#"import { Given } from "@cucumber/cucumber";
function step(handler, text) { Given(text, handler); }
step(() => work(), "wrapped step");"#,
        // Rewritten matcher.
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { Given("prefix " + text, handler); }
step("wrapped step", () => work());"#,
        // Destructured parameters carry no positional guarantee.
        r#"import { Given } from "@cucumber/cucumber";
function step({ text }, handler) { Given(text, handler); }
step({ text: "wrapped step" }, () => work());"#,
        // A local helper that never reaches a registration is not a wrapper.
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { return [text, handler]; }
step("wrapped step", () => work());"#,
        // A returned callback defers registration until that callback is invoked.
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { return () => Given(text, handler); }
step("wrapped step", () => work());"#,
        // A conditional call is not guaranteed to register when the wrapper is invoked.
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { if (enabled) Given(text, handler); }
step("wrapped step", () => work());"#,
        // Multiple statements require data-flow reasoning and use the explicit config escape hatch.
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { trace(text); Given(text, handler); }
step("wrapped step", () => work());"#,
        // Async bodies can suspend before reaching the registration call.
        r#"import { Given } from "@cucumber/cucumber";
async function step(text, handler) { Given(text, handler); }
step("wrapped step", () => work());"#,
        // Generator bodies do not run merely because the generator function is called.
        r#"import { Given } from "@cucumber/cucumber";
function* step(text, handler) { Given(text, handler); }
step("wrapped step", () => work());"#,
        // A nested declaration must not escape its lexical scope into a file-wide alias.
        r#"import { Given } from "@cucumber/cucumber";
function outer() { function step(text, handler) { Given(text, handler); } }
step("wrapped step", () => work());"#,
    ] {
        assert!(
            extract_ts(source).is_empty(),
            "must not infer a wrapper from: {source}"
        );
    }
}

#[test]
fn wrapper_inference_follows_chains_without_looping_on_self_reference() {
    let chained = extract_ts(
        r#"import { Given } from "@cucumber/cucumber";
function inner(text, handler) { Given(text, handler); }
function outer(text, handler) { inner(text, handler); }
outer("chained step", () => work());"#,
    );
    assert_eq!(chained.len(), 1);
    assert_eq!(chained[0].matcher, "chained step");

    // A self-recursive helper never reaches a registration, so it must not resolve to itself.
    assert!(extract_ts(
        r#"import { Given } from "@cucumber/cucumber";
function step(text, handler) { step(text, handler); }
step("looping step", () => work());"#,
    )
    .is_empty());
}

#[test]
fn wrapper_inference_resolves_long_chains_without_quadratic_work() {
    // Wrappers form a forward graph. Walking it from the known registrations visits each wrapper
    // once, so a chain declared in reverse order — the worst ordering for a naive fixpoint —
    // still resolves in linear time.
    let chain = |depth: usize| {
        let mut source = String::from("import { Given } from \"@cucumber/cucumber\";\n");
        for index in (1..depth).rev() {
            source.push_str(&format!(
                "function w{index}(text, handler) {{ w{}(text, handler); }}\n",
                index - 1
            ));
        }
        source.push_str("function w0(text, handler) { Given(text, handler); }\n");
        source.push_str(&format!("w{}(\"chained\", () => work());\n", depth - 1));
        source
    };

    assert_eq!(extract_ts(&chain(200)).len(), 1);
    assert_eq!(extract_ts(&chain(4000)).len(), 1);
}

#[test]
fn registration_discovery_covers_static_alias_and_shadowing_forms() {
    let source = r#"
import type { Given as TypeGiven } from '@cucumber/cucumber';
import /* comment before the modifier */ type { Given as CommentTypeGiven } from '@cucumber/cucumber';
import { type Then as SpecifierTypeThen } from '@cucumber/cucumber';
import { Given as ImportedGiven } from '@cucumber/cucumber';
export /* comment before the modifier */ type { When as ExportedTypeWhen } from '@cucumber/cucumber';
export { type Then as ExportedSpecifierTypeThen } from '@cucumber/cucumber';
const cucumber = require('@cucumber/cucumber');
const { When: RequiredWhen, Then } = require('@cucumber/cucumber');
const { Given: NamespaceGiven } = cucumber;
const AliasGiven = ImportedGiven;
let AssignedGiven;
AssignedGiven = AliasGiven;

export function wrapped(text: string, handler: () => void) {
  return (((ImportedGiven)))(text, handler);
}

ImportedGiven('imported', () => work());
cucumber.Then('namespace', () => work());
cucumber['When']('subscript', () => work());
RequiredWhen('required', () => work());
Then('destructured', () => work());
NamespaceGiven('namespace destructured', () => work());
AliasGiven('alias', () => work());
AssignedGiven('assigned', () => work());
wrapped('wrapped', () => work());
TypeGiven('type only', () => work());
CommentTypeGiven('commented type only', () => work());
SpecifierTypeThen('specifier type only', () => work());
ExportedTypeWhen('exported type only', () => work());
ExportedSpecifierTypeThen('exported specifier type only', () => work());
"#;
    let definitions = extract_ts(source);
    assert_eq!(definitions.len(), 9);
    assert!(definitions
        .iter()
        .all(|definition| definition.framework == Framework::CucumberJs));
    for matcher in [
        "type only",
        "commented type only",
        "specifier type only",
        "exported type only",
        "exported specifier type only",
    ] {
        assert!(!definitions
            .iter()
            .any(|definition| definition.matcher == matcher));
    }

    let untrusted_create_bdd = extract_ts(
        "const factory = createBdd(); const { Given } = factory; Given('not bdd', () => work());",
    );
    assert!(untrusted_create_bdd.is_empty());
}

#[test]
fn configured_registration_names_override_inference_without_duplicating_known_aliases() {
    let configured = vec!["step".to_owned(), "Given".to_owned()];
    let mut session = TypeScriptExtractionSession::for_root(std::path::Path::new("."), &configured);
    let source = "function step(text, handler) { dynamic(text, handler); }\nstep('configured', () => work());";
    let extracted =
        extract_detailed_impl(source, &file(SourceLanguage::TypeScript), &mut session).unwrap();
    assert_eq!(extracted.definitions.len(), 1);
    assert_eq!(extracted.definitions[0].registration, "step");
}
