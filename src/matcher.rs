//! Shared Cucumber-expression comparison policy, independent of source language.

use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

static CUCUMBER_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\s*([^{}]+?)\s*\}").expect("static placeholder regex"));

/// Normalizes comparison text only; executable matchers retain their original text.
pub(crate) fn normalize_cucumber_expression(matcher: &str) -> String {
    let normalized: String = matcher.nfkc().collect();
    CUCUMBER_PLACEHOLDER
        .replace_all(&normalized, |captures: &regex::Captures<'_>| {
            format!("{{{}}}", captures[1].trim())
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
