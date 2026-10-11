//! Configuration composition; language policy remains in its frontend.
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct FrontendConfig {
    #[serde(flatten)]
    ruby: crate::ruby::config::RubyConfig,
}

impl FrontendConfig {
    pub(crate) fn ruby_load_paths(&self) -> &[PathBuf] {
        &self.ruby.load_paths
    }
    pub(crate) fn set_ruby_load_paths(&mut self, paths: Vec<PathBuf>) {
        self.ruby.load_paths = paths;
    }
}
