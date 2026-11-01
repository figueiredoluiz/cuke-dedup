use super::super::semantics::{HandlerScope, StepMatching, StreamEvidence};
use super::super::{
    AdapterRegistry, AnalysisPolicy, DomainSpec, Extraction, IdentityFlags, RegexDialect, RegexKey,
    SourceFile, TranslatedRegex,
};
use super::*;
use crate::ruby::RUBY;

/// A dialect with no translations, for caller domains.
struct TestDialect;

impl RegexDialect for TestDialect {
    /// Translates nothing.
    fn translate(&self, _: &str, _: &str) -> Option<TranslatedRegex> {
        None
    }
    /// Keys on the canonical source pattern.
    fn regex_key(&self) -> RegexKey {
        RegexKey::CanonicalSource
    }
    /// Every flag counts.
    fn identity_flags(&self) -> IdentityFlags {
        IdentityFlags::All
    }
    /// Requires equal flags.
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool {
        left == right
    }
}

static CALLER_DOMAIN: DomainSpec = DomainSpec::new("caller", &TestDialect);
// Same label as `CALLER_DOMAIN`, different static.
static CALLER_TWIN: DomainSpec = DomainSpec::new("caller", &TestDialect);
static OTHER_DOMAIN: DomainSpec = DomainSpec::new("other", &TestDialect);
// Same label as the built-in Ruby domain.
static RUBY_LABEL: DomainSpec = DomainSpec::new("ruby", &TestDialect);

/// A shared, text-matched policy over `domain`.
const fn policy(domain: &'static DomainSpec) -> AnalysisPolicy {
    AnalysisPolicy::new(
        domain,
        HandlerScope::Shared,
        StepMatching::ByText,
        StreamEvidence::Complete,
        false,
    )
}

const SCOPED: Framework = Framework::new("caller-scoped");
static CONTRACTS: [FrameworkContract; 1] = [FrameworkContract::new(Framework::new("caller-fw"))];
// Same id as `CONTRACTS`, different slice.
static CONTRACTS_COPY: [FrameworkContract; 1] =
    [FrameworkContract::new(Framework::new("caller-fw"))];
static REPEATED: [FrameworkContract; 2] = [
    FrameworkContract::new(Framework::new("twice")),
    FrameworkContract::new(Framework::new("twice")),
];
static CUCUMBER_JS_AGAIN: [FrameworkContract; 1] =
    [FrameworkContract::new(Framework::new("cucumber-js"))];
static UNKNOWN: [FrameworkContract; 1] = [FrameworkContract::new(Framework::UNKNOWN)];
// A contract policy in the caller domain: valid for an adapter in that domain only.
static SCOPED_CONTRACTS: [FrameworkContract; 1] =
    [
        FrameworkContract::new(SCOPED).with_policy(AnalysisPolicy::new(
            &CALLER_DOMAIN,
            HandlerScope::Local,
            StepMatching::Positional,
            StreamEvidence::Complete,
            false,
        )),
    ];

/// A caller adapter whose identity, policy and contracts are fixed per static.
struct TestAdapter {
    name: &'static str,
    language: SourceLanguage,
    policy: AnalysisPolicy,
    contracts: &'static [FrameworkContract],
}

impl SourceAdapter for TestAdapter {
    /// Returns the fixed name.
    fn name(&self) -> &'static str {
        self.name
    }
    /// Returns the fixed language.
    fn language(&self) -> SourceLanguage {
        self.language
    }
    /// Never extracts; validation does not call it.
    fn extract(&self, _: &str, _: &SourceFile) -> anyhow::Result<Extraction> {
        anyhow::bail!("not extracted")
    }
    /// Returns the fixed policy.
    fn analysis_policy(&self) -> AnalysisPolicy {
        self.policy
    }
    /// Returns the fixed contracts.
    fn framework_contracts(&self) -> &'static [FrameworkContract] {
        self.contracts
    }
}

/// A caller adapter named and keyed by `id`, in `domain`, declaring `contracts`.
const fn adapter(
    id: &'static str,
    domain: &'static DomainSpec,
    contracts: &'static [FrameworkContract],
) -> TestAdapter {
    TestAdapter {
        name: id,
        language: SourceLanguage::new(id),
        policy: policy(domain),
        contracts,
    }
}

