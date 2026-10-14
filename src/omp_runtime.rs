use std::{
    collections::{HashMap, VecDeque},
    env,
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use flume::{Receiver, Sender};
use omp_rpc::{
    client::{ClientHostTool, HostToolContext, HostToolError, RpcClient, RpcClientOptions},
    protocol::{
        Environment, RpcAccount, RpcAccountState, RpcAuthAnswerFrame, RpcAuthInputKind,
        RpcAuthMethod, RpcEvent,
    },
};
use serde_json::Value;

use crate::attachments::PreparedPrompt;
use crate::{mock_inference::Backend, perf};

mod mock_runtime;

const DEFAULT_MODEL: &str = "openai-codex/gpt-5.6-luna";
const OPENAI_PROVIDER: &str = "openai-codex";
const OPENAI_API_PROVIDER: &str = "openai";
const ANTHROPIC_PROVIDER: &str = "anthropic";
const OMP_READY_TIMEOUT: Duration = Duration::from_secs(15);
// Serialize shared-store initialization, not interactive sessions. Acquired only
// asynchronously on RPC workers, never on the AppKit thread.
static STARTUP_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Native tool roster for GUI sessions, restricted to names the journal-first
/// OMP registry validates at launch.
/// The tools a native session advertises. `task` spawns sub-agents and `hub`
/// lets the agent wait on them, which is what the Agents panel shows.
// The pinned Codex provider has no standalone Search operation. Browser tools
// remain available for research; don't advertise the unsupported search route.
const NATIVE_GUI_TOOLS: &str = "ast_edit,bash,browser,debug,edit,glob,grep,hub,read,task,think,todo,write";

#[derive(Debug)]
enum Command {
    ExpectedGeneration(u64, Box<Command>),
    Prompt(PreparedPrompt),
    Steer(PreparedPrompt),
    Abort,
    ReleaseSession,
    /// Explicit retry after a failure before any session identity was announced.
    RetryStartup,
    ResumeSession {
        session_id: String,
        project: PathBuf,
    },
    SetModel(String),
    SetThinkingLevel(String),
    /// Ask OMP for, or stop asking for, the OpenAI `priority` tier on this
    /// session: what the ChatGPT client calls Fast.
    SetFastMode(bool),
    /// Offer the agent the `omperor_` tools in this session, or stop.
    SetOmperorTools(bool),
    SetAccount(String),
    LogoutAccount(String),
    /// Re-run account model discovery, then relaunch the child on the same
    /// session so its catalog picks up the widened list.
    RefreshModels,
    /// Read the child's sub-agent board again.
    RefreshSubagents,
    /// Ask every stored account's provider for its plan and quota windows.
    RefreshAccountUsage,
    /// Start signing in; `generation` numbers the host's attempts so a
    /// stale acknowledgement is not taken for the current one.
    Login(AuthProvider, AuthMethod, u64),
    AnswerAuthentication(AuthPrompt, String),
    /// Ends the login session OMP opened, whatever it is waiting on.
    CancelAuthentication(String),
    Shutdown,
}

impl Command {
    fn kind(&self) -> &'static str {
        match self {
            Self::ExpectedGeneration(_, command) => command.kind(),
            Self::Prompt(_) => "prompt",
            Self::Steer(_) => "steer",
            Self::Abort => "abort",
            Self::ReleaseSession => "release_session",
            Self::RetryStartup => "retry_startup",
            Self::ResumeSession { .. } => "resume_session",
            Self::SetModel(_) => "set_model",
            Self::SetThinkingLevel(_) => "set_thinking_level",
            Self::SetFastMode(_) => "set_fast_mode",
            Self::SetOmperorTools(_) => "set_omperor_tools",
            Self::SetAccount(_) => "set_account",
            Self::LogoutAccount(_) => "logout_account",
            Self::RefreshModels => "refresh_models",
            Self::RefreshSubagents => "refresh_subagents",
            Self::RefreshAccountUsage => "refresh_account_usage",
            Self::Login(provider, _, _) => provider.login_command_kind(),
            Self::CancelAuthentication(_) => "cancel_authentication",
            Self::AnswerAuthentication(..) => "answer_authentication",
            Self::Shutdown => "shutdown",
        }
    }

    fn payload_bytes(&self) -> usize {
        match self {
            Self::Prompt(value) | Self::Steer(value) => value.payload_bytes(),
            Self::SetModel(value)
            | Self::SetThinkingLevel(value)
            | Self::SetAccount(value)
            | Self::LogoutAccount(value) => value.len(),
            Self::AnswerAuthentication(_, value) => value.len(),
            Self::ResumeSession { session_id, .. } => session_id.len(),
            _ => 0,
        }
    }
}

/// Complete launch intent, supplied before any child is created.
#[derive(Debug, Clone)]
pub(crate) enum SessionStartup {
    Fresh,
    Resume(String),
    Fork(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeStartup {
    /// Explicit helper for headless runs sharing another channel’s store.
    pub(crate) helper: Option<PathBuf>,
    pub(crate) backend: Backend,
    pub(crate) project: PathBuf,
    pub(crate) model: Option<String>,
    pub(crate) thinking_level: Option<String>,
    pub(crate) fast_mode: bool,
    /// Whether the agent is offered the `omperor_` tools from the start.
    pub(crate) omperor_tools: bool,
    pub(crate) computer_use: bool,
    pub(crate) account: Option<String>,
    pub(crate) data_dir: PathBuf,
    pub(crate) session_dir: PathBuf,
    /// Extra variables for the child and the tools it runs (an eval task's
    /// `[env]`). The data-store pins always win over these.
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) session: SessionStartup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchConfiguration {
    helper: Option<PathBuf>,
    generation: u64,
    backend: Backend,
    project: PathBuf,
    model: String,
    thinking_level: Option<String>,
    fast_mode: bool,
    /// Whether the agent is offered the `omperor_` tools; re-offered to every
    /// child launched for the session.
    omperor_tools: bool,
    computer_use: bool,
    /// Where calls to those tools wait for the app's answer.
    host_calls: HostCalls,
    /// Account the GUI wants pinned for the active model's provider. It is
    /// re-applied after every child launch so recovery, new sessions, and
    /// resume keep serving from the account the user chose.
    account: Option<String>,
    data_dir: PathBuf,
    session_dir: PathBuf,
    environment: Vec<(String, String)>,
    resume: Option<String>,
    /// Session whose transcript seeds the next launch. Cleared once the child
    /// reports the new session so recovery resumes it instead of forking again.
    fork: Option<String>,
}

/// Lifecycle state of one account stored by OMP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountState {
    Active,
    RefreshRequired,
    Disabled,
    LoggedOut,
}

impl AccountState {
    /// Whether OMP would accept the account as a session pin.
    pub(crate) const fn is_usable(self) -> bool {
        matches!(self, Self::Active | Self::RefreshRequired)
    }
}

/// One account reported by OMP's credential store for a supported provider.
/// What an account's provider says about the account: the plan it is on and
/// how much of each quota window it has used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountUsage {
    pub(crate) account_id: String,
    /// The provider's plan name as reported (`pro`, `team`, …), if it says.
    pub(crate) plan: Option<String>,
    pub(crate) windows: Vec<UsageWindow>,
    /// Why the provider could not answer, when it could not.
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageWindow {
    /// The window's span as the provider names it: "5 hours", "7 days",
    /// with the feature it is scoped to in parentheses when it has one.
    pub(crate) label: String,
    /// The feature the window meters: `shared` for the account as a whole,
    /// or a feature slug such as `spark`.
    pub(crate) scope: Option<String>,
    /// Whole percent of the window used, when the provider says.
    pub(crate) used_percent: Option<u8>,
    pub(crate) resets_at_ms: Option<u64>,
    /// What a billing window has spent against its cap, when the window is
    /// metered in money rather than percent.
    pub(crate) spend: Option<Spend>,
}

/// Money spent against a cap, in cents so it compares exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Spend {
    pub(crate) used_cents: u64,
    pub(crate) limit_cents: Option<u64>,
}

impl Spend {
    /// The amounts of a `usd` window; other units are not money.
    fn from_window(window: &Value) -> Option<Self> {
        if window.get("unit").and_then(Value::as_str) != Some("usd") {
            return None;
        }
        let cents = |key: &str| {
            window
                .get(key)
                .and_then(Value::as_f64)
                .map(|dollars| (dollars * 100.0).round().max(0.0) as u64)
        };
        Some(Self {
            used_cents: cents("used")?,
            limit_cents: cents("limit"),
        })
    }
}

impl AccountUsage {
    /// One account's entry from a `get_account_usage` answer.
    fn from_value(value: &Value) -> Option<Self> {
        let account_id = value.get("accountId")?.as_str()?.to_owned();
        let windows = value
            .get("windows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|window| {
                Some(UsageWindow {
                    label: window.get("label")?.as_str()?.to_owned(),
                    scope: window
                        .get("scope")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    used_percent: window
                        .get("usedPercent")
                        .and_then(Value::as_f64)
                        .map(|percent| percent.round().clamp(0.0, 100.0) as u8),
                    resets_at_ms: window.get("resetsAtMs").and_then(Value::as_u64),
                    spend: Spend::from_window(window),
                })
            })
            .collect();
        Some(Self {
            account_id,
            plan: value.get("plan").and_then(Value::as_str).map(str::to_owned),
            windows,
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

/// Every account's usage from a `get_account_usage` answer.
fn account_usages(value: &Value) -> Vec<AccountUsage> {
    value
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(AccountUsage::from_value)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountSummary {
    pub(crate) id: String,
    pub(crate) provider: AuthProvider,
    /// Public principal when the provider exposes one. Anthropic reports the
    /// subscriber email; OpenAI Codex reports an opaque workspace identifier.
    pub(crate) principal: Option<String>,
    pub(crate) state: AccountState,
}

impl AccountSummary {
    /// The principal when it reads as an email address, which is the only
    /// principal form worth showing to a person.
    pub(crate) fn email(&self) -> Option<&str> {
        self.principal
            .as_deref()
            .filter(|principal| looks_like_email(principal))
    }

    fn from_rpc(account: &RpcAccount) -> Option<Self> {
        Some(Self {
            id: account.account_id.clone(),
            provider: AuthProvider::from_id(&account.provider_id)?,
            principal: account.principal.clone(),
            state: match account.state {
                RpcAccountState::Active => AccountState::Active,
                RpcAccountState::RefreshRequired => AccountState::RefreshRequired,
                RpcAccountState::Disabled => AccountState::Disabled,
                RpcAccountState::LoggedOut => AccountState::LoggedOut,
            },
        })
    }
}

pub(crate) fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.chars().any(char::is_whitespace)
}

fn supported_accounts(accounts: &[RpcAccount]) -> Vec<AccountSummary> {
    accounts
        .iter()
        .filter_map(AccountSummary::from_rpc)
        .filter(|account| account.state != AccountState::LoggedOut)
        .collect()
}

/// A provider the user can sign in to. The two subscriptions sign in through
/// a browser or a pasted credential; the OpenAI API is a separate provider
/// in OMP, with its own models and per-token billing, reached only by key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AuthProvider {
    OpenAi,
    OpenAiApi,
    Anthropic,
}

/// How a sign-in proceeds. Which of these a provider offers is fixed by
/// OMP's catalog; `AuthProvider::methods` says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AuthMethod {
    /// Sign in on the provider's website; the browser returns to the app.
    Browser,
    /// Paste an API key.
    ApiKey,
    /// Paste an access token obtained elsewhere, such as from another
    /// machine that is already signed in.
    Token,
}

impl AuthMethod {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Browser => "In your browser",
            Self::ApiKey => "API key",
            Self::Token => "Access token",
        }
    }

    const fn rpc(self) -> RpcAuthMethod {
        match self {
            Self::Browser => RpcAuthMethod::OauthPkce,
            Self::ApiKey | Self::Token => RpcAuthMethod::ApiKey,
        }
    }
}

impl AuthProvider {
    pub(crate) const ALL: [Self; 3] = [Self::OpenAi, Self::OpenAiApi, Self::Anthropic];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::OpenAi => OPENAI_PROVIDER,
            Self::OpenAiApi => OPENAI_API_PROVIDER,
            Self::Anthropic => ANTHROPIC_PROVIDER,
        }
    }

    pub(crate) const fn product_name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::OpenAiApi => "OpenAI API",
            Self::Anthropic => "Claude",
        }
    }

    /// What the account is: the noun a sign-in sheet and an accounts card
    /// use for it.
    pub(crate) const fn account_noun(self) -> &'static str {
        match self {
            Self::OpenAi => "ChatGPT subscription",
            Self::OpenAiApi => "OpenAI API key",
            Self::Anthropic => "Claude subscription",
        }
    }

    /// The ways this provider can be signed in to, the usual one first.
    pub(crate) const fn methods(self) -> &'static [AuthMethod] {
        match self {
            Self::OpenAi => &[AuthMethod::Browser, AuthMethod::Token],
            Self::OpenAiApi => &[AuthMethod::ApiKey],
            Self::Anthropic => &[AuthMethod::Browser, AuthMethod::ApiKey],
        }
    }

    pub(crate) fn for_model(model: &str) -> Option<Self> {
        let provider = model
            .split_once('/')
            .map_or(model, |(provider, _)| provider);
        Self::from_id(provider)
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            OPENAI_PROVIDER => Some(Self::OpenAi),
            OPENAI_API_PROVIDER => Some(Self::OpenAiApi),
            ANTHROPIC_PROVIDER => Some(Self::Anthropic),
            _ => None,
        }
    }

    const fn login_command_kind(self) -> &'static str {
        match self {
            Self::OpenAi => "login_openai",
            Self::OpenAiApi => "login_openai_api",
            Self::Anthropic => "login_anthropic",
        }
    }
}

pub(crate) fn is_supported_subscription_model(model: &str) -> bool {
    let Some((provider, model_id)) = model.split_once('/') else {
        return false;
    };
    match provider {
        // OMP's OpenAI Codex route is itself subscription-specific; its
        // catalog entries are the models advertised for that route. The API
        // route lists what a key can reach.
        OPENAI_PROVIDER | OPENAI_API_PROVIDER => !model_id.is_empty(),
        // Anthropic availability is account-specific and comes from OMP's
        // authenticated runtime discovery rather than a GUI-side whitelist.
        ANTHROPIC_PROVIDER => !model_id.is_empty(),
        _ => false,
    }
}

pub(crate) fn sanitized_model_preference(model: Option<&str>) -> Option<String> {
    let model = model?.trim();
    if let Some(model_id) = model
        .strip_prefix("anthropic/")
        .and_then(|model_id| model_id.strip_suffix("[1m]"))
        .filter(|model_id| !model_id.is_empty())
    {
        return Some(format!("anthropic/{model_id}"));
    }
    if is_supported_subscription_model(model) {
        return Some(model.to_owned());
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthPrompt {
    pub(crate) session_id: String,
    pub(crate) prompt_id: String,
    pub(crate) message: String,
    input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolOutcome {
    pub(crate) call_id: String,
    pub(crate) name: Option<String>,
    pub(crate) is_error: Option<bool>,
    pub(crate) result: Option<Value>,
}

/// One inference response's accounting, as OMP journals it: the provider's
/// token counts and OMP's own timings from the request leaving until the
/// first and last streamed token arrived.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct InferenceReceipt {
    /// Stable journaled receipt identity; absent on older helpers.
    pub(crate) receipt_id: Option<String>,
    pub(crate) recorded_at_ms: Option<u64>,
    /// `provider`, `measured`, `estimated`, `mixed`, or `unknown`; absent is unknown.
    pub(crate) usage_source: Option<String>,
    /// Amounts are estimates from the helper's model catalog, not billing receipts.
    pub(crate) cost_source: Option<String>,
    pub(crate) cost_nano_usd: Option<u64>,
    pub(crate) cache_write_1h: Option<u64>,
    pub(crate) cache_read_reported: Option<bool>,
    pub(crate) cache_write_reported: Option<bool>,
    pub(crate) cache_write_1h_reported: Option<bool>,
    pub(crate) prompt_cache_ttl_seconds: Option<u64>,
    pub(crate) prompt_cache_retention: Option<String>,
    pub(crate) response_completed_at_ms: Option<u64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) tokens_in: u64,
    /// Output tokens as the provider counts them, reasoning included.
    pub(crate) tokens_out: u64,
    pub(crate) tokens_reasoning: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) last_token_ms: Option<u64>,
    pub(crate) duration_ms: Option<u64>,
}

impl InferenceReceipt {
    fn from_event(event: &RpcEvent) -> Self {
        let text = |key: &str| {
            event
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let count = |key: &str| event.get(key).and_then(Value::as_u64);
        let reported = |key: &str| event.get(key).and_then(Value::as_bool);
        Self {
            receipt_id: text("receiptId"),
            recorded_at_ms: count("recordedAtMs"),
            usage_source: text("usageSource"),
            cost_source: text("costSource"),
            cost_nano_usd: count("costNanoUsd"),
            cache_write_1h: count("cacheWrite1h"),
            cache_read_reported: reported("cacheReadReported"),
            cache_write_reported: reported("cacheWriteReported"),
            cache_write_1h_reported: reported("cacheWrite1hReported"),
            prompt_cache_ttl_seconds: count("promptCacheTtlSeconds"),
            prompt_cache_retention: text("promptCacheRetention"),
            response_completed_at_ms: count("responseCompletedAtMs"),
            provider: text("provider"),
            model: text("model"),
            tokens_in: count("tokensIn").unwrap_or(0),
            tokens_out: count("tokensOut").unwrap_or(0),
            tokens_reasoning: count("tokensReasoning").unwrap_or(0),
            cache_read: count("cacheRead").unwrap_or(0),
            cache_write: count("cacheWrite").unwrap_or(0),
            ttft_ms: count("ttftMs"),
            last_token_ms: count("lastTokenMs"),
            duration_ms: count("durationMs"),
        }
    }
}

/// One sub-agent on the child's job board: spawned by the session's `task`
/// tool, still running or settled, with the journal its transcript lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentInfo {
    pub(crate) id: String,
    /// Spawn order.
    pub(crate) index: u64,
    /// Agent class: `scout`, `reviewer`, or the default `task`.
    pub(crate) agent: String,
    /// The label OMP recorded for the child; its id until it has one.
    pub(crate) label: String,
    /// `starting`, `running`, `completed`, `failed`, or `cancelled`.
    pub(crate) status: String,
    pub(crate) started_ms: Option<u64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) error: Option<String>,
    /// The task was done but something about its workspace was not, such as
    /// its changes not being merged back.
    pub(crate) warning: Option<String>,
    pub(crate) session_file: Option<PathBuf>,
}

/// Reads one sub-agent from OMP's snapshot object, as `get_subagents` lists
/// them and as a `subagent_progress` frame carries one.
pub(crate) fn subagent_info(value: &Value) -> Option<SubagentInfo> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let id = text("id")?;
    Some(SubagentInfo {
        index: value.get("index").and_then(Value::as_u64).unwrap_or(0),
        agent: text("agentClass")
            .or_else(|| text("agent"))
            .unwrap_or_else(|| "task".to_owned()),
        label: text("description").unwrap_or_else(|| id.clone()),
        status: text("status").unwrap_or_else(|| "running".to_owned()),
        started_ms: value.get("startedMs").and_then(Value::as_u64),
        duration_ms: value.get("durationMs").and_then(Value::as_u64),
        error: text("error"),
        warning: text("warning"),
        session_file: text("sessionFile").map(PathBuf::from),
        id,
    })
}

