//! Ruby-owned configuration with the existing flat JSON representation.
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RubyConfig {
    #[serde(rename = "rubyLoadPaths", skip_serializing_if = "Vec::is_empty")]
    pub(crate) load_paths: Vec<PathBuf>,
}
