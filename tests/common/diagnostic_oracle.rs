/// Unsafe diagnostic code points from Unicode Cc, Zl/Zp and the report ranges, as raw intervals.
pub(crate) fn diagnostic_unsafe(character: char) -> bool {
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
