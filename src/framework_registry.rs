//! Framework descriptor registry: the one place that names adapter descriptors.
//!
//! Each adapter type registers once, whatever number of source languages it serves.

use crate::framework_descriptor::{
    self, DiscoveryInput, FrameworkDescriptor, FrameworkFeatures, ScriptConfigReader, CUCUMBER,
};
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::path::Path;

/// Discovery precedence: recognized Playwright-BDD configuration, then Cypress configuration when
/// a supported Cucumber preprocessor dependency is present, then Cucumber configuration files,
/// then Cucumber dependency-based default paths.
const DESCRIPTORS: [FrameworkDescriptor; 3] = [
    crate::typescript::framework_config::PLAYWRIGHT_BDD,
    crate::typescript::framework_config::CYPRESS_CUCUMBER,
    CUCUMBER,
];

/// Scripted configuration readers, in the order their extensions are probed.
const SCRIPT_READERS: [ScriptConfigReader; 1] =
    [crate::typescript::framework_config::SCRIPT_CONFIG_READER];

/// Returns the first framework discovery match in registry order, propagating configuration errors.
pub(crate) fn detect(
    root: &Path,
    package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    let input = DiscoveryInput {
        root,
        package,
        script_readers: &SCRIPT_READERS,
    };
    framework_descriptor::detect(&DESCRIPTORS, &input)
}

#[cfg(test)]
mod tests;
