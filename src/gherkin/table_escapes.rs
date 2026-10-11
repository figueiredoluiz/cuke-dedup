//! Preserve Cucumber's unknown table escapes across the stricter gherkin crate grammar.
use anyhow::{bail, Context, Result};
use gherkin::{Feature, Table};
use std::borrow::Cow;

pub(super) fn prepare(source: &str) -> Result<(Cow<'_, str>, Option<char>)> {
    let mut output = None;
    let mut marker = None;
    let mut copied = 0;
    let mut offset = 0;
    let mut fence = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let active = fence;
        if let Some(delimiter) = active {
            if trimmed.starts_with(delimiter) {
                fence = None;
            }
        } else if trimmed.starts_with("\"\"\"") {
            fence = Some("\"\"\"");
        } else if trimmed.starts_with("```") {
            fence = Some("```");
        } else if trimmed.starts_with('|') {
            let mut chars = line.char_indices();
            while let Some((index, character)) = chars.next() {
                if character != '\\' {
                    continue;
                }
                // Consume known escape pairs, including doubled backslashes, as one unit.
                if matches!(chars.clone().next(), Some((_, 'n' | '|' | '\\'))) {
                    chars.next();
                    continue;
                }
                let replacement = match marker {
                    Some(value) => value,
                    None => {
                        let value = unused_marker(source)?;
                        marker = Some(value);
                        value
                    }
                };
                let output = output.get_or_insert_with(|| String::with_capacity(source.len()));
                let position = offset + index;
                output.push_str(&source[copied..position]);
                output.push(replacement);
                copied = position + 1;
            }
        }
        offset += line.len();
    }
    Ok((
        match output {
            None => Cow::Borrowed(source),
            Some(mut output) => {
                output.push_str(&source[copied..]);
                Cow::Owned(output)
            }
        },
        marker,
    ))
}

fn unused_marker(source: &str) -> Result<char> {
    // Fixed occupancy storage and one scan avoid a source scan for every candidate marker.
    let mut used = [false; 0x1900];
    for character in source.chars() {
        if ('\u{e000}'..='\u{f8ff}').contains(&character) {
            used[character as usize - 0xe000] = true;
        }
    }
    if let Some(index) = used.iter().position(|present| !present) {
        return Ok(char::from_u32(0xe000 + index as u32).expect("private-use scalar"));
    }
    bail!("Gherkin table escape compatibility exhausted its reserved-character budget")
}

pub(super) fn finish(feature: &mut Feature, source: &str, marker: Option<char>) -> Result<()> {
    let restore_table = |table: &mut Option<Table>| -> Result<()> {
        if let Some(table) = table {
            let original = source
                .get(table.span.start..table.span.end.min(source.len()))
                .context("Gherkin table source span is out of bounds")?;
            for (index, line) in original.lines().enumerate() {
                if !line.trim_start().starts_with('|') {
                    continue;
                }
                let mut delimiters = 0_usize;
                let mut characters = line.chars();
                while let Some(character) = characters.next() {
                    match character {
                        '\\' => {
                            characters.next();
                        }
                        '|' => delimiters += 1,
                        _ => {}
                    }
                }
                if delimiters.saturating_sub(1) != table.row_width() {
                    bail!(
                        "inconsistent Gherkin table width at line {}",
                        table.position.line + index
                    );
                }
            }
            if let Some(marker) = marker {
                for cell in table.rows.iter_mut().flatten() {
                    *cell = cell.replace(marker, "\\");
                }
            }
        }
        Ok(())
    };
    for background in feature.background.iter_mut().chain(
        feature
            .rules
            .iter_mut()
            .filter_map(|rule| rule.background.as_mut()),
    ) {
        for step in &mut background.steps {
            restore_table(&mut step.table)?;
        }
    }
    for scenario in feature.scenarios.iter_mut().chain(
        feature
            .rules
            .iter_mut()
            .flat_map(|rule| rule.scenarios.iter_mut()),
    ) {
        for example in &mut scenario.examples {
            restore_table(&mut example.table)?;
        }
        for step in &mut scenario.steps {
            restore_table(&mut step.table)?;
        }
    }
    Ok(())
}
