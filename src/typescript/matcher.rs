use super::node_text;
use crate::matcher::TranslatedRegex;
use crate::model::MatcherKind;
use crate::resource_limits::compile_regex;
use tree_sitter::Node;

/// Extracts matcher text, kind and flags from strings, static templates or regex literals.
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

/// Decodes a property key that is written as a static string literal.
///
/// Deciding this from text alone is unsound: any expression whose text merely begins and ends with
/// a matching quote — a sequence expression like `['a', f(), 'b']` or a concatenation like
/// `['a' + 'b']` — would otherwise decode into a key that names nothing, either inventing a
/// property or masking an unknown one. Requiring the node to be a string literal keeps an
/// undecodable key undecodable, so callers apply their conservative branch instead.
pub(super) fn static_string_key(node: Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let text = node_text(node, source);
    // Legacy numeric escapes are not decoded here: `'\157'` is `o`, but the shared decoder reads
    // the digits literally and would answer `157`. A wrong name is worse than no name — it grants
    // trust to a property the source never mentions — so an unsupported escape stays unreadable.
    //
    // Walk the escapes rather than scanning adjacent bytes: in `'foo\\5'` the first backslash
    // escapes the second and the `5` is an ordinary character, so the key is readable.
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            continue;
        }
        match characters.next() {
            // `\0` is NUL unless another digit follows, and the decoder handles it. Only the
            // legacy octal forms are unreadable here.
            Some('0')
                if !characters
                    .clone()
                    .next()
                    .is_some_and(|next| next.is_ascii_digit()) => {}
            Some(escaped) if escaped.is_ascii_digit() => return None,
            // Every other escape consumes its character, so a doubled backslash cannot be
            // mistaken for the start of one.
            Some(_) => {}
            None => return None,
        }
    }
    decode_js_string(text)
}

/// Decodes quoted JavaScript string escapes, returning `None` for malformed or non-scalar escapes.
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
            // U+2028 and U+2029 are JavaScript line terminators, so a backslash before either is a
            // line continuation that contributes nothing. Preserving the separator instead would
            // both lose the real name and conflate the key with one that genuinely contains it.
            '\u{2028}' | '\u{2029}' => {}
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

/// Consumes a fixed-width hexadecimal escape and rejects values that are not Unicode scalars.
fn decode_hex_escape(chars: &mut std::str::Chars<'_>, length: usize) -> Option<char> {
    char::from_u32(decode_hex_value(chars, length)?)
}

/// Consumes exactly `length` hexadecimal characters, rejecting truncated or invalid digits.
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

/// Translates a JavaScript matcher and its flags to the Rust regex dialect.
///
/// Extraction and usage analysis must agree on this translation exactly: extraction uses it to
/// decide whether a definition is analyzable, and usage analysis uses it to match feature steps.
/// Two copies could drift and report a limitation the analyzer did not actually hit.
fn translate_regex(matcher: &str, flags: &str) -> Option<TranslatedRegex> {
    // JavaScript's `v` flag enables Unicode set notation and changes character-class semantics.
    // Rust's regex engine has no equivalent, so no Rust expression is authoritative for it.
    if flags.contains('v') {
        return None;
    }
    Some(TranslatedRegex {
        inline_flags: flags
            .chars()
            .filter(|flag| matches!(flag, 'i' | 'm' | 's' | 'u'))
            .collect(),
        body: matcher.to_owned(),
    })
}

/// Builds the Rust regular expression for a JavaScript matcher and its flags.
pub(crate) fn rust_regex_expression(matcher: &str, flags: &str) -> Option<String> {
    translate_regex(matcher, flags).map(|translated| translated.expression())
}

/// Distinguishes supported regexes, unsupported syntax and compiled-size resource limits.
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

#[cfg(test)]
mod tests;