static CALLER: TestAdapter = adapter("caller", &CALLER_DOMAIN, &CONTRACTS);
// Same fields as `CALLER`, different object.
static CALLER_COPY: TestAdapter = adapter("caller", &CALLER_DOMAIN, &CONTRACTS);
static UNATTACHED: TestAdapter = TestAdapter {
    policy: AnalysisPolicy::unattached(),
    ..adapter("unattached-adapter", &CALLER_DOMAIN, &[])
};
static CALLER_OTHER_LANGUAGE: TestAdapter = TestAdapter {
    language: SourceLanguage::new("other"),
    ..adapter("caller", &CALLER_DOMAIN, &[])
};
static RUBY_CLAIM: TestAdapter = TestAdapter {
    language: RUBY,
    ..adapter("caller-ruby", &CALLER_DOMAIN, &[])
};
// Shares `CALLER`'s contract slice and domain, as one ecosystem's adapters do.
static SIBLING: TestAdapter = adapter("sibling", &CALLER_DOMAIN, &CONTRACTS);
static SAME_ID_OTHER_SLICE: TestAdapter = adapter("copy", &CALLER_DOMAIN, &CONTRACTS_COPY);
static REPEATS: TestAdapter = adapter("repeats", &CALLER_DOMAIN, &REPEATED);
static REPEATS_JS: TestAdapter = adapter("repeats-js", &CALLER_DOMAIN, &CUCUMBER_JS_AGAIN);
static DECLARES_UNKNOWN: TestAdapter = adapter("unknowns", &CALLER_DOMAIN, &UNKNOWN);
static TWIN: TestAdapter = adapter("twin", &CALLER_TWIN, &[]);
static RUBY_LABELLED: TestAdapter = adapter("labelled", &RUBY_LABEL, &[]);
static SCOPED_CALLER: TestAdapter = adapter("scoped", &CALLER_DOMAIN, &SCOPED_CONTRACTS);
// Shares `SCOPED_CALLER`'s contracts from another domain: the contract policy leaves its domain.
static SCOPED_OTHER: TestAdapter = adapter("scoped-other", &OTHER_DOMAIN, &SCOPED_CONTRACTS);

/// `suffix` routed to `adapter`.
fn registration(suffix: &'static str, adapter: &'static TestAdapter) -> SourceAdapterRegistration {
    SourceAdapterRegistration::new(suffix, adapter)
}

/// The built-in registrations followed by `extra`.
fn builtin_and(extra: &[SourceAdapterRegistration]) -> Vec<SourceAdapterRegistration> {
    let builtin = AdapterRegistry::builtin().registrations().iter().copied();
    builtin.chain(extra.iter().copied()).collect()
}

