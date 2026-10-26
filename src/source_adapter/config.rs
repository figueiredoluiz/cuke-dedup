//! Configuration composition; language policy remains in its frontend.
use super::RawFrontendConfig;
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct FrontendConfig {
    #[serde(flatten)]
    ruby: crate::ruby::config::RubyConfig,
}

impl FrontendConfig {
    /// Configured `rubyLoadPaths` entries as written, before resolution against the project root.
    pub(crate) fn ruby_load_paths(&self) -> &[PathBuf] {
        &self.ruby.load_paths
    }
    /// Applies the frontend keys one configuration source sets; each frontend validates its own.
    pub(crate) fn apply_raw(&mut self, raw: RawFrontendConfig) -> Result<()> {
        self.ruby.apply_raw(raw)
    }
}
