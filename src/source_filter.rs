//! Content-based exclusion of discovered inputs that are not authored source.
//!
//! Discovery selects definition sources by path alone, so vendored bundles, compressed payloads
//! and binary blobs reach extraction whenever they carry a source extension. None of them are
//! hand-written step definitions: parsing them cannot produce a finding, and on some inputs it
//! costs orders of magnitude more time than the rest of the run combined.
//!
//! Classification reads a bounded prefix rather than the whole file so that an oversized bundle
//! is excluded instead of failing the run against [`MAX_PROJECT_INPUT_BYTES`].
//!
//! [`MAX_PROJECT_INPUT_BYTES`]: crate::resource_limits::MAX_PROJECT_INPUT_BYTES

use crate::resource_limits::{MINIFIED_MEAN_LINE_BYTES, SOURCE_CLASSIFICATION_PREFIX_BYTES};
use crate::typescript::frameworks::registration_modules;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Why a discovered source was excluded from analysis without being parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExcludedSource {
    /// The file opens with the container magic of a compressed archive or stream.
    Compressed,
    /// The inspected prefix contains a NUL byte.
    Binary,
    /// Line geometry matches generated or minified output rather than authored code.
    Minified,
}

impl ExcludedSource {
    /// Returns the stable word used for this exclusion in diagnostics.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Compressed => "compressed",
            Self::Binary => "binary",
            Self::Minified => "minified",
        }
    }
}

/// Leading bytes of compressed container formats that can arrive with a source extension.
///
/// Raw zlib streams are deliberately absent: their first byte is ASCII `x`, so recognizing them
/// would rest on the second byte alone, and a stream that reaches here without a NUL byte is far
/// rarer than a source file opening with `x`.
const COMPRESSED_MAGICS: [&[u8]; 5] = [
    &[0x1f, 0x8b],                         // gzip
    &[0x50, 0x4b, 0x03, 0x04],             // zip
    &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00], // xz
    &[0x28, 0xb5, 0x2f, 0xfd],             // zstd
    &[0x04, 0x22, 0x4d, 0x18],             // lz4
];

/// Reports whether the prefix opens with a compressed container.
fn opens_compressed_container(prefix: &[u8]) -> bool {
    COMPRESSED_MAGICS
        .iter()
        .any(|magic| prefix.starts_with(magic))
        || opens_bzip2_stream(prefix)
}

/// Reports whether the prefix opens with a bzip2 stream.
///
/// `BZh` and even `BZh9` are legal JavaScript identifiers, so the block-size digit alone would
/// exclude an authored file that happens to start with one. A stream continues with the block
/// magic, and the whole sequence together is what distinguishes an archive from an identifier.
///
/// An empty bzip2 archive carries the end-of-stream magic instead and is not recognized here; it
/// is not valid UTF-8, so reading it still fails loudly rather than being analyzed.
fn opens_bzip2_stream(prefix: &[u8]) -> bool {
    const BLOCK_MAGIC: &[u8] = b"1AY&SY";

    prefix.starts_with(b"BZh")
        && prefix
            .get(3)
            .is_some_and(|digit| (b'1'..=b'9').contains(digit))
        && prefix.get(4..4 + BLOCK_MAGIC.len()) == Some(BLOCK_MAGIC)
}

