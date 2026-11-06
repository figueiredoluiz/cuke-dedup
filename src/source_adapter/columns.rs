//! One-based Unicode character columns for syntax-node byte offsets.

use crate::model::SourceLocation;
use crate::source_adapter::SourceFile;
use tree_sitter::Node;

/// Bytes counted from the nearest checkpoint on each lookup.
const BLOCK_BYTES: usize = 256;

/// Characters before every `BLOCK_BYTES` boundary of one source, so a column scans fewer than two
/// blocks: the tails after the checkpoints below the offset and below its line start.
///
/// Counting from the line start on every lookup is quadratic when many nodes share a long line.
pub(crate) struct ColumnIndex(Vec<usize>);

impl ColumnIndex {
    /// Indexes `source` in one pass.
    pub(crate) fn new(source: &str) -> Self {
        let mut total = 0;
        Self(
            std::iter::once(0)
                .chain(source.as_bytes().chunks(BLOCK_BYTES).map(|block| {
                    total += char_starts(block);
                    total
                }))
                .collect(),
        )
    }

    /// Maps a node of `source`, the text this index was built from, to its one-based location.
    pub(crate) fn location(
        &self,
        file: &SourceFile,
        node: Node<'_>,
        source: &(impl AsRef<[u8]> + ?Sized),
    ) -> SourceLocation {
        let source = source.as_ref();
        let (start, end) = (node.start_position(), node.end_position());
        SourceLocation::new(
            &file.path,
            start.row + 1,
            self.column(source, node.start_byte(), start.column),
            end.row + 1,
            self.column(source, node.end_byte(), end.column),
        )
    }

    /// One-based character column of `offset`, which lies `byte_column` bytes after its line start.
    /// A short line prefix is counted directly; a long one through the block checkpoints.
    pub(crate) fn column(&self, source: &[u8], offset: usize, byte_column: usize) -> usize {
        let line_start = offset - byte_column;
        1 + if byte_column < BLOCK_BYTES {
            char_starts(&source[line_start..offset])
        } else {
            self.chars_before(source, offset) - self.chars_before(source, line_start)
        }
    }

    /// Characters in `source[..offset]`; tree-sitter offsets into a `&str` are char boundaries.
    fn chars_before(&self, source: &[u8], offset: usize) -> usize {
        let block = offset / BLOCK_BYTES;
        self.0[block] + char_starts(&source[block * BLOCK_BYTES..offset])
    }
}

/// Counts UTF-8 leading bytes, which is the character count of a slice that starts and ends on
/// character boundaries.
fn char_starts(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| (**byte & 0xC0) != 0x80).count()
}

#[cfg(test)]
mod tests;
