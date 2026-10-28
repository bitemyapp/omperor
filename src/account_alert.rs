//! Credential failures are independent of banner visibility. Only live errors
//! create alerts; account listings are not evidence that a credential recovered.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::AccountEntry;
use crate::omp_runtime::{self, AuthProvider, SubagentInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alert {
    pub(crate) account_id: Option<String>,
    pub(crate) provider: Option<AuthProvider>,
    pub(crate) display: String,
    pub(crate) reason: &'static str,
}

impl Alert {
    fn key(&self) -> String {
        self.account_id
            .as_ref()
            .map(|id| format!("account:{id}"))
            .unwrap_or_else(|| {
                format!(
                    "provider:{}",
                    self.provider.map_or("unknown", AuthProvider::id)
                )
            })
    }

    pub(crate) fn message(&self) -> String {
        let subject = if self.account_id.is_some() {
            format!("Account “{}”", self.display)
        } else if let Some(provider) = self.provider {
            format!("{} authorization", provider.product_name())
        } else {
            "Agent authorization".into()
        };
        format!("{subject}: {}. Sign in again in Accounts.", self.reason)
    }
}

fn mentions(error: &str, identity: &str) -> bool {
    if identity.is_empty() {
        return false;
    }
    let identity_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '@');
    error.match_indices(identity).any(|(offset, _)| {
        !error[..offset]
            .chars()
            .next_back()
            .is_some_and(identity_char)
            && !error[offset + identity.len()..]
                .chars()
                .next()
                .is_some_and(identity_char)
    })
}

/// A primary's confirmed account is a valid fallback only for that provider.
/// Subagents do not report their account, so callers must pass no fallback.
pub(crate) fn resolve(
    error: &str,
    accounts: &[AccountEntry],
    primary: Option<&AccountEntry>,
    provider_hint: Option<AuthProvider>,
) -> Option<Alert> {
    if !omp_runtime::is_account_auth_failure(error) {
        return None;
    }
    let provider = omp_runtime::provider_in_error(error).or(provider_hint);
    let eligible = |entry: &&AccountEntry| provider.is_none_or(|p| entry.provider() == p);
    let ids: Vec<_> = accounts
        .iter()
        .filter(eligible)
        .filter(|a| mentions(error, a.id()))
        .collect();
    let principals: Vec<_> = accounts
        .iter()
        .filter(eligible)
        .filter(|a| {
            a.summary
                .principal
                .as_deref()
                .is_some_and(|p| p.len() > 3 && mentions(error, p))
        })
        .collect();
    let account = if ids.len() == 1 {
        Some(ids[0])
    } else if ids.len() > 1 || principals.len() > 1 {
        None
    } else if principals.len() == 1 {
        Some(principals[0])
    } else {
        // An explicitly named but unknown account must not be reassigned to
        // the primary. OMP uses “account <id>” for missing credentials.
        primary.filter(|a| {
            provider.is_none_or(|p| a.provider() == p)
                && !error.to_ascii_lowercase().contains(" account ")
        })
    };
    let lower = error.to_ascii_lowercase();
    Some(Alert {
        account_id: account.map(|a| a.id().to_owned()),
        provider: account.map(AccountEntry::provider).or(provider),
        display: account.map(AccountEntry::display_name).unwrap_or_default(),
        reason: if lower.contains("revoked") || lower.contains("invalidated oauth token") {
            "sign-in revoked"
        } else if lower.contains("no credential is stored") {
            "credential missing"
        } else if lower.contains("expired") {
            "authorization expired"
        } else {
            "authorization rejected"
        },
    })
}

#[derive(Debug, Default)]
pub(crate) struct Alerts {
    failures: Vec<Alert>,
    /// None means dismissed for this launch; Some is a monotonic snooze.
    suppressed: HashMap<String, Option<Instant>>,
}

impl Alerts {
    pub(crate) fn record(&mut self, alert: Alert) -> bool {
        if let Some(existing) = self.failures.iter_mut().find(|a| a.key() == alert.key()) {
            if *existing == alert {
                return false;
            }
            *existing = alert;
        } else {
            self.failures.push(alert);
        }
        true
    }

