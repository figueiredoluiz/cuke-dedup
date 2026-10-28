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
pub struct TranslatedRegex {
    /// Rust inline flag letters in the order the adapter chose; empty when no group is needed.
    pub(crate) inline_flags: String,
    /// The translated pattern without its flag group.
    pub(crate) body: String,
}

impl TranslatedRegex {
    /// A translation with `inline_flags` (Rust inline flag letters, empty for none) wrapping `body`.
    pub fn new(inline_flags: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            inline_flags: inline_flags.into(),
            body: body.into(),
        }
    }

    /// The complete Rust expression: the body, wrapped in a flag group when there are flags.
    pub fn expression(&self) -> String {
        if self.inline_flags.is_empty() {
            self.body.clone()
        } else {
            format!("(?{}:{})", self.inline_flags, self.body)
        }
    }
}

/// Which source flags count toward a matcher's identity, declared by its regex dialect.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityFlags {
    /// Only these flags count (JavaScript drops `g`, `y` and `d`).
    Listed(&'static str),
    /// Every flag counts, including the ones that name an encoding (Ruby `n`, `u`).
    All,
}

/// The flags of a matcher that count toward its identity, as a sorted, duplicate-free set: flag
/// order and repetition never separate two matchers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FlagIdentity(String);

impl FlagIdentity {
    /// Projects raw source flags onto the identity set the dialect declares.
    pub(crate) fn new(flags: &str, identity: IdentityFlags) -> Self {
        let mut kept: Vec<char> = flags
            .chars()
            .filter(|flag| match identity {
                IdentityFlags::Listed(listed) => listed.contains(*flag),
                IdentityFlags::All => true,
            })
            .collect();
        kept.sort_unstable();
        kept.dedup();
        Self(kept.into_iter().collect())
    }

    /// The set as letters in canonical order.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// How an adapter's regex matchers are keyed for normalized-matcher comparison, declared by its
/// regex dialect.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexKey {
    /// Key on the canonical form of the source pattern, with the identity flags written into the
    /// key text. A translation is ignored. Raw flags never partition keys.
    CanonicalSource,
    /// Key on the translated pattern as written, or on the source pattern when there is no
    /// translation, without canonicalization. The key text carries no flags; the identity flags
    /// partition keys instead.
    TranslatedBody,
}

impl RegexKey {
    /// The identity flags that partition keys of equal text, or `None` when the key text already
    /// carries them.
    pub(crate) fn partition_flags(
        self,
        raw: &str,
        identity: IdentityFlags,
    ) -> Option<FlagIdentity> {
        match self {
            Self::CanonicalSource => None,
            Self::TranslatedBody => Some(FlagIdentity::new(raw, identity)),
        }
    }
}

/// The comparison text of a matcher.
///
/// Cucumber expressions fold placeholders and whitespace; literals stay as written. A regular
/// expression is keyed as `key` declares, with `identity` choosing the flags a canonical key
/// carries.
pub(crate) fn normalized_matcher(
    kind: MatcherKind,
    matcher: &str,
    flags: &str,
    translated: Option<&TranslatedRegex>,
    key: RegexKey,
    identity: IdentityFlags,
) -> String {
    match kind {
        MatcherKind::Literal => matcher.to_owned(),
        MatcherKind::CucumberExpression => super::normalize_cucumber_expression(matcher),
        MatcherKind::RegularExpression => match key {
            RegexKey::CanonicalSource => format!(
                "[regex-flags:{}] {}",
                FlagIdentity::new(flags, identity).as_str(),
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
mod tests;
