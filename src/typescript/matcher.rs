use super::node_text;
use crate::model::MatcherKind;
use crate::resource_limits::compile_regex;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Node;
use unicode_normalization::UnicodeNormalization;

static CUCUMBER_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\s*([^{}]+?)\s*\}").expect("static placeholder regex"));

pub(super) fn matcher_value(
    node: Node<'_>,
    source: &[u8],
) -> Option<(String, MatcherKind, String)> {
    let text = node_text(node, source);
    match node.kind() {
        "string" => decode_js_string(text)
            .map(|value| (value, MatcherKind::CucumberExpression, String::new())),
        "template_string" if !text.contains("${") => Some((
            text.strip_prefix('`')?.strip_suffix('`')?.to_owned(),
            MatcherKind::CucumberExpression,
            String::new(),
        )),
        "regex" => {
            let body = text.strip_prefix('/')?;
            let final_slash = body.rfind('/')?;
            Some((
                body[..final_slash].to_owned(),
                MatcherKind::RegularExpression,
                body[final_slash + 1..].to_owned(),
            ))
        }
        _ => None,
    }
}

pub(super) fn decode_js_string(text: &str) -> Option<String> {
    let quote = text.chars().next()?;
    if text.chars().last()? != quote || !matches!(quote, '\'' | '"') {
        return None;
    }
    let body = &text[1..text.len() - 1];
    let mut decoded = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            'b' => decoded.push('\u{0008}'),
            'f' => decoded.push('\u{000c}'),
            'v' => decoded.push('\u{000b}'),
            '0' => decoded.push('\0'),
            '\'' => decoded.push('\''),
            '"' => decoded.push('"'),
            '\\' => decoded.push('\\'),
            '\n' => {}
            '\r' => {
                if chars.clone().next() == Some('\n') {
                    chars.next();
                }
            }
            'x' => decoded.push(decode_hex_escape(&mut chars, 2)?),
            'u' => {
                let value = if chars.clone().next() == Some('{') {
                    chars.next();
                    let mut digits = String::new();
                    loop {
                        let character = chars.next()?;
                        if character == '}' {
                            break;
                        }
                        digits.push(character);
                    }
                    u32::from_str_radix(&digits, 16).ok()?
                } else {
                    decode_hex_value(&mut chars, 4)?
                };
                decoded.push(char::from_u32(value)?);
            }
            other => decoded.push(other),
        }
    }
    Some(decoded)
}

fn decode_hex_escape(chars: &mut std::str::Chars<'_>, length: usize) -> Option<char> {
    char::from_u32(decode_hex_value(chars, length)?)
}

