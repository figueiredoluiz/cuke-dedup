use super::*;

const KEYS: [RegexKey; 2] = [RegexKey::CanonicalSource, RegexKey::TranslatedBody];
const JS: IdentityFlags = IdentityFlags::Listed("imsuv");

/// A translation whose body is `body` and whose inline flags are `flags`.
fn translated(body: &str, flags: &str) -> TranslatedRegex {
    TranslatedRegex {
        inline_flags: flags.to_owned(),
        body: body.to_owned(),
    }
}

/// Regex matchers: the canonical key folds the pattern and carries the identity flags in its
/// text; the translated key keeps the body as written and no flags. Each row's opposite-answer
/// control is the other policy's column.
#[test]
fn regex_key_text_follows_the_declared_policy() {
    // (pattern, flags, translation, canonical key, translated key)
    let rows: [(&str, &str, Option<TranslatedRegex>, &str, &str); 8] = [
        (
            r"(?<name>a)\s+b",
            "mi",
            Some(translated(r"(?<name>a)\s+b", "mi")),
            "[regex-flags:im] (a) b",
            r"(?<name>a)\s+b",
        ),
        // Execution-only flags never reach the key text.
        (
            "^VALUE$",
            "ygusm",
            None,
            "[regex-flags:msu] ^VALUE$",
            "^VALUE$",
        ),
        // The canonical key reads the source; the translated key reads the body.
        (
            "source",
            "",
            Some(translated("body", "")),
            "[regex-flags:] source",
            "body",
        ),
        // Compatibility forms fold to one spelling before comparison.
        ("ＡＢ  c", "", None, "[regex-flags:] AB c", "ＡＢ  c"),
        // An empty pattern still yields a key.
        ("", "", None, "[regex-flags:] ", ""),
        // `v` has no Rust translation; the source pattern is keyed and `v` stays in the key.
        ("x", "v", None, "[regex-flags:v] x", "x"),
        // A group-like prefix of a body is not a wrapper.
        (
            "(?i:a)",
            "",
            Some(translated("(?i:a)", "")),
            "[regex-flags:] (?i:a)",
            "(?i:a)",
        ),
        // Whitespace folding does not reach inside character classes.
        (r"[\s+]", "", None, r"[regex-flags:] [\s+]", r"[\s+]"),
    ];
    for (pattern, flags, translation, canonical, translated_key) in rows {
        let key = |policy| {
            normalized_matcher(
                MatcherKind::RegularExpression,
                pattern,
                flags,
                translation.as_ref(),
                policy,
                JS,
            )
        };
        assert_eq!(key(RegexKey::CanonicalSource), canonical, "{pattern:?}");
        assert_eq!(key(RegexKey::TranslatedBody), translated_key, "{pattern:?}");
    }
}

/// Cucumber expressions and literals ignore the regex policy and the flags.
#[test]
fn non_regex_kinds_ignore_policy_and_flags() {
    for key in KEYS {
        for flags in ["", "i", "mi"] {
            assert_eq!(
                normalized_matcher(
                    MatcherKind::CucumberExpression,
                    "I have { int }  items",
                    flags,
                    None,
                    key,
                    JS
                ),
                "I have {int} items"
            );
            assert_eq!(
                normalized_matcher(MatcherKind::Literal, "a  { b }", flags, None, key, JS),
                "a  { b }"
            );
        }
    }
}

/// Fail closed: a regex the adapter could not translate is keyed on its own source, so two
/// different untranslatable patterns never share a key, under either policy.
#[test]
fn untranslatable_patterns_keep_distinct_keys() {
    for key in KEYS {
        let of = |pattern: &str| {
            normalized_matcher(MatcherKind::RegularExpression, pattern, "v", None, key, JS)
        };
        assert_ne!(of("[a&&b]"), of("[a&&c]"), "{key:?}");
        assert_eq!(of("[a&&b]"), of("[a&&b]"), "{key:?}");
    }
}