    pub(crate) fn visible(&self, now: Instant) -> Option<&Alert> {
        self.failures
            .iter()
            .find(|a| match self.suppressed.get(&a.key()) {
                None => true,
                Some(Some(until)) => now >= *until,
                Some(None) => false,
            })
    }

    pub(crate) fn suppress(&mut self, alert: &Alert, later: bool, now: Instant) {
        self.suppressed
            .insert(alert.key(), later.then(|| now + Duration::from_secs(1800)));
    }

    pub(crate) fn for_provider(&self, provider: AuthProvider) -> Option<&Alert> {
        self.failures
            .iter()
            .find(|a| a.account_id.is_none() && a.provider == Some(provider))
    }

    pub(crate) fn for_account(&self, id: &str) -> Option<&Alert> {
        self.failures
            .iter()
            .find(|a| a.account_id.as_deref() == Some(id))
    }

    /// Call only for a successful login or authenticated inference, never for
    /// an account list's “active” flag or a successful unrelated account.
    pub(crate) fn recovered(&mut self, id: &str, provider: AuthProvider, signed_in: bool) -> bool {
        let previous = self.failures.len();
        self.failures.retain(|a| {
            let recovered = a.account_id.as_deref() == Some(id)
                || (signed_in && a.account_id.is_none() && a.provider == Some(provider));
            if recovered {
                self.suppressed.remove(&a.key());
            }
            !recovered
        });
        previous != self.failures.len()
    }

    pub(crate) fn retain_accounts(&mut self, accounts: &[AccountEntry]) {
        self.failures.retain(|a| {
            let keep = a
                .account_id
                .as_ref()
                .is_none_or(|id| accounts.iter().any(|a| a.id() == id));
            if !keep {
                self.suppressed.remove(&a.key());
            }
            keep
        });
    }
}