fn decode_hex_value(chars: &mut std::str::Chars<'_>, length: usize) -> Option<u32> {
    let digits: String = chars.take(length).collect();
    (digits.len() == length)
        .then(|| u32::from_str_radix(&digits, 16).ok())
        .flatten()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegexSupport {
    Supported,
    Unsupported,
    ResourceLimit,
}

/// Builds the Rust regular expression for a JavaScript matcher and its flags.
///
/// Extraction and usage analysis must agree on this text exactly: extraction uses it to decide
/// whether a definition is analyzable, and usage analysis uses it to match feature steps. Two
/// copies could drift and report a limitation the analyzer did not actually hit.
pub(crate) fn rust_regex_expression(matcher: &str, flags: &str) -> Option<String> {
    // JavaScript's `v` flag enables Unicode set notation and changes character-class semantics.
    // Rust's regex engine has no equivalent, so no Rust expression is authoritative for it.
    if flags.contains('v') {
        return None;
    }
    let flags: String = flags
        .chars()
        .filter(|flag| matches!(flag, 'i' | 'm' | 's' | 'u'))
        .collect();
    if flags.is_empty() {
        Some(matcher.to_owned())
    } else {
        Some(format!("(?{flags}:{matcher})"))
    }
}

pub(super) fn rust_regex_support(matcher: &str, flags: &str) -> RegexSupport {
    let Some(expression) = rust_regex_expression(matcher, flags) else {
        return RegexSupport::Unsupported;
    };
    match compile_regex(&expression) {
        Ok(_) => RegexSupport::Supported,
        Err(regex::Error::CompiledTooBig(_)) => RegexSupport::ResourceLimit,
        Err(_) => RegexSupport::Unsupported,
    }
}

/// Canonicalizes matcher text for stable equivalence comparisons.
pub fn normalize_matcher(matcher: &str, kind: MatcherKind) -> String {
    let normalized: String = matcher.nfkc().collect();
    let normalized = match kind {
        MatcherKind::CucumberExpression => CUCUMBER_PLACEHOLDER
            .replace_all(&normalized, |captures: &regex::Captures<'_>| {
                format!("{{{}}}", captures[1].trim())
            })
            .into_owned(),
        MatcherKind::RegularExpression => normalize_regular_expression(&normalized),
    };
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn normalize_matcher_with_flags(
    matcher: &str,
    kind: MatcherKind,
    flags: &str,
) -> String {
    let normalized = normalize_matcher(matcher, kind);
    let semantic_flags = semantic_regex_flags(flags);
    if kind == MatcherKind::RegularExpression {
        format!("[regex-flags:{semantic_flags}] {normalized}")
    } else {
        normalized
    }
}

fn semantic_regex_flags(flags: &str) -> String {
    ['i', 'm', 's', 'u']
        .into_iter()
        .filter(|flag| flags.contains(*flag))
        .collect()
}

pub(super) fn normalize_regular_expression(expression: &str) -> String {
    normalize_regex_whitespace(&normalize_capture_groups(expression))
}

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

    #[test]
    fn javascript_string_decoding_covers_escape_forms_and_rejections() {
        assert_eq!(
            decode_js_string(r#""\n\r\t\b\f\v\0\"\\\x41\u0042\u{43}z""#).unwrap(),
            "\n\r\t\u{0008}\u{000c}\u{000b}\0\"\\ABCz"
        );
        assert_eq!(
            decode_js_string("'line\\\njoin'"),
            Some("linejoin".to_owned())
        );
        assert_eq!(
            decode_js_string("'line\\\r\njoin'"),
            Some("linejoin".to_owned())
        );
        assert_eq!(decode_js_string(r#"'\q'"#), Some("q".to_owned()));
        for invalid in [
            "",
            "plain",
            "'unterminated",
            r#"'\xG0'"#,
            r#"'\u{}'"#,
            r#"'\u{110000}'"#,
        ] {
            assert!(decode_js_string(invalid).is_none(), "{invalid:?}");
        }
    }

    #[test]
    fn regex_normalization_preserves_semantics_while_canonicalizing_equivalence() {
        assert_eq!(
            rust_regex_expression("^value$", "gymi").as_deref(),
            Some("(?mi:^value$)")
        );
        assert_eq!(
            rust_regex_expression("^value$", "gy").as_deref(),
            Some("^value$")
        );
        assert_eq!(rust_regex_expression("[a&&b]", "v"), None);
        assert_eq!(rust_regex_support("^value$", ""), RegexSupport::Supported);
        assert_eq!(rust_regex_support("(", ""), RegexSupport::Unsupported);
        assert_eq!(rust_regex_support("[a&&b]", "v"), RegexSupport::Unsupported);
        assert_eq!(semantic_regex_flags("uugmisy"), "imsu");
        assert_eq!(
            normalize_matcher_with_flags("(?<name>a)\\s+b", MatcherKind::RegularExpression, "mi"),
            "[regex-flags:im] (a) b"
        );
        assert_eq!(
            normalize_matcher_with_flags("I have { int }", MatcherKind::CucumberExpression, "i"),
            "I have {int}"
        );
        assert_eq!(
            normalize_regular_expression(r"[a\ ]\s+\ value"),
            r"[a\ ]  value"
        );
        assert_eq!(normalize_regular_expression(r"\(literal\)"), r"\(literal\)");
        assert_eq!(normalize_regular_expression("(?<1bad>a)"), "(?<1bad>a)");
        assert_eq!(normalize_regular_expression("(?<open"), "(?<open");
        assert_eq!(normalize_regular_expression("[(?<name>)]"), "[(?<name>)]");
    }
}
