//! Framework discovery over the contracts registered adapters supply; it names no framework.

use crate::framework_descriptor::{self, DiscoveryInput, FrameworkFeatures};
use crate::source_adapter::registered_framework_contracts;
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::path::Path;

/// Returns the first framework discovery match in contract order, propagating configuration errors.
pub(crate) fn detect(
    root: &Path,
    package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    let descriptors =
        registered_framework_contracts().filter_map(|contract| contract.discovery.as_ref());
    framework_descriptor::detect(descriptors, &DiscoveryInput { root, package })
}

#[cfg(test)]
mod tests;