/// A listed identity keeps only its flags; the full identity keeps every flag. Both sort and
/// deduplicate. Each row's opposite-answer control is the other column: `n`, `x`, `o` and the
/// JavaScript execution flags count only under the full identity.
#[test]
fn flag_identity_keeps_the_declared_flags_only() {
    for (raw, listed, all) in [
        ("uugmisy", "imsu", "gimsuy"),
        ("vi", "iv", "iv"),
        ("", "", ""),
        ("gyd", "", "dgy"),
        ("nxo", "", "nox"),
        ("ms", "ms", "ms"),
        ("mii", "im", "im"),
    ] {
        assert_eq!(FlagIdentity::new(raw, JS).as_str(), listed, "{raw:?}");
        assert_eq!(
            FlagIdentity::new(raw, IdentityFlags::All).as_str(),
            all,
            "{raw:?}"
        );
    }
    let all = |flags| FlagIdentity::new(flags, IdentityFlags::All);
    assert_eq!(FlagIdentity::new("iv", JS), FlagIdentity::new("vi", JS));
    assert_ne!(FlagIdentity::new("v", JS), FlagIdentity::new("u", JS));
    assert_ne!(FlagIdentity::new("i", JS), FlagIdentity::new("", JS));
    assert_eq!(FlagIdentity::new("g", JS), FlagIdentity::new("", JS));
    assert_eq!(FlagIdentity::new("n", JS), FlagIdentity::new("", JS));
    assert_ne!(all("n"), all(""));
    assert_ne!(all("u"), all("n"));
    assert_eq!(all("im"), all("mii"));
}

/// A canonical key writes the declared identity flags into its text.
#[test]
fn canonical_key_carries_the_declared_identity_flags() {
    let key = |identity| {
        normalized_matcher(
            MatcherKind::RegularExpression,
            "x",
            "ng",
            None,
            RegexKey::CanonicalSource,
            identity,
        )
    };
    assert_eq!(key(JS), "[regex-flags:] x");
    assert_eq!(key(IdentityFlags::All), "[regex-flags:gn] x");
}

/// Only the translated-body policy partitions, on the declared identity flags: order and
/// repetition merge, a flag outside a listed identity merges, and under the full identity `n`
/// separates. The canonical policy never partitions.
#[test]
fn identity_flags_partition_only_under_the_translated_policy() {
    let all = IdentityFlags::All;
    let part = |flags, identity| RegexKey::TranslatedBody.partition_flags(flags, identity);
    assert_eq!(RegexKey::CanonicalSource.partition_flags("mi", all), None);
    assert_eq!(RegexKey::CanonicalSource.partition_flags("n", all), None);
    assert_eq!(part("mi", all), Some(FlagIdentity::new("im", all)));
    assert_eq!(part("mi", all), part("imm", all));
    assert_ne!(part("n", all), part("", all));
    assert_eq!(part("n", JS), part("", JS));
}

/// The expression wraps only when there are inline flags.
#[test]
fn translated_expression_wraps_only_with_flags() {
    assert_eq!(translated("a|b", "").expression(), "a|b");
    assert_eq!(translated("a|b", "ims").expression(), "(?ims:a|b)");
}

/// Canonicalization preserves anchors, alternatives, backreferences, classes and meaningful
/// group structure while folding capture names and whitespace forms.
#[test]
fn regular_expression_normalization_preserves_matching_semantics() {
    for same in [
        r"^I have (\d{2}) (foo|bar)$",
        r"^value (?=ahead)(\w+)$",
        "^a (?:optional )?step$",
        r"\(literal\)",
        "(?<1bad>a)",
        "(?<open",
        "[(?<name>)]",
        r"[\s+]",
    ] {
        assert_eq!(normalize_regular_expression(same), same);
    }
    assert_eq!(
        normalize_regular_expression(r"^(?<name>\w+) wins$"),
        normalize_regular_expression(r"^(\w+) wins$")
    );
    assert_eq!(
        normalize_regular_expression(r"[a\ ]\s+\ value"),
        r"[a\ ]  value"
    );
    for (left, right) in [
        (
            r"^I wait (\d+) seconds$",
            r"^I wait (a few|several) seconds$",
        ),
        (r"^(a)\1$", r"^(b)\1$"),
        (r"^a step$", "a step"),
    ] {
        assert_ne!(
            normalize_regular_expression(left),
            normalize_regular_expression(right)
        );
    }
}
