//! Ruby-owned configuration: the `ruby` section and its flat `rubyLoadPaths` alias.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// The top-level configuration keys the Ruby adapter owns.
pub(crate) static CONFIG_KEYS: [&str; 2] = ["ruby", "rubyLoadPaths"];

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RubyConfig {
    #[serde(rename = "rubyLoadPaths", skip_serializing_if = "Vec::is_empty")]
    pub(crate) load_paths: Vec<PathBuf>,
}

/// The Ruby keys of one configuration source, as [`read_key`] stores them; [`apply_keys`] reads
/// them back here. The section rejects keys of its own; the core rejects unknown top-level keys.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawRubyConfig {
    #[serde(default)]
    ruby: Option<RawRubySection>,
    #[serde(default)]
    ruby_load_paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRubySection {
    #[serde(default)]
    load_paths: Option<Vec<PathBuf>>,
}

impl RubyConfig {
    /// Replaces the load paths one source sets through `ruby.loadPaths` or the `rubyLoadPaths`
    /// alias; a source that sets both is rejected rather than letting one silently win.
    pub(crate) fn apply_raw(&mut self, raw: RawRubyConfig) -> Result<()> {
        let section = raw.ruby.and_then(|section| section.load_paths);
        match (section, raw.ruby_load_paths) {
            (Some(_), Some(_)) => {
                bail!("set Ruby load paths with `ruby.loadPaths` or `rubyLoadPaths`, not both")
            }
            (Some(paths), None) | (None, Some(paths)) => self.load_paths = paths,
            (None, None) => {}
        }
        Ok(())
    }
}

/// Reads one source's value for a Ruby key with its typed reader, so a bad value keeps today's
/// error text, and stores it as read; `null` stores as unset.
pub(crate) fn read_key(
    key: &str,
    value: &mut dyn erased_serde::Deserializer<'_>,
    into: &mut Map<String, Value>,
) -> std::result::Result<(), erased_serde::Error> {
    let read = match key {
        "ruby" => serde_json::to_value(erased_serde::deserialize::<Option<RawRubySection>>(value)?),
        "rubyLoadPaths" => {
            serde_json::to_value(erased_serde::deserialize::<Option<Vec<PathBuf>>>(value)?)
        }
        // fail-closed: registration routes only `CONFIG_KEYS` here; any other key is refused.
        _ => {
            return Err(serde::de::Error::custom(format!(
                "`{key}` is not a Ruby configuration key"
            )))
        }
    };
    into.insert(key.to_owned(), read.expect("a decoded Ruby value encodes"));
    Ok(())
}

/// Applies the Ruby keys one source set, as [`read_key`] stored them, and returns the section the
/// configuration keeps and prints: `rubyLoadPaths` when it holds paths.
pub(crate) fn apply_keys(keys: Map<String, Value>) -> Result<Map<String, Value>> {
    let mut config = RubyConfig::default();
    config.apply_raw(serde_json::from_value(Value::Object(keys))?)?;
    Ok(serde_json::to_value(config)?
        .as_object()
        .cloned()
        .unwrap_or_default())
}

/// The configured `rubyLoadPaths`, as written, before resolution against the project root.
pub fn load_paths(config: &crate::config::Config) -> Vec<PathBuf> {
    config
        .adapter_config(crate::source_adapter::SourceAdapter::name(
            &super::RUBY_ADAPTER,
        ))
        .and_then(|section| section.get("rubyLoadPaths"))
        .map(|paths| serde_json::from_value(paths.clone()).expect("Ruby keeps the paths it read"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
