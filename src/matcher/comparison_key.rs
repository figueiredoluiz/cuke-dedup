//! Comparison keys of regex matchers, derived from neutral inputs an adapter supplies.
//!
//! An adapter hands over the source pattern, its raw flags and, when it can, the Rust regex it
//! translates to. The core owns everything after that: which flags count toward identity, how
//! the pattern is canonicalized and where the flags sit in the resulting key.

use crate::model::MatcherKind;
use unicode_normalization::UnicodeNormalization;

/// A source matcher translated to the Rust `regex` dialect: the pattern body and the inline flags
/// that wrap it. Splitting them keeps a pattern that merely starts with `(?i:` from being taken
/// for a wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslatedRegex {
    /// Rust inline flag letters in the order the adapter chose; empty when no group is needed.
    pub(crate) inline_flags: String,
    /// The translated pattern without its flag group.
    pub(crate) body: String,
}

impl TranslatedRegex {
    /// The complete Rust expression: the body, wrapped in a flag group when there are flags.
    pub(crate) fn expression(&self) -> String {
        if self.inline_flags.is_empty() {
            self.body.clone()
        } else {
            format!("(?{}:{})", self.inline_flags, self.body)
        }
    }
}

/// The flags that change which inputs a matcher accepts: `i`, `m`, `s`, `u` and `v`, as a sorted,
/// duplicate-free set. `g`, `y`, `d` and every other flag drive a match or name an encoding
/// without changing the accepted language, so they never separate two exact-identity classes.
/// [`RegexKey::TranslatedBody`] partitions normalized keys on the raw flags instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FlagIdentity(String);

impl FlagIdentity {
    /// Projects raw source flags onto the identity set.
    pub(crate) fn new(flags: &str) -> Self {
        Self(
            ['i', 'm', 's', 'u', 'v']
                .into_iter()
                .filter(|flag| flags.contains(*flag))
                .collect(),
        )
    }

    /// The set as letters in canonical order.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// How an adapter's regex matchers are keyed for normalized-matcher comparison, declared by its
/// regex dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegexKey {
    /// Key on the canonical form of the source pattern, with the identity flags written into the
    /// key text. A translation is ignored. Raw flags never partition keys.
    CanonicalSource,
    /// Key on the translated pattern as written, or on the source pattern when there is no
    /// translation, without canonicalization. The key text carries no flags; the flag string the
    /// adapter extracted partitions keys instead, so a flag outside [`FlagIdentity`] (Ruby's `n`)
    /// still separates two keys.
    TranslatedBody,
}

impl RegexKey {
    /// The raw flags that partition keys of equal text, or `None` when the key text already
    /// carries them.
    pub(crate) fn partition_flags(self, raw: &str) -> Option<&str> {
        match self {
            Self::CanonicalSource => None,
            Self::TranslatedBody => Some(raw),
        }
    }
}

/// The comparison text of a matcher.
///
/// Cucumber expressions fold placeholders and whitespace; literals stay as written. A regular
/// expression is keyed as `key` declares.
pub(crate) fn normalized_matcher(
    kind: MatcherKind,
    matcher: &str,
    flags: &str,
    translated: Option<&TranslatedRegex>,
    key: RegexKey,
) -> String {
    match kind {
        MatcherKind::Literal => matcher.to_owned(),
        MatcherKind::CucumberExpression => super::normalize_cucumber_expression(matcher),
        MatcherKind::RegularExpression => match key {
            RegexKey::CanonicalSource => format!(
                "[regex-flags:{}] {}",
                FlagIdentity::new(flags).as_str(),
                canonical_regular_expression(matcher)
            ),
            RegexKey::TranslatedBody => translated
                .map_or(matcher, |translated| translated.body.as_str())
                .to_owned(),
        },
    }
}