/// How a turn ended, as OMP reports it on `agent_end`. Individual tool-call
/// failures do not make a turn fail — the model routinely retries them — so
/// only these terminal states are surfaced to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnOutcome {
    /// The turn ran to completion (including after the user steered it).
    Completed,
    /// The turn was cancelled before it finished (a Stop, or an abort).
    Interrupted,
    /// The turn ended with an error; the string is OMP's reason.
    Errored(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiEvent {
    /// FIFO boundary for the child producing subsequent events.
    RuntimeGeneration(u64),
    /// The child has exited and relinquished its exclusive journal writer.
    SessionReleased,
    Connecting {
        executable: PathBuf,
        project: PathBuf,
    },
    Ready {
        model: String,
        models: Vec<String>,
        /// The models among `models` whose backend advertises the OpenAI
        /// `priority` tier for this account, so a Fast switch can be shown.
        fast_models: Vec<String>,
        /// Whether this session asks for that tier.
        fast_mode: bool,
        thinking_level: String,
        session_id: Option<String>,
        session_file: Option<PathBuf>,
        project: PathBuf,
        authenticated_providers: Vec<AuthProvider>,
        /// Every supported-provider account OMP currently stores, or `None`
        /// when the listing failed and only the per-provider flags are known.
        accounts: Option<Vec<AccountSummary>>,
        /// Account OMP confirmed as pinned for the launch's model provider.
        account: Option<String>,
        session_is_fresh: bool,
        /// Session this child was launched to resume, when it was. A child
        /// launched for anything else (the app's first session, a fresh
        /// chat) says nothing about a resume the user has since asked for.
        resumed: Option<String>,
    },
    ModelChanged(String),
    ThinkingLevelChanged(String),
    /// OMP confirmed whether the session asks for the OpenAI `priority` tier.
    FastModeChanged(bool),
    /// OMP confirmed the pinned account for the active provider, or cleared it.
    AccountChanged(Option<String>),
    /// The stored account set changed outside a relaunch, such as after a
    /// removal.
    AccountsChanged(Vec<AccountSummary>),
    /// Each stored account's plan and quota windows, as its provider reports.
    AccountUsage(Vec<AccountUsage>),
    Notice(String),
    AuthenticationStarted {
        provider: AuthProvider,
        generation: u64,
    },
    /// OMP acknowledged the login the host numbered `generation`; the
    /// session id is what a cancel names, and what later events carry.
    AuthenticationSession {
        generation: u64,
        session_id: String,
    },
    /// The page to sign in on, and OMP's loopback launcher for it, which
    /// redirects there and is what a browser on this machine is handed.
    OpenAuthenticationUrl {
        session_id: String,
        url: String,
        launch: Option<String>,
    },
    /// A device-code flow: the code to enter at the page.
    AuthenticationDeviceCode {
        session_id: String,
        code: String,
        verification_url: String,
    },
    AuthenticationWaiting {
        session_id: String,
        message: String,
    },
    AuthenticationPrompt(AuthPrompt),
    /// The sign-in ended because the user cancelled it.
    AuthenticationCancelled {
        session_id: String,
    },
    AuthenticationCompleted {
        session_id: String,
        provider: AuthProvider,
        account_id: Option<String>,
        principal: Option<String>,
        account: Option<String>,
    },
    /// The sign-in failed: OMP's session, when it got as far as one.
    AuthenticationFailed {
        session_id: Option<String>,
        generation: Option<u64>,
        error: String,
    },
    AuthenticationRequired {
        provider: AuthProvider,
        error: String,
    },
    Running,
    AssistantStarted,
    AssistantDelta(String),
    AssistantFinished(Option<String>),
    ThinkingDelta(String),
    /// One inference response settled; its receipt as OMP journaled it.
    InferenceReceipt(InferenceReceipt),
    TurnFinished(TurnOutcome),
    ToolStarted {
        call_id: String,
        name: String,
        /// Full arguments JSON, when OMP reports it up front (RPC mode does).
        args: Option<String>,
        /// The model's stated intent for the call, when present.
        intent: Option<String>,
    },
    ToolArguments {
        call_id: String,
        delta: String,
    },
    ToolUpdated {
        call_id: String,
        content: String,
        /// Reassembled arguments carried alongside a progress update.
        args: Option<String>,
    },
    ToolFinished(ToolOutcome),
    TurnResults(Vec<ToolOutcome>),
    /// The agent called one of the `omperor_` tools. The app answers with
    /// [`OmpRuntime::answer_host_tool`], naming `call`.
    HostToolCall {
        call: u64,
        tool: String,
        arguments: serde_json::Map<String, Value>,
    },
    /// The turn that made `call` ended before the app answered it.
    HostToolCancelled { call: u64 },
    /// OMP confirmed whether the agent is offered the `omperor_` tools.
    OmperorToolsChanged(bool),
    /// The child's whole sub-agent board, at startup and on request.
    SubagentsSnapshot(Vec<SubagentInfo>),
    /// One sub-agent's snapshot changed.
    SubagentChanged(SubagentInfo),
    /// A sub-agent started or settled; the board should be read again.
    SubagentsChanged,
    Failed(String),
    Stopped,
}

impl UiEvent {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::RuntimeGeneration(_) => "runtime_generation",
            Self::SessionReleased => "session_released",
            Self::Connecting { .. } => "connecting",
            Self::Ready { .. } => "ready",
            Self::ModelChanged(_) => "model_changed",
            Self::ThinkingLevelChanged(_) => "thinking_level_changed",
            Self::FastModeChanged(_) => "fast_mode_changed",
            Self::AccountChanged(_) => "account_changed",
            Self::AccountsChanged(_) => "accounts_changed",
            Self::AccountUsage(_) => "account_usage",
            Self::Notice(_) => "notice",
            Self::AuthenticationStarted { .. } => "authentication_started",
            Self::AuthenticationSession { .. } => "authentication_session",
            Self::OpenAuthenticationUrl { .. } => "open_authentication_url",
            Self::AuthenticationDeviceCode { .. } => "authentication_device_code",
            Self::AuthenticationCancelled { .. } => "authentication_cancelled",
            Self::AuthenticationWaiting { .. } => "authentication_waiting",
            Self::AuthenticationPrompt(_) => "authentication_prompt",
            Self::AuthenticationCompleted { .. } => "authentication_completed",
            Self::AuthenticationFailed { .. } => "authentication_failed",
            Self::AuthenticationRequired { .. } => "authentication_required",
            Self::Running => "running",
            Self::AssistantStarted => "assistant_started",
            Self::AssistantDelta(_) => "assistant_delta",
            Self::AssistantFinished(_) => "assistant_finished",
            Self::ThinkingDelta(_) => "thinking_delta",
            Self::InferenceReceipt(_) => "inference_receipt",
            Self::TurnFinished(_) => "turn_finished",
            Self::ToolStarted { .. } => "tool_started",
            Self::ToolArguments { .. } => "tool_arguments",
            Self::ToolUpdated { .. } => "tool_updated",
            Self::ToolFinished(_) => "tool_finished",
            Self::TurnResults(_) => "turn_results",
            Self::HostToolCall { .. } => "host_tool_call",
            Self::HostToolCancelled { .. } => "host_tool_cancelled",
            Self::OmperorToolsChanged(_) => "omperor_tools_changed",
            Self::SubagentsSnapshot(_) => "subagents_snapshot",
            Self::SubagentChanged(_) => "subagent_changed",
            Self::SubagentsChanged => "subagents_changed",
            Self::Failed(_) => "failed",
            Self::Stopped => "stopped",
        }
    }

    fn payload_bytes(&self) -> usize {
        match self {
            Self::AssistantDelta(value)
            | Self::ThinkingDelta(value)
            | Self::Notice(value)
            | Self::ModelChanged(value)
            | Self::ThinkingLevelChanged(value) => value.len(),
            Self::AssistantFinished(value) | Self::AccountChanged(value) => {
                value.as_ref().map_or(0, String::len)
            }
            Self::AccountsChanged(accounts) => {
                accounts.iter().map(|account| account.id.len()).sum()
            }
            Self::AccountUsage(usages) => usages
                .iter()
                .map(|usage| usage.account_id.len() + usage.windows.len() * 16)
                .sum(),
            Self::AuthenticationRequired { error, .. } => error.len(),
            Self::AuthenticationWaiting { message: value, .. }
            | Self::AuthenticationFailed { error: value, .. }
            | Self::AuthenticationSession {
                session_id: value, ..
            }
            | Self::AuthenticationCancelled { session_id: value } => value.len(),
            Self::OpenAuthenticationUrl { url, launch, .. } => {
                url.len() + launch.as_ref().map_or(0, String::len)
            }
            Self::AuthenticationDeviceCode {
                code,
                verification_url,
                ..
            } => code.len() + verification_url.len(),
            Self::AuthenticationCompleted {
                account_id,
                principal,
                account,
                ..
            } => [account_id, principal, account]
                .into_iter()
                .map(|value| value.as_ref().map_or(0, String::len))
                .sum(),
            Self::ToolStarted {
                call_id,
                name,
                args,
                intent,
            } => {
                call_id.len()
                    + name.len()
                    + args.as_ref().map_or(0, String::len)
                    + intent.as_ref().map_or(0, String::len)
            }
            Self::ToolArguments { call_id, delta } => call_id.len() + delta.len(),
            Self::HostToolCall { tool, arguments, .. } => {
                tool.len() + arguments.iter().map(|(key, value)| key.len() + value.to_string().len()).sum::<usize>()
            }
            Self::ToolUpdated {
                call_id,
                content,
                args,
            } => call_id.len() + content.len() + args.as_ref().map_or(0, String::len),
            _ => 0,
        }
    }
}

/// An event with the moment the runtime thread saw it, so what the
/// interface times (turns, tool calls) is stamped on arrival rather than at
/// the main thread's next poll.
pub(crate) type ArrivedEvent = (std::time::Instant, UiEvent);

#[derive(Debug, Clone)]
struct UiEventSender(Sender<ArrivedEvent>);

impl UiEventSender {
    fn send(&self, event: UiEvent) -> bool {
        let kind = event.kind();
        let payload_bytes = event.payload_bytes();
        perf::event_with(
            "omp.ui_event.enqueue",
            || serde_json::json!({ "event": kind, "payload_bytes": payload_bytes }),
        );
        // Streaming stays frame-batched. Lifecycle changes wake an idle GUI
        // immediately without requiring a permanent 30 Hz poll.
        let wake = !matches!(
            event,
            UiEvent::AssistantDelta(_)
                | UiEvent::ThinkingDelta(_)
                | UiEvent::ToolArguments { .. }
                | UiEvent::ToolUpdated { .. }
        );
        let sent = self.0.send((std::time::Instant::now(), event)).is_ok();
        #[cfg(not(test))]
        if sent && wake {
            crate::gui_wake::notify();
        }
        #[cfg(test)]
        let _ = wake;
        sent
    }
}

/// Host-tool calls waiting for the app's answer, shared by the runtime's
/// thread and the app. The app answers here directly rather than through
/// the command queue, where an answer could wait behind queued commands.
#[derive(Debug, Clone, Default)]
pub(crate) struct HostCalls(Arc<Mutex<HostCallTable>>);

#[derive(Debug, Default)]
struct HostCallTable {
    next: u64,
    waiting: HashMap<u64, flume::Sender<Result<Value, String>>>,
}

/// Launch configurations compare by intent; every launch of a session
/// shares its one table.
impl PartialEq for HostCalls {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for HostCalls {}

impl HostCalls {
    fn open(&self) -> (u64, flume::Receiver<Result<Value, String>>) {
        let (answer, answered) = flume::bounded(1);
        let mut table = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        table.next += 1;
        let call = table.next;
        table.waiting.insert(call, answer);
        (call, answered)
    }

    fn forget(&self, call: u64) {
        let mut table = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        table.waiting.remove(&call);
    }

    /// Delivers the app's answer; `false` when the call is no longer
    /// waiting (its turn was stopped).
    fn answer(&self, call: u64, result: Result<Value, String>) -> bool {
        let answer = {
            let mut table = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            table.waiting.remove(&call)
        };
        answer.is_some_and(|answer| answer.send(result).is_ok())
    }

