//! SARIF `artifactLocation.uri` contract: RFC 3986 references that decode to the exact path.

use super::super::sarif_location;
use crate::model::SourceLocation;
use crate::resource_limits::MAX_REPORT_FIELD_BYTES;
use crate::tests::property_runner;
use proptest::prelude::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};

/// Emitted URI for one location at `path`.
fn uri(path: &Path, root: &Path) -> String {
    let location = SourceLocation::new(path, 1, 1, 1, 2);
    sarif_location(&location, root, None)["physicalLocation"]["artifactLocation"]["uri"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// RFC 3986 unreserved characters; everything else in a segment must be percent-encoded.
fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// Independent oracle: decodes one segment, rejecting anything outside the emitted alphabet.
fn decode_segment(segment: &str) -> Result<Vec<u8>, String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = bytes
                    .get(index + 1..index + 3)
                    .filter(|hex| hex.iter().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F')))
                    .ok_or_else(|| format!("bad escape at {index} in {segment:?}"))?;
                decoded.push(u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap());
                index += 3;
            }
            byte if unreserved(byte) => {
                decoded.push(byte);
                index += 1;
            }
            byte => return Err(format!("raw byte {byte:#04x} in {segment:?}")),
        }
    }
    Ok(decoded)
}

/// Decodes a relative reference into segment bytes; rejects absolute or empty-segment forms.
fn decode_relative(uri: &str) -> Result<Vec<Vec<u8>>, String> {
    if uri.starts_with('/') || uri.contains("//") {
        return Err(format!("not a relative path reference: {uri:?}"));
    }
    uri.split('/').map(decode_segment).collect()
}

/// Native bytes of one path component as the filesystem stores them.
fn component_bytes(component: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        component.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        component.to_str().unwrap().as_bytes().to_vec()
    }
}

/// Asserts the URI is a valid relative reference whose decoded segments are the path components.
fn assert_round_trip(relative: &Path) -> Result<(), TestCaseError> {
    let root = root();
    let emitted = uri(&root.join(relative), &root);
    let decoded = decode_relative(&emitted).map_err(TestCaseError::fail)?;
    let expected = relative
        .components()
        .map(|component| component_bytes(component.as_os_str()))
        .collect::<Vec<_>>();
    prop_assert_eq!(decoded, expected, "uri {:?} for {:?}", emitted, relative);
    Ok(())
}

/// Absolute analysis root in the host platform's syntax.
fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\repo")
    } else {
        PathBuf::from("/repo")
    }
}

/// Characters a real file name on this platform may contain, grouped by stratum.
fn strata() -> Vec<(&'static str, Vec<char>)> {
    let mut strata = vec![
        ("unreserved", "aZ09-._~".chars().collect::<Vec<_>>()),
        ("gen-delim", "#?[]@".chars().collect()),
        ("sub-delim", "!$&'()*+,;=".chars().collect()),
        ("percent", vec!['%']),
        ("space", vec![' ']),
        ("control", vec!['\u{1}', '\u{1b}', '\u{7f}']),
        ("bidi", vec!['\u{202e}', '\u{200b}', '\u{feff}']),
        ("non-ascii", vec!['é', 'ß', '日', '🙂']),
    ];
    if cfg!(unix) {
        strata.push((
            "unix-only",
            vec![':', '\\', '"', '*', '<', '>', '|', '\n', '\t'],
        ));
    }
    strata
}

/// Generated root-relative paths keep exact identity through encoding and decoding.
#[test]
fn generated_relative_uris_decode_to_the_exact_path() {
    let strata = strata();
    let alphabet = strata
        .iter()
        .flat_map(|(_, chars)| chars.iter().copied())
        .collect::<Vec<_>>();
    let seen = strata.iter().map(|_| Cell::new(0usize)).collect::<Vec<_>>();
    let unreserved_paths = Cell::new(0usize);
    let segment = proptest::collection::vec(proptest::sample::select(alphabet), 1..=6)
        .prop_map(|chars| chars.into_iter().collect::<String>())
        .prop_filter("dot segments are not file names", |name| {
            name != "." && name != ".."
        });
    property_runner::generated(proptest::collection::vec(segment, 1..=4), |segments| {
        for ((_, chars), count) in strata.iter().zip(&seen) {
            if segments
                .iter()
                .any(|segment| segment.chars().any(|c| chars.contains(&c)))
            {
                count.set(count.get() + 1);
            }
        }
        let relative = segments.iter().collect::<PathBuf>();
        assert_round_trip(&relative)?;
        // Backward compatibility: paths already made of unreserved bytes are emitted unchanged.
        if segments
            .iter()
            .all(|segment| segment.bytes().all(unreserved))
        {
            unreserved_paths.set(unreserved_paths.get() + 1);
            let root = root();
            prop_assert_eq!(uri(&root.join(&relative), &root), segments.join("/"));
        }
        Ok(())
    });
    for ((name, _), count) in strata.iter().zip(&seen) {
        assert!(count.get() > 0, "stratum {name} never generated");
    }
    assert!(
        unreserved_paths.get() > 0,
        "no unreserved-only path generated"
    );
}