/// Classifies a discovered source from a bounded prefix of its bytes.
///
/// Returns `None` when the file is ordinary source, and also when the prefix cannot be read:
/// reporting that failure is left to extraction, which already attributes read errors to the file
/// with full context.
pub(crate) fn inspect(path: &Path, registrations: &[String]) -> Option<ExcludedSource> {
    let mut file = File::open(path).ok()?;
    let mut prefix = vec![0_u8; SOURCE_CLASSIFICATION_PREFIX_BYTES];
    let mut filled = 0;
    while filled < prefix.len() {
        match file.read(&mut prefix[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return None,
        }
    }
    prefix.truncate(filled);
    classify(&prefix, registrations)
}

/// Classifies a source from a prefix of its bytes.
pub(crate) fn classify(prefix: &[u8], registrations: &[String]) -> Option<ExcludedSource> {
    if prefix.is_empty() {
        return None;
    }
    if opens_compressed_container(prefix) {
        return Some(ExcludedSource::Compressed);
    }
    // The same signal git uses to separate binary from text. It is a heuristic, not a proof:
    // JavaScript permits a NUL inside a comment or string, so an authored file can carry one and
    // still register steps. Every content-based exclusion therefore marks the corpus incomplete.
    // Invalid UTF-8 alone is deliberately not enough: a mis-encoded but authored file can hold
    // definitions, so it stays a read error rather than becoming a silent exclusion.
    if prefix.contains(&0) {
        return Some(ExcludedSource::Binary);
    }
    // Line geometry cannot tell a bundle apart from authored code that happens to be one very
    // long line, because the two are identical under any line statistic. Evidence of registering
    // steps is what separates them, so the geometric signal yields to it. The two signals above
    // do not yield: a NUL byte or an archive header next to a registration name says the name is
    // coincidental, not that the file is source.
    (is_minified(prefix) && !mentions_registration(prefix, registrations))
        .then_some(ExcludedSource::Minified)
}

/// Registration names that keep a file even when its geometry looks generated.
///
/// A `.` qualifier is honoured for these, because a namespace import makes `cucumber.Given(...)`
/// a registration. No such call appears anywhere in the reference bundles — qualified or not — so
/// accepting the qualifier costs no exclusion.
const QUALIFIABLE_REGISTRATIONS: [&str; 8] = [
    "Given",
    "When",
    "Then",
    "And",
    "But",
    "Step",
    "defineStep",
    "createBdd",
];

/// Lowercase registration aliases, which keep a file only through a bare call.
///
/// `.then(` is a promise continuation rather than a registration, and one bundle in the reference
/// corpus contains 66 of them. Honouring a qualifier here would keep nearly every bundle, so
/// these names are accepted only where nothing qualifies them.
const BARE_ONLY_REGISTRATIONS: [&str; 3] = ["given", "when", "then"];

/// Reports whether the prefix shows any sign of registering steps.
///
/// Scans the prefix once and looks each identifier up, rather than searching the prefix per name:
/// `registrations` is configuration and has no small bound, so a per-name search would let a
/// large configuration multiply the cost of classifying every file.
///
/// This is a lexical test, not a parse. It does not distinguish a name in code from the same
/// bytes inside a string or comment, and the asymmetry is deliberate: a name found where it does
/// not execute only keeps a file that would otherwise be skipped, costing analysis time, while a
/// call this test fails to see excludes a file and loses its definitions. Exclusion on geometry
/// therefore marks the corpus incomplete, so the losing direction cannot pass a strict gate.
fn mentions_registration(prefix: &[u8], registrations: &[String]) -> bool {
    // An import of a registration module is evidence on its own, and it is the only evidence a
    // renaming import leaves behind: `import { Given as G } from '@cucumber/cucumber'` calls
    // `G(...)`, so no registration name reaches a call site. No reference bundle mentions any of
    // these module paths.
    if registration_modules().any(|module| contains_bytes(prefix, module.as_bytes())) {
        return true;
    }
    // An import from inside the project is the only lexical evidence left when a registration
    // arrives through a local facade under a new name, as in `import { G } from './facade.js'` or
    // `import { Given as Action } from '~/facade'`, which the project resolver follows and this
    // scan otherwise cannot see. A bundle has already resolved its own imports, so it carries no
    // such specifier in import position.
    if imports_project_module(prefix) {
        return true;
    }

    let qualifiable = QUALIFIABLE_REGISTRATIONS
        .iter()
        .map(|name| name.as_bytes())
        // A configured name is an explicit declaration by the project, so it is trusted at least
        // as far as a built-in one.
        .chain(registrations.iter().map(String::as_bytes))
        .collect::<BTreeSet<_>>();
    let bare_only = BARE_ONLY_REGISTRATIONS
        .iter()
        .map(|name| name.as_bytes())
        .collect::<BTreeSet<_>>();

    identifiers(prefix).any(|identifier| {
        let name = &prefix[identifier.start..identifier.end];
        // The name is checked before the call, because a set lookup is far cheaper than scanning
        // the trivia that may follow every identifier in the prefix.
        (qualifiable.contains(&name) || (!identifier.qualified && bare_only.contains(&name)))
            && is_call_callee(prefix, identifier.end)
    })
}

/// Reports whether the prefix imports a module from inside the project.
///
/// Scans forward from each import keyword rather than backward from each string, so trivia
/// between the keyword and the specifier is skipped by the same routine the callee scan uses.
/// The import context is required rather than the bare specifier: `"./` occurs 80 times inside
/// one reference bundle's string data, while no bundle carries such a specifier in import
/// position.
fn imports_project_module(prefix: &[u8]) -> bool {
    identifiers(prefix).any(|identifier| {
        if identifier.qualified {
            return false;
        }
        let keyword = &prefix[identifier.start..identifier.end];
        if !matches!(keyword, b"from" | b"import" | b"require" | b"export") {
            return false;
        }
        let Some(mut index) = skip_trivia(prefix, identifier.end) else {
            return false;
        };
        // `require('./x')` and the dynamic `import('./x')` wrap the specifier in a call.
        if prefix.get(index) == Some(&b'(') {
            match skip_trivia(prefix, index + 1) {
                Some(next) => index = next,
                None => return false,
            }
        }
        prefix
            .get(index)
            .is_some_and(|byte| matches!(byte, b'\'' | b'"' | b'`'))
            && opens_local_specifier(&prefix[index + 1..])
    })
}

/// Specifier prefixes that name a module inside the project rather than an installed package.
///
/// `./` and `../` are relative paths; `~/` and `@/` are the usual `tsconfig` path aliases; `#`
/// introduces a package `imports` subpath. None of them appears in import position in any
/// reference bundle, while a bare package name does — `from '...'` occurs inside two bundles, and
/// once inside a jQuery error message — so only these prefixes can be treated as evidence.
///
/// A workspace package imported by its own name, such as `@myorg/steps`, is not covered: it is
/// indistinguishable from the installed packages that bundles genuinely import.
const LOCAL_SPECIFIER_PREFIXES: [&[u8]; 5] = [b"./", b"../", b"~/", b"@/", b"#"];

/// Reports whether `after` opens with a specifier naming a module inside the project.
fn opens_local_specifier(after: &[u8]) -> bool {
    LOCAL_SPECIFIER_PREFIXES
        .iter()
        .any(|prefix| after.starts_with(prefix))
}

/// Skips whitespace and comments from `at`, returning the next significant byte's index.
///
/// Returns `None` when an unterminated comment swallows the rest of the prefix, because nothing
/// significant can then follow within what was inspected.
fn skip_trivia(prefix: &[u8], at: usize) -> Option<usize> {
    let mut index = at;
    loop {
        while let Some(length) = whitespace_length(prefix, index) {
            index += length;
        }
        match prefix.get(index..index + 2) {
            Some(b"/*") => index += 2 + find_subslice(&prefix[index + 2..], b"*/")? + 2,
            Some(b"//") => {
                index += 2 + prefix[index + 2..].iter().position(|byte| *byte == b'\n')? + 1;
            }
            _ => return Some(index),
        }
    }
}

/// One identifier found in a prefix, with the byte range it occupies.
struct Identifier {
    start: usize,
    end: usize,
    /// Whether a `.` immediately precedes the identifier, making it a member access.
    qualified: bool,
}

/// Yields every identifier in `prefix`, in order.
fn identifiers(prefix: &[u8]) -> impl Iterator<Item = Identifier> + '_ {
    let mut index = 0;
    std::iter::from_fn(move || {
        while index < prefix.len() && !is_identifier_byte(prefix[index]) {
            index += 1;
        }
        if index == prefix.len() {
            return None;
        }
        let start = index;
        while index < prefix.len() && is_identifier_byte(prefix[index]) {
            index += 1;
        }
        Some(Identifier {
            start,
            end: index,
            qualified: start
                .checked_sub(1)
                .is_some_and(|before| prefix[before] == b'.'),
        })
    })
}