    /// Waits for the app to answer one call, or for OMP to cancel it.
    async fn ask(
        &self,
        events: &UiEventSender,
        tool: String,
        arguments: serde_json::Map<String, Value>,
        cancelled: impl Future<Output = ()>,
    ) -> Result<Value, String> {
        let (call, answered) = self.open();
        if !events.send(UiEvent::HostToolCall { call, tool, arguments }) {
            self.forget(call);
            return Err("Omperor is shutting down".into());
        }
        tokio::select! {
            answer = answered.recv_async() => {
                answer.unwrap_or_else(|_| Err("Omperor stopped answering this call".into()))
            }
            () = cancelled => {
                self.forget(call);
                events.send(UiEvent::HostToolCancelled { call });
                Err("The call was cancelled".into())
            }
        }
    }
}

/// The `omperor_` tools, each forwarding its calls to the app.
fn omperor_host_tools(events: &UiEventSender, calls: &HostCalls) -> Vec<ClientHostTool> {
    crate::omperor_tools::definitions()
        .into_iter()
        .map(|definition| {
            let events = events.clone();
            let calls = calls.clone();
            let tool = definition.name.clone();
            ClientHostTool::new(definition, move |arguments, mut context: HostToolContext| {
                let events = events.clone();
                let calls = calls.clone();
                let tool = tool.clone();
                tokio::spawn(async move {
                    calls
                        .ask(&events, tool, arguments, async move { context.cancelled().await })
                        .await
                        .map_err(HostToolError::new)
                })
            })
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct OmpRuntime {
    commands: Sender<Command>,
    events: Receiver<ArrivedEvent>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    observed_generation: std::sync::atomic::AtomicU64,
    host_calls: HostCalls,
}

impl OmpRuntime {
    pub(crate) fn start_configured(startup: RuntimeStartup) -> Self {
        let (commands, command_rx) = flume::unbounded();
        let (event_tx, events) = flume::unbounded();
        let host_calls = HostCalls::default();
        let launch_calls = host_calls.clone();
        let worker = thread::Builder::new()
            .name("omperor-omp-rpc".into())
            .spawn(move || {
                let (resume, fork) = match startup.session {
                    SessionStartup::Fresh => (None, None),
                    SessionStartup::Resume(id) => (Some(id), None),
                    SessionStartup::Fork(id) => (None, Some(id)),
                };
                let launch = LaunchConfiguration {
                    helper: startup.helper,
                    generation: 0,
                    backend: startup.backend,
                    project: startup.project,
                    model: startup.model.unwrap_or_else(model_selector),
                    thinking_level: startup.thinking_level,
                    fast_mode: startup.fast_mode,
                    omperor_tools: startup.omperor_tools,
                    computer_use: startup.computer_use,
                    host_calls: launch_calls,
                    account: startup.account,
                    data_dir: startup.data_dir,
                    session_dir: startup.session_dir,
                    environment: startup.environment,
                    resume,
                    fork,
                };
                run_thread(command_rx, event_tx, launch);
            })
            .expect("OMP RPC runtime thread must start");
        Self {
            commands,
            events,
            worker: Mutex::new(Some(worker)),
            observed_generation: std::sync::atomic::AtomicU64::new(1),
            host_calls,
        }
    }

    fn send(&self, command: Command) -> bool {
        let kind = command.kind();
        let payload_bytes = command.payload_bytes();
        perf::event_with(
            "omp.command.enqueue",
            || serde_json::json!({ "command": kind, "payload_bytes": payload_bytes }),
        );
        let command = command_for_generation(
            command,
            self.observed_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let sent = self.commands.send(command).is_ok();
        if !sent {
            perf::event_with(
                "omp.command.enqueue_failed",
                || serde_json::json!({ "command": kind }),
            );
        }
        sent
    }

    pub(crate) fn prompt(&self, prompt: PreparedPrompt) -> bool {
        self.send(Command::Prompt(prompt))
    }

    pub(crate) fn steer(&self, prompt: PreparedPrompt) -> bool {
        self.send(Command::Steer(prompt))
    }

    pub(crate) fn abort(&self) -> bool {
        self.send(Command::Abort)
    }

    pub(crate) fn set_account(&self, account_id: String) -> bool {
        self.send(Command::SetAccount(account_id))
    }

    pub(crate) fn logout_account(&self, account_id: String) -> bool {
        self.send(Command::LogoutAccount(account_id))
    }

    pub(crate) fn refresh_models(&self) -> bool {
        self.send(Command::RefreshModels)
    }

    pub(crate) fn refresh_subagents(&self) -> bool {
        self.send(Command::RefreshSubagents)
    }

    /// Release the journal writer while keeping the worker available to resume.
    pub(crate) fn release_session(&self) -> bool {
        self.send(Command::ReleaseSession)
    }

    pub(crate) fn retry_startup(&self) -> bool {
        self.send(Command::RetryStartup)
    }

    pub(crate) fn resume_session(&self, session_id: String, project: PathBuf) -> bool {
        self.send(Command::ResumeSession {
            session_id,
            project,
        })
    }

    pub(crate) fn set_model(&self, model: String) -> bool {
        self.send(Command::SetModel(model))
    }

    pub(crate) fn set_thinking_level(&self, level: String) -> bool {
        self.send(Command::SetThinkingLevel(level))
    }

    pub(crate) fn set_fast_mode(&self, enabled: bool) -> bool {
        self.send(Command::SetFastMode(enabled))
    }

    pub(crate) fn refresh_account_usage(&self) -> bool {
        self.send(Command::RefreshAccountUsage)
    }

    /// Offers the agent the `omperor_` tools in this session, or stops.
    /// Queued ahead of any prompt sent after it, so the tools are there for
    /// that prompt's turn.
    pub(crate) fn set_omperor_tools(&self, enabled: bool) -> bool {
        self.send(Command::SetOmperorTools(enabled))
    }

    /// Answers a [`UiEvent::HostToolCall`]. `false` when the call is no
    /// longer waiting, because its turn ended.
    pub(crate) fn answer_host_tool(&self, call: u64, result: Result<Value, String>) -> bool {
        self.host_calls.answer(call, result)
    }

    pub(crate) fn login(
        &self,
        provider: AuthProvider,
        method: AuthMethod,
        generation: u64,
    ) -> bool {
        self.send(Command::Login(provider, method, generation))
    }

    pub(crate) fn answer_authentication(&self, prompt: AuthPrompt, value: String) -> bool {
        self.send(Command::AnswerAuthentication(prompt, value))
    }

    pub(crate) fn cancel_authentication(&self, session_id: String) -> bool {
        self.send(Command::CancelAuthentication(session_id))
    }

    pub(crate) fn try_event(&self) -> Option<ArrivedEvent> {
        let (arrived, event) = self.events.try_recv().ok()?;
        if let UiEvent::RuntimeGeneration(generation) = &event {
            self.observed_generation
                .store(*generation, std::sync::atomic::Ordering::Relaxed);
        }
        let kind = event.kind();
        let payload_bytes = event.payload_bytes();
        perf::event_with(
            "omp.ui_event.dequeue",
            || serde_json::json!({ "event": kind, "payload_bytes": payload_bytes }),
        );
        Some((arrived, event))
    }

    pub(crate) fn begin_shutdown(&self) {
        perf::event("omp.runtime.shutdown_requested");
        let _ = self.send(Command::Shutdown);
    }

    /// Reaps the RPC worker once it has fully stopped without blocking the
    /// caller. A missing worker is already reaped.
    pub(crate) fn try_reap(&self) -> bool {
        let worker = {
            let Ok(mut worker) = self.worker.lock() else {
                return false;
            };
            match worker.as_ref() {
                Some(worker) if !worker.is_finished() => return false,
                Some(_) => worker.take(),
                None => return true,
            }
        };
        if let Some(worker) = worker {
            let _ = worker.join();
            perf::event("omp.runtime.worker_reaped");
        }
        true
    }
}

impl Drop for OmpRuntime {
    fn drop(&mut self) {
        // The registry retains retiring runtimes until try_reap succeeds.
        // Even an accidental early drop must never join a live worker on the
        // GUI thread; dropping JoinHandle detaches it after requesting stop.
        self.begin_shutdown();
        let _ = self.try_reap();
    }
}

fn run_thread(
    commands: Receiver<Command>,
    events: Sender<ArrivedEvent>,
    launch: LaunchConfiguration,
) {
    let _span = perf::span("omp.runtime.worker_lifetime");
    let events = UiEventSender(events);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = events.send(UiEvent::Failed(format!(
                "Could not start the OMP async runtime: {error}"
            )));
            return;
        }
    };
    runtime.block_on(run(commands, events, launch));
}

async fn run(commands: Receiver<Command>, events: UiEventSender, mut launch: LaunchConfiguration) {
    let mut generation = 0_u64;
    // Commands received while another was in flight. They belong to the
    // logical session, so a prompt sent during a Refresh Models or sign-in
    // restart runs on the replacement child instead of vanishing with the old.
    let mut pending = VecDeque::new();
    loop {
        generation += 1;
        launch.generation = generation;
        let _ = events.send(UiEvent::RuntimeGeneration(generation));
        let exit = match launch.backend {
            Backend::Omp => run_client(&commands, &events, &mut launch, &mut pending).await,
            Backend::Mock => mock_runtime::run(&commands, &events, &mut launch).await,
        };
        retain_session_commands(&mut pending);
        match exit {
            ClientExit::Restart => {}
            ClientExit::Shutdown => {
                let _ = events.send(UiEvent::Stopped);
                return;
            }
            ClientExit::Failed | ClientExit::Released => {
                let unsent = carry_over_while_offline(&mut pending, &mut launch);
                if unsent > 0 {
                    events.send(UiEvent::Notice(format!(
                        "{unsent} queued message{} not sent because the agent stopped • send again once it reconnects",
                        if unsent == 1 { " was" } else { "s were" }
                    )));
                }
                if exit == ClientExit::Released {
                    events.send(UiEvent::SessionReleased);
                } else {
                    events.send(UiEvent::Notice(
                        "The agent stopped • open this session to reconnect it".into(),
                    ));
                }
                match wait_for_relaunch(&commands, &events, &mut launch).await {
                    RecoveryAction::Restart => {}
                    RecoveryAction::Shutdown => {
                        let _ = events.send(UiEvent::Stopped);
                        return;
                    }
                }
            }
        }
    }
}

/// A steer, stop, or sign-in answer refers to work owned by the child that
/// just exited; only session-level intent may reach its replacement.
fn retain_session_commands(pending: &mut VecDeque<Command>) {
    pending.retain(|command| {
        !matches!(
            command,
            Command::Steer(_)
                | Command::Abort
                | Command::AnswerAuthentication(..)
                | Command::CancelAuthentication(_)
                | Command::ExpectedGeneration(..)
        )
    });
}

/// After a failure or release nothing runs until the user reconnects: keep
/// configuration the user chose meanwhile, and count the prompts that will
/// not be sent so the user is told instead of left waiting.
fn carry_over_while_offline(pending: &mut VecDeque<Command>, launch: &mut LaunchConfiguration) -> usize {
    let mut unsent = 0;
    for command in pending.drain(..) {
        match command {
            Command::Prompt(_) => unsent += 1,
            Command::SetModel(model) => launch.model = model,
            Command::SetAccount(account) => launch.account = Some(account),
            Command::SetOmperorTools(enabled) => launch.omperor_tools = enabled,
            _ => {}
        }
    }
    unsent
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientExit {
    Restart,
    Released,
    Shutdown,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryAction {
    Restart,
    Shutdown,
}

/// Keeps this session's command receiver alive after release or recoverable failure.
/// Explicit resume reconnects it; ambiguous turn commands are never replayed.
async fn wait_for_relaunch(
    commands: &Receiver<Command>,
    events: &UiEventSender,
    launch: &mut LaunchConfiguration,
) -> RecoveryAction {
    loop {
        match commands.recv_async().await {
            Ok(command) => {
                let kind = command.kind();
                perf::event_with(
                    "omp.command.dequeue_while_offline",
                    || serde_json::json!({ "command": kind }),
                );
                let Some(command) = current_command(command, launch.generation) else {
                    continue;
                };
                match command {
                    Command::RetryStartup => return RecoveryAction::Restart,
                    Command::ResumeSession {
                        session_id,
                        project,
                    } => {
                        launch.project = project;
                        launch.resume = Some(session_id);
                        launch.fork = None;
                        return RecoveryAction::Restart;
                    }
                    Command::SetModel(model) => launch.model = model,
                    Command::SetAccount(account) => launch.account = Some(account),
                    // Offered when the session is next opened.
                    Command::SetOmperorTools(enabled) => {
                        launch.omperor_tools = enabled;
                        events.send(UiEvent::OmperorToolsChanged(enabled));
                    }
                    Command::Shutdown => return RecoveryAction::Shutdown,
                    Command::ReleaseSession => {
                        events.send(UiEvent::SessionReleased);
                    }
                    _ => {
                        let _ = events.send(UiEvent::Notice(
                            "The agent is offline • start a new conversation or reopen this one to reconnect before sending".into(),
                        ));
                    }
                }
            }
            Err(_) => return RecoveryAction::Shutdown,
        }
    }
}

async fn run_client(
    commands: &Receiver<Command>,
    events: &UiEventSender,
    launch: &mut LaunchConfiguration,
    pending: &mut VecDeque<Command>,
) -> ClientExit {
    let executable = launch.helper.clone().unwrap_or_else(omp_executable);
    let _lifecycle = perf::span_with("omp.rpc.lifecycle", || {
        serde_json::json!({
            "executable": executable,
            "project": launch.project,
            "model": launch.model,
            "data_dir": launch.data_dir,
            "session_dir": launch.session_dir,
            "resume": launch.resume,
            "fork": launch.fork,
        })
    });
    let _ = events.send(UiEvent::Connecting {
        executable: executable.clone(),
        project: launch.project.clone(),
    });
    let session_is_fresh = launch.resume.is_none();
    let requested_resume = launch.resume.clone();
    let options = rpc_client_options(executable, launch);
    let startup_guard =
        match wait_during_startup(STARTUP_GATE.lock(), commands, pending, launch).await {
            StartupWait::Complete(guard) => guard,
            StartupWait::Restart => return ClientExit::Restart,
            StartupWait::Released => return ClientExit::Released,
            StartupWait::Shutdown => return ClientExit::Shutdown,
        };
    // Startup owns filesystem initialization on the runtime worker, never AppKit.
    // A fresh application HOME has no OMP directories or credential key yet.
    for directory in [&launch.data_dir, &launch.session_dir] {
        if let Err(error) = std::fs::create_dir_all(directory) {
            events.send(UiEvent::Failed(format!(
                "Could not initialize OMP directory {}: {error}",
                directory.display()
            )));
            return ClientExit::Failed;
        }
    }
    let spawn = {
        let _span = perf::span("omp.rpc.spawn");
        // Keep ownership of a spawning child until the SDK can reap it. Merely
        // dropping spawn sends a kill but does not establish journal release.
        let spawning = RpcClient::spawn(options);
        tokio::pin!(spawning);
        match wait_during_startup(&mut spawning, commands, pending, launch).await {
            StartupWait::Complete(spawn) => spawn,
            interrupted => {
                match spawning.await {
                    Ok(client) => {
                        if matches!(interrupted, StartupWait::Released) {
                            return release_client(&client, events).await;
                        }
                        shutdown_client(&client, "spawn_interrupted").await;
                    }
                    Err(error) if matches!(interrupted, StartupWait::Released) => {
                        events.send(UiEvent::Failed(format!(
                            "Could not confirm OMP session release during startup: {error}"
                        )));
                        return ClientExit::Shutdown;
                    }
                    Err(_) => {}
                }
                return match interrupted {
                    StartupWait::Released => ClientExit::Released,
                    StartupWait::Restart => ClientExit::Restart,
                    StartupWait::Shutdown => ClientExit::Shutdown,
                    StartupWait::Complete(_) => unreachable!(),
                };
            }
        }
    };
    let client = match spawn {
        Ok(client) => client,
        Err(error) => {
            let _ = events.send(UiEvent::Failed(format!("OMP RPC startup failed: {error}")));
            return ClientExit::Failed;
        }
    };

    let state = {
        let _span = perf::span("omp.rpc.get_state.startup");
        match wait_during_startup(client.get_state(), commands, pending, launch).await {
            StartupWait::Complete(state) => state,
            StartupWait::Restart => {
                shutdown_client(&client, "startup_interrupted_for_restart").await;
                return ClientExit::Restart;
            }
            StartupWait::Released => {
                return release_client(&client, events).await;
            }
            StartupWait::Shutdown => {
                shutdown_client(&client, "startup_interrupted_for_shutdown").await;
                return ClientExit::Shutdown;
            }
        }
    };
    let state = match state {
        Ok(state) => state,
        Err(error) => {
            let stderr = client.stderr().await;
            let detail = if stderr.trim().is_empty() {
                error.to_string()
            } else {
                format!("{error}\n{}", stderr.trim())
            };
            let _ = events.send(UiEvent::Failed(format!("OMP RPC state failed: {detail}")));
            shutdown_client(&client, "startup_state_failed").await;
            return ClientExit::Failed;
        }
    };
    let providers = {
        let _span = perf::span("omp.rpc.get_login_providers");
        match wait_during_startup(client.get_login_providers(), commands, pending, launch)
            .await
        {
            StartupWait::Complete(providers) => providers,
            StartupWait::Restart => {
                shutdown_client(&client, "provider_lookup_interrupted_for_restart").await;
                return ClientExit::Restart;
            }
            StartupWait::Released => {
                return release_client(&client, events).await;
            }
            StartupWait::Shutdown => {
                shutdown_client(&client, "provider_lookup_interrupted_for_shutdown").await;
                return ClientExit::Shutdown;
            }
        }
    };
    let authenticated_providers = providers
        .map(|providers| {
            providers
                .iter()
                .filter(|provider| provider.authenticated)
                .filter_map(|provider| AuthProvider::from_id(&provider.id))
                .collect()
        })
        .unwrap_or_default();
    let accounts = {
        let _span = perf::span("omp.rpc.list_accounts");
        match wait_during_startup(client.list_accounts(), commands, pending, launch).await {
            StartupWait::Complete(Ok(accounts)) => Some(supported_accounts(&accounts)),
            StartupWait::Complete(Err(error)) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not load OMP's account list: {error}"
                )));
                None
            }
            StartupWait::Restart => {
                shutdown_client(&client, "account_lookup_interrupted_for_restart").await;
                return ClientExit::Restart;
            }
            StartupWait::Released => {
                return release_client(&client, events).await;
            }
            StartupWait::Shutdown => {
                shutdown_client(&client, "account_lookup_interrupted_for_shutdown").await;
                return ClientExit::Shutdown;
            }
        }
    };
    // Pin the GUI's chosen account before the session is announced so the
    // first prompt cannot race ahead of the pin. A pin OMP no longer stores is
    // dropped rather than guessed at; the GUI resolves a replacement.
    let account = match launch.account.clone() {
        Some(account_id)
            if accounts.as_ref().is_some_and(|accounts| {
                accounts
                    .iter()
                    .any(|account| account.id == account_id && account.state.is_usable())
            }) =>
        {
            let _span = perf::span("omp.rpc.set_account.startup");
            let selected = match wait_during_startup(
                client.set_account(&account_id),
                commands,
                pending,
                launch,
            )
            .await
            {
                StartupWait::Complete(value) => value,
                StartupWait::Restart => {
                    shutdown_client(&client, "account_pin_interrupted").await;
                    return ClientExit::Restart;
                }
                StartupWait::Released => {
                    return release_client(&client, events).await;
                }
                StartupWait::Shutdown => {
                    shutdown_client(&client, "account_pin_interrupted").await;
                    return ClientExit::Shutdown;
                }
            };
            match selected {
                Ok(_) => Some(account_id),
                Err(error) => {
                    let _ = events.send(UiEvent::Notice(format!(
                        "Could not select OMP account `{account_id}`: {error}"
                    )));
                    None
                }
            }
        }
        Some(_) if accounts.is_none() => launch.account.clone(),
        Some(_) | None => None,
    };
    launch.account.clone_from(&account);
    let available = {
        let _span = perf::span("omp.rpc.get_available_models");
        match wait_during_startup(
            client.get_available_models(),
            commands,
            pending,
            launch,
        )
        .await
        {
            StartupWait::Complete(Ok(available)) => available,
            StartupWait::Complete(Err(error)) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not load OMP's model list: {error}"
                )));
                Value::Null
            }
            StartupWait::Restart => {
                shutdown_client(&client, "model_lookup_interrupted_for_restart").await;
                return ClientExit::Restart;
            }
            StartupWait::Released => {
                return release_client(&client, events).await;
            }
            StartupWait::Shutdown => {
                shutdown_client(&client, "model_lookup_interrupted_for_shutdown").await;
                return ClientExit::Shutdown;
            }
        }
    };
    let reported_model = state_model(&state)
        .or_else(|| {
            available
                .get("active")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| launch.model.clone());
    let models = available_models(&available);
    let model = preferred_available_model(&reported_model, &launch.model, &models)
        .unwrap_or_else(|| reported_model.clone());
    let model = if model != reported_model {
        let switched = match model.split_once('/') {
            Some((provider, model_id)) => match wait_during_startup(
                client.set_model(provider, model_id),
                commands,
                pending,
                launch,
            )
            .await
            {
                StartupWait::Complete(value) => value,
                StartupWait::Restart => {
                    shutdown_client(&client, "initial_model_interrupted").await;
                    return ClientExit::Restart;
                }
                StartupWait::Released => {
                    return release_client(&client, events).await;
                }
                StartupWait::Shutdown => {
                    shutdown_client(&client, "initial_model_interrupted").await;
                    return ClientExit::Shutdown;
                }
            },
            None => unreachable!("available RPC models are provider-qualified"),
        };
        match switched {
            Ok(_) => model,
            Err(error) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not replace unavailable OMP model `{reported_model}` with `{model}`: {error}"
                )));
                reported_model
            }
        }
    } else {
        model
    };
    launch.model.clone_from(&model);
    // A resumed journal owns its settings. Fresh launches (including draft
    // replacements) apply the GUI intent before Ready or any queued prompt.
    let thinking_level = launch.thinking_level.clone();
    let fast_mode = launch.fast_mode;
    // The session's own tools are offered again to every child launched for
    // it, before Ready, so no prompt can reach a child without them.
    let omperor_tools = launch
        .omperor_tools
        .then(|| omperor_host_tools(events, &launch.host_calls));
    let configure = async {
        if requested_resume.is_none() {
            if let Some(level) = &thinking_level {
                client.set_thinking_level(level).await?;
            }
            client.set_fast_mode(fast_mode).await?;
        }
        if let Some(tools) = omperor_tools {
            client.set_host_tools(tools).await?;
        }
        client.get_state().await
    };
    let state = match wait_during_startup(configure, commands, pending, launch).await {
        StartupWait::Complete(Ok(state)) => state,
        StartupWait::Complete(Err(error)) => {
            events.send(UiEvent::Failed(format!(
                "Could not configure OMP startup settings: {error}"
            )));
            shutdown_client(&client, "startup_settings_failed").await;
            return ClientExit::Failed;
        }
        StartupWait::Restart => {
            shutdown_client(&client, "startup_settings_interrupted").await;
            return ClientExit::Restart;
        }
        StartupWait::Released => return release_client(&client, events).await,
        StartupWait::Shutdown => {
            shutdown_client(&client, "startup_settings_interrupted").await;
            return ClientExit::Shutdown;
        }
    };
    launch.thinking_level = state
        .get("thinkingLevel")
        .and_then(Value::as_str)
        .map(str::to_owned);
    launch.fast_mode = state
        .get("fastModeEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ready_session_id = state
        .get("sessionId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let ready_session_file = state
        .get("sessionFile")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    if let Some(requested) = &requested_resume {
        let matches = ready_session_id.as_deref() == Some(requested.as_str())
            || ready_session_file.as_ref().is_some_and(|file| {
                file == Path::new(requested)
                    || std::fs::canonicalize(file)
                        .ok()
                        .zip(std::fs::canonicalize(requested).ok())
                        .is_some_and(|(actual, expected)| actual == expected)
            });
        if !matches {
            let _ = events.send(UiEvent::Failed(format!(
                "OMP resumed the wrong session: requested {requested}, received {ready_session_id:?}"
            )));
            shutdown_client(&client, "resume_identity_mismatch").await;
            return ClientExit::Failed;
        }
    }
    let live_session = ready_session_id.clone().map(|id| LiveSession {
        id,
        file: ready_session_file.clone(),
    });
    // A replacement always belongs to the same session, even when the first
    // launch was fresh or a fork and authentication triggers the restart.
    launch.fork = None;
    launch.resume = ready_session_id.clone();
    // Sub-agent lifecycle and progress frames feed the Agents panel; the
    // board is read once now so a resumed session shows the sub-agents it
    // already spawned.
    let subagents = {
        let _span = perf::span("omp.rpc.subagents.startup");
        let subscription = async {
            if let Err(error) = client
                .set_subagent_subscription(omp_rpc::protocol::SubscriptionLevel::Progress)
                .await
            {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not subscribe to OMP's sub-agent updates: {error}"
                )));
            }
            client
                .get_subagents()
                .await
                .map(|snapshots| subagent_infos(&snapshots))
                .unwrap_or_default()
        };
        match wait_during_startup(subscription, commands, pending, launch).await {
            StartupWait::Complete(value) => value,
            StartupWait::Restart => {
                shutdown_client(&client, "initial_subagents_interrupted").await;
                return ClientExit::Restart;
            }
            StartupWait::Released => {
                return release_client(&client, events).await;
            }
            StartupWait::Shutdown => {
                shutdown_client(&client, "initial_subagents_interrupted").await;
                return ClientExit::Shutdown;
            }
        }
    };
    drop(startup_guard);
    let _ = events.send(UiEvent::Ready {
        model,
        models,
        fast_models: fast_models(&available),
        fast_mode: state
            .get("fastModeEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        thinking_level: state
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .unwrap_or("medium")
            .to_owned(),
        session_id: ready_session_id,
        session_file: ready_session_file,
        project: launch.project.clone(),
        authenticated_providers,
        accounts,
        account,
        session_is_fresh,
        resumed: requested_resume,
    });
    let _ = events.send(UiEvent::SubagentsSnapshot(subagents));
    let mut rpc_events = client.events();
    let mut turn = TurnWatch::default();
    loop {
        if let Some(command) = pending.pop_front() {
            if let Some(exit) = drive_command(
                &client,
                events,
                launch,
                live_session.as_ref(),
                command,
                commands,
                pending,
                &mut rpc_events,
                &mut turn,
            )
            .await
            {
                return exit;
            }
            continue;
        }
        tokio::select! {
            command = commands.recv_async() => match command {
                Ok(command) => {
                    if let Some(exit) = drive_command(&client, events, launch, live_session.as_ref(), command, commands, pending, &mut rpc_events, &mut turn).await {
                        return exit;
                    }
                }
                Err(_) => {
                    shutdown_client(&client, "command_channel_disconnected").await;
                    return ClientExit::Shutdown;
                }
            },
            event = rpc_events.recv() => match event {
                Ok(event) => {
                    perf::event_with("omp.rpc.event_received", || {
                        serde_json::json!({
                            "event": event.kind.as_str(),
                            "payload_bytes": serde_json::to_vec(&event).map_or(0, |value| value.len()),
                        })
                    });
                    if turn.forward(&event, events) {
                        shutdown_client(&client, "authentication_completed").await;
                        return ClientExit::Restart;
                    }
                }
                Err(error) => {
                    let _ = events.send(UiEvent::Failed(format!("OMP event stream stopped: {error}")));
                    shutdown_client(&client, "event_stream_failed").await;
                    return ClientExit::Failed;
                }
            }
        }
    }
}

