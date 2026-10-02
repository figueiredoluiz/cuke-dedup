//! Typed authority boundaries for frontend completion.

use super::SourceLanguage;
use crate::model::{SourceLocation, StepDefinition};
use std::path::PathBuf;

/// The broadest source authority affected by unresolved static evidence.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UncertaintyScope {
    /// One handler or registration.
    Definition(SourceLocation),
    /// One source, only when frontend evidence proves its effects cannot escape.
    File(PathBuf),
    /// Every registration in a comparison domain; source-local syntax alone cannot narrow this.
    Registry(SourceLanguage),
}

/// What authority cannot be established without executing project code.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncertaintyCause {
    /// Registration ownership or execution effects may change the registry.
    Registration,
    /// Handler values or execution effects cannot establish equivalence.
    Handler,
    /// Source or dependency evidence is missing, without proof of registration invalidation.
    Source,
}

/// One conservative frontend limitation, independent of its diagnostic wording.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUncertainty {
    /// Authority affected by this limitation.
    pub scope: UncertaintyScope,
    /// Missing proof and its effect on comparisons.
    pub cause: UncertaintyCause,
    /// Human-readable diagnostic.
    pub message: String,
}

impl SourceUncertainty {
    /// Creates an explicitly scoped limitation; narrowing requires frontend isolation evidence.
    pub fn new(
        scope: UncertaintyScope,
        cause: UncertaintyCause,
        message: impl Into<String>,
    ) -> Self {
        Self {
            scope,
            cause,
            message: message.into(),
        }
    }

    fn affects(&self, definition: &StepDefinition) -> bool {
        match &self.scope {
            UncertaintyScope::Definition(location) => &definition.location == location,
            UncertaintyScope::File(path) => &definition.location.path == path,
            UncertaintyScope::Registry(language) => {
                definition.analysis_profile().comparison_domain == language.comparison_domain()
            }
        }
    }
}

/// Cross-source limitations collected after extraction, before shared analysis.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceFinalization {
    /// Limitations remain incomplete even when independent definitions can be retained.
    pub uncertainties: Vec<SourceUncertainty>,
}

impl SourceFinalization {
    /// Applies only the authority removal declared by the frontend; uncertainty never grants trust.
    pub fn apply(&self, definitions: &mut Vec<StepDefinition>) {
        for uncertainty in &self.uncertainties {
            match uncertainty.cause {
                UncertaintyCause::Registration => {
                    definitions.retain(|definition| !uncertainty.affects(definition))
                }
                UncertaintyCause::Handler => {
                    for definition in definitions
                        .iter_mut()
                        .filter(|definition| uncertainty.affects(definition))
                    {
                        definition.handler.comparable = false;
                    }
                }
                UncertaintyCause::Source => {}
            }
        }
    }
}
