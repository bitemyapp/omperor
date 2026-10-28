//! Pure cache-policy reference data; no network, filesystem access, or provider calls.
//!
//! Verified 2026-09-22 against:
//! - https://developers.openai.com/api/docs/guides/prompt-caching
//! - https://platform.claude.com/docs/en/build-with-claude/prompt-caching
//! - https://platform.claude.com/docs/en/build-with-claude/thinking
//! - https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-use-with-prompt-caching
//! Anthropic's default five-minute TTL was rechecked on 2026-10-06.
//! OpenAI response TTL metadata was rechecked on 2026-10-06. The live timer
//! starts at client-side response completion; missing metadata uses five minutes.
//! A retention window is not proof that an eligible cached prefix still exists.

use std::time::Duration;

pub(crate) const VERIFIED_ON: &str = "2026-09-22";
/// Working assumptions when effective request retention is unavailable.
pub(crate) const OPENAI_ASSUMED_TTL_SECONDS: u64 = 5 * 60;
// Anthropic defaults to five minutes; one hour requires an explicit request.
pub(crate) const ANTHROPIC_ASSUMED_TTL_SECONDS: u64 = 5 * 60;

/// Provider evidence for the most recent completed inference response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CacheObservation {
    pub completed_at_ms: u64,
    pub ttl_seconds: Option<u64>,
    pub retention: Option<String>,
}

/// Session-local clock. Repaints and history reads never restart it.
#[derive(Debug, Default)]
pub(crate) struct CacheClock {
    observation: Option<CacheObservation>,
    valid_after_ms: u64,
}

impl CacheClock {
    pub(crate) fn observe(&mut self, observation: Option<CacheObservation>, now_ms: u64) {
        if let Some(observation) = observation
            && observation.completed_at_ms > self.valid_after_ms
            && observation.completed_at_ms <= now_ms
            && self
                .observation
                .as_ref()
                .is_none_or(|old| observation.completed_at_ms > old.completed_at_ms)
        {
            self.observation = Some(observation);
        }
    }

    /// Namespace changes also reject stale journal reads already in flight.
    pub(crate) fn reset(&mut self, now_ms: u64) {
        self.observation = None;
        self.valid_after_ms = now_ms;
    }