/// Whether a turn is running, so a completed sign-in reloads the child only
/// once the agent is idle instead of killing the work in flight.
#[derive(Default)]
struct TurnWatch {
    active: bool,
    restart_when_idle: bool,
}

impl TurnWatch {
    /// Forwards one RPC event's projection; true when the child should
    /// restart now.
    fn forward(&mut self, event: &RpcEvent, events: &UiEventSender) -> bool {
        let signed_in = authentication_completed(event);
        for projected in project_event(event) {
            match projected {
                UiEvent::Running => self.active = true,
                UiEvent::TurnFinished(_) => self.active = false,
                _ => {}
            }
            let _ = events.send(projected);
        }
        if signed_in {
            self.restart_when_idle = true;
            if self.active {
                let _ = events.send(UiEvent::Notice(
                    "Signed in • the agent picks up the account when this turn ends".into(),
                ));
            }
        }
        self.restart_when_idle && !self.active
    }
}

#[derive(Debug, PartialEq, Eq)]
enum StartupWait<T> {
    Complete(T),
    Released,
    Restart,
    Shutdown,
}

async fn wait_during_startup<T>(
    future: impl Future<Output = T>,
    commands: &Receiver<Command>,
    pending: &mut VecDeque<Command>,
    launch: &mut LaunchConfiguration,
) -> StartupWait<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return StartupWait::Complete(result),
            command = commands.recv_async() => match command {
                Ok(command) => {
                    let kind = command.kind();
                    perf::event_with("omp.command.dequeue_during_startup", || {
                        serde_json::json!({ "command": kind })
                    });
                    let Some(command) = current_command(command, launch.generation) else { continue; };
                    match command {
                        Command::ResumeSession {
                            session_id,
                            project,
                        } => {
                            launch.project = project;
                            launch.resume = Some(session_id);
                            launch.fork = None;
                            return StartupWait::Restart;
                        }
                        Command::ReleaseSession => return StartupWait::Released,
                        Command::Shutdown => return StartupWait::Shutdown,
                        command => pending.push_back(command),
                    }
                }
                Err(_) => return StartupWait::Shutdown,
            }
        }
    }
}

// Never announce journal release when the SDK could not confirm child exit.
async fn release_client(client: &RpcClient, events: &UiEventSender) -> ClientExit {
    match client.shutdown().await {
        Ok(()) => ClientExit::Released,
        Err(error) => {
            events.send(UiEvent::Failed(format!(
                "Could not release OMP session: {error}"
            )));
            ClientExit::Shutdown
        }
    }
}

async fn shutdown_client(client: &RpcClient, reason: &'static str) {
    let _span = perf::span_with(
        "omp.rpc.shutdown",
        || serde_json::json!({ "reason": reason }),
    );
    let _ = client.shutdown().await;
}

/// The session the running child reported at Ready.
#[derive(Debug)]
struct LiveSession {
    id: String,
    file: Option<PathBuf>,
}

impl LiveSession {
    /// The session id to resume after a relaunch, when its transcript holds at
    /// least one persisted message. OMP writes an empty envelope as soon as a
    /// session opens but refuses to resume it ("chat session does not exist"),
    /// so a session without turns is replaced by a fresh one instead.
    fn resume_target(&self) -> Option<String> {
        self.file
            .as_deref()
            .is_some_and(transcript_has_messages)
            .then(|| self.id.clone())
    }
}

/// Whether a session file on disk contains any persisted message: a JSON
/// envelope with a non-empty `messages` array, or a non-blank JSONL
/// transcript.
fn transcript_has_messages(path: &Path) -> bool {
    if crate::journal::is_journal(path) {
        return crate::journal::has_conversation(path);
    }
    let Ok(contents) = std::fs::read(path) else {
        return false;
    };
    match serde_json::from_slice::<Value>(&contents) {
        Ok(Value::Object(envelope)) if envelope.contains_key("messages") => envelope
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| !messages.is_empty()),
        _ => contents.iter().any(|byte| !byte.is_ascii_whitespace()),
    }
}

fn current_command(command: Command, generation: u64) -> Option<Command> {
    match command {
        Command::ExpectedGeneration(expected, command) if expected == generation => Some(*command),
        Command::ExpectedGeneration(..) => None,
        command => Some(command),
    }
}

/// Bind only commands whose meaning belongs to one child generation. User
/// prompts and configuration/lifecycle intent belong to the logical session:
/// they must remain FIFO across an intentional account or model restart. A
/// steer, stop, or authentication answer instead refers to work owned by the
/// current child and must never leak into its replacement.
fn command_for_generation(command: Command, generation: u64) -> Command {
    if matches!(
        command,
        Command::Steer(_)
            | Command::Abort
            | Command::AnswerAuthentication(..)
            | Command::CancelAuthentication(_)
    ) {
        Command::ExpectedGeneration(generation, Box::new(command))
    } else {
        command
    }
}

/// Poll the ordered mutation and RPC event stream together. Only abort and
/// shutdown may overtake a pending command; ordinary mutations remain FIFO.
async fn drive_command(
    client: &RpcClient,
    events: &UiEventSender,
    launch: &mut LaunchConfiguration,
    live_session: Option<&LiveSession>,
    command: Command,
    commands: &Receiver<Command>,
    pending: &mut VecDeque<Command>,
    rpc_events: &mut omp_rpc::client::EventStream,
    turn: &mut TurnWatch,
) -> Option<ClientExit> {
    let generation = launch.generation;
    let exit = {
        let operation = handle_command(client, events, launch, live_session, command);
        tokio::pin!(operation);
        let mut completed = None;
        let mut abort: Option<std::pin::Pin<Box<dyn Future<Output = ()> + '_>>> = None;
        loop {
            tokio::select! {
                result = &mut operation, if completed.is_none() => {
                    if abort.is_none() {
                        match result {
                            Some(exit) => break exit,
                            None => return None,
                        }
                    }
                    completed = Some(result);
                },
                command = commands.recv_async() => match command.ok().and_then(|command| current_command(command, generation)) {
                    Some(Command::Shutdown) => break ClientExit::Shutdown,
                    Some(Command::Abort) => {
                        // Coalesce repeated stops while one stop is awaiting its
                        // correlated response; never cancel the ordered mutation.
                        if abort.is_none() {
                            abort = Some(Box::pin(async {
                                if let Err(error) = client.abort().await {
                                    let _ = events.send(UiEvent::Notice(format!("Stopping failed: {error}")));
                                }
                            }));
                        }
                    }
                    Some(command) => pending.push_back(command),
                    None if commands.is_disconnected() => break ClientExit::Shutdown,
                    None => {},
                },
                () = async { abort.as_mut().unwrap().await }, if abort.is_some() => {
                    abort = None;
                    if let Some(result) = completed {
                        match result {
                            Some(exit) => break exit,
                            None => return None,
                        }
                    }
                }
                event = rpc_events.recv() => match event {
                    Ok(event) => {
                        if turn.forward(&event, events) { break ClientExit::Restart; }
                    }
                    Err(error) => {
                        let _ = events.send(UiEvent::Failed(format!("OMP event stream stopped: {error}")));
                        break ClientExit::Failed;
                    }
                }
            }
        }
    };
    // SAFETY: lifecycle commands declare their outcome before the transport is
    // closed. If shutdown happened inside `operation`, its intentional stream
    // closure could win the select above and be misclassified as `Failed`,
    // leaving `run` parked in recovery after consuming the shutdown command.
    let reason = match exit {
        ClientExit::Restart => "command_requested_restart",
        ClientExit::Shutdown => "command_requested_shutdown",
        ClientExit::Released => "session_released",
        ClientExit::Failed => "pending_command_interrupted",
    };
    if exit == ClientExit::Released {
        return Some(release_client(client, events).await);
    }
    shutdown_client(client, reason).await;
    Some(exit)
}

async fn handle_command(
    client: &RpcClient,
    events: &UiEventSender,
    launch: &mut LaunchConfiguration,
    live_session: Option<&LiveSession>,
    command: Command,
) -> Option<ClientExit> {
    let command = current_command(command, launch.generation)?;
    let kind = command.kind();
    perf::event_with(
        "omp.command.dequeue",
        || serde_json::json!({ "command": kind }),
    );
    let _span = perf::span_with("omp.rpc.command", || serde_json::json!({ "command": kind }));
    match command {
        Command::ExpectedGeneration(..) => unreachable!("generation envelope was removed"),
        Command::ReleaseSession => return Some(ClientExit::Released),
        // A retry that raced with readiness must not replace a healthy child.
        Command::RetryStartup => {}
        Command::Prompt(prompt) => {
            if let Err(error) = client.prompt(prompt.message, prompt.images).await {
                let _ = events.send(UiEvent::Failed(format!("Sending failed: {error}")));
            }
        }
        Command::Steer(prompt) => {
            // OMP's steer handler queues text only. Prepared messages include
            // local file paths, so no attachment silently disappears.
            if let Err(error) = client.steer(prompt.message, Vec::new()).await {
                let _ = events.send(UiEvent::Notice(format!("Steering failed: {error}")));
            }
        }
        Command::Abort => {
            if let Err(error) = client.abort().await {
                let _ = events.send(UiEvent::Notice(format!("Stopping failed: {error}")));
            }
        }
        Command::SetAccount(account_id) => match client.set_account(&account_id).await {
            Ok(response) => {
                launch.account = Some(account_id.clone());
                let _ = events.send(UiEvent::AccountChanged(Some(account_id)));
                // The child's catalog was composed for the account pinned at
                // launch; a different account may be offered other models
                // and tiers, so the same session resumes on a child launched
                // for it (OMP has already fetched its listing).
                if response
                    .get("restartRequired")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    launch.resume = live_session.and_then(LiveSession::resume_target);
                    launch.fork = None;
                    let _ = events.send(UiEvent::Notice(
                        "Account changed • restarting the agent to load what it offers…".into(),
                    ));
                    return Some(ClientExit::Restart);
                }
            }
            Err(error) => {
                let _ = events.send(UiEvent::AccountChanged(launch.account.clone()));
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not select the OMP account: {error}"
                )));
            }
        },
        Command::RefreshAccountUsage => {
            // Every account's provider is asked at once, each on a bounded
            // clock inside the child; the answer can still take a while.
            let answer = client
                .request_with_timeout::<_, Value>(
                    "get_account_usage",
                    &serde_json::json!({}),
                    Duration::from_secs(45),
                )
                .await;
            match answer {
                Ok(value) => {
                    let _ = events.send(UiEvent::AccountUsage(account_usages(&value)));
                }
                Err(error) => {
                    let _ = events.send(UiEvent::AccountUsage(Vec::new()));
                    let _ = events.send(UiEvent::Notice(format!(
                        "Could not read account usage: {error}"
                    )));
                }
            }
        }
        Command::RefreshSubagents => match client.get_subagents().await {
            Ok(snapshots) => {
                let _ = events.send(UiEvent::SubagentsSnapshot(subagent_infos(&snapshots)));
            }
            Err(error) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not read OMP's sub-agents: {error}"
                )));
            }
        },
        Command::RefreshModels => match client.refresh_models().await {
            Ok(result) => {
                if result
                    .get("restartRequired")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                {
                    let _ = events.send(UiEvent::Notice(
                        "No authenticated providers to refresh.".into(),
                    ));
                    return None;
                }
                let warnings = result
                    .get("warnings")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|warning| {
                        let provider = warning.get("providerId")?.as_str()?;
                        let error = warning.get("error")?.as_str()?;
                        Some(format!("{provider}: {error}"))
                    })
                    .collect::<Vec<_>>();
                // The child's catalog was composed at launch; resume the same
                // session on a fresh child so the new models become selectable.
                // A session that never persisted has nothing to resume, so it
                // is simply replaced by a fresh one.
                launch.resume = live_session.and_then(LiveSession::resume_target);
                launch.fork = None;
                let notice = if warnings.is_empty() {
                    "Model list refreshed • restarting the agent to load it…".into()
                } else {
                    format!(
                        "Model list partly refreshed; keeping cached models for {} • restarting the agent…",
                        warnings.join("; ")
                    )
                };
                let _ = events.send(UiEvent::Notice(notice));
                return Some(ClientExit::Restart);
            }
            Err(error) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not refresh the model list: {error}"
                )));
            }
        },
        Command::LogoutAccount(account_id) => {
            if let Err(error) = client.logout_account(&account_id).await {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not remove the OMP account: {error}"
                )));
                return None;
            }
            if launch.account.as_deref() == Some(account_id.as_str()) {
                launch.account = None;
                let _ = events.send(UiEvent::AccountChanged(None));
            }
            match client.list_accounts().await {
                Ok(accounts) => {
                    let _ = events.send(UiEvent::AccountsChanged(supported_accounts(&accounts)));
                }
                Err(error) => {
                    let _ = events.send(UiEvent::Notice(format!(
                        "Removed the account, but could not reload OMP's account list: {error}"
                    )));
                }
            }
        }
        Command::ResumeSession {
            session_id,
            project,
        } => {
            launch.project = project;
            launch.resume = Some(session_id);
            launch.fork = None;
            return Some(ClientExit::Restart);
        }
        Command::SetModel(model) => {
            let Some((provider, model_id)) = model.split_once('/') else {
                let _ = events.send(UiEvent::Notice(format!(
                    "OMP model selector `{model}` is invalid"
                )));
                return None;
            };
            match client.set_model(provider, model_id).await {
                Ok(_) => {
                    launch.model.clone_from(&model);
                    let _ = events.send(UiEvent::ModelChanged(model));
                }
                Err(error) => {
                    let _ = events.send(UiEvent::ModelChanged(launch.model.clone()));
                    let _ = events.send(UiEvent::Notice(format!(
                        "Could not change the OMP model: {error}"
                    )));
                }
            }
        }
        Command::SetThinkingLevel(level) => match client.set_thinking_level(&level).await {
            Ok(_) => {
                launch.thinking_level = Some(level.clone());
                let _ = events.send(UiEvent::ThinkingLevelChanged(level));
            }
            Err(error) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not change OMP reasoning: {error}"
                )));
            }
        },
        Command::SetFastMode(enabled) => match client.set_fast_mode(enabled).await {
            Ok(response) => {
                let enabled = response
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(enabled);
                launch.fast_mode = enabled;
                let _ = events.send(UiEvent::FastModeChanged(enabled));
            }
            Err(error) => {
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not change OMP's fast mode: {error}"
                )));
            }
        },
        Command::SetOmperorTools(enabled) => {
            let tools = if enabled {
                omperor_host_tools(events, &launch.host_calls)
            } else {
                Vec::new()
            };
            match client.set_host_tools(tools).await {
                Ok(_) => {
                    launch.omperor_tools = enabled;
                    let _ = events.send(UiEvent::OmperorToolsChanged(enabled));
                }
                Err(error) => {
                    let _ = events.send(UiEvent::OmperorToolsChanged(launch.omperor_tools));
                    let _ = events.send(UiEvent::Notice(format!(
                        "Could not {} Omperor's tools: {error}",
                        if enabled { "offer" } else { "withdraw" }
                    )));
                }
            }
        }
        Command::Login(provider, method, generation) => {
            let _ = events.send(UiEvent::AuthenticationStarted {
                provider,
                generation,
            });
            match client
                .login_with_method(provider.id(), Some(method.rpc()))
                .await
            {
                Ok(started) => {
                    if let Some(session_id) = started.get("sessionId").and_then(Value::as_str) {
                        let _ = events.send(UiEvent::AuthenticationSession {
                            generation,
                            session_id: session_id.to_owned(),
                        });
                    }
                }
                Err(error) => {
                    let _ = events.send(UiEvent::AuthenticationFailed {
                        session_id: None,
                        generation: Some(generation),
                        error: format!(
                            "{} sign-in failed to start: {error}",
                            provider.product_name()
                        ),
                    });
                }
            }
        }
        Command::CancelAuthentication(session_id) => {
            let cancel = RpcAuthAnswerFrame {
                kind: "auth_answer".into(),
                session_id,
                prompt_id: None,
                input: RpcAuthInputKind::Cancel,
                value: None,
            };
            if let Err(error) = client.respond_auth(cancel).await {
                // The session may already have ended; either way the
                // interface is no longer waiting on it.
                let _ = events.send(UiEvent::Notice(format!(
                    "Could not cancel the sign-in cleanly: {error}"
                )));
            }
        }
        Command::AnswerAuthentication(prompt, value) => {
            let session_id = prompt.session_id.clone();
            if let Err(error) = client
                .respond_auth(authentication_answer(prompt, value))
                .await
            {
                let _ = events.send(UiEvent::AuthenticationFailed {
                    session_id: Some(session_id),
                    generation: None,
                    error: format!("Could not submit account sign-in: {error}"),
                });
            }
        }
        Command::Shutdown => {
            return Some(ClientExit::Shutdown);
        }
    }
    None
}

fn native_tools(computer_use: bool) -> String {
    if computer_use {
        format!("{NATIVE_GUI_TOOLS},computer")
    } else {
        NATIVE_GUI_TOOLS.into()
    }
}

