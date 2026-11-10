use crate::model::{FeatureStep, SourceLocation};
use proptest::prelude::*;
use std::path::Path;

#[derive(Clone, Debug)]
pub(crate) struct FeatureCase {
    pub mode: u8,
    pub rows: Vec<u8>,
    pub background: bool,
    pub two_steps: bool,
    pub markdown: bool,
    pub portuguese: bool,
}

/// Generates only owned supported encodings, plus a separately named invalid-source class.
pub(crate) fn cases() -> impl Strategy<Value = FeatureCase> {
    (
        0u8..=3,
        proptest::collection::vec(0u8..=6, 0..=3),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(mode, rows, background, two_steps, markdown, portuguese)| FeatureCase {
                mode,
                rows,
                background,
                two_steps,
                markdown,
                portuguese,
            },
        )
}

/// Authored decoded cells; expected values never come from the Gherkin parser.
fn value(index: u8) -> &'static str {
    ["", "x", "界", "a|b", "line\nnext", "\\path", "<v>"][index as usize]
}

impl FeatureCase {
    /// Renders the seed, encoding table cells independently of the parser under test.
    pub(crate) fn source(&self) -> String {
        if self.mode == 3 {
            return if self.markdown { "# Feature: bad\n## Scenario Outline: rows\n* Given <v>\n### Examples:\n  | v |\n  | broken\n" } else { "Feature: bad\n Scenario: row\n  Given value\n    | broken\n" }.into();
        }
        let (feature, section, step, examples) = if self.markdown {
            ("# ", "## ", "* ", "### ")
        } else {
            ("", " ", "  ", " ")
        };
        let (name, background, scenario, outline, examples_name, given, then) = if self.portuguese {
            (
                "Funcionalidade",
                "Contexto",
                "Cenário",
                "Esquema do Cenário",
                "Exemplos",
                "Dado",
                "Então",
            )
        } else {
            (
                "Feature",
                "Background",
                "Scenario",
                "Scenario Outline",
                "Examples",
                "Given",
                "Then",
            )
        };
        let mut source = if self.portuguese {
            "# language: pt\n".to_owned()
        } else {
            String::new()
        };
        source.push_str(&format!("{feature}{name}: generated\n"));
        if self.background {
            source.push_str(&format!(
                "{section}{background}: setup\n{step}{given} base\n"
            ));
        }
        source.push_str(&format!(
            "{section}{}: sample\n{step}{given} {}\n",
            if self.mode == 0 { scenario } else { outline },
            if self.mode == 0 {
                "literal <v>"
            } else {
                "<v>/<v>/é/<missing>"
            }
        ));
        if self.two_steps {
            source.push_str(&format!("{step}{then} copy <v>\n"));
        }
        if self.mode != 0 {
            source.push_str(&format!("{examples}{examples_name}:\n  | v |\n"));
            if self.mode == 1 {
                for row in &self.rows {
                    let encoded = value(*row)
                        .replace('\\', "\\\\")
                        .replace('|', "\\|")
                        .replace('\n', "\\n");
                    source.push_str(&format!("  | {encoded} |\n"));
                }
            }
        }
        source
    }

    /// Lists authored visits in row-major admission order, before the final stable source sort.
    fn visits(&self, path: &Path) -> Vec<(FeatureStep, usize)> {
        let mut visits = Vec::new();
        let mut add = |keyword: &str, template: &str, text: String, line, scan| {
            visits.push((
                FeatureStep {
                    keyword: keyword.into(),
                    text,
                    location: SourceLocation::new(
                        path,
                        line,
                        3,
                        line,
                        4 + keyword.chars().count() + template.chars().count(),
                    ),
                },
                scan,
            ));
        };
        let (given, then) = if self.portuguese {
            ("Dado", "Então")
        } else {
            ("Given", "Then")
        };
        let offset = usize::from(self.portuguese);
        if self.background {
            add(given, "base", "base".into(), 3 + offset, 0);
        }
        let line = if self.background { 5 } else { 3 } + offset;
        let template = if self.mode == 0 {
            "literal <v>"
        } else {
            "<v>/<v>/é/<missing>"
        };
        let expanded = self.mode == 1 && !self.rows.is_empty();
        let rows: Vec<_> = if expanded {
            self.rows.iter().map(|n| Some(value(*n))).collect()
        } else {
            vec![None]
        };
        for row in rows {
            let text = row.map_or_else(|| template.to_owned(), |v| format!("{v}/{v}/é/<missing>"));
            add(
                given,
                template,
                text,
                line,
                if expanded { template.len() } else { 0 },
            );
            if self.two_steps {
                let text = row.map_or_else(|| "copy <v>".into(), |v| format!("copy {v}"));
                add(
                    then,
                    "copy <v>",
                    text,
                    line + 1,
                    if expanded { 8 } else { 0 },
                );
            }
        }
        visits
    }

    /// Returns independently authored complete output for a valid seed.
    pub(crate) fn complete(&self, path: &Path) -> Vec<FeatureStep> {
        let mut steps: Vec<_> = self
            .visits(path)
            .into_iter()
            .map(|(step, _)| step)
            .collect();
        steps.sort_by_key(|step| (step.location.line, step.location.column));
        steps
    }
}

#[derive(Default, Debug)]
pub(crate) struct Ledger {
    pub spent: [usize; 3],
}

impl Ledger {
    /// Models atomic whole-file retention and non-refundable admitted scan prefixes.
    pub(crate) fn admit(
        &mut self,
        case: &FeatureCase,
        path: &Path,
        limits: [usize; 3],
    ) -> Result<Vec<FeatureStep>, &'static str> {
        if case.mode == 3 {
            return Err("parse");
        }
        let mut next = self.spent;
        for (step, scan) in case.visits(path) {
            if next[0] + 1 > limits[0] {
                return Err("concrete steps");
            }
            next[0] += 1;
            let base = path.as_os_str().as_encoded_bytes().len() + step.keyword.len();
            if next[1] + base > limits[1] {
                return Err("payload bytes");
            }
            next[1] += base;
            if self.spent[2] + scan > limits[2] {
                return Err("scanned template bytes");
            }
            self.spent[2] += scan;
            if next[1] + step.text.len() > limits[1] {
                return Err("payload bytes");
            }
            next[1] += step.text.len();
        }
        self.spent[0] = next[0];
        self.spent[1] = next[1];
        Ok(case.complete(path))
    }
}