/// Reports whether a call opens at `from`, skipping everything transparent in between.
///
/// `typescript::registrations::unwrap_registration_callee` treats parentheses, `as`, `satisfies`,
/// non-null `!`, type assertions and instantiation `<T>` as transparent wrappers around a
/// registration callee, so `Given!('a step', handler)` and `(Given as typeof Given)('a step', h)`
/// both register. This skips the same wrappers, plus whitespace and comments, so neither
/// formatting nor type syntax decides whether an authored file is excluded. None of these
/// sequences follows a registration name anywhere in the reference bundles, so accepting them
/// costs no exclusion.
fn is_call_callee(prefix: &[u8], from: usize) -> bool {
    let mut index = from;
    loop {
        let Some(next) = skip_trivia(prefix, index) else {
            return false;
        };
        index = next;
        match prefix.get(index) {
            Some(b'(') => return true,
            // A closing parenthesis belongs to a wrapper around the callee, as in
            // `(Given)('a step', handler)`, and `!` is a non-null assertion.
            Some(b')' | b'!') => index += 1,
            // An instantiation expression, `Given<Type>('a step', handler)`.
            Some(b'<') => match skip_angle_brackets(prefix, index) {
                Some(next) => index = next,
                None => return false,
            },
            _ => match skip_type_operator(prefix, index) {
                Some(next) => index = next,
                None => return false,
            },
        }
    }
}

/// Skips a balanced `<...>` starting at `from`, returning the byte after it.
fn skip_angle_brackets(prefix: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, byte) in prefix[from..].iter().enumerate() {
        match byte {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(from + offset + 1);
                }
            }
            // A type argument list cannot span a statement, so an unbalanced `<` was a comparison.
            b';' | b'{' | b'}' => return None,
            _ => {}
        }
    }
    None
}

