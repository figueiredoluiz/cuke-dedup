use super::*;

/// `v` enables Unicode set notation and changes character-class semantics, so it belongs to a
/// matcher's identity exactly as `u` does — more so, since `v` implies `u`. Dropping it made
/// `/x/v` normalize to the text of `/x/` and reported the two as equivalent.
///
/// The non-semantic flags are pinned alongside it, so widening the set cannot pass this test by
/// making every flag significant.
#[test]
fn unicode_sets_flag_belongs_to_matcher_identity() {
    let normalized = |flags: &str| {
        crate::typescript::semantics::default_policy().normalized_matcher(
            MatcherKind::RegularExpression,
            "^the gauge$",
            flags,
            None,
        )
    };

    // The defect: these three were indistinguishable from the unflagged spelling.
    for flags in ["v", "vi", "uv"] {
        assert_ne!(
            normalized(flags),
            normalized(""),
            "`/x/{flags}` must not share an identity with the unflagged matcher"
        );
    }
    assert_ne!(
        normalized("v"),
        normalized("u"),
        "`v` is not `u`: it changes character-class semantics"
    );
    assert_ne!(normalized("vi"), normalized("i"), "`v` survives beside `i`");

    // Execution-only flags stay stripped, so the set was widened rather than abandoned.
    for flags in ["g", "y", "d", "gy"] {
        assert_eq!(
            normalized(flags),
            normalized(""),
            "`{flags}` does not change what a matcher matches and must stay non-semantic"
        );
    }

    // `v` is a flag, not an opaque marker: two matchers carrying it still share an identity.
    assert_eq!(normalized("v"), normalized("v"));
    assert_eq!(
        normalized("iv"),
        normalized("vi"),
        "flag order is not identity"
    );

    // A `v` matcher is still unsupported for compilation, which this must not change.
    assert_eq!(rust_regex_expression("[a&&b]", "v"), None);
}

/// Verify JavaScript string decoding accepts supported escapes and rejects malformed literals.
#[test]
fn javascript_string_decoding_covers_escape_forms_and_rejections() {
    assert_eq!(
        decode_js_string(r#""\n\r\t\b\f\v\0\"\\\x41\u0042\u{43}z""#).unwrap(),
        "\n\r\t\u{0008}\u{000c}\u{000b}\0\"\\ABCz"
    );
    assert_eq!(
        decode_js_string("'line\\\njoin'"),
        Some("linejoin".to_owned())
    );
    assert_eq!(
        decode_js_string("'line\\\r\njoin'"),
        Some("linejoin".to_owned())
    );
    assert_eq!(decode_js_string(r#"'\q'"#), Some("q".to_owned()));
    for invalid in [
        "",
        "plain",
        "'unterminated",
        r#"'\xG0'"#,
        r#"'\u{}'"#,
        r#"'\u{110000}'"#,
    ] {
        assert!(decode_js_string(invalid).is_none(), "{invalid:?}");
    }
}

/// Verify the JavaScript translation keeps only the flags Rust shares and refuses `v`.
#[test]
fn translation_keeps_shared_flags_and_refuses_unicode_sets() {
    assert_eq!(
        rust_regex_expression("^value$", "gymi").as_deref(),
        Some("(?mi:^value$)")
    );
    assert_eq!(
        rust_regex_expression("^value$", "gy").as_deref(),
        Some("^value$")
    );
    // A pattern that begins like a flag group is a body, not a wrapper, when no flag applies.
    assert_eq!(
        translate_regex("(?i:a)", ""),
        Some(TranslatedRegex {
            inline_flags: String::new(),
            body: "(?i:a)".to_owned()
        })
    );
    assert_eq!(rust_regex_expression("[a&&b]", "v"), None);
    assert_eq!(rust_regex_support("^value$", ""), RegexSupport::Supported);
    assert_eq!(rust_regex_support("(", ""), RegexSupport::Unsupported);
    assert_eq!(rust_regex_support("[a&&b]", "v"), RegexSupport::Unsupported);
}