/// The environment that confines an OMP process to one data store: its
/// credentials, caches, configuration, and worktrees. Shared by the RPC child
/// and the headless `omp auth` commands the eval CLI runs.
pub(crate) fn data_store_environment(data_dir: &Path) -> Vec<(String, String)> {
    let mut environment = vec![
        (
            "OMP_LLM_KEY_SOURCE".to_owned(),
            credential_source(env::var("OMP_LLM_KEY_SOURCE").ok()),
        ),
        // A custom session directory alone is insufficient isolation: OMP's
        // headless layer otherwise publishes those sessions into the TUI's
        // normal per-project index. Keep every mutable OMP store private.
        ("OMP_DATA_DIR".to_owned(), data_dir.to_string_lossy().into_owned()),
        ("OMP_PROFILE".to_owned(), "default".to_owned()),
    ];
    // OMP resolves these independently of OMP_DATA_DIR. Pin every mutable
    // namespace so a Dev child cannot inherit the installed app's or TUI's
    // configuration, caches, worktrees, or secret-placeholder keys.
    for (variable, directory) in [
        ("OMP_STATE_DIR", "state"),
        ("OMP_CACHE_DIR", "cache"),
        ("OMP_CONFIG_DIR", "config"),
        ("OMP_WORKTREE_DIR", "worktrees"),
    ] {
        environment.push((
            variable.to_owned(),
            data_dir.join(directory).to_string_lossy().into_owned(),
        ));
    }
    // A sub-agent's isolated copy lives under the worktree directory and has
    // no `.git` of its own. Without a ceiling, git run there climbs to the
    // nearest enclosing repository: a home directory kept in git, where a
    // sub-agent's `git status` scanned the whole home and `git add -A` or
    // `git clean` would act on the user's own files.
    let ceiling = data_dir.join("worktrees").to_string_lossy().into_owned();
    let ceilings = match env::var("GIT_CEILING_DIRECTORIES") {
        Ok(existing) if !existing.is_empty() => format!("{ceiling}:{existing}"),
        _ => ceiling,
    };
    environment.push(("GIT_CEILING_DIRECTORIES".to_owned(), ceilings));
    environment
}

fn rpc_client_options(executable: PathBuf, launch: &LaunchConfiguration) -> RpcClientOptions {
    let mut child_environment = Environment::new();
    child_environment.insert("OMP_TOKEN_EFFICIENT".into(), "1".into());
    child_environment.extend(launch.environment.iter().cloned());
    child_environment.extend(data_store_environment(&launch.data_dir));

    let mut extra_args = vec![
        "--tools".into(),
        native_tools(launch.computer_use).into(),
        "--envd-idle-timeout".into(),
        "1".into(),
        // Also gate dynamic device access; the model tool roster alone cannot.
        "--computer-use".into(),
        launch.computer_use.to_string().into(),
        // OpenAI routes a request to a machine that holds its prompt prefix
        // only when the request names a cache key; without one, consecutive
        // turns of a conversation rarely reuse the prefix and every turn pays
        // for its whole context. Codecs without such a key ignore it.
        "--prompt-cache-key".into(),
        prompt_cache_key(launch).into(),
    ];
    if let Some(session_id) = &launch.resume {
        extra_args.push("--resume".into());
        extra_args.push(session_id.into());
    }
    if let Some(session_id) = &launch.fork {
        // OMP copies the source journal into the launch project's session
        // directory, so a fork may start in another worktree without a flag.
        extra_args.push("--fork".into());
        extra_args.push(session_id.into());
    }
    if let Some(account) = &launch.account {
        // Pinned before the child's model discovery, so the catalog it
        // composes reflects what this account is offered (models, the Fast
        // tier) rather than the provider's default account.
        extra_args.push("--account".into());
        extra_args.push(account.clone().into());
    }
    RpcClientOptions {
        executable: executable.into_os_string(),
        cwd: Some(launch.project.clone()),
        env: child_environment,
        model: Some(launch.model.clone()),
        session_dir: Some(launch.session_dir.clone()),
        extra_args,
        ready_timeout: OMP_READY_TIMEOUT,
        request_timeout: Duration::from_secs(60),
        ..RpcClientOptions::default()
    }
}

/// One key per conversation: a resumed or forked session keeps the key its
/// history was cached under (a fork shares its parent's prefix); a new one
/// gets a fresh key for the life of its child.
fn prompt_cache_key(launch: &LaunchConfiguration) -> String {
    if let Some(session) = launch.resume.as_ref().or(launch.fork.as_ref()) {
        return format!("omperor-{session}");
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("omperor-{nanos:x}-{:x}-{:x}", std::process::id(), launch.generation)
}

fn credential_source(configured: Option<String>) -> String {
    configured
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "local-file".into())
}

/// Evals share Dev's store, so prefer its exact packaged helper identity too.
/// Called by the headless dispatcher; filesystem probes never run on AppKit.
pub(crate) fn eval_executable() -> PathBuf {
    if let Some(path) = env::var_os("OMPEROR_OMP_EXECUTABLE").filter(|value| !value.is_empty()) {
        return path.into();
    }
    if let Ok(current) = env::current_exe()
        && let Some(contents) = current.parent().and_then(Path::parent)
        && contents.file_name().is_some_and(|name| name == "Contents")
        && let Some(parent) = contents.parent().and_then(Path::parent)
    {
        let development = parent.join("Omperor Dev.app/Contents/Helpers/omp");
        if development.is_file() {
            return development;
        }
    }
    let source = Path::new(option_env!("OMPEROR_SOURCE_DIR").unwrap_or(env!("CARGO_MANIFEST_DIR")));
    let development = source.join("target/Omperor Dev.app/Contents/Helpers/omp");
    if development.is_file() {
        return development;
    }
    omp_executable()
}

fn omp_executable() -> PathBuf {
    if let Some(path) = env::var_os("OMPEROR_OMP_EXECUTABLE").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(current) = env::current_exe()
        && let Some(contents) = current.parent().and_then(Path::parent)
    {
        let bundled = contents.join("Helpers/omp");
        if bundled.is_file() {
            return bundled;
        }
    }
    // Prefer this channel's packaged helper so bare launches use the same
    // patched runtime as the GUI. State namespaces isolate environment services.
    let source = Path::new(option_env!("OMPEROR_SOURCE_DIR").unwrap_or(env!("CARGO_MANIFEST_DIR")));
    let packaged = source.join(format!(
        "target/{}.app/Contents/Helpers/omp",
        crate::app_state::Channel::current().app_name(),
    ));
    if packaged.is_file() {
        return packaged;
    }
    source.join("target/omperor-helpers/omp")
}

pub(crate) fn default_project_root() -> PathBuf {
    if let Some(path) = env::var_os("OMPEROR_PROJECT").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    let development = PathBuf::from(option_env!("OMPEROR_SOURCE_DIR").unwrap_or(env!("CARGO_MANIFEST_DIR")));
    if development.is_dir() {
        return development;
    }
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn model_selector() -> String {
    env::var("OMPEROR_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
}

fn state_model(state: &Value) -> Option<String> {
    let model = state.get("model")?;
    if let Some(model) = model.as_str() {
        return Some(model.to_owned());
    }
    let model = model.as_object()?;
    let provider = model.get("provider")?.as_str()?;
    let id = model.get("id")?.as_str()?;
    Some(format!("{provider}/{id}"))
}

/// Model selectors from `get_available_models`: the journal-first server
/// reports `{provider, id}` objects while older servers reported selectors.
fn available_models(value: &Value) -> Vec<String> {
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            if let Some(selector) = model.as_str() {
                return Some(selector.to_owned());
            }
            let provider = model.get("provider")?.as_str()?;
            let id = model.get("id")?.as_str()?;
            Some(format!("{provider}/{id}"))
        })
        .collect()
}

/// The selectors in `get_available_models` whose backend advertises the
/// OpenAI `priority` tier for this account: where a Fast switch belongs.
fn fast_models(value: &Value) -> Vec<String> {
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| model.get("fast").and_then(Value::as_bool) == Some(true))
        .filter_map(|model| {
            let provider = model.get("provider")?.as_str()?;
            let id = model.get("id")?.as_str()?;
            Some(format!("{provider}/{id}"))
        })
        .collect()
}

fn preferred_available_model(
    reported: &str,
    requested: &str,
    available: &[String],
) -> Option<String> {
    if available.iter().any(|model| model == reported) {
        return Some(reported.to_owned());
    }
    let requested_provider = requested.split_once('/').map(|(provider, _)| provider);
    available
        .iter()
        .find(|model| {
            requested_provider.is_some_and(|provider| model.starts_with(&format!("{provider}/")))
        })
        .or_else(|| available.first())
        .cloned()
}

fn project_event(event: &RpcEvent) -> Vec<UiEvent> {
    match event.kind.as_str() {
        "agent_start" => vec![UiEvent::Running],
        "agent_end" => vec![UiEvent::TurnFinished(turn_outcome(event))],
        "message_start" if matches!(message_role(event), Some("assistant")) => {
            vec![UiEvent::AssistantStarted]
        }
        "message_update" => project_message_update(event).into_iter().collect(),
        "message_end" if matches!(message_role(event), Some("assistant")) => {
            vec![UiEvent::AssistantFinished(final_message_text(event))]
        }
        "inference_receipt" => vec![UiEvent::InferenceReceipt(InferenceReceipt::from_event(
            event,
        ))],
        "tool_execution_start" => vec![UiEvent::ToolStarted {
            call_id: event
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("unknown-tool-call")
                .to_owned(),
            name: event
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned(),
            args: tool_arguments_text(event.get("args")),
            intent: event
                .get("intent")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|intent| !intent.is_empty())
                .map(str::to_owned),
        }],
        "tool_execution_update" => project_tool_update(event).into_iter().collect(),
        "tool_execution_end" => vec![UiEvent::ToolFinished(tool_outcome(event))],
        "turn_end" => project_turn_results(event),
        "auth_event" => project_auth_event(event),
        "auth_terminal" => project_auth_terminal(event),
        "subagent_progress" => event
            .get("payload")
            .and_then(|payload| payload.get("progress"))
            .and_then(subagent_info)
            .map(|info| vec![UiEvent::SubagentChanged(info)])
            .unwrap_or_else(|| vec![UiEvent::SubagentsChanged]),
        "subagent_lifecycle" => vec![UiEvent::SubagentsChanged],
        _ => Vec::new(),
    }
}

/// Converts the client's typed snapshots back through their JSON form, the
/// one shape `subagent_info` reads for snapshots and progress frames alike.
fn subagent_infos(snapshots: &[omp_rpc::protocol::SubagentSnapshot]) -> Vec<SubagentInfo> {
    snapshots
        .iter()
        .filter_map(|snapshot| serde_json::to_value(snapshot).ok())
        .filter_map(|value| subagent_info(&value))
        .collect()
}

/// Reads the terminal state OMP reports on `agent_end`. An `error` means the
/// turn failed; `cancelled` means it was stopped or aborted; a steered or plain
/// completion is `Completed`.
fn turn_outcome(event: &RpcEvent) -> TurnOutcome {
    if let Some(error) = event
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        return TurnOutcome::Errored(turn_error_reason(event, error));
    }
    if event.get("cancelled").and_then(Value::as_bool) == Some(true) {
        return TurnOutcome::Interrupted;
    }
    TurnOutcome::Completed
}

/// Phrases a failed turn for the notice. OMP's structured companions to
/// `error` (`errorKind`, `errorProvider`, `errorStatus`, `errorDetail`) give a
/// sentence a person can act on: "OpenAI usage limit reached (HTTP 429): The
/// usage limit has been reached", or "OpenAI authentication failed: no
/// credential is stored for … ; sign in again". Without them the raw string is
/// shown exactly as OMP wrote it. Local encoding failures retain their phase
/// and stable diagnostic code so they are not mistaken for provider rejections.
fn turn_error_reason(event: &RpcEvent, raw: &str) -> String {
    let text = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let Some(kind) = text("errorKind") else {
        return raw.to_owned();
    };
    let provider = text("errorProvider").map(|id| {
        AuthProvider::from_id(id).map_or_else(
            || id.to_owned(),
            |provider| provider.product_name().to_owned(),
        )
    });
    let with_provider = |tail: &str, bare: &str| {
        provider
            .as_deref()
            .map_or_else(|| bare.to_owned(), |provider| format!("{provider} {tail}"))
    };
    let encoding_failed = text("errorPhase") == Some("Encoding");
    let mut reason = if encoding_failed {
        let words = humanize_error_kind(kind);
        with_provider(
            &format!("request preparation failed ({words})"),
            &format!("Request preparation failed ({words})"),
        )
    } else {
        match kind {
            "Authentication" | "CredentialStorageUnavailable" => {
                with_provider("authentication failed", "Authentication failed")
            }
            "Authorization" | "AccountDisabled" => {
                with_provider("refused the account", "The provider refused the account")
            }
            "RateLimited" | "QuotaExhausted" => {
                with_provider("usage limit reached", "Usage limit reached")
            }
            "PaymentRequired" => with_provider("billing problem", "Billing problem"),
            "ContextOverflow" => "The conversation no longer fits the model's context".to_owned(),
            other => {
                let words = humanize_error_kind(other);
                with_provider(
                    &format!("request failed ({words})"),
                    &format!("Request failed ({words})"),
                )
            }
        }
    };
    if encoding_failed {
        if let Some(code) = text("errorCode").filter(|code| {
            code.len() <= 128
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
        }) {
            reason.push_str(&format!(" [{code}]"));
        }
    }
    if let Some(status) = event.get("errorStatus").and_then(Value::as_u64) {
        reason.push_str(&format!(" (HTTP {status})"));
    }
    if let Some(detail) = text("errorDetail") {
        reason.push_str(": ");
        reason.push_str(detail);
    }
    reason
}

/// "ResourceExhausted" → "resource exhausted".
fn humanize_error_kind(kind: &str) -> String {
    let mut words = String::with_capacity(kind.len() + 4);
    for (index, character) in kind.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                words.push(' ');
            }
            words.extend(character.to_lowercase());
        } else {
            words.push(character);
        }
    }
    words
}

fn project_auth_event(event: &RpcEvent) -> Vec<UiEvent> {
    let Some(session_id) = event.get("sessionId").and_then(Value::as_str) else {
        return Vec::new();
    };
    match event.get("event").and_then(Value::as_str) {
        Some("open_url") => event
            .get("url")
            .and_then(Value::as_str)
            .map(|url| {
                vec![UiEvent::OpenAuthenticationUrl {
                    session_id: session_id.to_owned(),
                    url: url.to_owned(),
                    launch: event
                        .get("launch")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                }]
            })
            .unwrap_or_default(),
        Some("device_code") => {
            let (Some(code), Some(verification_url)) = (
                event.get("code").and_then(Value::as_str),
                event.get("verification_url").and_then(Value::as_str),
            ) else {
                return Vec::new();
            };
            vec![UiEvent::AuthenticationDeviceCode {
                session_id: session_id.to_owned(),
                code: code.to_owned(),
                verification_url: verification_url.to_owned(),
            }]
        }
        Some("prompt") => {
            let Some(prompt_id) = event.get("prompt_id").and_then(Value::as_str) else {
                return Vec::new();
            };
            let Some(input) = event.get("input").and_then(Value::as_str) else {
                return Vec::new();
            };
            vec![UiEvent::AuthenticationPrompt(AuthPrompt {
                session_id: session_id.to_owned(),
                prompt_id: prompt_id.to_owned(),
                message: event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Complete authentication in your browser")
                    .to_owned(),
                input: input.to_owned(),
            })]
        }
        Some("waiting") => vec![UiEvent::AuthenticationWaiting {
            session_id: session_id.to_owned(),
            message: "Waiting for account authorization…".into(),
        }],
        _ => Vec::new(),
    }
}

fn authentication_answer(prompt: AuthPrompt, value: String) -> RpcAuthAnswerFrame {
    let input = match prompt.input.as_str() {
        // A callback URL carried back by hand completes a browser sign-in
        // whether OMP asked for one or was only waiting to be told.
        "authorization_code" | "confirmation" if value.contains("://") => {
            RpcAuthInputKind::CallbackUrl
        }
        "authorization_code" => RpcAuthInputKind::AuthorizationCode,
        "api_key" => RpcAuthInputKind::ApiKey,
        "session_token" => RpcAuthInputKind::SessionToken,
        "plain_text" => RpcAuthInputKind::PlainText,
        "optional_secret" => RpcAuthInputKind::OptionalSecret,
        "confirmation" => RpcAuthInputKind::DeviceConfirmed,
        _ => RpcAuthInputKind::Cancel,
    };
    let value = (!matches!(
        input,
        RpcAuthInputKind::DeviceConfirmed | RpcAuthInputKind::Cancel
    ))
    .then_some(value);
    RpcAuthAnswerFrame {
        kind: "auth_answer".into(),
        session_id: prompt.session_id,
        prompt_id: Some(prompt.prompt_id),
        input,
        value,
    }
}

fn project_auth_terminal(event: &RpcEvent) -> Vec<UiEvent> {
    let session_id = event
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let failed = |error: String| UiEvent::AuthenticationFailed {
        session_id: Some(session_id.clone()),
        generation: None,
        error,
    };
    match event.get("outcome").and_then(Value::as_str) {
        Some("completed") => {
            let Some(account) = event.get("account") else {
                return vec![failed(
                    "OMP completed sign-in without identifying the provider".into(),
                )];
            };
            let Some(provider) = account
                .get("providerId")
                .and_then(Value::as_str)
                .and_then(AuthProvider::from_id)
            else {
                return vec![failed(
                    "OMP completed sign-in for an unsupported provider".into(),
                )];
            };
            let field = |name: &str| {
                account
                    .get(name)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            };
            vec![UiEvent::AuthenticationCompleted {
                session_id: session_id.clone(),
                provider,
                account_id: field("accountId"),
                principal: field("principal"),
                account: field("label").or_else(|| field("principal")),
            }]
        }
        Some("cancelled") => vec![UiEvent::AuthenticationCancelled { session_id }],
        Some("failed") => vec![failed(
            event
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Account sign-in failed")
                .to_owned(),
        )],
        _ => Vec::new(),
    }
}

fn authentication_completed(event: &RpcEvent) -> bool {
    event.kind == "auth_terminal" // ubs:ignore — public event kind, not a secret
        && event.get("outcome").and_then(Value::as_str) == Some("completed") // ubs:ignore — public outcome, not a secret
}

fn message_role(event: &RpcEvent) -> Option<&str> {
    event.get("message")?.get("role")?.as_str()
}

fn project_message_update(event: &RpcEvent) -> Option<UiEvent> {
    let update = event.get("assistantMessageEvent")?;
    match update.get("type")?.as_str()? {
        "text_delta" => Some(UiEvent::AssistantDelta(
            update.get("delta")?.as_str()?.to_owned(),
        )),
        "thinking_delta" => Some(UiEvent::ThinkingDelta(
            update.get("delta")?.as_str()?.to_owned(),
        )),
        "toolcall_delta" => Some(UiEvent::ToolArguments {
            call_id: update.get("toolCallId")?.as_str()?.to_owned(),
            delta: update.get("delta")?.as_str()?.to_owned(),
        }),
        "error" => {
            let error = update
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("OMP inference failed")
                .to_owned();
            Some(
                if let Some(provider) = authentication_failure_provider(&error) {
                    UiEvent::AuthenticationRequired { provider, error }
                } else {
                    UiEvent::Failed(error)
                },
            )
        }
        _ => None,
    }
}

fn authentication_failure_provider(error: &str) -> Option<AuthProvider> {
    is_account_auth_failure(error)
        .then(|| provider_in_error(error))
        .flatten()
}

