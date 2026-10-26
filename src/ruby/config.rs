//! Ruby-owned configuration: the `ruby` section and its flat `rubyLoadPaths` alias.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RubyConfig {
    #[serde(rename = "rubyLoadPaths", skip_serializing_if = "Vec::is_empty")]
    pub(crate) load_paths: Vec<PathBuf>,
}

/// The Ruby keys of one configuration source. Flattened into the core config, so unknown keys at
/// the top level are rejected there; the section rejects its own.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawRubyConfig {
    #[serde(default)]
    ruby: Option<RawRubySection>,
    #[serde(default)]
    ruby_load_paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
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