/// Skips an `as` or `satisfies` operator and the type that follows, returning the next byte.
///
/// Returns `None` when no such operator is present, which ends the callee scan. The type is
/// consumed by bracket depth rather than by a set of permitted bytes, so an inline object or
/// function type — `as { (pattern: string, fn: () => void): void }` — is skipped whole.
fn skip_type_operator(prefix: &[u8], from: usize) -> Option<usize> {
    let operator = [&b"as"[..], &b"satisfies"[..]].into_iter().find(|word| {
        prefix[from..].starts_with(word)
            && !prefix[from + word.len()..]
                .first()
                .is_some_and(|byte| is_identifier_byte(*byte))
    })?;

    let body = &prefix[from + operator.len()..];
    let mut depth = 0_usize;
    for (offset, byte) in body.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            // The `>` of an arrow is punctuation, not a closing bracket, so `() => void` inside a
            // type argument list must not be read as closing it.
            b'>' if offset > 0 && body[offset - 1] == b'=' => {}
            b')' | b']' | b'}' | b'>' if depth > 0 => depth -= 1,
            // At depth zero these end the type: the wrapper closing, or a boundary meaning this
            // was never a call. A `(` is not among them, because at depth zero it opens a
            // function type, as in `as (pattern: string) => void`.
            b')' | b',' | b';' if depth == 0 => return Some(from + operator.len() + offset),
            _ => {}
        }
    }
    None
}

/// Returns the offset of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty() && haystack.len() >= needle.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

/// Reports whether `needle` occurs anywhere in `haystack`.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    find_subslice(haystack, needle).is_some()
}

/// Reports whether a byte can appear inside a JavaScript identifier.
///
/// Non-ASCII bytes are excluded even though an identifier may contain them. Every name this
/// module matches is ASCII, so ending a token at a non-ASCII byte still finds `Given` beside one,
/// while treating those bytes as identifier characters would swallow the ECMAScript whitespace
/// that may legally separate `Given` from its arguments. Splitting a non-ASCII identifier is
/// harmless here: at worst it keeps a file that would otherwise be excluded.
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// Returns the length of the whitespace character at `at`, or `None` if there is not one.
///
/// ECMAScript counts more than ASCII space as whitespace — NBSP, the Unicode `Zs` category, the
/// line and paragraph separators, and the byte-order mark — and any of them may sit between a
/// registration name and its arguments.
fn whitespace_length(prefix: &[u8], at: usize) -> Option<usize> {
    let byte = *prefix.get(at)?;
    if byte.is_ascii() {
        return byte.is_ascii_whitespace().then_some(1);
    }
    // A UTF-8 character is at most four bytes, and an invalid sequence is not whitespace.
    let end = (at + 4).min(prefix.len());
    let character = std::str::from_utf8(&prefix[at..end])
        .ok()
        .and_then(|text| text.chars().next())
        .or_else(|| {
            (at + 1..end)
                .rev()
                .filter_map(|shorter| std::str::from_utf8(&prefix[at..shorter]).ok())
                .find_map(|text| text.chars().next())
        })?;
    (character.is_whitespace() || character == '\u{feff}').then(|| character.len_utf8())
}

