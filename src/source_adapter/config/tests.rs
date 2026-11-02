use super::*;

/// An adapter claiming `level`, whose section defaults it to 1.
struct Defaulted;

impl SourceAdapter for Defaulted {
    /// Returns the adapter name.
    fn name(&self) -> &'static str {
        "defaulted"
    }
    /// Returns the adapter name as its language.
    fn language(&self) -> super::super::SourceLanguage {
        super::super::SourceLanguage::new("defaulted")
    }
    /// Extracts nothing.
    fn extract(&self, _: &str, _: &super::super::SourceFile) -> Result<super::super::Extraction> {
        Ok(super::super::Extraction::new(Vec::new(), Vec::new()))
    }
    /// Compares in its own domain.
    fn analysis_policy(&self) -> super::super::AnalysisPolicy {
        crate::ruby::semantics::policy()
    }
    /// Claims `level`.
    fn config_keys(&self) -> &'static [&'static str] {
        &["level"]
    }
    /// Defaults `level` to 1; a `level` of 0 also returns `threshold`, which it does not claim.
    fn apply_config(&self, mut keys: ConfigKeys) -> Result<ConfigKeys> {
        if keys.get("level") == Some(&serde_json::json!(0)) {
            keys.insert("threshold".to_owned(), serde_json::json!(1));
        }
        keys.entry("level").or_insert(serde_json::json!(1));
        Ok(keys)
    }
}

static DEFAULTED: Defaulted = Defaulted;

/// `text` read as the adapter keys of one source; `scope` claims them, else no scope is set.
fn read(text: &str, scope: Option<&AdapterRegistry>) -> std::result::Result<AdapterKeys, String> {
    let parse = || serde_json::from_str::<AdapterKeys>(text).map_err(|error| error.to_string());
    match scope {
        Some(adapters) => reading_with(adapters, parse),
        None => parse(),
    }
}

/// Outside a scope no registry claims keys, so reading fails; read directly, not flattened, an
/// unclaimed key is refused and a repeated one is a duplicate field.
#[test]
fn adapter_keys_read_claimed_keys_and_refuse_others() {
    let builtin = AdapterRegistry::builtin();
    assert_eq!(
        read(r#"{"rubyLoadPaths":["a"]}"#, None).unwrap_err(),
        "adapter configuration keys read with no adapter registry in scope"
    );
    let mut keys = read(r#"{"rubyLoadPaths":["a"],"ruby":null}"#, Some(builtin)).unwrap();
    let ruby = keys.take("cucumber-ruby");
    assert_eq!(ruby.keys().collect::<Vec<_>>(), ["rubyLoadPaths", "ruby"]);
    assert!(keys.take("cucumber-ruby").is_empty());
    assert_eq!(
        read(r#"{"other":1}"#, Some(builtin)).unwrap_err(),
        "unknown field `other`, expected `ruby` or `rubyLoadPaths` at line 1 column 8"
    );
    assert_eq!(
        read(r#"{"ruby":null,"ruby":null}"#, Some(builtin)).unwrap_err(),
        "duplicate field `ruby` at line 1 column 19"
    );
    assert_eq!(
        read("1", Some(builtin)).unwrap_err(),
        "invalid type: integer `1`, expected adapter configuration keys at line 1 column 1"
    );
    // A registry whose adapters claim nothing has no adapter keys.
    let none = AdapterRegistry::new([]).unwrap();
    assert_eq!(none.config_keys(), [""; 0]);
    assert!(read(r#"{"ruby":null}"#, Some(&none)).is_err());
}

/// Every claiming adapter applies, the ones a source left out with no keys, and the sections
/// serialize as their entries in registry order.
#[test]
fn adapter_sections_apply_every_claimant_and_print_entries() {
    let builtin = AdapterRegistry::builtin();
    let mut sections = AdapterSections::default();
    assert_eq!(sections.get("cucumber-ruby"), None);
    sections.apply(builtin, AdapterKeys::default()).unwrap();
    assert_eq!(sections.get("cucumber-ruby"), Some(&ConfigKeys::new()));
    let keys = read(r#"{"rubyLoadPaths":["a"]}"#, Some(builtin)).unwrap();
    sections.apply(builtin, keys).unwrap();
    assert_eq!(
        serde_json::to_string(&sections).unwrap(),
        r#"{"rubyLoadPaths":["a"]}"#
    );
    let conflict = read(
        r#"{"rubyLoadPaths":["a"],"ruby":{"loadPaths":["b"]}}"#,
        Some(builtin),
    )
    .unwrap();
    assert!(sections.apply(builtin, conflict).is_err());
    // No adapter that claims no keys keeps a section.
    for registration in builtin.registrations() {
        let adapter = registration.adapter;
        if adapter.config_keys().is_empty() {
            assert_eq!(sections.get(adapter.name()), None, "{}", adapter.name());
        }
    }

    // An adapter on two suffixes applies once, so a default it adds prints once.
    let defaulted = AdapterRegistry::new([
        super::super::SourceAdapterRegistration::new(".a", &DEFAULTED),
        super::super::SourceAdapterRegistration::new(".b", &DEFAULTED),
    ])
    .unwrap();
    let mut sections = AdapterSections::default();
    sections.apply(&defaulted, AdapterKeys::default()).unwrap();
    assert_eq!(serde_json::to_string(&sections).unwrap(), r#"{"level":1}"#);
    // A key an adapter returns without claiming it could print twice, so it is refused.
    let stray = read(r#"{"level":0}"#, Some(&defaulted)).unwrap();
    assert_eq!(
        sections.apply(&defaulted, stray).unwrap_err().to_string(),
        "adapter `defaulted` returned the configuration key `threshold`, which it does not claim"
    );

    // One adapter on two suffixes claims its keys once, so its section prints once.
    let twice = AdapterRegistry::new([
        super::super::SourceAdapterRegistration::new(".rb", &crate::ruby::RUBY_ADAPTER),
        super::super::SourceAdapterRegistration::new(".rake", &crate::ruby::RUBY_ADAPTER),
    ])
    .unwrap();
    let keys = read(r#"{"rubyLoadPaths":["a"]}"#, Some(&twice)).unwrap();
    let mut sections = AdapterSections::default();
    sections.apply(&twice, keys).unwrap();
    assert_eq!(
        serde_json::to_string(&sections).unwrap(),
        r#"{"rubyLoadPaths":["a"]}"#
    );
}

/// A configuration load applies each claiming adapter once: with the selected source's keys, or,
/// with no source, with none, so the adapter's default shows only then.
#[test]
fn configuration_loads_apply_each_claimant_once() {
    let adapters = AdapterRegistry::new([super::super::SourceAdapterRegistration::new(
        ".defaulted",
        &DEFAULTED,
    )])
    .unwrap();
    let level = |source: Option<&str>| {
        let root = tempfile::tempdir().unwrap();
        if let Some(text) = source {
            std::fs::write(root.path().join(".cuke-dedup.json"), text).unwrap();
        }
        let config = crate::config::Config::load_with_adapters(
            root.path(),
            crate::config::ConfigOverrides::default(),
            adapters.clone(),
        )
        .unwrap();
        config.adapter_config("defaulted").unwrap()["level"].clone()
    };
    assert_eq!(level(Some(r#"{"level":5}"#)), serde_json::json!(5));
    assert_eq!(level(None), serde_json::json!(1));
}