/// Whether an error reads as an account's stored credential failing — a
/// revoked or invalid token, a rejected authorization, a missing credential,
/// or an explicit request to sign in again — rather than a transient
/// inference error. What the account-trouble banner keys on, so a bad
/// credential is surfaced out of band instead of only as a turn error.
pub(crate) fn is_account_auth_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    const MARKERS: [&str; 9] = [
        "token_revoked",
        "invalidated oauth token",
        "no credential is stored",
        "sign in again",
        "authentication failed",
        "(auth)",
        "http 401",
        "401 unauthorized",
        "invalid_grant",
    ];
    if MARKERS.iter().any(|marker| error.contains(marker)) {
        return true;
    }
    // A handshake OMP rejected: "Authentication error during Handshake …".
    error.contains("authentication error") && error.contains("during")
}

/// The supported provider named in an error, if any, so the banner can point
/// at the right accounts even when no single account could be identified.
pub(crate) fn provider_in_error(error: &str) -> Option<AuthProvider> {
    let error = error.to_ascii_lowercase();
    // “OpenAI” in a human error is shared by subscriptions and API keys.
    // Require OMP's precise provider spelling before naming an API account.
    let codex = error.contains("openai-codex");
    let api = !codex
        && ["provider openai", "openai/", "openai:", "openai account"]
            .iter()
            .any(|marker| error.contains(marker));
    let anthropic = error.contains("anthropic");
    match (codex, api, anthropic) {
        (true, false, false) => Some(AuthProvider::OpenAi),
        (false, true, false) => Some(AuthProvider::OpenAiApi),
        (false, false, true) => Some(AuthProvider::Anthropic),
        _ => None,
    }
}

fn project_tool_update(event: &RpcEvent) -> Option<UiEvent> {
    Some(UiEvent::ToolUpdated {
        call_id: event.get("toolCallId")?.as_str()?.to_owned(),
        content: event
            .get("content")
            .or_else(|| event.get("partialResult"))
            .map(|content| {
                content
                    .as_str()
                    .map_or_else(|| content.to_string(), ToOwned::to_owned)
            })
            .unwrap_or_default(),
        args: tool_arguments_text(event.get("args")),
    })
}

/// Renders a tool call's `args` value from an RPC event as a JSON string, or
/// `None` when the argument object is absent or empty. OMP reports the full
/// arguments on `tool_execution_start`/`_update` in RPC mode (it never streams
/// `toolcall_delta`), so this is the live transcript's only source of them.
fn tool_arguments_text(args: Option<&Value>) -> Option<String> {
    let args = args?;
    if args.is_null() {
        return None;
    }
    let text = args
        .as_str()
        .map_or_else(|| args.to_string(), ToOwned::to_owned);
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return None;
    }
    Some(text)
}

fn tool_outcome(event: &RpcEvent) -> ToolOutcome {
    let result = event.get("result").cloned();
    ToolOutcome {
        call_id: event
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or("unknown-tool-call")
            .to_owned(),
        name: event
            .get("toolName")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                result
                    .as_ref()?
                    .get("toolName")?
                    .as_str()
                    .map(ToOwned::to_owned)
            }),
        is_error: event
            .get("isError")
            .and_then(Value::as_bool)
            .or_else(|| result.as_ref()?.get("isError")?.as_bool()),
        result,
    }
}

fn project_turn_results(event: &RpcEvent) -> Vec<UiEvent> {
    let outcomes = event
        .get("toolResults")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|result| ToolOutcome {
            call_id: result
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("unknown-tool-call")
                .to_owned(),
            name: result
                .get("toolName")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            is_error: result.get("isError").and_then(Value::as_bool),
            result: Some(result.clone()),
        })
        .collect::<Vec<_>>();
    (!outcomes.is_empty())
        .then_some(UiEvent::TurnResults(outcomes))
        .into_iter()
        .collect()
}

fn final_message_text(event: &RpcEvent) -> Option<String> {
    let content = event.get("message")?.get("content")?.as_array()?;
    let mut output = String::new();
    for part in content {
        if part.get("type").and_then(Value::as_str) == Some("text") // ubs:ignore — content discriminator, not a secret
            && let Some(text) = part.get("text").and_then(Value::as_str)
        {
            output.push_str(text);
        }
    }
    (!output.is_empty()).then_some(output)
}

#[cfg(test)]
mod tests {
    use crate::mock_inference::Backend;
    use std::{path::PathBuf, time::Duration};

    use serde_json::json;

    #[test]
    fn a_host_tool_call_waits_for_the_apps_answer_and_a_cancelled_one_is_withdrawn() {
        let (sender, events) = flume::unbounded();
        let events_tx = UiEventSender(sender);
        let calls = super::HostCalls::default();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let app = {
            let calls = calls.clone();
            std::thread::spawn(move || {
                let (_, event) = events.recv().unwrap();
                let UiEvent::HostToolCall { call, tool, arguments } = event else { panic!("{event:?}") };
                assert_eq!(tool, "omperor_list_projects");
                assert_eq!(arguments.get("limit"), Some(&json!(3)));
                assert!(calls.answer(call, Ok(json!({ "projects": [] }))));
                events
            })
        };
        let arguments = json!({ "limit": 3 }).as_object().cloned().unwrap();
        let answer = runtime.block_on(calls.ask(
            &events_tx,
            "omperor_list_projects".into(),
            arguments,
            std::future::pending(),
        ));
        assert_eq!(answer, Ok(json!({ "projects": [] })));
        let events = app.join().unwrap();

        // OMP stops the turn before the app answers: the app is told, and a
        // late answer goes nowhere.
        let answer = runtime.block_on(calls.ask(
            &events_tx,
            "omperor_delete_sessions".into(),
            serde_json::Map::new(),
            async {},
        ));
        assert!(answer.is_err());
        let UiEvent::HostToolCall { call, .. } = events.recv().unwrap().1 else { panic!() };
        assert!(matches!(events.recv().unwrap().1, UiEvent::HostToolCancelled { call: cancelled } if cancelled == call));
        assert!(!calls.answer(call, Ok(json!(null))));
    }

    use omp_rpc::protocol::RpcAuthInputKind;

    use super::{
        AccountState, AccountSummary, AuthPrompt, AuthProvider, Command, LaunchConfiguration,
        LiveSession, RecoveryAction, StartupWait, UiEvent, UiEventSender, authentication_answer,
        authentication_completed, available_models, credential_source, fast_models,
        is_supported_subscription_model, looks_like_email, preferred_available_model,
        project_event, rpc_client_options, sanitized_model_preference, state_model,
        supported_accounts, wait_during_startup, wait_for_relaunch,
    };
    use omp_rpc::protocol::{RpcAccount, RpcAccountState};

    fn event(value: serde_json::Value) -> omp_rpc::protocol::RpcEvent {
        serde_json::from_value(value).expect("valid RPC event")
    }