/// NFKC-normalizes a pattern, then folds capture names and the whitespace forms of
/// [`normalize_regular_expression`] and every whitespace run.
fn canonical_regular_expression(pattern: &str) -> String {
    normalize_regular_expression(&pattern.nfkc().collect::<String>())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Canonicalizes named capture spelling and selected regex whitespace forms for comparison.
fn normalize_regular_expression(expression: &str) -> String {
    normalize_regex_whitespace(&normalize_capture_groups(expression))
}

/// Folds escaped spaces and `\s+` outside character classes while preserving other escapes.
fn normalize_regex_whitespace(expression: &str) -> String {
    let characters: Vec<_> = expression.chars().collect();
    let mut output = String::with_capacity(expression.len());
    let mut offset = 0;
    let mut in_class = false;
    while offset < characters.len() {
        match characters[offset] {
            '[' => {
                in_class = true;
                output.push('[');
                offset += 1;
            }
            ']' => {
                in_class = false;
                output.push(']');
                offset += 1;
            }
            '\\' if !in_class && characters.get(offset + 1) == Some(&' ') => {
                output.push(' ');
                offset += 2;
            }
            '\\' if !in_class
                && characters.get(offset + 1) == Some(&'s')
                && characters.get(offset + 2) == Some(&'+') =>
            {
                output.push(' ');
                offset += 3;
            }
            '\\' => {
                output.push('\\');
                offset += 1;
                if let Some(character) = characters.get(offset) {
                    output.push(*character);
                    offset += 1;
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

/// Removes recognized capture names outside character classes while preserving escapes and group bodies.
fn normalize_capture_groups(expression: &str) -> String {
    let characters: Vec<_> = expression.chars().collect();
    let mut output = String::new();
    let mut offset = 0;
    let mut in_class = false;
    while offset < characters.len() {
        match characters[offset] {
            '\\' => {
                output.push('\\');
                offset += 1;
                if let Some(character) = characters.get(offset) {
                    output.push(*character);
                    offset += 1;
                }
            }
            '[' => {
                in_class = true;
                output.push('[');
                offset += 1;
            }
            ']' => {
                in_class = false;
                output.push(']');
                offset += 1;
            }
            '(' if !in_class => {
                output.push('(');
                offset += 1;
                // Named and unnamed captures have the same matching semantics. Strip only a
                // syntactically valid name; preserve the group expression itself, non-capturing
                // groups, lookarounds, and backreferences.
                if characters.get(offset) == Some(&'?') && characters.get(offset + 1) == Some(&'<')
                {
                    let name_start = offset + 2;
                    if characters
                        .get(name_start)
                        .is_some_and(|character| character.is_alphabetic() || *character == '_')
                    {
                        if let Some(end) = characters[name_start..]
                            .iter()
                            .position(|character| *character == '>')
                        {
                            offset = name_start + end + 1;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [RegexKey; 2] = [RegexKey::CanonicalSource, RegexKey::TranslatedBody];

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
                        key
                    ),
                    "I have {int} items"
                );
                assert_eq!(
                    normalized_matcher(MatcherKind::Literal, "a  { b }", flags, None, key),
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
                normalized_matcher(MatcherKind::RegularExpression, pattern, "v", None, key)
            };
            assert_ne!(of("[a&&b]"), of("[a&&c]"), "{key:?}");
            assert_eq!(of("[a&&b]"), of("[a&&b]"), "{key:?}");
        }
    }

    /// Identity keeps `i`, `m`, `s`, `u`, `v` in canonical order and drops everything else; the
    /// controls are that dropped flags and order never separate two sets.
    #[test]
    fn flag_identity_keeps_accepted_language_flags_only() {
        for (raw, identity) in [
            ("uugmisy", "imsu"),
            ("vi", "iv"),
            ("", ""),
            ("gyd", ""),
            ("nxo", ""),
            ("ms", "ms"),
        ] {
            assert_eq!(FlagIdentity::new(raw).as_str(), identity, "{raw:?}");
        }
        assert_eq!(FlagIdentity::new("iv"), FlagIdentity::new("vi"));
        assert_ne!(FlagIdentity::new("v"), FlagIdentity::new("u"));
        assert_ne!(FlagIdentity::new("i"), FlagIdentity::new(""));
        assert_eq!(FlagIdentity::new("g"), FlagIdentity::new(""));
    }

    /// Only the translated-body policy partitions on the extracted flags, so a flag outside the
    /// identity set separates its keys while the canonical policy never does.
    #[test]
    fn raw_flags_partition_only_under_the_translated_policy() {
        assert_eq!(RegexKey::CanonicalSource.partition_flags("mi"), None);
        assert_eq!(RegexKey::CanonicalSource.partition_flags("n"), None);
        assert_eq!(RegexKey::TranslatedBody.partition_flags("mi"), Some("mi"));
        assert_eq!(RegexKey::TranslatedBody.partition_flags(""), Some(""));
        assert_ne!(
            RegexKey::TranslatedBody.partition_flags("n"),
            RegexKey::TranslatedBody.partition_flags("")
        );
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
}