/// Reports whether line geometry matches generated output rather than authored code.
///
/// The statistic is the mean length of non-blank lines. It separates both minification styles:
/// output collapsed onto one line, and output wrapped at a fixed width, which a longest-line test
/// misses entirely. Measured across seven real repositories, authored sources reach a mean of 104
/// bytes per line while minified files start at 506.
///
/// A prefix that stops before end of file ends mid-line, and that trailing fragment is measured
/// like any other line. Its true length is only longer, so counting it cannot invent a long line
/// where none exists, and dropping it would hide the most common minified shape of all: a short
/// license comment followed by one line holding the entire program.
///
/// The known limitation is a source file whose bytes are dominated by a single long literal, such
/// as an inlined data URI. Its geometry is indistinguishable from one-line minification, so it is
/// excluded too. No such file appeared in 2,413 real sources, and every exclusion is reported.
fn is_minified(prefix: &[u8]) -> bool {
    let mut lines = 0_usize;
    let mut bytes = 0_usize;
    for line in prefix.split(|byte| *byte == b'\n') {
        let line = match line {
            [body @ .., b'\r'] => body,
            body => body,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        lines += 1;
        bytes += line.len();
    }
    lines > 0 && bytes / lines > MINIFIED_MEAN_LINE_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typescript::frameworks::{DEFAULT_REGISTRATIONS, REGISTRATIONS};

    /// Length of a line that unambiguously clears [`MINIFIED_MEAN_LINE_BYTES`].
    ///
    /// Only the ratio to the threshold matters, so this stays kilobytes rather than the megabytes
    /// a real bundle reaches: `contains_call_to` scans every window, and megabyte fixtures made
    /// this module the slowest in the suite for no added coverage.
    const LONG_LINE_BYTES: usize = 4_000;
    use std::fs;
    use std::io::Write;

    /// Authored source at the widest geometry observed in the reference repositories.
    fn authored_source() -> Vec<u8> {
        let mut source = b"import { Given } from '@cucumber/cucumber';\n\n".to_vec();
        for index in 0..200 {
            source.extend_from_slice(
                format!(
                    "Given('step number {index}', async function () {{\n  await work();\n}});\n\n"
                )
                .as_bytes(),
            );
        }
        source
    }

    /// Minified output collapsed onto a single line, with no terminator at all.
    fn one_line_minified() -> Vec<u8> {
        format!("!function(n){{{}}}(0);", "var a=1,b=2,c=3;".repeat(500)).into_bytes()
    }

    /// Minified output wrapped at a fixed width.
    ///
    /// This is the shape a longest-line test cannot see: no single line is unusual, yet every
    /// line is far wider than authored code. `highcharts.js` in the reference corpus is exactly
    /// this, at 601 lines whose longest is only 637 bytes.
    fn wrapped_minified() -> Vec<u8> {
        let mut bytes = Vec::new();
        for _ in 0..40 {
            bytes.extend_from_slice("var a=1,b=2,c=3;".repeat(32).as_bytes());
            bytes.push(b'\n');
        }
        bytes
    }

    #[test]
    fn classifies_every_content_kind_discovery_can_deliver() {
        // Padded with ten blank lines per content line, so the verdict flips if blank lines are
        // counted: skipping them the mean stays above 500, counting them it falls below 50.
        let mut blank_padded = wrapped_minified();
        blank_padded.extend_from_slice(&vec![b'\n'; 400]);
        let mut crlf_minified = Vec::new();
        for line in wrapped_minified().split(|byte| *byte == b'\n') {
            crlf_minified.extend_from_slice(line);
            crlf_minified.extend_from_slice(b"\r\n");
        }
        let mut gzip = vec![0x1f, 0x8b, 0x08];
        gzip.extend_from_slice(b"compressed payload that is otherwise textual");
        // A single long literal that does not dominate the file: the realistic shape of an
        // inlined query or fixture inside authored code, which must survive.
        let mut long_literal = authored_source();
        long_literal
            .extend_from_slice(format!("const BLOB = '{}';\n", "x".repeat(5_000)).as_bytes());
        // Multibyte text: line length is counted in bytes, so wide characters must not push an
        // authored file over the threshold on their own.
        let mut multibyte = Vec::new();
        for _ in 0..50 {
            multibyte.extend_from_slice("// 日本語のコメントがここに書かれています\n".as_bytes());
        }

        for (label, bytes, expected) in [
            ("authored source", authored_source(), None),
            ("empty file", Vec::new(), None),
            ("long literal in authored source", long_literal, None),
            ("multibyte authored source", multibyte, None),
            (
                "one-line minified",
                one_line_minified(),
                Some(ExcludedSource::Minified),
            ),
            (
                "wrapped minified",
                wrapped_minified(),
                Some(ExcludedSource::Minified),
            ),
            (
                "minified padded with blank lines",
                blank_padded,
                Some(ExcludedSource::Minified),
            ),
            (
                "minified with CRLF endings",
                crlf_minified,
                Some(ExcludedSource::Minified),
            ),
            ("gzip container", gzip, Some(ExcludedSource::Compressed)),
            (
                "NUL bytes",
                b"var a = 1;\0\0\0binary\0".to_vec(),
                Some(ExcludedSource::Binary),
            ),
            // Invalid UTF-8 without a NUL stays unclassified on purpose. A mis-encoded but
            // authored file can hold definitions, so extraction must fail loudly on it rather
            // than have it disappear as an exclusion.
            (
                "invalid UTF-8 without NUL",
                b"const a = '\xff\xfe';\n".to_vec(),
                None,
            ),
        ] {
            assert_eq!(classify(&bytes, &[]), expected, "{label}");
        }
    }

    #[test]
    fn a_line_wider_than_the_whole_prefix_is_minified_whatever_precedes_it() {
        // The shape that defeats measuring only whole lines: a short license comment, then one
        // line holding the entire program. The prefix stops inside that line, so the only
        // terminator in it belongs to the comment. Measuring just the completed comment reads as
        // ordinary source and lets the file through, which is how `jquery-3.5.1.min.js` reached
        // the parser and cost over 130 seconds.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vendor.min.js");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(b"/*! library v1.2.3 | (c) authors | license MIT */\n")
            .unwrap();
        file.write_all(&one_line_minified()).unwrap();
        file.write_all(&vec![b'z'; SOURCE_CLASSIFICATION_PREFIX_BYTES])
            .unwrap();
        drop(file);

        assert_eq!(inspect(&path, &[]), Some(ExcludedSource::Minified));
    }

    #[test]
    fn authored_sources_longer_than_the_prefix_are_still_analyzed() {
        // The opposite-answer control for the case above: same truncated-prefix path, same
        // absence of a final terminator, but authored geometry. Without it, a rule that excluded
        // every truncated prefix would pass the test above and look correct.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("steps.ts");
        let mut bytes = authored_source();
        while bytes.len() <= SOURCE_CLASSIFICATION_PREFIX_BYTES {
            bytes.extend_from_slice(&authored_source());
        }
        bytes.extend_from_slice(b"Given('unterminated last line', () => work());");
        fs::write(&path, &bytes).unwrap();

        assert_eq!(inspect(&path, &[]), None);
    }

    #[test]
    fn an_unreadable_path_is_left_for_extraction_to_report() {
        assert_eq!(inspect(Path::new("/nonexistent/vendor.js"), &[]), None);
    }

    #[test]
    fn the_guard_vocabulary_covers_every_registration_the_analyzer_understands() {
        // The guard decides only whether to keep a file, so it must never know fewer names than
        // the analyzer resolves. A name missing here would let a file holding real definitions be
        // excluded on geometry alone.
        let guarded = QUALIFIABLE_REGISTRATIONS
            .iter()
            .chain(BARE_ONLY_REGISTRATIONS.iter());
        let guarded = guarded.collect::<Vec<_>>();
        for name in REGISTRATIONS.iter().chain(DEFAULT_REGISTRATIONS.iter()) {
            assert!(
                guarded.contains(&name),
                "registration `{name}` is not guarded"
            );
        }
    }

    #[test]
    fn authored_code_that_is_one_enormous_line_survives_on_its_registrations() {
        // Geometrically indistinguishable from a one-line bundle: a single line, megabytes wide.
        // Only the registration call separates them, so this is the case that decides whether the
        // guard exists at all.
        let authored = format!(
            "Given('{}', () => work());",
            str::repeat("a", LONG_LINE_BYTES)
        );
        assert_eq!(classify(authored.as_bytes(), &[]), None);

        // The opposite-answer control: identical geometry, no registration, so it is excluded.
        let generated = format!("!function(){{{}}}();", str::repeat("a", LONG_LINE_BYTES));
        assert_eq!(
            classify(generated.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn a_configured_registration_name_also_guards_a_file() {
        let source = format!(
            "myStep('{}', () => work());",
            str::repeat("a", LONG_LINE_BYTES)
        );
        assert_eq!(
            classify(source.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
        assert_eq!(classify(source.as_bytes(), &["myStep".to_owned()]), None);
    }

    #[test]
    fn a_registration_name_inside_a_longer_identifier_does_not_guard_a_bundle() {
        // `promisedPatchThen(` and `splitWhen(` both occur in the reference bundles. Reading them
        // as registrations would keep every bundle and defeat the exclusion.
        for callee in ["promisedPatchThen", "splitWhen", "myGiven", "define$Step"] {
            let bundle = format!(
                "!function(){{{}({})}}();",
                callee,
                "a,".repeat(LONG_LINE_BYTES / 2)
            );
            assert_eq!(
                classify(bundle.as_bytes(), &[]),
                Some(ExcludedSource::Minified),
                "{callee}"
            );
        }
    }

    #[test]
    fn a_namespace_qualified_registration_guards_a_file_but_a_promise_chain_does_not() {
        // Both are a capitalized or lowercase name behind a `.`, and only the case distinguishes
        // them. `cucumber.Given(...)` is a registration reached through a namespace import, while
        // `.then(...)` is a promise continuation that appears 66 times in one reference bundle —
        // so honouring a qualifier for the lowercase aliases would keep nearly every bundle.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        let qualified = format!("{filler}cucumber.Given('a step', () => work());{filler}");
        assert_eq!(classify(qualified.as_bytes(), &[]), None);

        let chained = format!("{filler}p.then(function(){{return 1}});{filler}");
        assert_eq!(
            classify(chained.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn a_bare_lowercase_alias_guards_a_file() {
        // `then('a step', fn)` is a registration the analyzer resolves, so geometry must not
        // discard it. The control above pins that the qualified form still does not guard.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        let aliased = format!("{filler}then('a step', () => work());{filler}");
        assert_eq!(classify(aliased.as_bytes(), &[]), None);
    }

    #[test]
    fn a_renaming_import_guards_a_file_through_its_module_path() {
        // The one shape that leaves no registration name at a call site: the import renames it,
        // so the file calls `G(...)`. Only the module path identifies it as a step source.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        let renamed = format!(
            "import {{ Given as G }} from '@cucumber/cucumber';{filler}G('a step', () => work());{filler}"
        );
        assert_eq!(classify(renamed.as_bytes(), &[]), None);

        // The opposite-answer control: same geometry and the same lone-identifier call, but no
        // module path and no registration name, so it is excluded.
        let anonymous = format!("{filler}G('a step', () => work());{filler}");
        assert_eq!(
            classify(anonymous.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn a_project_facade_import_guards_a_file_whatever_the_callee_form() {
        // Extraction resolves a registration re-exported by a local module, so the callee can be
        // any name and any supported syntax. Rather than enumerate those forms, the relative
        // import is taken as evidence that the file is an authored module.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        for importer in [
            "import { G } from './facade.js';",
            "import { G } from '../steps/facade';",
            "export { G } from \"./facade\";",
            "const { G } = require('./facade');",
            "const { G } = await import('./facade');",
            "import './facade';",
            // Trivia between the keyword and the specifier, in both comment forms.
            "import { G } from /* facade */ './facade';",
            "import { G } from // facade\n './facade';",
            "export { G } from './facade';",
            // Project aliases the resolver follows: `tsconfig` paths and package `imports`.
            "import { Given as Action } from '~/facade';",
            "import { Given as Action } from '@/steps/facade';",
            "import { Given as Action } from '#steps/facade';",
        ] {
            let source = format!("{importer}{filler}G<string>('a step', () => work());{filler}");
            assert_eq!(classify(source.as_bytes(), &[]), None, "{importer}");
        }
    }

    #[test]
    fn a_project_specifier_outside_import_position_does_not_guard_a_file() {
        // The opposite-answer control, and the reason the import context is required: one
        // reference bundle contains 80 occurrences of `"./` inside its string data, and treating
        // those as imports would keep the bundle and defeat the exclusion.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        for bundled in [
            "var p = \"./assets/icon.png\";",
            "t.resolve(\"./chunk\");",
            "var base = '../';",
            // A bare package name is not evidence: two reference bundles import packages, and
            // jQuery contains `from '` inside an error message.
            "import x from 'lodash';",
            "var m = \"No conversion from \" + u;",
        ] {
            let source = format!("{filler}{bundled}{filler}");
            assert_eq!(
                classify(source.as_bytes(), &[]),
                Some(ExcludedSource::Minified),
                "{bundled}"
            );
        }
    }

    #[test]
    fn every_registered_framework_module_guards_a_file() {
        // Driven from the framework registry so a newly supported framework cannot be left out.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        for module in crate::typescript::frameworks::registration_modules() {
            let source = format!("import {{ X }} from '{module}';{filler}X('s', () => work());");
            assert_eq!(classify(source.as_bytes(), &[]), None, "{module}");
        }
    }

    #[test]
    fn comment_trivia_between_the_name_and_the_call_still_guards_a_file() {
        // `Given /* ... */ ('a step', handler)` is a call, so formatting must not decide whether
        // an authored file is excluded.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        for spaced in [
            "Given/* matcher below */('a step', () => work());",
            "Given // matcher below\n('a step', () => work());",
            "Given\n  ('a step', () => work());",
        ] {
            let source = format!("{filler}{spaced}{filler}");
            assert_eq!(classify(source.as_bytes(), &[]), None, "{spaced}");
        }

        // The opposite-answer control: a name that never reaches a call is not evidence, or every
        // bundle mentioning `Step` in passing would be kept.
        let mentioned = format!("{filler}var Given = 1, When = 2, Step = 3;{filler}");
        assert_eq!(
            classify(mentioned.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn every_transparent_callee_wrapper_still_guards_a_file() {
        // `(Given)('a step', handler)` extracts two definitions in the CLI, so geometry must not
        // discard the same form. The name is followed by `)` rather than `(`.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        for wrapped in [
            "(Given)('a step', () => work());",
            "((Given))('a step', () => work());",
            "(Given) /* wrapped */ ('a step', () => work());",
            // The transparent callee wrappers `unwrap_registration_callee` accepts.
            "Given!('a step', () => work());",
            "Given<string>('a step', () => work());",
            "Given<Map<string, number>>('a step', () => work());",
            "(Given as typeof Given)('a step', () => work());",
            "(Given satisfies Registration)('a step', () => work());",
            "(Given!)('a step', () => work());",
            // Casts whose type contains the very bytes a call is recognized by: braces, nested
            // parentheses, colons and commas.
            "(Given as { (pattern: string, fn: () => void): void })('a step', () => work());",
            "(Given as (pattern: string) => void)('a step', () => work());",
            "(Given as Record<string, () => void>)('a step', () => work());",
            // An arrow before a comma: if the `>` of `=>` were counted as closing the type
            // argument list, the following comma would read as the end of the cast.
            "(Given as Map<() => void, string>)('a step', () => work());",
        ] {
            let source = format!("{filler}{wrapped}{filler}");
            assert_eq!(classify(source.as_bytes(), &[]), None, "{wrapped}");
        }

        // The opposite-answer control: closing parentheses are trivia only on the way to a call,
        // so a name that merely appears inside one is not evidence.
        // An unbalanced `<` is a comparison, not a type argument list. The `>` and `(` further on
        // belong to later statements, so a scan that ran past the statement boundary would pair
        // them with this `<` and read a call that is not there.
        let compared = format!("{filler}if(Given<b){{c()}};var d=e>(f);{filler}");
        assert_eq!(
            classify(compared.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );

        let argument = format!("{filler}register(Given);{filler}");
        assert_eq!(
            classify(argument.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn a_bzip2_header_needs_its_whole_signature() {
        // `BZh`, `BZh0` and `BZh9` are all legal JavaScript identifiers, so an authored file may
        // open with any of them. Only the full stream signature separates an archive from one.
        assert_eq!(
            classify(b"BZh91AY&SY\x00payload", &[]),
            Some(ExcludedSource::Compressed)
        );
        for authored in [
            "BZh = 1;\nGiven('a step', () => work());\n",
            "BZh0 = 1;\nGiven('a step', () => work());\n",
            "BZh9 = 1;\nGiven('a step', () => work());\n",
            "BZh91AYNOT = 1;\nGiven('a step', () => work());\n",
            // Block sizes run 1 to 9, so `BZh0` is not a stream header however it continues.
            "BZh01AY&SY = 1;\nGiven('a step', () => work());\n",
        ] {
            assert_eq!(classify(authored.as_bytes(), &[]), None, "{authored}");
        }
    }

    #[test]
    fn ecmascript_whitespace_beyond_ascii_still_guards_a_file() {
        // NBSP, the Unicode space separators, the line and paragraph separators and the byte-order
        // mark are all legal between a callee and its arguments, so none of them may hide a call.
        let filler = "var a=1,b=2,c=3;".repeat(500);
        for space in [
            '\u{00a0}', '\u{2003}', '\u{2028}', '\u{2029}', '\u{3000}', '\u{feff}',
        ] {
            let source = format!("{filler}Given{space}('a step', () => work());{filler}");
            assert_eq!(
                classify(source.as_bytes(), &[]),
                None,
                "U+{:04X}",
                space as u32
            );
        }

        // The opposite-answer control: non-ASCII bytes that are not whitespace do not manufacture
        // a call, so a bundle carrying non-ASCII text is still excluded.
        let translated = format!("{filler}var 日本語={{}};var t=\"ステップ\";{filler}");
        assert_eq!(
            classify(translated.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn an_unterminated_comment_does_not_run_past_the_prefix() {
        let filler = "var a=1,b=2,c=3;".repeat(500);
        let source = format!("{filler}Given/* never closed{filler}");
        assert_eq!(
            classify(source.as_bytes(), &[]),
            Some(ExcludedSource::Minified)
        );
    }

    #[test]
    fn a_large_registration_configuration_does_not_multiply_classification_cost() {
        // `registrations` is configuration with no small bound, so searching the prefix once per
        // configured name made one file cost seconds. The prefix is scanned once instead, and the
        // bound below is far above that but far below the per-name cost it replaced.
        let prefix = "var a=1,b=2,c=3;"
            .repeat(SOURCE_CLASSIFICATION_PREFIX_BYTES / 16)
            .into_bytes();
        let names = (0..20_000)
            .map(|index| format!("absentRegistration{index}"))
            .collect::<Vec<_>>();

        let started = std::time::Instant::now();
        let verdict = classify(&prefix, &names);
        let elapsed = started.elapsed();

        assert_eq!(verdict, Some(ExcludedSource::Minified));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "classification took {elapsed:?} with {} configured names",
            names.len()
        );
    }

    #[test]
    fn binary_and_compressed_content_is_excluded_even_when_it_mentions_a_registration() {
        // The guard is a concession to a heuristic. Binary and compressed content are not
        // heuristics, so a registration name appearing in their bytes must not rescue them.
        let mut binary = b"Given('step', () => work());".to_vec();
        binary.push(0);
        assert_eq!(classify(&binary, &[]), Some(ExcludedSource::Binary));

        let mut compressed = vec![0x1f, 0x8b, 0x08];
        compressed.extend_from_slice(b"Given('step', () => work());");
        assert_eq!(classify(&compressed, &[]), Some(ExcludedSource::Compressed));
    }
}