/// An initial snapshot contains history. Alert only for a task observed alive
/// in this runtime, or one started since this runtime became ready.
pub(crate) fn fresh_subagent_failure<'a>(
    previous: Option<&SubagentInfo>,
    next: &'a SubagentInfo,
    live_since_ms: u64,
) -> Option<&'a str> {
    if next.status != "failed" {
        return None;
    }
    if previous.is_some_and(|p| p.status == "failed" && p.error == next.error) {
        return None;
    }
    let was_running = previous.is_some_and(|p| matches!(p.status.as_str(), "starting" | "running"));
    let started_here = live_since_ms > 0 && next.started_ms.is_some_and(|t| t >= live_since_ms);
    (was_running || started_here)
        .then_some(next.error.as_deref())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::omp_runtime::{AccountState, AccountSummary};

    fn account(id: &str, provider: AuthProvider) -> AccountEntry {
        AccountEntry::joined(
            AccountSummary {
                id: id.into(),
                provider,
                principal: None,
                state: AccountState::Active,
            },
            &[],
        )
    }

    #[test]
    fn subagent_provider_never_inherits_the_primary_account() {
        let accounts = vec![
            account("openai-codex:one", AuthProvider::OpenAi),
            account("anthropic:one", AuthProvider::Anthropic),
        ];
        let alert = resolve(
            "anthropic authentication failed (HTTP 401)",
            &accounts,
            Some(&accounts[0]),
            None,
        )
        .unwrap();
        assert_eq!(alert.provider, Some(AuthProvider::Anthropic));
        assert_eq!(alert.account_id, None);
        let unknown = resolve("HTTP 401", &accounts, None, None).unwrap();
        assert_eq!(unknown.account_id, None);
        assert_eq!(unknown.provider, None);
        assert_eq!(
            resolve("HTTP 401", &accounts, Some(&accounts[0]), None)
                .unwrap()
                .account_id
                .as_deref(),
            Some("openai-codex:one")
        );
    }

    #[test]
    fn an_account_named_by_omp_outranks_the_primary_pin() {
        let accounts = vec![
            account("openai-codex:dos", AuthProvider::OpenAi),
            account("openai-codex:tres", AuthProvider::OpenAi),
        ];
        // OMP closes a credential failure with the account that served the
        // failing attempt. A child the balancer routed onto another account
        // must be blamed on that account, never on the primary's pin, which
        // would name the wrong account to the user.
        let error = "inference Authentication error during Handshake (token_revoked) [http 401]: \
                     Encountered invalidated oauth token for user, failing request for account \
                     openai-codex:tres";
        let alert = resolve(error, &accounts, Some(&accounts[0]), None).unwrap();
        assert_eq!(alert.account_id.as_deref(), Some("openai-codex:tres"));
        assert_eq!(alert.reason, "sign-in revoked");
        assert_eq!(alert.provider, Some(AuthProvider::OpenAi));
    }

    #[test]
    fn account_names_are_exact_and_ambiguous_principals_stay_unattributed() {
        let mut accounts = vec![
            account("openai-codex:a", AuthProvider::OpenAi),
            account("openai-codex:abc", AuthProvider::OpenAi),
        ];
        let alert = resolve(
            "no credential is stored for openai-codex account openai-codex:abc; sign in again",
            &accounts,
            None,
            None,
        )
        .unwrap();
        assert_eq!(alert.account_id.as_deref(), Some("openai-codex:abc"));
        assert!(resolve("no credential is stored for openai-codex account openai-codex:absent; sign in again", &accounts, Some(&accounts[0]), None).unwrap().account_id.is_none());
        for a in &mut accounts {
            a.summary.principal = Some("same@example.test".into());
        }
        assert!(
            resolve(
                "authentication failed for same@example.test",
                &accounts,
                Some(&accounts[0]),
                None
            )
            .unwrap()
            .account_id
            .is_none()
        );
    }

    #[test]
    fn snooze_dismiss_and_recovery_preserve_independent_failures() {
        let now = Instant::now();
        let accounts = vec![
            account("one", AuthProvider::OpenAi),
            account("two", AuthProvider::OpenAi),
        ];
        let one = resolve("HTTP 401", &accounts, Some(&accounts[0]), None).unwrap();
        let two = resolve("HTTP 401", &accounts, Some(&accounts[1]), None).unwrap();
        let mut state = Alerts::default();
        state.record(one.clone());
        state.record(two.clone());
        state.suppress(&one, true, now);
        assert_eq!(state.visible(now), Some(&two));
        state.suppress(&two, false, now);
        state.record(one.clone()); // Duplicate events must not undo snoozing.
        state.retain_accounts(&accounts); // “Active” is not recovery.
        assert!(state.visible(now).is_none());
        assert!(state.for_account("one").is_some() && state.for_account("two").is_some());
        assert_eq!(state.visible(now + Duration::from_secs(1800)), Some(&one));
        state.recovered("one", AuthProvider::OpenAi, false);
        assert!(!state.for_account("one").is_some() && state.for_account("two").is_some());
        assert!(state.visible(now + Duration::from_secs(3600)).is_none());
        state.record(one.clone());
        assert_eq!(state.visible(now), Some(&one));
    }

    #[test]
    fn another_working_account_does_not_clear_an_unattributed_failure() {
        let now = Instant::now();
        let account = account("openai-codex:one", AuthProvider::OpenAi);
        let alert = resolve("openai-codex HTTP 401", &[], None, None).unwrap();
        let mut state = Alerts::default();
        assert!(state.record(alert.clone()));
        assert!(!state.record(alert.clone()));
        assert!(!state.recovered(account.id(), account.provider(), false));
        assert_eq!(state.visible(now), Some(&alert));
        assert!(state.recovered(account.id(), account.provider(), true));
        assert!(state.visible(now).is_none());
    }

    #[test]
    fn restored_failures_are_history_but_live_transitions_raise_alerts() {
        let mut task = SubagentInfo {
            id: "t".into(),
            index: 0,
            agent: "task".into(),
            label: "task".into(),
            status: "failed".into(),
            started_ms: Some(10),
            duration_ms: None,
            error: Some("HTTP 401".into()),
            warning: None,
            session_file: None,
        };
        assert_eq!(fresh_subagent_failure(None, &task, 100), None);
        assert_eq!(fresh_subagent_failure(Some(&task), &task, 100), None);
        let mut running = task.clone();
        running.status = "running".into();
        running.error = None;
        assert_eq!(
            fresh_subagent_failure(Some(&running), &task, 100),
            Some("HTTP 401")
        );
        task.started_ms = Some(101);
        assert_eq!(fresh_subagent_failure(None, &task, 100), Some("HTTP 401"));
    }
}
