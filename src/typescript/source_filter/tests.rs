use super::*;
use crate::typescript::frameworks::{DEFAULT_REGISTRATIONS, REGISTRATIONS};

/// Length of a line that unambiguously clears [`MINIFIED_MEAN_LINE_BYTES`].
///
/// Only the ratio to the threshold matters, so this stays kilobytes rather than the megabytes
/// a real bundle reaches: `contains_call_to` scans every window, and megabyte fixtures made
/// this module the slowest in the suite for no added coverage.
const LONG_LINE_BYTES: usize = 4_000;
use std::fs;
use std::io::Write;

/// Authored source at the widest geometry observed in the reference repositories.
fn authored_source() -> Vec<u8> {
    let mut source = b"import { Given } from '@cucumber/cucumber';\n\n".to_vec();
    for index in 0..200 {
        source.extend_from_slice(
            format!("Given('step number {index}', async function () {{\n  await work();\n}});\n\n")
                .as_bytes(),
        );
    }
    source
}

/// Minified output collapsed onto a single line, with no terminator at all.
fn one_line_minified() -> Vec<u8> {
    format!("!function(n){{{}}}(0);", "var a=1,b=2,c=3;".repeat(500)).into_bytes()
}

/// Minified output wrapped at a fixed width.
///
/// This is the shape a longest-line test cannot see: no single line is unusual, yet every
/// line is far wider than authored code. `highcharts.js` in the reference corpus is exactly
/// this, at 601 lines whose longest is only 637 bytes.
fn wrapped_minified() -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..40 {
        bytes.extend_from_slice("var a=1,b=2,c=3;".repeat(32).as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

#[test]
fn classifies_every_content_kind_discovery_can_deliver() {
    // Padded with ten blank lines per content line, so the verdict flips if blank lines are
    // counted: skipping them the mean stays above 500, counting them it falls below 50.
    let mut blank_padded = wrapped_minified();
    blank_padded.extend_from_slice(&vec![b'\n'; 400]);
    let mut crlf_minified = Vec::new();
    for line in wrapped_minified().split(|byte| *byte == b'\n') {
        crlf_minified.extend_from_slice(line);
        crlf_minified.extend_from_slice(b"\r\n");
    }
    let mut gzip = vec![0x1f, 0x8b, 0x08];
    gzip.extend_from_slice(b"compressed payload that is otherwise textual");
    // A single long literal that does not dominate the file: the realistic shape of an
    // inlined query or fixture inside authored code, which must survive.
    let mut long_literal = authored_source();
    long_literal.extend_from_slice(format!("const BLOB = '{}';\n", "x".repeat(5_000)).as_bytes());
    // Multibyte text: line length is counted in bytes, so wide characters must not push an
    // authored file over the threshold on their own.
    let mut multibyte = Vec::new();
    for _ in 0..50 {
        multibyte.extend_from_slice("// 日本語のコメントがここに書かれています\n".as_bytes());
    }

    for (label, bytes, expected) in [
        ("authored source", authored_source(), None),
        ("empty file", Vec::new(), None),
        ("long literal in authored source", long_literal, None),
        ("multibyte authored source", multibyte, None),
        (
            "one-line minified",
            one_line_minified(),
            Some(ExcludedSource::Minified),
        ),
        (
            "wrapped minified",
            wrapped_minified(),
            Some(ExcludedSource::Minified),
        ),
        (
            "minified padded with blank lines",
            blank_padded,
            Some(ExcludedSource::Minified),
        ),
        (
            "minified with CRLF endings",
            crlf_minified,
            Some(ExcludedSource::Minified),
        ),
        ("gzip container", gzip, Some(ExcludedSource::Compressed)),
        (
            "NUL bytes",
            b"var a = 1;\0\0\0binary\0".to_vec(),
            Some(ExcludedSource::Binary),
        ),
        // Invalid UTF-8 without a NUL stays unclassified on purpose. A mis-encoded but
        // authored file can hold definitions, so extraction must fail loudly on it rather
        // than have it disappear as an exclusion.
        (
            "invalid UTF-8 without NUL",
            b"const a = '\xff\xfe';\n".to_vec(),
            None,
        ),
    ] {
        assert_eq!(classify(&bytes, &[]), expected, "{label}");
    }
}

#[test]
fn a_line_wider_than_the_whole_prefix_is_minified_whatever_precedes_it() {
    // The shape that defeats measuring only whole lines: a short license comment, then one
    // line holding the entire program. The prefix stops inside that line, so the only
    // terminator in it belongs to the comment. Measuring just the completed comment reads as
    // ordinary source and lets the file through, which is how `jquery-3.5.1.min.js` reached
    // the parser and cost over 130 seconds.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vendor.min.js");
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(b"/*! library v1.2.3 | (c) authors | license MIT */\n")
        .unwrap();
    file.write_all(&one_line_minified()).unwrap();
    file.write_all(&vec![b'z'; SOURCE_CLASSIFICATION_PREFIX_BYTES])
        .unwrap();
    drop(file);

    assert_eq!(inspect(&path, &[]), Some(ExcludedSource::Minified));
}

#[test]
fn authored_sources_longer_than_the_prefix_are_still_analyzed() {
    // The opposite-answer control for the case above: same truncated-prefix path, same
    // absence of a final terminator, but authored geometry. Without it, a rule that excluded
    // every truncated prefix would pass the test above and look correct.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("steps.ts");
    let mut bytes = authored_source();
    while bytes.len() <= SOURCE_CLASSIFICATION_PREFIX_BYTES {
        bytes.extend_from_slice(&authored_source());
    }
    bytes.extend_from_slice(b"Given('unterminated last line', () => work());");
    fs::write(&path, &bytes).unwrap();

    assert_eq!(inspect(&path, &[]), None);
}

#[test]
fn an_unreadable_path_is_left_for_extraction_to_report() {
    assert_eq!(inspect(Path::new("/nonexistent/vendor.js"), &[]), None);
}

#[test]
fn the_guard_vocabulary_covers_every_registration_the_analyzer_understands() {
    // The guard decides only whether to keep a file, so it must never know fewer names than
    // the analyzer resolves. A name missing here would let a file holding real definitions be
    // excluded on geometry alone.
    let guarded = QUALIFIABLE_REGISTRATIONS
        .iter()
        .chain(BARE_ONLY_REGISTRATIONS.iter());
    let guarded = guarded.collect::<Vec<_>>();
    for name in REGISTRATIONS.iter().chain(DEFAULT_REGISTRATIONS.iter()) {
        assert!(
            guarded.contains(&name),
            "registration `{name}` is not guarded"
        );
    }
}

#[test]
fn authored_code_that_is_one_enormous_line_survives_on_its_registrations() {
    // Geometrically indistinguishable from a one-line bundle: a single line, megabytes wide.
    // Only the registration call separates them, so this is the case that decides whether the
    // guard exists at all.
    let authored = format!(
        "Given('{}', () => work());",
        str::repeat("a", LONG_LINE_BYTES)
    );
    assert_eq!(classify(authored.as_bytes(), &[]), None);

    // The opposite-answer control: identical geometry, no registration, so it is excluded.
    let generated = format!("!function(){{{}}}();", str::repeat("a", LONG_LINE_BYTES));
    assert_eq!(
        classify(generated.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn a_configured_registration_name_also_guards_a_file() {
    let source = format!(
        "myStep('{}', () => work());",
        str::repeat("a", LONG_LINE_BYTES)
    );
    assert_eq!(
        classify(source.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
    assert_eq!(classify(source.as_bytes(), &["myStep".to_owned()]), None);
}

#[test]
fn a_registration_name_inside_a_longer_identifier_does_not_guard_a_bundle() {
    // `promisedPatchThen(` and `splitWhen(` both occur in the reference bundles. Reading them
    // as registrations would keep every bundle and defeat the exclusion.
    for callee in ["promisedPatchThen", "splitWhen", "myGiven", "define$Step"] {
        let bundle = format!(
            "!function(){{{}({})}}();",
            callee,
            "a,".repeat(LONG_LINE_BYTES / 2)
        );
        assert_eq!(
            classify(bundle.as_bytes(), &[]),
            Some(ExcludedSource::Minified),
            "{callee}"
        );
    }
}

#[test]
fn a_namespace_qualified_registration_guards_a_file_but_a_promise_chain_does_not() {
    // Both are a capitalized or lowercase name behind a `.`, and only the case distinguishes
    // them. `cucumber.Given(...)` is a registration reached through a namespace import, while
    // `.then(...)` is a promise continuation that appears 66 times in one reference bundle —
    // so honouring a qualifier for the lowercase aliases would keep nearly every bundle.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    let qualified = format!("{filler}cucumber.Given('a step', () => work());{filler}");
    assert_eq!(classify(qualified.as_bytes(), &[]), None);

    let chained = format!("{filler}p.then(function(){{return 1}});{filler}");
    assert_eq!(
        classify(chained.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn a_bare_lowercase_alias_guards_a_file() {
    // `then('a step', fn)` is a registration the analyzer resolves, so geometry must not
    // discard it. The control above pins that the qualified form still does not guard.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    let aliased = format!("{filler}then('a step', () => work());{filler}");
    assert_eq!(classify(aliased.as_bytes(), &[]), None);
}

#[test]
fn a_renaming_import_guards_a_file_through_its_module_path() {
    // The one shape that leaves no registration name at a call site: the import renames it,
    // so the file calls `G(...)`. Only the module path identifies it as a step source.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    let renamed = format!(
        "import {{ Given as G }} from '@cucumber/cucumber';{filler}G('a step', () => work());{filler}"
    );
    assert_eq!(classify(renamed.as_bytes(), &[]), None);

    // The opposite-answer control: same geometry and the same lone-identifier call, but no
    // module path and no registration name, so it is excluded.
    let anonymous = format!("{filler}G('a step', () => work());{filler}");
    assert_eq!(
        classify(anonymous.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn a_project_facade_import_guards_a_file_whatever_the_callee_form() {
    // Extraction resolves a registration re-exported by a local module, so the callee can be
    // any name and any supported syntax. Rather than enumerate those forms, the relative
    // import is taken as evidence that the file is an authored module.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for importer in [
        "import { G } from './facade.js';",
        "import { G } from '../steps/facade';",
        "export { G } from \"./facade\";",
        "const { G } = require('./facade');",
        "const { G } = await import('./facade');",
        "import './facade';",
        // Trivia between the keyword and the specifier, in both comment forms.
        "import { G } from /* facade */ './facade';",
        "import { G } from // facade\n './facade';",
        "export { G } from './facade';",
        // Project aliases the resolver follows: `tsconfig` paths and package `imports`.
        "import { Given as Action } from '~/facade';",
        "import { Given as Action } from '@/steps/facade';",
        "import { Given as Action } from '#steps/facade';",
    ] {
        let source = format!("{importer}{filler}G<string>('a step', () => work());{filler}");
        assert_eq!(classify(source.as_bytes(), &[]), None, "{importer}");
    }
}

#[test]
fn a_project_specifier_outside_import_position_does_not_guard_a_file() {
    // The opposite-answer control, and the reason the import context is required: one
    // reference bundle contains 80 occurrences of `"./` inside its string data, and treating
    // those as imports would keep the bundle and defeat the exclusion.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for bundled in [
        "var p = \"./assets/icon.png\";",
        "t.resolve(\"./chunk\");",
        "var base = '../';",
        // A bare package name is not evidence: two reference bundles import packages, and
        // jQuery contains `from '` inside an error message.
        "import x from 'lodash';",
        "var m = \"No conversion from \" + u;",
    ] {
        let source = format!("{filler}{bundled}{filler}");
        assert_eq!(
            classify(source.as_bytes(), &[]),
            Some(ExcludedSource::Minified),
            "{bundled}"
        );
    }
}

#[test]
fn every_registered_framework_module_guards_a_file() {
    // Driven from the framework registry so a newly supported framework cannot be left out.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for module in crate::typescript::frameworks::registration_modules() {
        let source = format!("import {{ X }} from '{module}';{filler}X('s', () => work());");
        assert_eq!(classify(source.as_bytes(), &[]), None, "{module}");
    }
}

#[test]
fn comment_trivia_between_the_name_and_the_call_still_guards_a_file() {
    // `Given /* ... */ ('a step', handler)` is a call, so formatting must not decide whether
    // an authored file is excluded.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for spaced in [
        "Given/* matcher below */('a step', () => work());",
        "Given // matcher below\n('a step', () => work());",
        "Given\n  ('a step', () => work());",
    ] {
        let source = format!("{filler}{spaced}{filler}");
        assert_eq!(classify(source.as_bytes(), &[]), None, "{spaced}");
    }

    // The opposite-answer control: a name that never reaches a call is not evidence, or every
    // bundle mentioning `Step` in passing would be kept.
    let mentioned = format!("{filler}var Given = 1, When = 2, Step = 3;{filler}");
    assert_eq!(
        classify(mentioned.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn every_transparent_callee_wrapper_still_guards_a_file() {
    // `(Given)('a step', handler)` extracts two definitions in the CLI, so geometry must not
    // discard the same form. The name is followed by `)` rather than `(`.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for wrapped in [
        "(Given)('a step', () => work());",
        "((Given))('a step', () => work());",
        "(Given) /* wrapped */ ('a step', () => work());",
        // The transparent callee wrappers `unwrap_registration_callee` accepts.
        "Given!('a step', () => work());",
        "Given<string>('a step', () => work());",
        "Given<Map<string, number>>('a step', () => work());",
        "(Given as typeof Given)('a step', () => work());",
        "(Given satisfies Registration)('a step', () => work());",
        "(Given!)('a step', () => work());",
        // Casts whose type contains the very bytes a call is recognized by: braces, nested
        // parentheses, colons and commas.
        "(Given as { (pattern: string, fn: () => void): void })('a step', () => work());",
        "(Given as (pattern: string) => void)('a step', () => work());",
        "(Given as Record<string, () => void>)('a step', () => work());",
        // An arrow before a comma: if the `>` of `=>` were counted as closing the type
        // argument list, the following comma would read as the end of the cast.
        "(Given as Map<() => void, string>)('a step', () => work());",
        // A bare instantiation whose type argument contains an arrow. The `>` of `=>` is
        // punctuation, so pairing it with the `<` would end the type list early.
        "Given<() => void>('a step', () => work());",
        "Given<Array<() => void>>('a step', () => work());",
        // Object types inside a type argument list: the braces belong to the type, not to a
        // statement, and a `;` between members is a separator rather than a boundary.
        "Given<{ value: string }>('a step', () => work());",
        "Given<{ value: { nested: string } }>('a step', () => work());",
        "Given<{ value: string; other: number }>('a step', () => work());",
        "Given<Record<string, { value: string }>>('a step', () => work());",
    ] {
        let source = format!("{filler}{wrapped}{filler}");
        assert_eq!(classify(source.as_bytes(), &[]), None, "{wrapped}");
    }

    // The opposite-answer control: closing parentheses are trivia only on the way to a call,
    // so a name that merely appears inside one is not evidence.
    // An unbalanced `<` is a comparison, not a type argument list. The `>` and `(` further on
    // belong to later statements, so a scan that ran past the statement boundary would pair
    // them with this `<` and read a call that is not there.
    let compared = format!("{filler}if(Given<b){{c()}};var d=e>(f);{filler}");
    assert_eq!(
        classify(compared.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );

    let argument = format!("{filler}register(Given);{filler}");
    assert_eq!(
        classify(argument.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn a_bzip2_header_needs_its_whole_signature() {
    // `BZh`, `BZh0` and `BZh9` are all legal JavaScript identifiers, so an authored file may
    // open with any of them. Only the full stream signature separates an archive from one.
    assert_eq!(
        classify(b"BZh91AY&SY\x00payload", &[]),
        Some(ExcludedSource::Compressed)
    );
    for authored in [
        "BZh = 1;\nGiven('a step', () => work());\n",
        "BZh0 = 1;\nGiven('a step', () => work());\n",
        "BZh9 = 1;\nGiven('a step', () => work());\n",
        "BZh91AYNOT = 1;\nGiven('a step', () => work());\n",
        // Block sizes run 1 to 9, so `BZh0` is not a stream header however it continues.
        "BZh01AY&SY = 1;\nGiven('a step', () => work());\n",
    ] {
        assert_eq!(classify(authored.as_bytes(), &[]), None, "{authored}");
    }
}

#[test]
fn ecmascript_whitespace_beyond_ascii_still_guards_a_file() {
    // NBSP, the Unicode space separators, the line and paragraph separators and the byte-order
    // mark are all legal between a callee and its arguments, so none of them may hide a call.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for space in [
        '\u{00a0}', '\u{2003}', '\u{2028}', '\u{2029}', '\u{3000}', '\u{feff}',
    ] {
        let source = format!("{filler}Given{space}('a step', () => work());{filler}");
        assert_eq!(
            classify(source.as_bytes(), &[]),
            None,
            "U+{:04X}",
            space as u32
        );
    }

    // The opposite-answer control: non-ASCII bytes that are not whitespace do not manufacture
    // a call, so a bundle carrying non-ASCII text is still excluded.
    let translated = format!("{filler}var 日本語={{}};var t=\"ステップ\";{filler}");
    assert_eq!(
        classify(translated.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn each_exclusion_reports_its_own_word() {
    // The word reaches the diagnostic, so each variant needs to round-trip.
    for (bytes, expected, word) in [
        (
            b"BZh91AY&SY\x00payload".to_vec(),
            ExcludedSource::Compressed,
            "compressed",
        ),
        (b"var a = 1;\0".to_vec(), ExcludedSource::Binary, "binary"),
        (
            "var a=1,b=2,c=3;".repeat(500).into_bytes(),
            ExcludedSource::Minified,
            "minified",
        ),
    ] {
        assert_eq!(classify(&bytes, &[]), Some(expected));
        assert_eq!(expected.as_str(), word);
    }
}

#[test]
fn an_import_keyword_reached_through_a_member_access_is_not_an_import() {
    // `t.from('./x')` is a method call on some object, not a module import, so it must not
    // stand in for the provenance evidence a real import provides.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for member in [
        "t.from('./chunk')",
        "t.require('./chunk')",
        "t.import('./chunk')",
    ] {
        let source = format!("{filler}{member};{filler}");
        assert_eq!(
            classify(source.as_bytes(), &[]),
            Some(ExcludedSource::Minified),
            "{member}"
        );
    }
}

#[test]
fn an_unterminated_comment_after_an_import_keyword_is_not_an_import() {
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for broken in ["import(/* never closed", "from /* never closed"] {
        let source = format!("{filler}{broken}{filler}");
        assert_eq!(
            classify(source.as_bytes(), &[]),
            Some(ExcludedSource::Minified),
            "{broken}"
        );
    }
}

#[test]
fn a_type_operator_that_runs_to_the_end_of_the_prefix_is_not_a_call() {
    // The type never closes within what was inspected, so no call can be proven from it.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    let source = format!(
        "{filler}Given as SomeVeryLongTypeName{}",
        "Alias".repeat(200)
    );
    assert_eq!(
        classify(source.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn a_value_block_inside_a_comparison_chain_does_not_guard_a_file() {
    // `Given < class { field; } > ('x')` is a comparison chain, not a registration: the
    // braces are a class body, so the `;` in them ends a statement and abandons the `<`.
    // Object-type braces are exempt from that only because their members carry a `:`.
    let filler = "var a=1,b=2,c=3;".repeat(500);
    for chain in [
        "var r = Given < class { field; } > ('x');",
        "var r = Given < class Named { field; } > ('x');",
        "var r = Given < function () { work(); } > ('x');",
        "var r = Given < class { first; second; } > ('x');",
    ] {
        let source = format!("{filler}{chain}{filler}");
        assert_eq!(
            classify(source.as_bytes(), &[]),
            Some(ExcludedSource::Minified),
            "{chain}"
        );
    }
}

#[test]
fn an_unterminated_comment_does_not_run_past_the_prefix() {
    let filler = "var a=1,b=2,c=3;".repeat(500);
    let source = format!("{filler}Given/* never closed{filler}");
    assert_eq!(
        classify(source.as_bytes(), &[]),
        Some(ExcludedSource::Minified)
    );
}

#[test]
fn repeated_unbalanced_type_arguments_do_not_scale_quadratically() {
    // Scanning forward from each `<` separately makes this input quadratic: every `Given<`
    // starts an unbalanced scan that runs to the end of the prefix. A valid type argument
    // list has no length limit, so the fix has to be a single pass rather than a cap.
    let prefix = "Given<"
        .repeat(SOURCE_CLASSIFICATION_PREFIX_BYTES / 6)
        .into_bytes();

    let started = std::time::Instant::now();
    let verdict = classify(&prefix, &[]);
    let elapsed = started.elapsed();

    assert_eq!(verdict, Some(ExcludedSource::Minified));
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "classification took {elapsed:?} on {} bytes of unbalanced type arguments",
        prefix.len()
    );
}

#[test]
fn a_type_argument_list_longer_than_any_cap_still_guards_a_file() {
    // The control for the case above: a cap on the scan would stop recognizing this call and
    // exclude an authored file, so length alone must not decide.
    let union = (0..400)
        .map(|index| format!("Type{index}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let source = format!("Given<{union}>('a step', () => work());");
    assert!(
        source.len() > 2_048,
        "the type list must exceed any plausible cap"
    );
    assert_eq!(classify(source.as_bytes(), &[]), None);
}

#[test]
fn a_large_registration_configuration_does_not_multiply_classification_cost() {
    // `registrations` is configuration with no small bound, so searching the prefix once per
    // configured name made one file cost seconds. The prefix is scanned once instead, and the
    // bound below is far above that but far below the per-name cost it replaced.
    let prefix = "var a=1,b=2,c=3;"
        .repeat(SOURCE_CLASSIFICATION_PREFIX_BYTES / 16)
        .into_bytes();
    let names = (0..20_000)
        .map(|index| format!("absentRegistration{index}"))
        .collect::<Vec<_>>();

    let started = std::time::Instant::now();
    let verdict = classify(&prefix, &names);
    let elapsed = started.elapsed();

    assert_eq!(verdict, Some(ExcludedSource::Minified));
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "classification took {elapsed:?} with {} configured names",
        names.len()
    );
}

#[test]
fn binary_and_compressed_content_is_excluded_even_when_it_mentions_a_registration() {
    // The guard is a concession to a heuristic. Binary and compressed content are not
    // heuristics, so a registration name appearing in their bytes must not rescue them.
    let mut binary = b"Given('step', () => work());".to_vec();
    binary.push(0);
    assert_eq!(classify(&binary, &[]), Some(ExcludedSource::Binary));

    let mut compressed = vec![0x1f, 0x8b, 0x08];
    compressed.extend_from_slice(b"Given('step', () => work());");
    assert_eq!(classify(&compressed, &[]), Some(ExcludedSource::Compressed));
}
