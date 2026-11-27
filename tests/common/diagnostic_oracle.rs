/// Controls, separators and invisible-format characters from the original report ranges.
pub(crate) fn original_unsafe(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00..=0x1f
            | 0x7f..=0x9f
            | 0xad
            | 0x61c
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x2069
            | 0xfeff
            | 0xe0000..=0xe007f
    )
}

/// Diagnostic code points that require escaping under the Unicode 17 default-ignorable policy.
pub(crate) fn diagnostic_unsafe(character: char) -> bool {
    original_unsafe(character)
        || matches!(
            u32::from(character),
            0x034f
                | 0x115f..=0x1160
                | 0x17b4..=0x17b5
                | 0x180b..=0x180f
                | 0x206a..=0x206f
                | 0x3164
                | 0xfe00..=0xfe0f
                | 0xffa0
                | 0xfff0..=0xfff8
                | 0x1bca0..=0x1bca3
                | 0x1d173..=0x1d17a
                | 0xe0080..=0xe0fff
        )
}
