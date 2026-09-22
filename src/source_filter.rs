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
use std::collections::{BTreeMap, BTreeSet};
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
            // A read that fails partway leaves classification undecided. Extraction reports the
            // same failure with full context, so it is not duplicated here.
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

    let angles = angle_bracket_pairs(prefix);
    identifiers(prefix).any(|identifier| {
        let name = &prefix[identifier.start..identifier.end];
        // The name is checked before the call, because a set lookup is far cheaper than scanning
        // the trivia that may follow every identifier in the prefix.
        (qualifiable.contains(&name) || (!identifier.qualified && bare_only.contains(&name)))
            && is_call_callee(prefix, identifier.end, &angles)
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
fn is_call_callee(prefix: &[u8], from: usize, angles: &BTreeMap<usize, usize>) -> bool {
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
            Some(b'<') => match angles.get(&index) {
                Some(close) => index = close + 1,
                None => return false,
            },
            _ => match skip_type_operator(prefix, index) {
                Some(next) => index = next,
                None => return false,
            },
        }
    }
}

/// Pairs every `<` in the prefix with its closing `>`, in one pass.
///
/// Scanning forward from each `<` separately is quadratic: a prefix of repeated `Given<` gives one
/// unbalanced scan per occurrence, each running to the end. Pairing with a stack answers every
/// query from a single pass and keeps the same semantics — the innermost unclosed `<` matches, a
/// type argument list cannot span a statement, and the `>` of an arrow is punctuation. Capping the
/// scan length instead would stop recognizing a long but valid type argument list, and so could
/// exclude an authored file.
fn angle_bracket_pairs(prefix: &[u8]) -> BTreeMap<usize, usize> {
    let mut unclosed = Vec::new();
    let mut pairs = BTreeMap::new();
    // One entry per open brace, recording whether a `:` has appeared inside it.
    let mut braces: Vec<bool> = Vec::new();
    for (index, byte) in prefix.iter().enumerate() {
        match byte {
            b'<' => unclosed.push(index),
            b'>' if index > 0 && prefix[index - 1] == b'=' => {}
            b'>' => {
                if let Some(start) = unclosed.pop() {
                    pairs.insert(start, index);
                }
            }
            // A brace is not itself a boundary: `Given<{ value: string }>(...)` puts an object
            // type inside the type argument list, so treating `{` as the end of a statement would
            // abandon a list that does close.
            b'{' => braces.push(false),
            b'}' => {
                braces.pop();
            }
            b':' => {
                if let Some(brace) = braces.last_mut() {
                    *brace = true;
                }
            }
            // An unbalanced `<` before a statement boundary was a comparison, not a type argument
            // list, so everything still open is abandoned.
            //
            // A `;` inside braces is only exempt when those braces can be an object type, whose
            // members are all `name: type`. Without a `:` the braces are a block — the body of a
            // `class` or `function` expression — and `Given < class { field; } > ('x')` is a
            // comparison chain, not a registration. A `:` from something else, such as a ternary
            // or a label, only keeps a file that would otherwise be excluded.
            b';' if !braces.last().copied().unwrap_or(false) => unclosed.clear(),
            _ => {}
        }
    }
    pairs
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
    // Take the character's length from its lead byte and decode exactly that, so a character cut
    // off by the end of the prefix, a continuation byte or an invalid lead all fall out as "not
    // whitespace" without a second decoding attempt.
    let length = match byte {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let text = std::str::from_utf8(prefix.get(at..at + length)?).ok()?;
    (text.starts_with(char::is_whitespace) || text.starts_with('\u{feff}')).then_some(length)
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
mod tests;