/// Checks registry construction accepts unambiguous sets and rejects each conflict with its named
/// error; every rejected row differs from an accepted one by the conflict alone.
#[test]
fn registry_construction_rejects_each_conflict() {
    let ruby_only = AdapterRegistry::builtin()
        .registrations()
        .iter()
        .copied()
        .filter(|registration| registration.adapter.language() == RUBY)
        .collect::<Vec<_>>();
    for accepted in [
        builtin_and(&[]),
        ruby_only,
        Vec::new(),
        builtin_and(&[registration(".caller", &CALLER)]),
        vec![registration(".a", &CALLER), registration(".b", &CALLER)],
        vec![registration(".a", &CALLER), registration(".b", &SIBLING)],
        vec![registration(".a", &SCOPED_CALLER)],
        vec![
            registration(".c++", &CALLER),
            registration(".x_y-z9", &SIBLING),
        ],
    ] {
        let suffixes: Vec<_> = accepted.iter().map(|entry| entry.suffix).collect();
        let registry = AdapterRegistry::new(accepted).unwrap();
        assert_eq!(registry.suffixes(), suffixes);
    }

    use RegistryError::*;
    let rejected: Vec<(Vec<SourceAdapterRegistration>, RegistryError)> = vec![
        (
            vec![registration("caller", &CALLER)],
            InvalidSuffix { suffix: "caller" },
        ),
        (
            vec![registration("", &CALLER)],
            InvalidSuffix { suffix: "" },
        ),
        (
            vec![registration(".", &CALLER)],
            InvalidSuffix { suffix: "." },
        ),
        (
            vec![registration(".d.ts", &CALLER)],
            InvalidSuffix { suffix: ".d.ts" },
        ),
        (
            vec![registration("./x", &CALLER)],
            InvalidSuffix { suffix: "./x" },
        ),
        (
            vec![registration(".a\\b", &CALLER)],
            InvalidSuffix { suffix: ".a\\b" },
        ),
        (
            vec![registration(".map", &CALLER)],
            InvalidSuffix { suffix: ".map" },
        ),
        (
            vec![registration(".*", &CALLER)],
            InvalidSuffix { suffix: ".*" },
        ),
        (
            vec![registration(".[jt]s", &CALLER)],
            InvalidSuffix { suffix: ".[jt]s" },
        ),
        (
            vec![registration(". rb", &CALLER)],
            InvalidSuffix { suffix: ". rb" },
        ),
        (
            builtin_and(&[registration(".rb", &CALLER)]),
            DuplicateSuffix { suffix: ".rb" },
        ),
        (
            builtin_and(&[registration(".rbx", &RUBY_CLAIM)]),
            LanguageClaimedTwice {
                language: RUBY,
                first: "cucumber-ruby",
                second: "caller-ruby",
            },
        ),
        (
            vec![
                registration(".a", &CALLER),
                registration(".b", &CALLER_OTHER_LANGUAGE),
            ],
            NameDeclaresTwoLanguages {
                name: "caller",
                first: SourceLanguage::new("caller"),
                second: SourceLanguage::new("other"),
            },
        ),
        (
            vec![
                registration(".a", &CALLER),
                registration(".b", &CALLER_COPY),
            ],
            DuplicateAdapterName { name: "caller" },
        ),
        (
            vec![
                registration(".a", &CALLER),
                registration(".b", &SAME_ID_OTHER_SLICE),
            ],
            FrameworkDeclaredTwice {
                framework: Framework::new("caller-fw"),
            },
        ),
        (
            vec![registration(".a", &REPEATS)],
            FrameworkDeclaredTwice {
                framework: Framework::new("twice"),
            },
        ),
        (
            builtin_and(&[registration(".caller", &REPEATS_JS)]),
            FrameworkDeclaredTwice {
                framework: Framework::new("cucumber-js"),
            },
        ),
        (
            vec![registration(".a", &DECLARES_UNKNOWN)],
            UnknownFrameworkContract {
                adapter: "unknowns",
            },
        ),
        (
            vec![registration(".a", &CALLER), registration(".b", &TWIN)],
            DomainLabelShared { label: "caller" },
        ),
        (
            builtin_and(&[registration(".caller", &RUBY_LABELLED)]),
            DomainLabelShared { label: "ruby" },
        ),
        (
            vec![
                registration(".a", &SCOPED_CALLER),
                registration(".b", &SCOPED_OTHER),
            ],
            ContractDomainMismatch {
                adapter: "scoped-other",
                framework: SCOPED,
            },
        ),
        (
            vec![registration(".a", &CALLER), registration(".b", &UNATTACHED)],
            UnattachedPolicy {
                adapter: "unattached-adapter",
            },
        ),
    ];
    assert_eq!(
        LanguageClaimedTwice {
            language: RUBY,
            first: "cucumber-ruby",
            second: "caller-ruby",
        }
        .to_string(),
        "language `ruby` is declared by adapters `cucumber-ruby` and `caller-ruby`"
    );
    assert_eq!(
        InvalidSuffix { suffix: "caller" }.to_string(),
        "source adapter suffix `caller` is not a leading `.` followed by one file extension (ASCII letters, digits, `_`, `-`, `+`) other than `map`"
    );
    for (registrations, expected) in rejected {
        let error = AdapterRegistry::new(registrations).unwrap_err();
        assert_eq!(error, expected);
        assert!(!error.to_string().is_empty());
    }
}