    fn lifecycle_ready(runtime: &super::OmpRuntime) -> String {
        loop {
            match runtime
                .events
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .1
            {
                UiEvent::Ready {
                    session_id: Some(id),
                    ..
                } => return id,
                UiEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
    }

    fn lifecycle_release(runtime: &super::OmpRuntime) {
        assert!(runtime.release_session());
        loop {
            match runtime
                .events
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .1
            {
                UiEvent::SessionReleased => break,
                UiEvent::Failed(error) => panic!("{error}"),
                UiEvent::Stopped => panic!("release retired the worker"),
                _ => {}
            }
        }
        assert!(!runtime.try_reap());
    }

    fn release_resume_scenario(backend: Backend) {
        use super::{OmpRuntime, RuntimeStartup, SessionStartup};
        let root = std::env::temp_dir().join(format!(
            "omperor-release-{}-{:?}",
            std::process::id(),
            backend
        ));
        let start = |session| {
            OmpRuntime::start_configured(RuntimeStartup {
                helper: None,
                backend,
                project: root.clone(),
                model: None,
                thinking_level: None,
                fast_mode: false,
                omperor_tools: false,
                computer_use: false,
                account: None,
                data_dir: root.join("data"),
                session_dir: root.join("sessions"),
                environment: vec![],
                session,
            })
        };
        // Two fresh children race the very first shared credential-store boot.
        let source = start(SessionStartup::Fresh);
        let other = start(SessionStartup::Fresh);
        let id = lifecycle_ready(&source);
        assert_ne!(id, lifecycle_ready(&other));
        lifecycle_release(&source);
        let fork = start(SessionStartup::Fork(id.clone()));
        assert_ne!(id, lifecycle_ready(&fork));
        assert!(source.resume_session(id.clone(), root.clone()));
        assert_eq!(id, lifecycle_ready(&source));
        lifecycle_release(&source);
        // Already offline release must still acknowledge, without ending worker.
        lifecycle_release(&source);
        assert!(source.resume_session(id.clone(), root.clone()));
        assert_eq!(id, lifecycle_ready(&source));
        for runtime in [&source, &other, &fork] {
            runtime.begin_shutdown();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while [&source, &other, &fork]
            .iter()
            .any(|runtime| !runtime.try_reap())
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        // Mock workers keep journals in memory; no directory is created.
        if backend == Backend::Omp {
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn release_keeps_mock_worker_resumable() {
        release_resume_scenario(Backend::Mock);
    }

    #[test]
    #[ignore = "requires the built OMP helper; exercises real cold stores and journal locks"]
    fn real_helper_cold_concurrent_release_fork_resume() {
        release_resume_scenario(Backend::Omp);
    }

    #[test]
    #[ignore = "requires the built OMP helper"]
    fn real_helper_startup_settings_precede_ready_and_resume_preserves_journal() {
        use super::{OmpRuntime, RuntimeStartup, SessionStartup};
        let root =
            std::env::temp_dir().join(format!("omperor-startup-settings-{}", std::process::id()));
        let start = |session, thinking_level, fast_mode| {
            OmpRuntime::start_configured(RuntimeStartup {
                helper: None,
                backend: Backend::Omp,
                project: root.clone(),
                model: None,
                thinking_level,
                fast_mode,
                omperor_tools: false,
                computer_use: false,
                account: None,
                data_dir: root.join("data"),
                session_dir: root.join("sessions"),
                environment: vec![],
                session,
            })
        };
        let ready = |runtime: &OmpRuntime| loop {
            match runtime
                .events
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .1
            {
                UiEvent::Ready {
                    session_id: Some(id),
                    thinking_level,
                    fast_mode,
                    ..
                } => break (id, thinking_level, fast_mode),
                UiEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        };
        let fresh = start(SessionStartup::Fresh, Some("high".into()), true);
        let (id, thinking, fast) = ready(&fresh);
        assert_eq!(thinking, "high");
        assert!(fast);
        lifecycle_release(&fresh);
        let resumed = start(SessionStartup::Resume(id), Some("low".into()), false);
        let (_, thinking, fast) = ready(&resumed);
        assert_eq!(thinking, "high");
        assert!(fast);
        lifecycle_release(&resumed);
        for runtime in [&fresh, &resumed] {
            runtime.begin_shutdown();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !fresh.try_reap() || !resumed.try_reap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_gate_wait_is_cancellable_without_spawning() {
        use super::{OmpRuntime, RuntimeStartup, STARTUP_GATE, SessionStartup};
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let guard = executor.block_on(STARTUP_GATE.lock());
        let runtime = OmpRuntime::start_configured(RuntimeStartup {
            helper: None,
            backend: Backend::Omp,
            project: "/does-not-exist-gate-test".into(),
            model: None,
            thinking_level: None,
            fast_mode: false,
            omperor_tools: false,
            computer_use: false,
            account: None,
            data_dir: "/does-not-exist-gate-test/data".into(),
            session_dir: "/does-not-exist-gate-test/sessions".into(),
            environment: vec![],
            session: SessionStartup::Fresh,
        });
        loop {
            if matches!(
                runtime
                    .events
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .1,
                UiEvent::Connecting { .. }
            ) {
                break;
            }
        }
        lifecycle_release(&runtime);
        assert!(runtime.resume_session("same-session".into(), "/does-not-exist-gate-test".into()));
        loop {
            if matches!(
                runtime
                    .events
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .1,
                UiEvent::Connecting { .. }
            ) {
                break;
            }
        }
        runtime.begin_shutdown();
        assert!(matches!(
            runtime
                .events
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .1,
            UiEvent::Stopped
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !runtime.try_reap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(guard);
        assert!(STARTUP_GATE.try_lock().is_ok());
    }

    #[test]
    fn projects_streaming_text_delta() {
        let projected = project_event(&event(json!({
            "type":"message_update",
            "assistantMessageEvent":{"type":"text_delta","delta":"hello"}
        })));

        assert_eq!(projected, vec![UiEvent::AssistantDelta("hello".into())]);
    }

    #[test]
    fn receipt_cache_observations_distinguish_missing_from_reported_zero() {
        let recorded = super::InferenceReceipt::from_event(&event(json!({
            "type":"inference_receipt",
            "receiptId":"01CACHEACCOUNTING",
            "recordedAtMs":1750000000123_u64,
            "usageSource":"provider",
            "costSource":"catalog_estimate",
            "costNanoUsd":24000000,
            "tokensIn":200,
            "cacheRead":800,
            "cacheWrite":0,
            "cacheReadReported":true,
            "cacheWriteReported":true,
            "cacheWrite1hReported":false,
            "promptCacheTtlSeconds":1800,
            "promptCacheRetention":"24h",
            "responseCompletedAtMs":1750000000000_u64
        })));
        assert_eq!(recorded.receipt_id.as_deref(), Some("01CACHEACCOUNTING"));
        assert_eq!(recorded.recorded_at_ms, Some(1750000000123));
        assert_eq!(recorded.usage_source.as_deref(), Some("provider"));
        assert_eq!(recorded.cost_source.as_deref(), Some("catalog_estimate"));
        assert_eq!(recorded.cost_nano_usd, Some(24000000));
        assert_eq!(recorded.cache_write_reported, Some(true));
        assert_eq!(recorded.cache_write, 0);
        assert_eq!(recorded.cache_write_1h, None);
        assert_eq!(recorded.cache_write_1h_reported, Some(false));
        assert_eq!(recorded.prompt_cache_ttl_seconds, Some(1800));
        assert_eq!(recorded.prompt_cache_retention.as_deref(), Some("24h"));
        assert_eq!(recorded.response_completed_at_ms, Some(1750000000000));
        let missing = super::InferenceReceipt::from_event(&event(json!({
            "type":"inference_receipt"
        })));
        assert_eq!(missing.usage_source, None);
        assert_eq!(missing.cost_nano_usd, None);
        assert_eq!(missing.cache_write_reported, None);
        assert_eq!(missing.cache_write_1h, None);
        assert_eq!(missing.prompt_cache_ttl_seconds, None);
        assert_eq!(missing.prompt_cache_retention, None);
        assert_eq!(missing.response_completed_at_ms, None);
    }

    #[test]
    fn projects_an_inference_receipt_with_its_timings() {
        let projected = project_event(&event(json!({
            "type":"inference_receipt",
            "provider":"openai-codex",
            "model":"gpt-5.6-sol",
            "tokensIn":1200,
            "tokensOut":340,
            "tokensReasoning":80,
            "cacheRead":1000,
            "cacheWrite":0,
            "costNanoUsd":0,
            "ttftMs":812,
            "lastTokenMs":4211,
            "durationMs":4260
        })));
        assert_eq!(
            projected,
            vec![UiEvent::InferenceReceipt(super::InferenceReceipt {
                provider: Some("openai-codex".into()),
                model: Some("gpt-5.6-sol".into()),
                tokens_in: 1200,
                tokens_out: 340,
                tokens_reasoning: 80,
                cache_read: 1000,
                cache_write: 0,
                ttft_ms: Some(812),
                last_token_ms: Some(4211),
                duration_ms: Some(4260),
                cost_nano_usd: Some(0),
                ..super::InferenceReceipt::default()
            })]
        );
        // A receipt without timings (a failed stream) still names its counts.
        let bare = project_event(&event(json!({
            "type":"inference_receipt",
            "provider":null,
            "model":null,
            "tokensIn":0,
            "tokensOut":0,
            "ttftMs":null,
            "lastTokenMs":null,
            "durationMs":null
        })));
        assert_eq!(
            bare,
            vec![UiEvent::InferenceReceipt(super::InferenceReceipt::default())]
        );
    }

    #[test]
    fn subscription_model_filter_accepts_runtime_discovered_first_party_models() {
        for supported in [
            "openai-codex/gpt-5.6-luna",
            "anthropic/claude-fable-5-1",
            "anthropic/claude-opus-5",
            "anthropic/claude-sonnet-5",
            "anthropic/claude-haiku-4-5",
        ] {
            assert!(is_supported_subscription_model(supported), "{supported}");
        }
        for unsupported in [
            "cursor/default",
            "aimlapi/alibaba/qwen-max",
            "openai-codex/",
            "anthropic/",
            "gpt-5.6-luna",
        ] {
            assert!(
                !is_supported_subscription_model(unsupported),
                "{unsupported}"
            );
        }
    }

    #[test]
    fn model_preferences_never_invent_a_provider_default() {
        assert_eq!(
            sanitized_model_preference(Some("anthropic/claude-fable-5-1[1m]")).as_deref(),
            Some("anthropic/claude-fable-5-1")
        );
        assert_eq!(
            sanitized_model_preference(Some("anthropic/claude-fable-5-1")).as_deref(),
            Some("anthropic/claude-fable-5-1")
        );
        assert_eq!(
            sanitized_model_preference(Some("openai-codex/gpt-5.6-sol")).as_deref(),
            Some("openai-codex/gpt-5.6-sol")
        );
        assert_eq!(sanitized_model_preference(Some("cursor/default")), None);
        assert_eq!(sanitized_model_preference(Some("anthropic/")), None);
        assert_eq!(sanitized_model_preference(None), None);
    }

    #[test]
    fn unavailable_saved_model_moves_to_a_returned_model_from_the_same_provider() {
        let available = vec![
            "openai-codex/gpt-5.6-sol".to_owned(),
            "anthropic/claude-fable-5".to_owned(),
            "anthropic/claude-opus-5".to_owned(),
        ];
        assert_eq!(
            preferred_available_model(
                "anthropic/claude-mythos-5",
                "anthropic/claude-mythos-5",
                &available,
            )
            .as_deref(),
            Some("anthropic/claude-fable-5")
        );
        assert_eq!(
            preferred_available_model(
                "openai-codex/gpt-5.6-sol",
                "anthropic/claude-mythos-5",
                &available,
            )
            .as_deref(),
            Some("openai-codex/gpt-5.6-sol")
        );
    }

    #[test]
    fn projects_thinking_and_addressed_tool_arguments() {
        assert_eq!(
            project_event(&event(json!({
                "type":"message_update",
                "assistantMessageEvent":{"type":"thinking_delta","delta":"Considering paths"}
            }))),
            vec![UiEvent::ThinkingDelta("Considering paths".into())]
        );
        assert_eq!(
            project_event(&event(json!({
                "type":"message_update",
                "assistantMessageEvent":{
                    "type":"toolcall_delta",
                    "toolCallId":"call-1",
                    "delta":"{\"path\":\"README.md\"}"
                }
            }))),
            vec![UiEvent::ToolArguments {
                call_id: "call-1".into(),
                delta: "{\"path\":\"README.md\"}".into(),
            }]
        );
    }

    #[test]
    fn ignores_duplicate_unaddressed_tool_argument_fragments() {
        let projected = project_event(&event(json!({
            "type":"message_update",
            "assistantMessageEvent":{"type":"toolcall_delta","delta":"duplicate"}
        })));

        assert!(projected.is_empty());
    }

    #[test]
    fn projects_expired_openai_credentials_as_reauthentication() {
        let error = "OMP error: terminal turn error (Auth): Authentication failed for provider openai-codex. Use /login openai-codex in chat.";
        let projected = project_event(&event(json!({
            "type":"message_update",
            "assistantMessageEvent":{"type":"error","error":error}
        })));

        assert_eq!(
            projected,
            vec![UiEvent::AuthenticationRequired {
                provider: AuthProvider::OpenAi,
                error: error.into(),
            }]
        );
    }

    #[test]
    fn projects_expired_anthropic_credentials_as_reauthentication() {
        let error = "OMP error: terminal turn error (Auth): Authentication failed for provider anthropic. Use /login anthropic in chat.";
        let projected = project_event(&event(json!({
            "type":"message_update",
            "assistantMessageEvent":{"type":"error","error":error}
        })));

        assert_eq!(
            projected,
            vec![UiEvent::AuthenticationRequired {
                provider: AuthProvider::Anthropic,
                error: error.into(),
            }]
        );
    }

    #[test]
    fn credential_errors_and_provider_attribution_are_specific() {
        for error in [
            "token_revoked",
            "invalid_grant",
            "invalidated oauth token",
            "http 401",
            "401 Unauthorized",
            "no credential is stored",
            "Authentication error during Handshake",
        ] {
            assert!(super::is_account_auth_failure(error), "{error}");
        }
        for error in [
            "stream corruption (HTTP 200)",
            "tool timed out",
            "HTTP 429 rate limit",
            "disk full",
        ] {
            assert!(!super::is_account_auth_failure(error), "{error}");
        }
        assert_eq!(
            super::provider_in_error("OpenAI request failed (HTTP 401)"),
            None
        );
        assert_eq!(
            super::provider_in_error("provider openai authentication failed"),
            Some(AuthProvider::OpenAiApi)
        );
        assert_eq!(
            super::provider_in_error("openai-codex account revoked"),
            Some(AuthProvider::OpenAi)
        );
        assert_eq!(
            super::provider_in_error("anthropic HTTP 401"),
            Some(AuthProvider::Anthropic)
        );
        assert_eq!(
            super::authentication_failure_provider("openai-codex token_revoked [http 401]"),
            Some(AuthProvider::OpenAi)
        );
    }

    #[test]
    fn leaves_non_authentication_inference_errors_as_failures() {
        let error = "OMP error: upstream request failed";
        let projected = project_event(&event(json!({
            "type":"message_update",
            "assistantMessageEvent":{"type":"error","error":error}
        })));

        assert_eq!(projected, vec![UiEvent::Failed(error.into())]);
    }

    #[test]
    fn projects_detailed_tool_completion_and_turn_fallback() {
        let result = json!({
            "role":"toolResult",
            "toolCallId":"call-1",
            "toolName":"bash",
            "content":[],
            "details":{"kind":"faulted","value":{"message":"README.md missing"}},
            "isError":true
        });
        assert_eq!(
            project_event(&event(json!({
                "type":"tool_execution_end",
                "toolCallId":"call-1",
                "toolName":"bash",
                "result":result,
                "isError":true
            }))),
            vec![UiEvent::ToolFinished(super::ToolOutcome {
                call_id: "call-1".into(),
                name: Some("bash".into()),
                is_error: Some(true),
                result: Some(result.clone()),
            })]
        );
        assert_eq!(
            project_event(&event(json!({
                "type":"turn_end",
                "message":{"role":"assistant","content":[]},
                "toolResults":[result.clone()]
            }))),
            vec![UiEvent::TurnResults(vec![super::ToolOutcome {
                call_id: "call-1".into(),
                name: Some("bash".into()),
                is_error: Some(true),
                result: Some(result),
            }])]
        );
    }

    #[test]
    fn phrases_structured_turn_errors_and_keeps_raw_ones() {
        let usage_limit = project_event(&event(json!({
            "type":"agent_end",
            "isTerminal":true,
            "cancelled":false,
            "error":"inference RateLimited error during Handshake (usage_limit_reached) [http 429]: The usage limit has been reached",
            "errorKind":"RateLimited",
            "errorPhase":"Handshake",
            "errorProvider":"openai-codex",
            "errorStatus":429,
            "errorCode":"usage_limit_reached",
            "errorDetail":"The usage limit has been reached"
        })));
        assert_eq!(
            usage_limit,
            vec![UiEvent::TurnFinished(super::TurnOutcome::Errored(
                "OpenAI usage limit reached (HTTP 429): The usage limit has been reached".into()
            ))]
        );

        let signed_out = project_event(&event(json!({
            "type":"agent_end",
            "isTerminal":true,
            "cancelled":false,
            "error":"inference Authentication error during Authentication: no credential is stored for openai-codex account openai-codex:dos; sign in again",
            "errorKind":"Authentication",
            "errorPhase":"Authentication",
            "errorProvider":"openai-codex",
            "errorDetail":"no credential is stored for openai-codex account openai-codex:dos; sign in again"
        })));
        assert_eq!(
            signed_out,
            vec![UiEvent::TurnFinished(super::TurnOutcome::Errored(
                "OpenAI authentication failed: no credential is stored for openai-codex account openai-codex:dos; sign in again".into()
            ))]
        );

        let unknown_kind = project_event(&event(json!({
            "type":"agent_end",
            "isTerminal":true,
            "cancelled":false,
            "error":"inference ResourceExhausted error during Handshake [http 503]",
            "errorKind":"ResourceExhausted",
            "errorPhase":"Handshake",
            "errorStatus":503
        })));
        assert_eq!(
            unknown_kind,
            vec![UiEvent::TurnFinished(super::TurnOutcome::Errored(
                "Request failed (resource exhausted) (HTTP 503)".into()
            ))]
        );

        let raw = project_event(&event(json!({
            "type":"agent_end",
            "isTerminal":true,
            "cancelled":false,
            "error":"inference Authentication error during Authentication"
        })));
        assert_eq!(
            raw,
            vec![UiEvent::TurnFinished(super::TurnOutcome::Errored(
                "inference Authentication error during Authentication".into()
            ))]
        );
    }

    #[test]
    fn identifies_local_encoding_failures_without_losing_the_diagnostic_code() {
        let mut terminal = json!({
            "type":"agent_end",
            "isTerminal":true,
            "error":"inference InvalidRequest error during Encoding",
            "errorKind":"InvalidRequest",
            "errorPhase":"Encoding",
            "errorProvider":"openai-codex",
            "errorCode":"codex_unsupported_responses_output_format"
        });
        assert_eq!(
            project_event(&event(terminal.clone())),
            vec![UiEvent::TurnFinished(super::TurnOutcome::Errored(
                "OpenAI request preparation failed (invalid request) [codex_unsupported_responses_output_format]".into()
            ))]
        );
        terminal["errorCode"] = json!("unexpected diagnostic body\nwith details");
        assert_eq!(
            project_event(&event(terminal)),
            vec![UiEvent::TurnFinished(super::TurnOutcome::Errored(
                "OpenAI request preparation failed (invalid request)".into()
            ))]
        );
    }

    #[test]
    fn projects_final_assistant_text() {
        let projected = project_event(&event(json!({
            "type":"message_end",
            "message":{"role":"assistant","content":[{"type":"text","text":"done"}]}
        })));

        assert_eq!(
            projected,
            vec![UiEvent::AssistantFinished(Some("done".into()))]
        );
    }

    #[test]
    fn extracts_model_from_rpc_state() {
        assert_eq!(
            state_model(&json!({"model":{"provider":"openai-codex","id":"gpt-5.6-luna"}})),
            Some("openai-codex/gpt-5.6-luna".into())
        );
        assert_eq!(
            state_model(&json!({"model":"openai-codex/gpt-5.6-sol"})),
            Some("openai-codex/gpt-5.6-sol".into())
        );
        assert_eq!(
            available_models(&json!({
                "models":["openai-codex/gpt-5.6-sol", "openai-codex/gpt-5.6-luna"]
            })),
            ["openai-codex/gpt-5.6-sol", "openai-codex/gpt-5.6-luna"]
        );
    }

    #[test]
    fn only_models_whose_backend_advertises_fast_get_the_switch() {
        let listing = json!({
            "models": [
                {"provider": "openai-codex", "id": "gpt-5.6-sol", "fast": true},
                {"provider": "openai-codex", "id": "gpt-daybreak-blue-latest", "fast": false},
                {"provider": "anthropic", "id": "claude-fable-5-1"},
                "openai-codex/gpt-5.5"
            ]
        });
        assert_eq!(fast_models(&listing), ["openai-codex/gpt-5.6-sol"]);
        assert!(fast_models(&json!({})).is_empty());
    }

    #[test]
    fn carries_both_the_sign_in_page_and_its_loopback_launcher() {
        let projected = project_event(&event(json!({
            "type":"auth_event",
            "sessionId":"auth-1",
            "event":"open_url",
            "url":"https://auth.example/authorize",
            "launch":"http://localhost:1455/launch"
        })));

        assert_eq!(
            projected,
            vec![UiEvent::OpenAuthenticationUrl {
                session_id: "auth-1".into(),
                url: "https://auth.example/authorize".into(),
                launch: Some("http://localhost:1455/launch".into()),
            }]
        );
    }

    #[test]
    fn a_device_code_is_carried_whole_and_a_cancelled_login_says_so() {
        let projected = project_event(&event(json!({
            "type":"auth_event",
            "sessionId":"auth-1",
            "event":"device_code",
            "code":"ABCD-EFGH",
            "verification_url":"https://auth.example/device"
        })));
        assert_eq!(
            projected,
            vec![UiEvent::AuthenticationDeviceCode {
                session_id: "auth-1".into(),
                code: "ABCD-EFGH".into(),
                verification_url: "https://auth.example/device".into(),
            }]
        );
        let projected = project_event(&event(json!({
            "type":"auth_terminal",
            "sessionId":"auth-1",
            "outcome":"cancelled"
        })));
        assert_eq!(
            projected,
            vec![UiEvent::AuthenticationCancelled {
                session_id: "auth-1".into()
            }]
        );
    }

    #[test]
    fn a_pasted_callback_url_answers_a_waiting_browser_sign_in() {
        let prompt = AuthPrompt {
            session_id: "auth-1".into(),
            prompt_id: "oauth-callback".into(),
            message: "Complete authorization in the opened browser".into(),
            input: "confirmation".into(),
        };
        let answer = authentication_answer(
            prompt.clone(),
            "http://localhost:1455/auth/callback?code=c&state=s".into(),
        );
        assert_eq!(answer.input, RpcAuthInputKind::CallbackUrl);
        assert!(answer.value.is_some());
        let confirmed = authentication_answer(prompt, String::new());
        assert_eq!(confirmed.input, RpcAuthInputKind::DeviceConfirmed);
        assert_eq!(confirmed.value, None);
    }

    #[test]
    fn projects_successful_authentication() {
        let event = event(json!({
            "type":"auth_terminal",
            "sessionId":"auth-1",
            "outcome":"completed",
            "account":{"accountId":"openai-codex:ws-1","providerId":"openai-codex","principal":"ws-1","label":"OpenAI user"}
        }));
        let projected = project_event(&event);

        assert_eq!(
            projected,
            vec![UiEvent::AuthenticationCompleted {
                session_id: "auth-1".into(),
                provider: AuthProvider::OpenAi,
                account_id: Some("openai-codex:ws-1".into()),
                principal: Some("ws-1".into()),
                account: Some("OpenAI user".into()),
            }]
        );
        assert!(authentication_completed(&event));
    }

    #[test]
    fn only_email_principals_are_shown_as_subscriber_identity() {
        let anthropic = AccountSummary {
            id: "anthropic:person@example.com".into(),
            provider: AuthProvider::Anthropic,
            principal: Some("person@example.com".into()),
            state: AccountState::Active,
        };
        let codex = AccountSummary {
            id: "openai-codex:00000000-0000-4000-8000-000000000001".into(),
            provider: AuthProvider::OpenAi,
            principal: Some("00000000-0000-4000-8000-000000000001".into()),
            state: AccountState::Active,
        };
        assert_eq!(anthropic.email(), Some("person@example.com"));
        assert_eq!(codex.email(), None);
        assert!(!looks_like_email("@example.com"));
        assert!(!looks_like_email("person@localhost"));
        assert!(!looks_like_email("person name@example.com"));
    }

    #[test]
    fn account_listing_keeps_only_supported_live_accounts() {
        let accounts = vec![
            RpcAccount {
                account_id: "anthropic:a@example.com".into(),
                provider_id: "anthropic".into(),
                principal: Some("a@example.com".into()),
                label: None,
                state: RpcAccountState::Active,
            },
            RpcAccount {
                account_id: "anthropic:gone@example.com".into(),
                provider_id: "anthropic".into(),
                principal: Some("gone@example.com".into()),
                label: None,
                state: RpcAccountState::LoggedOut,
            },
            RpcAccount {
                account_id: "google:user".into(),
                provider_id: "google".into(),
                principal: None,
                label: None,
                state: RpcAccountState::Active,
            },
            RpcAccount {
                account_id: "openai-codex:ws".into(),
                provider_id: "openai-codex".into(),
                principal: Some("ws".into()),
                label: None,
                state: RpcAccountState::RefreshRequired,
            },
        ];
        let supported = supported_accounts(&accounts);
        assert_eq!(
            supported
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            ["anthropic:a@example.com", "openai-codex:ws"]
        );
        assert!(supported.iter().all(|account| account.state.is_usable()));
    }

    #[test]
    fn rpc_helper_sets_a_credential_source_and_selects_native_gui_tools() {
        let options = rpc_client_options(
            "/tmp/omp".into(),
            &LaunchConfiguration {
                helper: None,
                generation: 0,
                backend: Backend::Omp,
                project: "/tmp/project".into(),
                model: "openai-codex/gpt-5.6-sol".into(),
                thinking_level: None,
                fast_mode: false,
                omperor_tools: false,
                computer_use: false,
                host_calls: crate::omp_runtime::HostCalls::default(),
                account: None,
                data_dir: "/tmp/data".into(),
                session_dir: "/tmp/sessions".into(),
                environment: vec![],
                resume: Some("session-existing".into()),
                fork: None,
            },
        );

        assert!(options.env.contains_key("OMP_LLM_KEY_SOURCE"));
        assert_eq!(
            options.env.get("OMP_TOKEN_EFFICIENT").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            options.env.get("OMP_DATA_DIR").map(String::as_str),
            Some("/tmp/data")
        );
        for (variable, path) in [
            ("OMP_STATE_DIR", "/tmp/data/state"),
            ("OMP_CACHE_DIR", "/tmp/data/cache"),
            ("OMP_CONFIG_DIR", "/tmp/data/config"),
            ("OMP_WORKTREE_DIR", "/tmp/data/worktrees"),
            ("OMP_PROFILE", "default"),
        ] {
            assert_eq!(options.env.get(variable).map(String::as_str), Some(path));
        }
        // Git run in an isolated sub-agent copy never climbs out of it.
        assert!(
            options
                .env
                .get("GIT_CEILING_DIRECTORIES")
                .is_some_and(|ceilings| ceilings.split(':').next() == Some("/tmp/data/worktrees"))
        );
        assert_eq!(
            options.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        assert_eq!(options.model.as_deref(), Some("openai-codex/gpt-5.6-sol"));
        assert_eq!(
            options.session_dir.as_deref(),
            Some(std::path::Path::new("/tmp/sessions"))
        );
        assert_eq!(options.ready_timeout, Duration::from_secs(15));
        assert_eq!(
            options.extra_args,
            [
                "--tools",
                "ast_edit,bash,browser,debug,edit,glob,grep,hub,read,task,think,todo,write",
                "--envd-idle-timeout",
                "1",
                "--computer-use",
                "false",
                "--prompt-cache-key",
                "omperor-session-existing",
                "--resume",
                "session-existing"
            ]
        );
    }

    #[test]
    fn a_pinned_account_is_handed_to_the_child_before_it_discovers_models() {
        for computer_use in [false, true] {
            let options = rpc_client_options(
                "/tmp/omp".into(),
                &LaunchConfiguration {
                    helper: None,
                    generation: 0,
                    backend: Backend::Omp,
                    project: "/tmp/project".into(),
                    model: "openai-codex/gpt-5.6-sol".into(),
                    thinking_level: None,
                    fast_mode: false,
                    omperor_tools: false,
                    computer_use,
                    host_calls: crate::omp_runtime::HostCalls::default(),
                    account: Some("openai-codex:tres".into()),
                    data_dir: "/tmp/data".into(),
                    session_dir: "/tmp/sessions".into(),
                    environment: vec![],
                    resume: None,
                    fork: None,
                },
            );
            assert_eq!(
                &options.extra_args[options.extra_args.len() - 2..],
                ["--account", "openai-codex:tres"],
                "{:?}",
                options.extra_args
            );
            assert_eq!(options.extra_args[1].to_string_lossy().split(',').any(|tool| tool == "computer"), computer_use);
            let config = options.extra_args.windows(2).find(|args| args[0] == "--computer-use");
            assert!(config.is_some());
            if let Some(config) = config {
                assert_eq!(config[1], computer_use.to_string().as_str());
            }
        }
    }

    #[test]
    fn a_completed_sign_in_waits_for_the_running_turn() {
        let (sender, receiver) = flume::unbounded();
        let events = super::UiEventSender(sender);
        let mut turn = super::TurnWatch::default();
        let signed_in = event(serde_json::json!({"type": "auth_terminal", "outcome": "completed"}));
        assert!(!turn.forward(&event(serde_json::json!({"type": "agent_start"})), &events));
        assert!(!turn.forward(&signed_in, &events), "a running turn must not be killed");
        assert!(turn.forward(&event(serde_json::json!({"type": "agent_end", "messages": []})), &events));
        assert!(receiver.drain().any(|(_, event)| matches!(event, super::UiEvent::Notice(_))));
        let mut idle = super::TurnWatch::default();
        assert!(idle.forward(&signed_in, &events));
    }

    #[test]
    fn queued_prompts_survive_a_restart_but_stops_and_steers_do_not() {
        let prompt = || Command::Prompt(String::from("hello").into());
        let mut pending = std::collections::VecDeque::from([
            prompt(),
            Command::Steer(String::from("steer").into()),
            Command::Abort,
            Command::AnswerAuthentication(AuthPrompt { session_id: "s".into(), prompt_id: "p".into(), message: String::new(), input: String::new() }, "code".into()),
            Command::SetModel("p/next".into()),
        ]);
        super::retain_session_commands(&mut pending);
        assert_eq!(pending.len(), 2);
        assert!(matches!(pending[0], Command::Prompt(_)));
        let mut launch = LaunchConfiguration {
            helper: None, generation: 1, backend: Backend::Omp, project: "/p".into(), model: "p/old".into(),
            thinking_level: None, fast_mode: false, omperor_tools: false, computer_use: false,
            host_calls: crate::omp_runtime::HostCalls::default(), account: None, data_dir: "/d".into(), session_dir: "/s".into(),
            environment: vec![],
            resume: None, fork: None,
        };
        assert_eq!(super::carry_over_while_offline(&mut pending, &mut launch), 1);
        assert_eq!(launch.model, "p/next");
        assert!(pending.is_empty());
    }

    #[test]
    fn local_file_is_the_noninteractive_credential_default() {
        assert_eq!(credential_source(None), "local-file");
        assert_eq!(credential_source(Some("os-keychain".into())), "os-keychain");
        assert_eq!(credential_source(Some("  ".into())), "local-file");
    }

    #[test]
    fn shutdown_interrupts_rpc_startup_without_dropping_queued_commands() {
        let (sender, receiver) = flume::unbounded();
        sender
            .send(Command::Prompt("queued".into()))
            .expect("startup command queued");
        sender
            .send(Command::Shutdown)
            .expect("shutdown command queued");
        let mut pending = std::collections::VecDeque::new();
        let mut launch = LaunchConfiguration {
            helper: None,
            generation: 0,
            backend: Backend::Omp,
            project: "/tmp/project".into(),
            model: "provider/model".into(),
            thinking_level: None,
            fast_mode: false,
            omperor_tools: false,
            computer_use: false,
            host_calls: crate::omp_runtime::HostCalls::default(),
            account: None,
            data_dir: "/tmp/data".into(),
            session_dir: "/tmp/sessions".into(),
            environment: vec![],
            resume: None,
            fork: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");

        let result = runtime.block_on(wait_during_startup(
            std::future::pending::<()>(),
            &receiver,
            &mut pending,
            &mut launch,
        ));

        assert_eq!(result, StartupWait::Shutdown);
        let command = pending.pop_front();
        assert!(matches!(command, Some(Command::Prompt(_))));
        if let Some(Command::Prompt(value)) = command {
            assert_eq!(value.message, "queued");
            assert!(value.images.is_empty());
        }
    }

    #[test]
    fn resume_restarts_startup_with_the_requested_project_and_session() {
        let (sender, receiver) = flume::unbounded();
        sender
            .send(Command::ResumeSession {
                session_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
                project: "/tmp/resumed-project".into(),
            })
            .expect("resume command queued");
        let mut pending = std::collections::VecDeque::new();
        let mut launch = LaunchConfiguration {
            helper: None,
            generation: 0,
            backend: Backend::Omp,
            project: "/tmp/original-project".into(),
            model: "provider/model".into(),
            thinking_level: None,
            fast_mode: false,
            omperor_tools: false,
            computer_use: false,
            host_calls: crate::omp_runtime::HostCalls::default(),
            account: None,
            data_dir: "/tmp/data".into(),
            session_dir: "/tmp/sessions".into(),
            environment: vec![],
            resume: None,
            fork: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");

        let result = runtime.block_on(wait_during_startup(
            std::future::pending::<()>(),
            &receiver,
            &mut pending,
            &mut launch,
        ));

        assert_eq!(result, StartupWait::Restart);
        assert_eq!(launch.project, PathBuf::from("/tmp/resumed-project"));
        assert_eq!(launch.resume.as_deref(), Some("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(pending.is_empty());
    }

    #[test]
    fn failed_rpc_supervisor_accepts_lifecycle_relaunches() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        // Explicit recovery always reconnects the same session.
        for _ in 0..100 {
            let (command_sender, command_receiver) = flume::unbounded();
            let (event_sender, event_receiver) = flume::unbounded();
            let event_sender = UiEventSender(event_sender);
            let mut launch = LaunchConfiguration {
                helper: None,
                generation: 0,
                backend: Backend::Omp,
                project: "/tmp/original".into(),
                model: "provider/model".into(),
                thinking_level: None,
                fast_mode: false,
                omperor_tools: false,
                computer_use: false,
                host_calls: crate::omp_runtime::HostCalls::default(),
                account: None,
                data_dir: "/tmp/data".into(),
                session_dir: "/tmp/sessions".into(),
                environment: vec![],
                resume: Some("old-session".into()),
                fork: None,
            };
            command_sender
                .send(Command::ResumeSession {
                    session_id: "old-session".into(),
                    project: "/tmp/original".into(),
                })
                .expect("same-session relaunch command");

            let action = runtime.block_on(wait_for_relaunch(
                &command_receiver,
                &event_sender,
                &mut launch,
            ));

            assert_eq!(action, RecoveryAction::Restart);
            assert!(event_receiver.is_empty());
            assert_eq!(launch.project, PathBuf::from("/tmp/original"));
            assert_eq!(launch.resume.as_deref(), Some("old-session"));
        }
    }

    #[test]
    fn failed_rpc_supervisor_does_not_replay_an_ambiguous_prompt() {
        let (command_sender, command_receiver) = flume::unbounded();
        let (event_sender, event_receiver) = flume::unbounded();
        let event_sender = UiEventSender(event_sender);
        command_sender
            .send(Command::Prompt("possibly accepted".into()))
            .expect("offline prompt");
        command_sender
            .send(Command::ResumeSession {
                session_id: "same-session".into(),
                project: "/tmp/project".into(),
            })
            .expect("explicit relaunch");
        let mut launch = LaunchConfiguration {
            helper: None,
            generation: 0,
            backend: Backend::Omp,
            project: "/tmp/project".into(),
            model: "provider/model".into(),
            thinking_level: None,
            fast_mode: false,
            omperor_tools: false,
            computer_use: false,
            host_calls: crate::omp_runtime::HostCalls::default(),
            account: None,
            data_dir: "/tmp/data".into(),
            session_dir: "/tmp/sessions".into(),
            environment: vec![],
            resume: None,
            fork: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        let action = runtime.block_on(wait_for_relaunch(
            &command_receiver,
            &event_sender,
            &mut launch,
        ));

        assert_eq!(action, RecoveryAction::Restart);
        assert!(matches!(
            event_receiver.try_recv(),
            Ok((_, UiEvent::Notice(_)))
        ));
        assert!(event_receiver.is_empty());
    }

    #[test]
    fn startup_can_be_retried_without_a_session_id_or_implicit_prompt_replay() {
        let (sender, receiver) = flume::unbounded();
        let (events, received) = flume::unbounded();
        sender.send(Command::RetryStartup).unwrap();
        let mut launch = LaunchConfiguration {
            helper: None,
            generation: 1, backend: Backend::Omp, project: "/tmp/project".into(),
            model: "provider/model".into(), thinking_level: None, fast_mode: false,
            omperor_tools: false, computer_use: false, host_calls: super::HostCalls::default(),
            account: Some("testing-account".into()), data_dir: "/tmp/data".into(),
            session_dir: "/tmp/sessions".into(), resume: None, fork: None,
            environment: vec![],
        };
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let action = runtime.block_on(wait_for_relaunch(&receiver, &UiEventSender(events), &mut launch));
        assert_eq!(action, RecoveryAction::Restart);
        assert!(launch.resume.is_none());
        assert_eq!(launch.account.as_deref(), Some("testing-account"));
        assert!(received.is_empty(), "no prompt or fake Ready is synthesized");
    }

    #[test]
    fn callback_urls_use_the_typed_callback_answer() {
        let answer = authentication_answer(
            AuthPrompt {
                session_id: "auth-1".into(),
                prompt_id: "oauth-callback-url".into(),
                message: "Paste the callback".into(),
                input: "authorization_code".into(),
            },
            "http://localhost:1455/auth/callback?code=abc".into(),
        );

        assert_eq!(answer.input, RpcAuthInputKind::CallbackUrl);
        assert_eq!(answer.prompt_id.as_deref(), Some("oauth-callback-url"));
    }

    #[test]
    fn refresh_resumes_only_sessions_that_have_persisted() {
        let dir = std::env::temp_dir().join(format!("omperor-live-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let persisted = dir.join("persisted.rpc.json");
        std::fs::write(
            &persisted,
            r#"{"id":"ses_persisted","messages":[{"role":"user","content":"ping"}],"revision":1}"#,
        )
        .expect("session file");
        let live = LiveSession {
            id: "ses_persisted".into(),
            file: Some(persisted),
        };
        assert_eq!(live.resume_target().as_deref(), Some("ses_persisted"));

        let legacy = dir.join("legacy.jsonl");
        std::fs::write(&legacy, "{\"type\":\"message\"}\n").expect("legacy file");
        let live_legacy = LiveSession {
            id: "ses_legacy".into(),
            file: Some(legacy),
        };
        assert_eq!(live_legacy.resume_target().as_deref(), Some("ses_legacy"));

        // OMP writes this envelope when a session opens, before any turn.
        let empty = dir.join("empty.rpc.json");
        std::fs::write(
            &empty,
            r#"{"id":"ses_fresh","name":null,"parent":null,"messages":[],"revision":0}"#,
        )
        .expect("empty envelope");
        let opened_only = LiveSession {
            id: "ses_fresh".into(),
            file: Some(empty),
        };
        assert_eq!(opened_only.resume_target(), None);

        let unwritten = LiveSession {
            id: "ses_unwritten".into(),
            file: Some(dir.join("never-written.rpc.json")),
        };
        assert_eq!(unwritten.resume_target(), None);

        let unknown = LiveSession {
            id: "ses_unknown".into(),
            file: None,
        };
        assert_eq!(unknown.resume_target(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn concurrent_runtime_smoke_preserves_resume_and_replacement_generations() {
        use super::{OmpRuntime, RuntimeStartup, SessionStartup};
        let start = |session| {
            OmpRuntime::start_configured(RuntimeStartup {
                helper: None,
                backend: Backend::Mock,
                project: "/mock-unused".into(),
                model: None,
                thinking_level: None,
                fast_mode: false,
                omperor_tools: false,
                computer_use: false,
                account: None,
                data_dir: "/mock-unused/data".into(),
                session_dir: "/mock-unused/sessions".into(),
                environment: vec![],
                session,
            })
        };
        let runtimes = [
            start(SessionStartup::Fresh),
            start(SessionStartup::Resume("mock-resumed".into())),
            start(SessionStartup::Fork("mock-source".into())),
        ];
        let mut ids = Vec::new();
        for runtime in &runtimes {
            assert!(matches!(
                runtime
                    .events
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .1,
                UiEvent::RuntimeGeneration(1)
            ));
            match runtime
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .1
            {
                UiEvent::Ready {
                    session_id: Some(id),
                    ..
                } => ids.push(id),
                event => panic!("expected ready, got {event:?}"),
            }
            assert!(runtime.prompt("/mock scenario=agent ttft=0 rate=0 size=200".into()));
        }
        assert_eq!(ids[1], "mock-resumed");
        assert_ne!(ids[0], ids[2]);
        assert_ne!(ids[2], "mock-source");
        for runtime in &runtimes {
            loop {
                match runtime
                    .events
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .1
                {
                    UiEvent::TurnFinished(super::TurnOutcome::Completed) => break,
                    UiEvent::Failed(error) => panic!("{error}"),
                    _ => {}
                }
            }
        }
        assert!(runtimes[1].refresh_models());
        assert!(matches!(
            runtimes[1]
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .1,
            UiEvent::RuntimeGeneration(2)
        ));
        assert!(
            matches!(runtimes[1].events.recv_timeout(Duration::from_secs(3)).unwrap().1,
            UiEvent::Ready { session_id: Some(ref id), resumed: Some(ref resume), .. }
                if id == "mock-resumed" && resume == id)
        );
        for runtime in &runtimes {
            runtime.begin_shutdown();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while runtimes.iter().any(|runtime| !runtime.try_reap()) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn direct_shutdown_cannot_be_reclassified_as_transport_failure() {
        use std::os::unix::fs::PermissionsExt;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omperor-direct-shutdown-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("helper.py");
        std::fs::write(
            &helper,
            r#"#!/usr/bin/env python3
import json, sys
print(json.dumps({'type':'ready','protocolVersion':1,'supportedProtocolVersions':[1], 'maxFrameBytes':1048576,'maxReassembledFrameBytes':67108864}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    print(json.dumps({'type':'response','id':request.get('id'),'command':request['type'],'success':True,'data':{}}), flush=True)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = omp_rpc::client::RpcClient::spawn(omp_rpc::client::RpcClientOptions {
                executable: helper.into_os_string(),
                termination_grace: Duration::from_millis(100),
                ..Default::default()
            })
            .await
            .unwrap();
            let mut rpc_events = client.events();
            let (_tx, commands) = flume::unbounded();
            let (events, _rx) = flume::unbounded();
            let events = UiEventSender(events);
            let mut launch = LaunchConfiguration {
                helper: None,
                generation: 0,
                backend: Backend::Omp,
                project: dir.clone(),
                model: "p/m".into(),
                thinking_level: None,
                fast_mode: false,
                omperor_tools: false,
                computer_use: false,
                host_calls: crate::omp_runtime::HostCalls::default(),
                account: None,
                data_dir: dir.clone(),
                session_dir: dir.clone(),
                environment: vec![],
                resume: None,
                fork: None,
            };
            let mut pending = std::collections::VecDeque::new();
            let exit = tokio::time::timeout(
                Duration::from_secs(2),
                super::drive_command(
                    &client,
                    &events,
                    &mut launch,
                    None,
                    Command::Shutdown,
                    &commands,
                    &mut pending,
                    &mut rpc_events,
                    &mut super::TurnWatch::default(),
                ),
            )
            .await
            .expect("direct shutdown must settle");
            assert_eq!(exit, Some(super::ClientExit::Shutdown));
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prompt_queued_behind_restart_runs_on_replacement_generation() {
        use super::{OmpRuntime, RuntimeStartup, SessionStartup, TurnOutcome};

        let runtime = OmpRuntime::start_configured(RuntimeStartup {
            helper: None,
            backend: Backend::Mock,
            project: "/mock-unused".into(),
            model: None,
            thinking_level: None,
            fast_mode: false,
            omperor_tools: false,
            computer_use: false,
            account: None,
            data_dir: "/mock-unused/data".into(),
            session_dir: "/mock-unused/sessions".into(),
            environment: vec![],
            session: SessionStartup::Fresh,
        });
        assert!(matches!(
            runtime
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .1,
            UiEvent::RuntimeGeneration(1)
        ));
        assert!(matches!(
            runtime
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .1,
            UiEvent::Ready { .. }
        ));
        assert!(matches!(
            runtime
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .1,
            UiEvent::SubagentsSnapshot(_)
        ));

        assert!(runtime.refresh_models());
        assert!(runtime.prompt("/mock scenario=agent ttft=0 rate=0 size=20".into()));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut replacement = false;
        let mut ran = false;
        while std::time::Instant::now() < deadline && !ran {
            let event = runtime
                .events
                .recv_timeout(Duration::from_millis(250))
                .map(|(_, event)| event);
            match event {
                Ok(UiEvent::RuntimeGeneration(2)) => replacement = true,
                Ok(UiEvent::TurnFinished(TurnOutcome::Completed)) => ran = true,
                Ok(UiEvent::Failed(error)) => panic!("{error}"),
                Ok(_) | Err(flume::RecvTimeoutError::Timeout) => {}
                Err(flume::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(
            replacement,
            "the configuration refresh must replace the child"
        );
        assert!(ran, "the FIFO prompt must run on the replacement child");

        runtime.begin_shutdown();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !runtime.try_reap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn slow_rpc_drains_more_than_broadcast_capacity_and_allows_abort_shutdown() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("omperor-rpc-pump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("helper.py");
        std::fs::write(&helper, r#"#!/usr/bin/env python3
import json, sys, threading, time
lock = threading.Lock()
def emit(v):
    with lock:
        print(json.dumps(v), flush=True)
def stream():
    for i in range(1600):
        emit({'type':'message_update','assistantMessageEvent':{'type':'text_delta','delta':'x'}})
        time.sleep(0.0002)
emit({'type':'ready','protocolVersion':1,'supportedProtocolVersions':[1], 'maxFrameBytes':1048576,'maxReassembledFrameBytes':67108864})
for line in sys.stdin:
    request = json.loads(line)
    kind = request['type']
    if kind == 'get_account_usage':
        threading.Thread(target=stream, daemon=True).start()
        continue
    if kind == 'abort':
        emit({'type':'agent_end','cancelled':True})
    emit({'type':'response','id':request.get('id'),'command':kind,'success':True,'data':{}})
"#).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = omp_rpc::client::RpcClient::spawn(omp_rpc::client::RpcClientOptions {
                executable: helper.into_os_string(), termination_grace: Duration::from_millis(100),
                ..Default::default()
            }).await.unwrap();
            let mut rpc_events = client.events();
            let (tx, commands) = flume::unbounded();
            let (events, rx) = flume::unbounded();
            let events = UiEventSender(events);
            let mut launch = LaunchConfiguration {
                helper: None,
            generation: 0,
                backend: Backend::Omp, project: dir.clone(), model: "p/m".into(),
                thinking_level: None,
                fast_mode: false,
                omperor_tools: false,
                computer_use: false,
                host_calls: crate::omp_runtime::HostCalls::default(),
                account: None, data_dir: dir.clone(), session_dir: dir.clone(), resume: None, fork: None,
                environment: vec![],
            };
            let mut pending = std::collections::VecDeque::new();
            let observe = async {
                let mut deltas = 0;
                while deltas < 1600 {
                    let (_, event) = rx.recv_async().await.unwrap();
                    if matches!(event, UiEvent::AssistantDelta(_)) { deltas += 1; }
                    assert!(!matches!(event, UiEvent::Failed(_)), "{event:?}");
                }
                tx.send(Command::SetModel("p/first".into())).unwrap();
                tx.send(Command::SetModel("p/second".into())).unwrap();
                tx.send(Command::Abort).unwrap();
                loop {
                    if matches!(rx.recv_async().await.unwrap().1, UiEvent::TurnFinished(_)) { break; }
                }
                tx.send(Command::Shutdown).unwrap();
            };
            let mut turn = super::TurnWatch::default();
            let pump = super::drive_command(&client, &events, &mut launch, None,
                Command::RefreshAccountUsage, &commands, &mut pending, &mut rpc_events, &mut turn);
            let (exit, ()) = tokio::time::timeout(Duration::from_secs(8), async {
                tokio::join!(pump, observe)
            }).await.expect("events, abort and shutdown must not wait for the 45-second usage response");
            assert_eq!(exit, Some(super::ClientExit::Shutdown));
            assert!(matches!(pending.pop_front(), Some(Command::SetModel(ref model)) if model == "p/first"));
            assert!(matches!(pending.pop_front(), Some(Command::SetModel(ref model)) if model == "p/second"));
        });
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn turn_scoped_commands_cannot_mutate_a_replacement_child() {
        let prompt = super::command_for_generation(Command::Prompt("continue".into()), 1);
        assert!(matches!(
            super::current_command(prompt, 2),
            Some(Command::Prompt(ref prompt)) if prompt.message == "continue"
        ));
        assert!(
            super::current_command(
                super::command_for_generation(Command::Steer("old steer".into()), 1),
                2
            )
            .is_none()
        );
        assert!(
            super::current_command(super::command_for_generation(Command::Abort, 1), 2).is_none()
        );
        assert!(matches!(
            super::current_command(
                super::command_for_generation(Command::SetModel("provider/model".into()), 1),
                2
            ),
            Some(Command::SetModel(_))
        ));
        // Runtime lifetime shutdown does not belong to one replaceable child.
        assert!(matches!(
            super::current_command(Command::Shutdown, 2),
            Some(Command::Shutdown)
        ));
    }
}
