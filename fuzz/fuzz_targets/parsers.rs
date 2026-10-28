#![no_main]
#![forbid(unsafe_code)]

use cuke_dedup::discovery::SourceFile;
use cuke_dedup::gherkin::{self, FeatureFormat};
use cuke_dedup::typescript;
use cuke_dedup::typescript::{JAVASCRIPT, TSX, TYPESCRIPT};
use libfuzzer_sys::fuzz_target;
use std::path::{Path, PathBuf};

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let _ = gherkin::extract(&source, Path::new("fuzz.feature"));
    let _ = gherkin::extract_with_format(
        &source,
        Path::new("fuzz.feature.md"),
        FeatureFormat::GherkinMarkdown,
    );
    for language in [JAVASCRIPT, TYPESCRIPT, TSX] {
        let file = SourceFile {
            path: PathBuf::from("fuzz.steps.ts"),
            language,
        };
        let _ = typescript::extract_detailed(&source, &file);
    }
});
