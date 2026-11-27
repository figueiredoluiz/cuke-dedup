//! Character classes and escapes for text shown in reports and diagnostics.

use crate::resource_limits::{
    sample_text, WorkBudget, MAX_DIAGNOSTIC_BYTES, MAX_DIAGNOSTIC_RECORDS, MAX_REPORT_FIELD_BYTES,
};
use std::borrow::Cow;
use std::fmt::Write as _;

/// Unicode 17 default-ignorable scalars that can change how other characters are displayed.
pub(crate) fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{034f}'
            | '\u{061c}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fff8}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// Identifies the controls, separators and invisible-format scalars escaped on diagnostic lines.
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

/// The fixed extra detail rendered after a channel refuses more text.
pub(crate) const DIAGNOSTIC_OMISSION: &str =
    "diagnostic details omitted after reaching the safety limit";

/// Maximum CLI line prefix from any channel, plus its newline.
const DIAGNOSTIC_LINE_OVERHEAD: usize = "cuke-dedup: warning: \n".len();

/// Preserves an ordered diagnostic prefix. A refusal is recorded even when no detail fits.
pub(crate) struct DiagnosticDetails {
    details: Vec<String>,
    max_records: usize,
    bytes: WorkBudget,
    omitted: bool,
}

impl Default for DiagnosticDetails {
    /// Starts an empty channel with the CLI diagnostic allowances.
    fn default() -> Self {
        Self::with_limits(MAX_DIAGNOSTIC_RECORDS, MAX_DIAGNOSTIC_BYTES)
    }
}

impl DiagnosticDetails {
    /// Starts a channel with an independent record and escaped-byte allowance.
    pub(crate) fn with_limits(max_records: usize, max_bytes: usize) -> Self {
        Self {
            details: Vec::new(),
            max_records,
            bytes: WorkBudget::new(max_bytes),
            omitted: false,
        }
    }

    /// Samples an individual detail, then admits it only if its escaped line fits.
    pub(crate) fn push(&mut self, detail: String) {
        if self.omitted || self.details.len() == self.max_records {
            self.omitted = true;
            return;
        }
        let detail = if detail.len() > MAX_REPORT_FIELD_BYTES {
            sample_text(&detail, MAX_REPORT_FIELD_BYTES).into_owned()
        } else {
            detail
        };
        let bytes = diagnostic_safe(&detail).len() + DIAGNOSTIC_LINE_OVERHEAD;
        if self.bytes.admit(bytes) {
            self.details.push(detail);
        } else {
            self.omitted = true;
        }
    }

    /// Admits an ordered batch, ending on the first refusal.
    pub(crate) fn extend(&mut self, details: impl IntoIterator<Item = String>) {
        for detail in details {
            self.push(detail);
            if self.omitted {
                break;
            }
        }
    }

    /// Reports whether the channel ever received a diagnostic, including an omitted one.
    pub(crate) fn is_empty(&self) -> bool {
        self.details.is_empty() && !self.omitted
    }

    /// Visits the admitted details, followed by an explicit marker if any were omitted.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        self.details
            .iter()
            .map(String::as_str)
            .chain(self.omitted.then_some(DIAGNOSTIC_OMISSION))
    }

    /// Returns the bounded traversal details in the public discovery result's existing format.
    pub(crate) fn into_lines(mut self) -> Vec<String> {
        if self.omitted {
            self.details.push(DIAGNOSTIC_OMISSION.to_owned());
        }
        self.details
    }

    /// Counts the diagnostic lines, including the omission marker.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.details.len() + usize::from(self.omitted)
    }
}

#[cfg(test)]
mod tests;
