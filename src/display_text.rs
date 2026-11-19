//! Character classes for text shown to people: reports drop unsafe characters, diagnostics escape
//! them.

use std::borrow::Cow;
use std::fmt::Write as _;

/// Invisible-format characters (soft hyphen, bidi marks, embeddings, overrides and isolates,
/// zero-width characters, byte order mark, tags) that can make one string display as another.
pub(crate) fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// Characters a diagnostic line must not carry raw: controls can rewrite the terminal or start a
/// CI workflow command on a new line, separators break lines in some viewers, and invisible-format
/// characters disguise text.
fn is_diagnostic_unsafe(character: char) -> bool {
    character.is_control()
        || is_invisible_format(character)
        || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Escapes unsafe characters so repository-derived text stays visible on one stderr line:
/// `\n`, `\r` and `\t` by name, others as `\u{hex}`. Safe text is returned borrowed. Backslashes
/// are not escaped, so the output is for reading, not for decoding.
pub(crate) fn diagnostic_safe(value: &str) -> Cow<'_, str> {
    let Some(first) = value.find(is_diagnostic_unsafe) else {
        return Cow::Borrowed(value);
    };
    let mut escaped = String::with_capacity(value.len() + 8);
    escaped.push_str(&value[..first]);
    for character in value[first..].chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            unsafe_character if is_diagnostic_unsafe(unsafe_character) => {
                // Writing to a String cannot fail.
                let _ = write!(escaped, "{}", unsafe_character.escape_unicode());
            }
            safe => escaped.push(safe),
        }
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests;