/// Minimized cases from the base reproduction, each beside a plain control.
#[test]
fn reserved_characters_are_encoded_in_named_cases() {
    let root = root();
    let mut cases = vec![
        ("steps/a.ts", "steps/a.ts"),
        ("dir #1/a?b c.ts", "dir%20%231/a%3Fb%20c.ts"),
        ("sub%41/a.ts", "sub%2541/a.ts"),
        ("café/日.ts", "caf%C3%A9/%E6%97%A5.ts"),
        ("a\u{202e}b.ts", "a%E2%80%AEb.ts"),
        ("@scope/a+b.ts", "%40scope/a%2Bb.ts"),
        ("../up/a b.ts", "../up/a%20b.ts"),
    ];
    if cfg!(unix) {
        cases.push(("x:y/z.ts", "x%3Ay/z.ts"));
        cases.push((r"x\z.ts", "x%5Cz.ts"));
    }
    for (relative, expected) in cases {
        assert_eq!(uri(&root.join(relative), &root), expected, "{relative:?}");
    }
}

/// Non-UTF-8 Unix names keep their raw bytes instead of a lossy replacement character.
#[cfg(unix)]
#[test]
fn non_utf8_unix_names_encode_raw_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let root = root();
    let name = std::ffi::OsStr::from_bytes(b"a\xffb.ts");
    assert_eq!(uri(&root.join(name), &root), "a%FFb.ts");
}

/// Oversized paths keep the bounded sample marker and stay a valid encoded reference.
#[test]
fn oversized_paths_stay_bounded_and_valid() {
    let root = root();
    let relative = format!("{}/a.ts", "é".repeat(MAX_REPORT_FIELD_BYTES));
    let emitted = uri(&root.join(&relative), &root);
    assert!(decode_relative(&emitted).is_ok(), "{emitted:?}");
    assert!(
        emitted.len() <= 3 * MAX_REPORT_FIELD_BYTES,
        "{}",
        emitted.len()
    );
    // Native length 2 * limit + 5 minus the kept `limit - 64` bytes: every omitted byte is counted.
    let omitted = 2 * MAX_REPORT_FIELD_BYTES + 5 - (MAX_REPORT_FIELD_BYTES - 64);
    assert!(
        emitted.ends_with(&format!("%E2%80%A6%20%5B{omitted}%20bytes%20omitted%5D")),
        "{}",
        &emitted[emitted.len().saturating_sub(80)..]
    );
}

/// The cut lands on native bytes: a split UTF-8 character is dropped whole and the marker counts
/// every source byte past the cut.
#[test]
fn oversized_paths_cut_before_a_split_character() {
    let root = root();
    // `a` shifts every `é` by one byte, so the `limit - 64` cut falls inside a character.
    let relative = format!("a{}", "é".repeat(MAX_REPORT_FIELD_BYTES));
    let emitted = uri(&root.join(&relative), &root);
    let kept = MAX_REPORT_FIELD_BYTES - 65;
    let expected = format!(
        "a{}%E2%80%A6%20%5B{}%20bytes%20omitted%5D",
        "%C3%A9".repeat(kept / 2),
        relative.len() - kept
    );
    assert_eq!(emitted, expected);
}

/// Oversized non-UTF-8 names keep their raw bytes; lone continuation bytes never move the cut.
#[cfg(unix)]
#[test]
fn oversized_non_utf8_names_sample_native_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let root = root();
    let kept = MAX_REPORT_FIELD_BYTES - 64;
    for byte in [0xFF_u8, 0x80] {
        let name = vec![byte; 9000];
        let emitted = uri(&root.join(std::ffi::OsStr::from_bytes(&name)), &root);
        let expected = format!(
            "{}%E2%80%A6%20%5B{}%20bytes%20omitted%5D",
            format!("%{byte:02X}").repeat(kept),
            9000 - kept
        );
        assert_eq!(emitted, expected, "byte {byte:#04x}");
    }
}

/// Paths outside the root become RFC 8089 `file:` URIs rather than scheme-ambiguous text.
#[test]
fn paths_outside_the_root_become_file_uris() {
    let root = root();
    let cases: &[(&str, &str)] = if cfg!(windows) {
        &[
            (r"D:\out side\a#.ts", "file:///D:/out%20side/a%23.ts"),
            (r"\\server\share\a.ts", "file://server/share/a.ts"),
            (r"\\?\D:\out\a.ts", "file:///D:/out/a.ts"),
            (r"\\?\UNC\server\share\a.ts", "file://server/share/a.ts"),
        ]
    } else {
        &[
            ("/out side/a#.ts", "file:///out%20side/a%23.ts"),
            ("/out/x:y.ts", "file:///out/x%3Ay.ts"),
        ]
    };
    for (path, expected) in cases {
        assert_eq!(uri(Path::new(path), &root), *expected, "{path:?}");
    }
}