    pub(crate) fn warning(&self, now_ms: u64) -> Option<CacheWarning> {
        let observation = self.observation.as_ref()?;
        let elapsed = Duration::from_millis(now_ms.checked_sub(observation.completed_at_ms)?);
        Some(CacheWarning::for_response(observation, elapsed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheProvider {
    OpenAiApi,
    CodexSubscription,
    Anthropic,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RetentionSelection {
    ProviderDefault,
    ApplicationAssumption,
    Configured(String),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheTtl {
    Unknown,
    Assumed {
        seconds: u64,
    },
    Minimum {
        seconds: u64,
    },
    Typical {
        min_seconds: u64,
        max_seconds: u64,
        maximum_seconds: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetentionOption {
    pub value: &'static str,
    pub description: &'static str,
    /// False means support depends on the model or organization.
    pub verified_for_model: bool,
}

const OPENAI_CURRENT_OPTIONS: &[RetentionOption] = &[RetentionOption {
    value: "30m",
    description: "prompt_cache_options.ttl: minimum 30 minutes; default",
    verified_for_model: true,
}];
const OPENAI_LEGACY_OPTIONS: &[RetentionOption] = &[
    RetentionOption {
        value: "in_memory",
        description: "Typically 5–10 minutes; at most 1 hour; model dependent",
        verified_for_model: false,
    },
    RetentionOption {
        value: "24h",
        description: "Typically 30 minutes; at most 24 hours; model dependent",
        verified_for_model: false,
    },
];
const ANTHROPIC_OPTIONS: &[RetentionOption] = &[
    RetentionOption {
        value: "5m",
        description: "Minimum 5 minutes when caching is enabled; default",
        verified_for_model: true,
    },
    RetentionOption {
        value: "1h",
        description: "Minimum 1 hour when caching is enabled",
        verified_for_model: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PricingBasis {
    ApiListPrice,
    ApiReferenceOnly,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CachePricing {
    pub write_multiplier: Option<f64>,
    pub read_multiplier: Option<f64>,
    pub basis: PricingBasis,
}

impl CachePricing {
    const UNKNOWN: Self = Self {
        write_multiplier: None,
        read_multiplier: None,
        basis: PricingBasis::Unknown,
    };
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CachePolicy {
    pub provider: CacheProvider,
    pub model: String,
    pub ttl: CacheTtl,
    pub retention: RetentionSelection,
    pub minimum_tokens: Option<u64>,
    pub pricing: CachePricing,
    pub supported_retention: &'static [RetentionOption],
}

/// Compare effective values, after defaults/aliases have been resolved. A change
/// from an omitted setting to its equivalent explicit default is a no-op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheAction<'a> {
    ModelSwitch {
        from: &'a str,
        to: &'a str,
    },
    AccountSwitch {
        from: &'a str,
        to: &'a str,
    },
    ReasoningChange {
        from: &'a str,
        to: &'a str,
    },
    FastModeChange {
        from: bool,
        to: bool,
    },
    NewSession,
    ForkSession,
    SystemInstructionsChange,
    ToolsChange,
    // The policy covers these, and its tests use them, though nothing in the
    // app edits history or reports an append-only update yet.
    #[allow(dead_code)]
    HistoryEdit,
    Compaction,
    ImagePresenceChange {
        from: bool,
        to: bool,
    },
    #[allow(dead_code)]
    AppendMessage,
    /// Only use when the codec has confirmed a supported append-only wire update.
    #[allow(dead_code)]
    AppendOnlyReasoningChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheSeverity {
    Info,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheScope {
    Model,
    CacheNamespace,
    ToolsAndFollowing,
    SystemAndMessages,
    Messages,
    ChangedSuffix,
    ModelDependentPrefix,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheImpact {
    pub headline: &'static str,
    pub detail: &'static str,
    pub severity: CacheSeverity,
    pub scope: CacheScope,
}

/// A possible cache miss after a user change or near the end of a retention window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CacheWarning {
    pub headline: &'static str,
    pub badge_label: String,
    pub urgent: bool,
    pub explanation: String,
    pub tooltip: String,
}

impl CacheWarning {
    pub(crate) fn for_response(observation: &CacheObservation, elapsed: Duration) -> Self {
        let ttl = observation.ttl_seconds.filter(|&seconds| seconds > 0);
        let seconds = ttl.unwrap_or(5 * 60);
        let remaining = Duration::from_secs(seconds).saturating_sub(elapsed);
        // Round up so the badge does not show zero before the window ends.
        let remaining_seconds = remaining
            .as_secs()
            .saturating_add(u64::from(remaining.subsec_nanos() > 0));
        let clock = if remaining_seconds >= 3600 {
            format!(
                "{}:{:02}:{:02}",
                remaining_seconds / 3600,
                remaining_seconds / 60 % 60,
                remaining_seconds % 60
            )
        } else {
            format!(
                "{:02}:{:02}",
                remaining_seconds / 60,
                remaining_seconds % 60
            )
        };
        let estimated = ttl.is_none();
        let headline = match (estimated, remaining.is_zero()) {
            (true, false) => "Estimated KV cache timer",
            (true, true) => "Estimated cache window elapsed",
            (false, false) => "Provider-reported KV cache timer",
            (false, true) => "Provider-reported cache window elapsed",
        };
        let basis = if estimated {
            let legacy = match observation.retention.as_deref() {
                Some("24h") => {
                    " The provider returned a 24-hour maximum retention policy, which does not establish a minimum lifetime."
                }
                Some("in_memory") => {
                    " The provider returned an in-memory retention policy, which does not establish a minimum lifetime."
                }
                _ => "",
            };
            format!(
                "Estimate: the provider did not return a usable minimum cache TTL. Omperor assumes five minutes.{legacy}"
            )
        } else {
            format!(
                "Provider-reported: the API response explicitly supplied a minimum cache TTL of {seconds} seconds. This duration was not inferred from the model or account settings."
            )
        };
        let explanation = format!(
            "{basis}\n\nThe countdown starts when the API response finishes streaming to the client and restarts after each completed inference response.\n\nThis is a retention-window timer, not a confirmed expiry. A response may not have written a cache entry; matching prefixes and the same account are still required for reuse. Reaching zero does not prove the cache was evicted."
        );
        Self {
            headline,
            badge_label: format!("KV {}{clock}", if estimated { "~" } else { "" }),
            urgent: remaining <= Duration::from_secs(60),
            tooltip: format!(
                "{headline}. {basis}\nTimed from response completion; cache reuse is not guaranteed.\nClick for cache details."
            ),
            explanation,
        }
    }

    pub(crate) fn for_action(model: &str, action: CacheAction<'_>) -> Option<Self> {
        let policy = CachePolicy::for_route(model.split('/').next().unwrap_or(""), model, None);
        let impact = policy.impact(action)?;
        if impact.severity != CacheSeverity::Warning {
            return None;
        }
        let retention = match policy.ttl {
            CacheTtl::Assumed { seconds } => format!(
                "\n\nOmperor assumes a {}-minute cache lifetime. Changes to the prefix or account can prevent reuse within that window.",
                seconds / 60
            ),
            _ => String::new(),
        };
        Some(Self {
            headline: impact.headline,
            badge_label: "KV —".into(),
            urgent: true,
            explanation: format!(
                "{}{retention}\n\nAffected input may be processed again at ordinary input or cache-write rates. This does not mean existing cache entries were deleted. The provider does not report whether a reusable entry still exists.\n\nOpen Usage for recorded counts and the provider's cache policy.",
                impact.detail
            ),
            tooltip: format!(
                "{}: {}\nClick for cache details.",
                impact.headline, impact.detail
            ),
        })
    }
}

impl CachePolicy {
    /// `retention` is preferably the effective provider wire value. `None` or
    /// `auto` selects Omperor's provider assumptions where defined, otherwise
    /// published defaults. Neither confirms that caching is active.
    /// Helper preferences (`none`, `short`, `long`) remain unknown unless their
    /// concrete mapping is established below. Unknown models are not extrapolated.
    pub(crate) fn for_route(provider: &str, model: &str, retention: Option<&str>) -> Self {
        let model = model.rsplit('/').next().unwrap_or(model);
        let provider = match provider {
            "openai" => CacheProvider::OpenAiApi,
            "openai-codex" => CacheProvider::CodexSubscription,
            "anthropic" => CacheProvider::Anthropic,
            _ => CacheProvider::Unknown,
        };
        let selected = retention.filter(|value| !value.is_empty() && *value != "auto");
        let mut policy = Self {
            provider,
            model: model.to_owned(),
            ttl: CacheTtl::Unknown,
            retention: selected.map_or(RetentionSelection::ProviderDefault, |s| {
                RetentionSelection::Configured(s.to_owned())
            }),
            minimum_tokens: None,
            pricing: CachePricing::UNKNOWN,
            supported_retention: &[],
        };
        match provider {
            CacheProvider::OpenAiApi | CacheProvider::CodexSubscription => {
                if is_current_openai(model) {
                    policy.supported_retention = OPENAI_CURRENT_OPTIONS;
                    policy.minimum_tokens = Some(1024);
                    policy.pricing = CachePricing {
                        write_multiplier: Some(1.25),
                        read_multiplier: Some(0.1),
                        basis: PricingBasis::ApiListPrice,
                    };
                    if selected == Some("30m") {
                        policy.ttl = CacheTtl::Minimum { seconds: 1800 };
                    }
                } else if is_legacy_openai(model) {
                    policy.supported_retention = OPENAI_LEGACY_OPTIONS;
                    // Earlier read prices and minimum lengths depend on the model
                    // and request settings; do not infer a universal 0.1x/1024.
                    policy.pricing = CachePricing {
                        write_multiplier: Some(1.0),
                        read_multiplier: None,
                        basis: PricingBasis::ApiListPrice,
                    };
                    policy.ttl = match selected {
                        Some("in_memory") => CacheTtl::Typical {
                            min_seconds: 300,
                            max_seconds: 600,
                            maximum_seconds: 3600,
                        },
                        Some("24h") => CacheTtl::Typical {
                            min_seconds: 1800,
                            max_seconds: 1800,
                            maximum_seconds: 86400,
                        },
                        _ => CacheTtl::Unknown,
                    };
                }
                if selected.is_none() {
                    // Published model defaults are not observed request settings.
                    // Use the user's conservative estimate until the helper
                    // preserves the response's applied retention in receipts.
                    policy.retention = RetentionSelection::ApplicationAssumption;
                    policy.ttl = CacheTtl::Assumed {
                        seconds: OPENAI_ASSUMED_TTL_SECONDS,
                    };
                }
                if provider == CacheProvider::CodexSubscription {
                    // API terms are reference material, not subscription promises.
                    if let Some(selected) = selected {
                        policy.ttl = if selected == "30m" {
                            policy.retention = RetentionSelection::ApplicationAssumption;
                            CacheTtl::Assumed { seconds: 1800 }
                        } else {
                            CacheTtl::Unknown
                        }
                    }
                    policy.minimum_tokens = None;
                    policy.supported_retention = &[];
                    policy.pricing.basis = PricingBasis::ApiReferenceOnly;
                }
            }
            CacheProvider::Anthropic => {
                if selected.is_none() {
                    policy.retention = RetentionSelection::ApplicationAssumption;
                    policy.ttl = CacheTtl::Assumed {
                        seconds: ANTHROPIC_ASSUMED_TTL_SECONDS,
                    };
                }
                if let Some((minimum, read_multiplier)) = anthropic_model(model) {
                    policy.minimum_tokens = Some(minimum);
                    policy.supported_retention = ANTHROPIC_OPTIONS;
                    // OMP maps Short/Long to 5m/1h in anthropic::CacheTtl.
                    // `none` is deliberately not treated as proven disabled:
                    // settings maps it to Request, whose codec mapping is 5m.
                    let seconds = match selected {
                        Some("5m" | "short") => Some(300),
                        Some("1h" | "long") => Some(3600),
                        _ => None,
                    };
                    if selected.is_some() {
                        policy.ttl = seconds
                            .map_or(CacheTtl::Unknown, |seconds| CacheTtl::Minimum { seconds });
                    }
                    policy.pricing = CachePricing {
                        write_multiplier: seconds.map(|s| if s == 3600 { 2.0 } else { 1.25 }),
                        read_multiplier: Some(read_multiplier),
                        basis: PricingBasis::ApiListPrice,
                    };
                }
            }
            CacheProvider::Unknown => policy.retention = RetentionSelection::Unknown,
        }
        policy
    }

    pub(crate) fn summary(&self) -> String {
        let prefix = match self.provider {
            CacheProvider::OpenAiApi => "OpenAI API policy",
            CacheProvider::CodexSubscription => "Codex subscription cache policy",
            CacheProvider::Anthropic => "Claude API policy when caching is enabled",
            CacheProvider::Unknown => "Provider cache policy unknown",
        };
        format!("{prefix}. {}", self.ttl_summary())
    }

    pub(crate) fn ttl_summary(&self) -> String {
        match self.ttl {
            CacheTtl::Assumed { seconds } => format!(
                "Omperor assumes a {}-minute TTL from response completion when the provider does not report a minimum lifetime. This is an application assumption, not provider-confirmed retention.", seconds / 60
            ),
            CacheTtl::Minimum { seconds } => {
                let anchor = if self.provider == CacheProvider::Anthropic { "request start on each read/write" } else { "each read/write" };
                format!("Minimum {} after {anchor}; later availability is unconfirmed.", duration_label(seconds))
            }
            CacheTtl::Typical { min_seconds, max_seconds, maximum_seconds } => {
                let typical = if min_seconds == max_seconds { duration_label(max_seconds) } else { format!("{}–{}", duration_label(min_seconds), duration_label(max_seconds)) };
                format!("Typically {typical} idle; up to {}. Reuse refreshes the window.", duration_label(maximum_seconds))
            }
            CacheTtl::Unknown if self.provider == CacheProvider::CodexSubscription => {
                let api = Self::for_route("openai", &self.model, None);
                format!("No subscription TTL confirmed. API reference only: {}", api.ttl_summary())
            }
            CacheTtl::Unknown if is_legacy_openai(&self.model) && self.provider == CacheProvider::OpenAiApi => "Effective retention unknown; legacy defaults depend on model and organization data policy.".to_owned(),
            CacheTtl::Unknown => "Retention unavailable; no expiry estimate.".to_owned(),
        }
    }

    pub(crate) fn pricing_summary(&self) -> String {
        let basis = match self.pricing.basis {
            PricingBasis::ApiListPrice => "API input-price reference",
            PricingBasis::ApiReferenceOnly => "API reference only; subscription billing is unknown",
            PricingBasis::Unknown => return "Cache input-price ratios unknown.".to_owned(),
        };
        let ratio = |value: Option<f64>| value.map_or("unknown".to_owned(), |n| format!("{n}×"));
        format!(
            "{basis}: writes {}, reads {}. These replace the base input rate; output is separate.",
            ratio(self.pricing.write_multiplier),
            ratio(self.pricing.read_multiplier)
        )
    }

    pub(crate) fn impact(&self, action: CacheAction<'_>) -> Option<CacheImpact> {
        use CacheAction as Action;
        use CacheScope as Scope;
        use CacheSeverity::{Info, Warning};
        match action {
            Action::ModelSwitch { from, to }
            | Action::AccountSwitch { from, to }
            | Action::ReasoningChange { from, to }
                if from == to =>
            {
                return None;
            }
            Action::FastModeChange { from, to } | Action::ImagePresenceChange { from, to }
                if from == to =>
            {
                return None;
            }
            Action::AppendMessage | Action::AppendOnlyReasoningChange => return None,
            _ => {}
        }
        let (headline, detail, severity, scope) = match action {
            Action::ModelSwitch { .. } => (
                "Model changes cache identity",
                "The target model cannot be assumed to reuse this model's prefix. Its existing matching entries may still be available.",
                Warning,
                Scope::Model,
            ),
            Action::AccountSwitch { .. } => (
                "Account changes cache scope",
                "The new account may use a different organization or workspace. Cache sharing across that boundary is not assured.",
                Warning,
                Scope::CacheNamespace,
            ),
            _ if self.provider == CacheProvider::Unknown => (
                "Cache impact unknown",
                "This provider has no verified cache policy. Compare reported cache usage after the change.",
                Info,
                Scope::Unknown,
            ),
            Action::NewSession | Action::ForkSession => (
                "Session identity changes",
                "A new journal changes Omperor's cache routing key and replayed context. Shared text alone does not guarantee reuse; existing entries are not deleted.",
                if self.provider == CacheProvider::CodexSubscription {
                    Warning
                } else {
                    Info
                },
                Scope::CacheNamespace,
            ),
            Action::ReasoningChange { .. } if self.provider == CacheProvider::Anthropic => (
                "Reasoning changes the cached prefix",
                "Message breakpoints are affected. Tool and system breakpoints can also change, depending on where this model renders thinking and effort settings.",
                Warning,
                Scope::ModelDependentPrefix,
            ),
            Action::ReasoningChange { .. } => (
                "Reasoning may reduce prefix reuse",
                "Request-level effort can change hidden instructions. A supported append-only configuration update can preserve earlier context.",
                Warning,
                Scope::ModelDependentPrefix,
            ),
            Action::FastModeChange { .. } if self.provider == CacheProvider::Anthropic => (
                "Speed changes system and message caches",
                "Claude's speed setting changes the system prefix; tool definitions can remain reusable.",
                Warning,
                Scope::SystemAndMessages,
            ),
            Action::FastModeChange { .. } => (
                "Fast-mode cache impact unconfirmed",
                "Service-tier routing can change. This route has no verified guarantee that switching speed preserves or discards cached input.",
                Info,
                Scope::Unknown,
            ),
            Action::ToolsChange => (
                "Tool changes affect following context",
                "Changed tool definitions or ordering alter the prefix from the changed definition onward.",
                Warning,
                Scope::ToolsAndFollowing,
            ),
            Action::SystemInstructionsChange => (
                "Instructions change the following prefix",
                "Earlier unchanged tool definitions may remain reusable; changed instructions and later context need a matching entry.",
                Warning,
                Scope::SystemAndMessages,
            ),
            Action::HistoryEdit | Action::Compaction => (
                "History changes reduce suffix reuse",
                "Reuse can stop at the first changed token. Earlier matching prefixes may survive, and compaction can reduce total input despite fewer cache hits.",
                Warning,
                Scope::ChangedSuffix,
            ),
            Action::ImagePresenceChange { .. } if self.provider == CacheProvider::Anthropic => (
                "Image changes affect message caches",
                "Adding or removing images changes Claude's message cache; tool and system prefixes can remain reusable.",
                Warning,
                Scope::Messages,
            ),
            Action::ImagePresenceChange { .. } => (
                "Images change the rendered prefix",
                "Reuse depends on identical image input and settings. Context before the changed content may remain reusable.",
                Warning,
                Scope::ChangedSuffix,
            ),
            Action::AppendMessage | Action::AppendOnlyReasoningChange => unreachable!(),
        };
        Some(CacheImpact {
            headline,
            detail,
            severity,
            scope,
        })
    }
}

fn duration_label(seconds: u64) -> String {
    if seconds >= 3600 && seconds.is_multiple_of(3600) {
        format!("{}h", seconds / 3600)
    } else if seconds >= 60 && seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

// Match exact names or dated snapshots, never arbitrary/future names that merely
// start with a known model. Keep more specific Claude names before base names.
fn is_model(model: &str, name: &str) -> bool {
    model == name
        || model
            .strip_prefix(name)
            .and_then(|s| s.strip_prefix('-'))
            .is_some_and(|suffix| {
                (suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_digit()))
                    || (suffix.len() == 10
                        && suffix.bytes().enumerate().all(|(i, b)| {
                            if i == 4 || i == 7 {
                                b == b'-'
                            } else {
                                b.is_ascii_digit()
                            }
                        }))
            })
}

fn is_current_openai(model: &str) -> bool {
    [
        "gpt-5.6",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-6",
        "gpt-6-astra",
        "gpt-6-sol",
        "gpt-6-luna",
    ]
    .iter()
    .any(|name| is_model(model, name))
}

fn is_legacy_openai(model: &str) -> bool {
    [
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.4-nano",
        "gpt-5.3-codex",
        "gpt-5.2",
        "gpt-5.2-codex",
        "gpt-5.1",
        "gpt-5.1-codex",
        "gpt-5",
        "gpt-5-mini",
        "gpt-5-nano",
        "gpt-5-codex",
        "gpt-4.1",
        "gpt-4.1-mini",
        "gpt-4.1-nano",
        "gpt-4o",
        "gpt-4o-mini",
        "o4-mini",
        "o3",
        "o3-mini",
        "o1",
        "o1-mini",
    ]
    .iter()
    .any(|name| is_model(model, name))
}

fn anthropic_model(model: &str) -> Option<(u64, f64)> {
    const MODELS: &[(&str, u64, f64)] = &[
        ("claude-fable-5-1", 512, 0.025),
        ("claude-mythos-5-1", 512, 0.025),
        ("claude-opus-5-5", 512, 0.05),
        ("claude-opus-5", 512, 0.1),
        ("claude-fable-5", 512, 0.1),
        ("claude-mythos-5", 512, 0.1),
        ("claude-mythos-preview", 2048, 0.1),
        ("claude-opus-4-8", 1024, 0.1),
        ("claude-opus-4-7", 2048, 0.1),
        ("claude-opus-4-6", 4096, 0.1),
        ("claude-opus-4-5", 4096, 0.1),
        ("claude-opus-4-1", 1024, 0.1),
        ("claude-opus-4", 1024, 0.1),
        ("claude-sonnet-5", 1024, 0.1),
        ("claude-sonnet-4-6", 1024, 0.1),
        ("claude-sonnet-4-5", 1024, 0.1),
        ("claude-sonnet-4", 1024, 0.1),
        ("claude-haiku-4-5", 4096, 0.1),
        ("claude-3-5-haiku", 2048, 0.1),
    ];
    MODELS
        .iter()
        .find(|(name, _, _)| is_model(model, name))
        .map(|(_, minimum, multiplier)| (*minimum, *multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_uses_the_application_assumption_without_inheriting_api_billing() {
        let policy = CachePolicy::for_route("openai-codex", "gpt-5.6", Some("30m"));
        assert_eq!(policy.ttl, CacheTtl::Assumed { seconds: 1800 });
        assert_eq!(policy.retention, RetentionSelection::ApplicationAssumption);
        assert_eq!(policy.pricing.basis, PricingBasis::ApiReferenceOnly);
        assert!(policy.supported_retention.is_empty());
    }

    #[test]
    fn provider_assumptions_and_explicit_anthropic_retention_have_distinct_semantics() {
        for (provider, model, seconds) in [
            ("openai", "openai/gpt-6-astra", 300),
            ("openai", "openai/gpt-5.4", 300),
            ("openai", "future-model", 300),
            ("openai-codex", "openai-codex/gpt-6-luna", 300),
            ("anthropic", "anthropic/claude-opus-5", 300),
            ("anthropic", "future-model", 300),
        ] {
            for selection in [None, Some("auto")] {
                let policy = CachePolicy::for_route(provider, model, selection);
                assert_eq!(policy.ttl, CacheTtl::Assumed { seconds });
            }
        }
        let short = CachePolicy::for_route("anthropic", "claude-opus-5", Some("5m"));
        assert_eq!(short.ttl, CacheTtl::Minimum { seconds: 300 });
        let long = CachePolicy::for_route("anthropic", "claude-opus-5", Some("1h"));
        assert_eq!(long.ttl, CacheTtl::Minimum { seconds: 3600 });
        // Assuming retention does not fabricate a cache-write billing tier.
        assert_eq!(
            CachePolicy::for_route("anthropic", "claude-opus-5", None)
                .pricing
                .write_multiplier,
            None
        );
    }

    fn observation(at: u64, ttl: Option<u64>) -> CacheObservation {
        CacheObservation {
            completed_at_ms: at,
            ttl_seconds: ttl,
            retention: None,
        }
    }

    #[test]
    fn timer_is_visible_for_the_whole_window_and_distinguishes_provider_evidence() {
        for (ttl, seconds, prefix) in [(None, 300, "KV ~"), (Some(1800), 1800, "KV ")] {
            let at = observation(1000, ttl);
            let first = CacheWarning::for_response(&at, Duration::ZERO);
            assert_eq!(first.badge_label, format!("{prefix}{:02}:00", seconds / 60));
            assert!(!first.urgent);
            assert!(first.explanation.contains("finishes streaming"));
            assert!(first.tooltip.contains(if ttl.is_some() {
                "Provider-reported"
            } else {
                "Estimate:"
            }));
            let final_minute = CacheWarning::for_response(&at, Duration::from_secs(seconds - 60));
            assert!(final_minute.urgent);
            assert_eq!(final_minute.badge_label, format!("{prefix}01:00"));
            let fractional =
                CacheWarning::for_response(&at, Duration::from_millis(seconds * 1000 - 1));
            assert_eq!(fractional.badge_label, format!("{prefix}00:01"));
            let expired = CacheWarning::for_response(&at, Duration::from_secs(seconds + 999));
            assert_eq!(expired.badge_label, format!("{prefix}00:00"));
            assert!(expired.explanation.contains("not a confirmed expiry"));
        }
    }

    #[test]
    fn legacy_maximum_and_invalid_ttl_still_use_a_five_minute_estimate() {
        for ttl in [None, Some(0)] {
            let mut at = observation(1000, ttl);
            at.retention = Some("24h".into());
            let badge = CacheWarning::for_response(&at, Duration::ZERO);
            assert_eq!(badge.badge_label, "KV ~05:00");
            assert!(badge.tooltip.contains("24-hour maximum"));
        }
    }

    #[test]
    fn cache_clock_rejects_stale_reads_and_resets_metadata_on_each_response() {
        let mut clock = CacheClock::default();
        assert!(clock.warning(1000).is_none());
        clock.observe(Some(observation(1000, Some(1800))), 1000);
        assert_eq!(clock.warning(1000).unwrap().badge_label, "KV 30:00");
        clock.observe(Some(observation(1000, None)), 2000); // duplicate replay
        clock.observe(Some(observation(500, None)), 2000); // stale journal
        clock.observe(Some(observation(3000, None)), 2000); // future timestamp
        assert_eq!(clock.warning(2000).unwrap().badge_label, "KV 29:59");
        assert!(clock.warning(0).is_none());
        clock.observe(Some(observation(2000, None)), 2000); // next response omitted metadata
        assert_eq!(clock.warning(2000).unwrap().badge_label, "KV ~05:00");
        clock.reset(3000);
        clock.observe(Some(observation(3000, Some(1800))), 4000);
        assert!(clock.warning(4000).is_none());
        clock.observe(Some(observation(4000, None)), 4000);
        assert_eq!(clock.warning(5000).unwrap().badge_label, "KV ~04:59");
    }

    #[test]
    fn full_runtime_identifiers_resolve_the_same_policy() {
        assert_eq!(
            CachePolicy::for_route("anthropic", "anthropic/claude-opus-5", None),
            CachePolicy::for_route("anthropic", "claude-opus-5", None)
        );
        assert_eq!(
            CachePolicy::for_route("openai", "openai/gpt-6-astra", None).ttl,
            CacheTtl::Assumed { seconds: 300 }
        );
    }

    #[test]
    fn explicit_openai_policy_is_preserved_but_missing_retention_uses_a_conservative_estimate() {
        let policy = CachePolicy::for_route("openai", "gpt-6-astra", None);
        assert_eq!(policy.ttl, CacheTtl::Assumed { seconds: 300 });
        assert_eq!(policy.minimum_tokens, Some(1024));
        assert_eq!(policy.pricing.write_multiplier, Some(1.25));
        assert_eq!(
            CachePolicy::for_route("openai", "gpt-6-astra", Some("30m")).ttl,
            CacheTtl::Minimum { seconds: 1800 }
        );
        for selection in [Some("short"), Some("none"), Some("long")] {
            assert_eq!(
                CachePolicy::for_route("openai", "gpt-5.4", selection).ttl,
                CacheTtl::Unknown
            );
        }
    }

    #[test]
    fn anthropic_thresholds_ratios_and_retention_are_model_specific() {
        let fable = CachePolicy::for_route("anthropic", "claude-fable-5-1", Some("1h"));
        assert_eq!(fable.minimum_tokens, Some(512));
        assert_eq!(fable.pricing.read_multiplier, Some(0.025));
        assert_eq!(fable.pricing.write_multiplier, Some(2.0));
        assert_eq!(fable.ttl, CacheTtl::Minimum { seconds: 3600 });
        let opus = CachePolicy::for_route("anthropic", "claude-opus-4-6", None);
        assert_eq!(opus.minimum_tokens, Some(4096));
        assert!(opus.summary().contains("when caching is enabled"));
        assert!(opus.ttl_summary().contains("response completion"));
        assert_eq!(
            CachePolicy::for_route("anthropic", "claude-opus-5-5", None)
                .pricing
                .read_multiplier,
            Some(0.05)
        );
    }

    #[test]
    fn unknown_names_do_not_acquire_fabricated_ttls_or_prices() {
        for (provider, model) in [
            ("other", "gpt-6"),
            ("anthropic", "claude-opus-9"),
            ("openai", "gpt-6-invented"),
            ("openai", "gpt-10"),
        ] {
            let policy = CachePolicy::for_route(provider, model, Some("30m"));
            assert_eq!(policy.ttl, CacheTtl::Unknown);
            assert_eq!(policy.minimum_tokens, None);
            assert_eq!(policy.pricing.read_multiplier, None);
        }
        assert!(is_current_openai("gpt-5.6-2026-07-01"));
        assert_eq!(
            anthropic_model("claude-opus-4-5-20251101"),
            Some((4096, 0.1))
        );
    }

    #[test]
    fn unchanged_and_confirmed_append_only_actions_do_not_warn() {
        let policy = CachePolicy::for_route("anthropic", "claude-opus-4-6", None);
        for action in [
            CacheAction::ModelSwitch { from: "a", to: "a" },
            CacheAction::AccountSwitch { from: "a", to: "a" },
            CacheAction::ReasoningChange {
                from: "high",
                to: "high",
            },
            CacheAction::FastModeChange {
                from: false,
                to: false,
            },
            CacheAction::ImagePresenceChange {
                from: true,
                to: true,
            },
            CacheAction::AppendMessage,
            CacheAction::AppendOnlyReasoningChange,
        ] {
            assert_eq!(policy.impact(action), None);
        }
    }

    #[test]
    fn warnings_preserve_unaffected_prefix_scope() {
        let policy = CachePolicy::for_route("anthropic", "claude-opus-4-6", None);
        assert_eq!(
            policy
                .impact(CacheAction::FastModeChange {
                    from: false,
                    to: true
                })
                .unwrap()
                .scope,
            CacheScope::SystemAndMessages
        );
        assert_eq!(
            policy
                .impact(CacheAction::ImagePresenceChange {
                    from: false,
                    to: true
                })
                .unwrap()
                .scope,
            CacheScope::Messages
        );
        assert_eq!(
            policy.impact(CacheAction::HistoryEdit).unwrap().scope,
            CacheScope::ChangedSuffix
        );
        assert_eq!(
            policy
                .impact(CacheAction::ReasoningChange {
                    from: "low",
                    to: "high"
                })
                .unwrap()
                .scope,
            CacheScope::ModelDependentPrefix
        );
    }
}
