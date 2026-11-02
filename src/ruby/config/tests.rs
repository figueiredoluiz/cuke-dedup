use super::*;

/// `entries` through the hooks: each value read by `read_key` from its own deserializer, then
/// `apply_keys`. Errors drop their position, which only a whole-document reader can give.
fn through_hooks(entries: &[(&str, &str)]) -> std::result::Result<Map<String, Value>, String> {
    let mut keys = Map::new();
    for (key, value) in entries {
        let mut json = serde_json::Deserializer::from_str(value);
        read_key(
            key,
            &mut <dyn erased_serde::Deserializer>::erase(&mut json),
            &mut keys,
        )
        .map_err(|error| without_position(&error.to_string()))?;
    }
    apply_keys(keys).map_err(|error| format!("{error:#}"))
}

/// `entries` as one object through today's typed reader and apply step.
fn through_reader(entries: &[(&str, &str)]) -> std::result::Result<Map<String, Value>, String> {
    let fields: Vec<_> = entries
        .iter()
        .map(|(key, value)| format!("\"{key}\":{value}"))
        .collect();
    let raw: RawRubyConfig = serde_json::from_str(&format!("{{{}}}", fields.join(",")))
        .map_err(|error| without_position(&error.to_string()))?;
    let mut config = RubyConfig::default();
    config
        .apply_raw(raw)
        .map_err(|error| format!("{error:#}"))?;
    Ok(serde_json::to_value(config)
        .unwrap()
        .as_object()
        .cloned()
        .unwrap())
}

/// `message` without serde_json's trailing ` at line L column C`.
fn without_position(message: &str) -> String {
    message.split(" at line ").next().unwrap().to_owned()
}

/// The Ruby hooks read and apply every Ruby key exactly as today's reader does, error text
/// included; each row's expected answer is pinned too, so a shared regression cannot pass.
#[test]
fn ruby_hooks_read_and_apply_keys_as_the_reader_does() {
    let paths = |paths: &[&str]| -> std::result::Result<Map<String, Value>, String> {
        Ok(if paths.is_empty() {
            Map::new()
        } else {
            Map::from_iter([("rubyLoadPaths".to_owned(), serde_json::json!(paths))])
        })
    };
    let rejects = |text: &str| Err::<Map<String, Value>, String>(text.to_owned());
    let both = "set Ruby load paths with `ruby.loadPaths` or `rubyLoadPaths`, not both";
    for (entries, expected) in [
        (&[("ruby", r#"{"loadPaths":["a"]}"#)][..], paths(&["a"])),
        (&[("ruby", r#"{"loadPaths":null}"#)][..], paths(&[])),
        (&[("ruby", "null")][..], paths(&[])),
        (&[("ruby", "{}")][..], paths(&[])),
        (
            &[("ruby", r#"{"other":1}"#)][..],
            rejects("unknown field `other`, expected `loadPaths`"),
        ),
        (
            &[("ruby", "42")][..],
            rejects("invalid type: integer `42`, expected struct RawRubySection"),
        ),
        (
            &[("ruby", r#"{"loadPaths":"a"}"#)][..],
            rejects("invalid type: string \"a\", expected a sequence"),
        ),
        (
            &[("ruby", r#"{"loadPaths":["a"],"loadPaths":["b"]}"#)][..],
            rejects("duplicate field `loadPaths`"),
        ),
        (&[("rubyLoadPaths", r#"["a"]"#)][..], paths(&["a"])),
        (&[("rubyLoadPaths", "null")][..], paths(&[])),
        (&[("rubyLoadPaths", "[]")][..], paths(&[])),
        (
            &[("rubyLoadPaths", "42")][..],
            rejects("invalid type: integer `42`, expected a sequence"),
        ),
        (
            &[
                ("ruby", r#"{"loadPaths":["a"]}"#),
                ("rubyLoadPaths", r#"["b"]"#),
            ][..],
            rejects(both),
        ),
        (
            &[("ruby", r#"{"loadPaths":[]}"#), ("rubyLoadPaths", "[]")][..],
            rejects(both),
        ),
        (
            &[("ruby", "null"), ("rubyLoadPaths", r#"["b"]"#)][..],
            paths(&["b"]),
        ),
    ] {
        assert_eq!(through_reader(entries), expected, "reader: {entries:?}");
        assert_eq!(through_hooks(entries), expected, "hooks: {entries:?}");
    }
}

/// A key Ruby does not own is refused rather than read as load paths.
#[test]
fn ruby_hooks_refuse_a_key_ruby_does_not_own() {
    let mut json = serde_json::Deserializer::from_str(r#"["a"]"#);
    let mut keys = Map::new();
    let error = read_key(
        "rubyLoadpath",
        &mut <dyn erased_serde::Deserializer>::erase(&mut json),
        &mut keys,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "`rubyLoadpath` is not a Ruby configuration key"
    );
    assert!(keys.is_empty());
}
