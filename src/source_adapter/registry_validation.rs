//! Conflicts that make a set of source-adapter registrations ambiguous.

use super::{FrameworkContract, SourceAdapter, SourceAdapterRegistration, SourceLanguage};
use crate::model::Framework;

/// Why [`super::AdapterRegistry::new`] rejected a set of registrations. Each variant names the first
/// conflict found, in registration order.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// A suffix is not a leading `.` followed by one file extension of ASCII letters, digits, `_`,
    /// `-` or `+`, or names source maps.
    InvalidSuffix {
        /// The rejected suffix.
        suffix: &'static str,
    },
    /// Two registrations claim one suffix.
    DuplicateSuffix {
        /// The suffix registered twice.
        suffix: &'static str,
    },
    /// Two adapters with different names declare one language.
    LanguageClaimedTwice {
        /// The language both adapters declare.
        language: SourceLanguage,
        /// The adapter registered first.
        first: &'static str,
        /// The adapter registered later.
        second: &'static str,
    },
    /// Adapters sharing one name declare different languages.
    NameDeclaresTwoLanguages {
        /// The shared adapter name.
        name: &'static str,
        /// The language declared first.
        first: SourceLanguage,
        /// The language declared later.
        second: SourceLanguage,
    },
    /// Two different adapter objects share one name and language. Objects are told apart by
    /// address; zero-sized adapters of different types may share one, so give each adapter its own
    /// name.
    DuplicateAdapterName {
        /// The shared adapter name.
        name: &'static str,
    },
    /// A framework id appears in more than one contract.
    FrameworkDeclaredTwice {
        /// The repeated framework id.
        framework: Framework,
    },
    /// An adapter declares a contract for [`Framework::UNKNOWN`], which is reserved.
    UnknownFrameworkContract {
        /// The declaring adapter.
        adapter: &'static str,
    },
    /// Two different comparison domains share one label.
    DomainLabelShared {
        /// The shared label.
        label: &'static str,
    },
    /// A contract policy compares in a domain other than its adapter's default policy.
    ContractDomainMismatch {
        /// The declaring adapter.
        adapter: &'static str,
        /// The framework whose policy differs.
        framework: Framework,
    },
}

impl std::fmt::Display for RegistryError {
    /// Describes the conflict.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSuffix { suffix } => write!(
                f,
                "source adapter suffix `{suffix}` is not a leading `.` followed by one file extension (ASCII letters, digits, `_`, `-`, `+`) other than `map`"
            ),
            Self::DuplicateSuffix { suffix } => write!(f, "suffix `{suffix}` is registered twice"),
            Self::LanguageClaimedTwice {
                language,
                first,
                second,
            } => write!(
                f,
                "language `{}` is declared by adapters `{first}` and `{second}`",
                language.id()
            ),
            Self::NameDeclaresTwoLanguages {
                name,
                first,
                second,
            } => write!(
                f,
                "adapters named `{name}` declare languages `{}` and `{}`",
                first.id(),
                second.id()
            ),
            Self::DuplicateAdapterName { name } => {
                write!(f, "two different adapters are named `{name}`")
            }
            Self::FrameworkDeclaredTwice { framework } => write!(
                f,
                "framework `{}` is declared by more than one framework contract",
                framework.id()
            ),
            Self::UnknownFrameworkContract { adapter } => write!(
                f,
                "adapter `{adapter}` declares a framework contract for the reserved id `unknown`"
            ),
            Self::DomainLabelShared { label } => {
                write!(f, "comparison domain label `{label}` names two different domains")
            }
            Self::ContractDomainMismatch { adapter, framework } => write!(
                f,
                "framework `{}` of adapter `{adapter}` compares in a domain other than the adapter's",
                framework.id()
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Returns the first conflict among `registrations`, in registration order.
pub(super) fn validate(registrations: &[SourceAdapterRegistration]) -> Result<(), RegistryError> {
    let mut suffixes: Vec<&'static str> = Vec::new();
    let mut adapters: Vec<&'static dyn SourceAdapter> = Vec::new();
    let mut checks = Contracts::default();
    for registration in registrations {
        let suffix = registration.suffix;
        let extension = suffix.strip_prefix('.').unwrap_or_default();
        let extension_character =
            |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+');
        if extension.is_empty() || extension == "map" || !extension.chars().all(extension_character)
        {
            return Err(RegistryError::InvalidSuffix { suffix });
        }
        if suffixes.contains(&suffix) {
            return Err(RegistryError::DuplicateSuffix { suffix });
        }
        suffixes.push(suffix);
        let adapter = registration.adapter;
        let (name, language) = (adapter.name(), adapter.language());
        match adapters
            .iter()
            .find(|seen| seen.name() == name || seen.language() == language)
        {
            Some(seen) if seen.name() != name => {
                return Err(RegistryError::LanguageClaimedTwice {
                    language,
                    first: seen.name(),
                    second: name,
                })
            }
            Some(seen) if seen.language() != language => {
                return Err(RegistryError::NameDeclaresTwoLanguages {
                    name,
                    first: seen.language(),
                    second: language,
                })
            }
            Some(seen) if !std::ptr::addr_eq(*seen, adapter) => {
                return Err(RegistryError::DuplicateAdapterName { name })
            }
            Some(_) => {}
            None => {
                adapters.push(adapter);
                checks.adapter(adapter)?;
            }
        }
    }
    Ok(())
}

/// The framework ids and domain labels the adapters checked so far declare.
#[derive(Default)]
struct Contracts {
    slices: Vec<(*const FrameworkContract, usize)>,
    frameworks: Vec<Framework>,
    domains: Vec<&'static super::DomainSpec>,
}

impl Contracts {
    /// Checks one newly registered adapter's contracts and domains against those seen before.
    fn adapter(&mut self, adapter: &'static dyn SourceAdapter) -> Result<(), RegistryError> {
        let name = adapter.name();
        let domain = adapter.analysis_policy().domain();
        self.domain(domain)?;
        let contracts = adapter.framework_contracts();
        for contract in contracts {
            if contract
                .policy
                .is_some_and(|policy| policy.domain() != domain)
            {
                return Err(RegistryError::ContractDomainMismatch {
                    adapter: name,
                    framework: contract.framework,
                });
            }
        }
        // Adapters sharing one contract slice (one ecosystem, several languages) declare its ids
        // once.
        let slice = (contracts.as_ptr(), contracts.len());
        if self.slices.contains(&slice) {
            return Ok(());
        }
        self.slices.push(slice);
        for framework in contracts.iter().map(|contract| contract.framework) {
            if framework == Framework::UNKNOWN {
                return Err(RegistryError::UnknownFrameworkContract { adapter: name });
            }
            if self.frameworks.contains(&framework) {
                return Err(RegistryError::FrameworkDeclaredTwice { framework });
            }
            self.frameworks.push(framework);
        }
        Ok(())
    }

    /// Records `domain`, rejecting a different domain with the same label.
    fn domain(&mut self, domain: &'static super::DomainSpec) -> Result<(), RegistryError> {
        match self.domains.iter().find(|seen| seen.label == domain.label) {
            Some(seen) if *seen != domain => Err(RegistryError::DomainLabelShared {
                label: domain.label,
            }),
            Some(_) => Ok(()),
            None => {
                self.domains.push(domain);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests;
