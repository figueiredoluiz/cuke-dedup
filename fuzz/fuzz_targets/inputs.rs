#![no_main]
#![forbid(unsafe_code)]

//! Fuzzes the untrusted inputs that reach CukeDedup outside its source parsers.
//!
//! `parsers.rs` covers Gherkin and JavaScript/TypeScript sources. The remaining attacker-supplied
//! inputs are the configuration files discovered beneath the analysis root and the baseline a
//! repository may check in. Both are parsed before any analysis runs, so a panic there aborts the
//! whole run rather than degrading one file.
//!
//! `Config::load` parses `package.json` eagerly and propagates its error, so writing the same
//! bytes to every candidate would stop almost every input at that first parse and leave the
//! standalone, framework, and project-config parsers unreached. One leading byte therefore
//! selects a single target file; every other candidate keeps minimal valid content so discovery
//! proceeds far enough to reach the selected parser.

use cuke_dedup::config::{Config, ConfigOverrides};
use cuke_dedup::modes::apply_baseline;
use cuke_dedup::source_adapter::{
    adapter_for_language, SourceExtractionSession, SourceFile, SourceLanguage,
};
use libfuzzer_sys::fuzz_target;
use std::fs;

/// Candidate files and the valid content used when a file is not the fuzzed target.
const CANDIDATES: [(&str, &str); 9] = [
    ("package.json", "{}"),
    (".cuke-dedup.json", "{}"),
    ("cuke-dedup.config.json", "{}"),
    ("cucumber.json", "{}"),
    ("cucumber.yaml", "default: {}"),
    ("cucumber.js", "module.exports = {};"),
    ("tsconfig.json", "{}"),
    ("playwright.config.ts", "export default {};"),
    ("cypress.config.ts", "export default {};"),
];

fuzz_target!(|data: &[u8]| {
    // The selector byte is consumed so the remaining bytes are the file's whole content.
    let Some((selector, payload)) = data.split_first() else {
        return;
    };
    let Ok(directory) = tempfile::Builder::new().prefix("cuke-dedup-fuzz").tempdir() else {
        return;
    };
    let root = directory.path();
    let selected = usize::from(*selector) % (CANDIDATES.len() + 1);

    for (index, (name, valid)) in CANDIDATES.iter().enumerate() {
        let contents: &[u8] = if index == selected {
            payload
        } else {
            valid.as_bytes()
        };
        if fs::write(root.join(name), contents).is_err() {
            return;
        }
    }
    // Configuration loading must fail cleanly on malformed input, never panic.
    let _ = Config::load(root, ConfigOverrides::default());

    // Exercise project module metadata through the same shared extraction session the CLI uses.
    // Config loading alone does not read tsconfig/jsconfig paths, package imports, or workspace
    // exports, so without this probe those candidate files would be written but never parsed.
    let definition_path = root.join("steps.ts");
    let definition_source = r##"
        import { Given as PathGiven } from "@support/world";
        import { When as ImportWhen } from "#support";
        import * as WorkspaceSteps from "support";
        PathGiven("path step", () => {});
        ImportWhen("import step", () => {});
        WorkspaceSteps.Then("workspace step", () => {});
    "##;
    if fs::write(&definition_path, definition_source).is_ok() {
        let file = SourceFile {
            path: definition_path,
            language: SourceLanguage::TypeScript,
        };
        let mut session = SourceExtractionSession::new(root);
        let _ = adapter_for_language(SourceLanguage::TypeScript)
            .extract_file_with_session(&file, &mut session);
    }

    // The final selector value fuzzes the baseline instead, against a valid configuration.
    if selected == CANDIDATES.len() {
        let baseline = root.join("baseline.json");
        if fs::write(&baseline, payload).is_ok() {
            let _ = apply_baseline(&mut [], &baseline);
        }
    }
});
