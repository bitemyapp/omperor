//! The local backend owns no child process, credentials, files, or tools. It
//! plays a scripted turn through the normal runtime event queue — the same
//! events OMP sends, in the same order, on a schedule — so the real chat
//! renderer, sidebar, Agents panel, and speedometer are what is exercised.
use super::*;
use crate::mock_inference::{Action, Child, MODEL, Outcome, PROVIDER, Request, Step};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// A turn in progress: what is left of its script, and what it has opened
/// that a stop must close.
struct Turn {
    steps: VecDeque<Step>,
    started: Instant,
    open_tools: Vec<(String, String)>,
    children: Vec<SubagentInfo>,
    steers: usize,
    /// Which turn of the session this is. The script names its calls and
    /// children the same way every time; the chat treats a known call id as
    /// the same tool, so each turn's ids are salted with its number, as
    /// OMP's are unique.
    number: u64,
}

impl Turn {
    fn new(request: &Request, number: u64) -> Self {
        Self {
            steps: request.script().steps.into(),
            started: Instant::now(),
            open_tools: Vec::new(),
            children: Vec::new(),
            steers: 0,
            number,
        }
    }

    fn id(&self, name: &str) -> String {
        // The README tour runs one turn a session and shows its children's
        // names as OMP would, unsalted.
        if crate::mock_inference::tour() {
            return name.to_owned();
        }
        format!("mock-t{}-{name}", self.number)
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.steps
            .front()
            .map(|step| (self.started + step.at).into())
    }

    /// A steer is acknowledged as reasoning the moment it arrives — as OMP
    /// takes one up in its next thought — rather than scheduled, so a stop
    /// right behind it cannot discard it.
    fn steer(&mut self, events: &UiEventSender, text: &str) {
        self.steers += 1;
        let note = format!("\n\n**Taking the steer into account**\n\n{}", text.trim());
        events.send(UiEvent::ThinkingDelta(note));
    }
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    format!("mock-{nanos:x}-{sequence:x}")
}

fn tool_result(text: &str) -> Value {
    serde_json::json!({ "content": [{ "type": "text", "text": text }] })
}

fn child_info(index: u64, child: &Child, started_ms: u64) -> SubagentInfo {
    SubagentInfo {
        id: child.id.clone(),
        index,
        agent: "scout".into(),
        label: child.label.clone(),
        status: "running".into(),
        started_ms: Some(started_ms),
        duration_ms: None,
        error: None,
        warning: None,
        session_file: None,
    }
}

/// The model a turn reports, and the ones on offer: the README tour's
/// stand-ins, else the mock's own.
fn model() -> &'static str {
    if crate::mock_inference::tour() { crate::mock_inference::TOUR_MODELS[0] } else { MODEL }
}

fn models() -> Vec<String> {
    if crate::mock_inference::tour() {
        crate::mock_inference::TOUR_MODELS.iter().map(|model| (*model).to_owned()).collect()
    } else {
        vec![MODEL.into()]
    }
}

fn ready(events: &UiEventSender, launch: &LaunchConfiguration, session: &str) {
    events.send(UiEvent::Ready {
        model: model().into(),
        models: models(),
        fast_models: Vec::new(),
        fast_mode: launch.fast_mode,
        thinking_level: launch
            .thinking_level
            .clone()
            .unwrap_or_else(|| "off".into()),
        session_id: Some(session.to_owned()),
        session_file: None,
        project: launch.project.clone(),
        authenticated_providers: Vec::new(),
        accounts: None,
        account: None,
        session_is_fresh: launch.resume.is_none(),
        resumed: launch.resume.clone(),
    });
    events.send(UiEvent::SubagentsSnapshot(Vec::new()));
}

/// Plays one step. `false` when the event queue has gone.
fn apply(events: &UiEventSender, turn: &mut Turn, action: Action) -> Option<Outcome> {
    match action {
        Action::Started => {
            events.send(UiEvent::Running);
            events.send(UiEvent::AssistantStarted);
        }
        Action::Thinking(text) => {
            events.send(UiEvent::ThinkingDelta(text));
        }
        Action::ToolStart {
            call_id,
            name,
            args,
            intent,
        } => {
            let call_id = turn.id(&call_id);
            // The wait names the children by their salted ids too.
            let args = turn.children.iter().fold(args, |args, child| {
                args.replace(
                    &format!(
                        "\"{}\"",
                        child
                            .id
                            .trim_start_matches(&format!("mock-t{}-", turn.number))
                    ),
                    &format!("\"{}\"", child.id),
                )
            });
            turn.open_tools.push((call_id.clone(), name.clone()));
            events.send(UiEvent::ToolStarted {
                call_id,
                name,
                args: Some(args),
                intent: Some(intent),
            });
        }
        Action::ToolUpdate { call_id, content } => {
            events.send(UiEvent::ToolUpdated {
                call_id: turn.id(&call_id),
                content,
                args: None,
            });
        }
        Action::ToolFinish {
            call_id,
            name,
            text,
            is_error,
        } => {
            let call_id = turn.id(&call_id);
            turn.open_tools.retain(|(open, _)| *open != call_id);
            events.send(UiEvent::ToolFinished(ToolOutcome {
                call_id,
                name: Some(name),
                is_error: Some(is_error),
                result: Some(tool_result(&text)),
            }));
        }
        Action::Spawn(children) => {
            let now = wall_clock_ms();
            turn.children = children
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    let mut info = child_info(index as u64 + 1, child, now);
                    info.id = turn.id(&child.id);
                    info
                })
                .collect();
            events.send(UiEvent::SubagentsSnapshot(turn.children.clone()));
        }
        Action::ChildDone { id, output: _ } => {
            let id = turn.id(&id);
            if let Some(child) = turn.children.iter_mut().find(|child| child.id == id) {
                child.status = "completed".into();
                child.duration_ms = child
                    .started_ms
                    .map(|started| wall_clock_ms().saturating_sub(started));
                events.send(UiEvent::SubagentChanged(child.clone()));
            }
        }
        Action::Answer(chunk) => {
            events.send(UiEvent::AssistantDelta(chunk));
        }
        Action::Receipt(receipt) => {
            events.send(UiEvent::InferenceReceipt(InferenceReceipt {
                provider: Some(if crate::mock_inference::tour() { "anthropic" } else { PROVIDER }.into()),
                model: Some(model().into()),
                tokens_in: receipt.tokens_in,
                tokens_out: receipt.tokens_out,
                tokens_reasoning: receipt.tokens_reasoning,
                // The tour reads like a session whose prompt prefix is
                // cached, as a real one's is after its first turn.
                cache_read: if crate::mock_inference::tour() { receipt.tokens_in * 3 / 4 } else { 0 },
                cache_write: 0,
                ttft_ms: Some(receipt.ttft_ms),
                last_token_ms: Some(receipt.last_token_ms),
                duration_ms: Some(receipt.duration_ms),
                usage_source: Some("estimated".into()),
                ..Default::default()
            }));
        }
        Action::Finish(outcome) => return Some(outcome),
        Action::AuthFailure(error) => {
            events.send(UiEvent::AuthenticationRequired {
                provider: AuthProvider::OpenAi,
                error,
            });
            return Some(Outcome::Errored(String::new()));
        }
    }
    None
}

/// The mock agent tidying Omperor through the `omperor_` tools, against the
/// app's real answers. The turn ends early, as interrupted, once `cancel`
/// is dropped by a stop.
async fn omperor_turn(events: UiEventSender, calls: HostCalls, number: u64, cancel: flume::Receiver<()>) {
    let stopped = || {
        let cancel = cancel.clone();
        async move {
            let _ = cancel.recv_async().await;
        }
    };
    let finish_turn = |outcome: TurnOutcome| {
        events.send(UiEvent::AssistantFinished(None));
        events.send(UiEvent::TurnFinished(outcome));
    };
    events.send(UiEvent::Running);
    events.send(UiEvent::AssistantStarted);
    let mut step = 0;
    let mut call = |tool: &str, arguments: Value, intent: &str| {
        step += 1;
        let call_id = format!("mock-t{number}-omperor-{step}");
        events.send(UiEvent::ToolStarted {
            call_id: call_id.clone(),
            name: tool.to_owned(),
            args: Some(arguments.to_string()),
            intent: Some(intent.to_owned()),
        });
        let asked = calls.ask(
            &events,
            tool.to_owned(),
            arguments.as_object().cloned().unwrap_or_default(),
            stopped(),
        );
        let events = events.clone();
        let tool = tool.to_owned();
        async move {
            let answer = asked.await;
            events.send(UiEvent::ToolFinished(ToolOutcome {
                call_id,
                name: Some(tool),
                is_error: Some(answer.is_err()),
                result: Some(tool_result(&match &answer {
                    Ok(value) => value.to_string(),
                    Err(error) => error.clone(),
                })),
            }));
            answer
        }
    };
    let listed = call(
        "omperor_list_sessions",
        serde_json::json!({ "limit": 200 }),
        "Find mock sessions and drafts",
    )
    .await;
    if cancel.is_disconnected() {
        finish_turn(TurnOutcome::Interrupted);
        return;
    }
    let listed = match listed {
        Ok(listed) => listed,
        Err(error) => {
            events.send(UiEvent::AssistantDelta(format!("I couldn't list Omperor's sessions: {error}")));
            finish_turn(TurnOutcome::Completed);
            return;
        }
    };
    let ids: Vec<Value> = listed["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|session| {
            matches!(session["kind"].as_str(), Some("mock" | "draft")) && session["current"].as_bool() != Some(true)
        })
        .filter_map(|session| session["id"].as_str().map(|id| Value::String(id.to_owned())))
        .collect();
    if ids.is_empty() {
        events.send(UiEvent::AssistantDelta(
            "Nothing to tidy: there are no mock sessions or drafts besides this one.".into(),
        ));
        finish_turn(TurnOutcome::Completed);
        return;
    }
    let count = ids.len();
    let deleted = call(
        "omperor_delete_sessions",
        serde_json::json!({
            "session_ids": ids,
            "reason": "They are mock-backend test runs and drafts nothing was sent from.",
        }),
        "Delete the mock sessions and drafts",
    )
    .await;
    if cancel.is_disconnected() {
        finish_turn(TurnOutcome::Interrupted);
        return;
    }
    events.send(UiEvent::AssistantDelta(match deleted {
        Ok(answer) => format!(
            "I found {count} mock session{} and drafts. {}.",
            if count == 1 { "" } else { "s" },
            answer["summary"].as_str().unwrap_or("Omperor deleted them")
        ),
        Err(error) => format!("I found {count} to tidy, but none were deleted: {error}"),
    }));
    finish_turn(TurnOutcome::Completed);
}

/// Ends the turn. A stop closes what is open the way OMP does: tools and
/// children still running settle as cancelled, then the turn is interrupted.
fn finish(events: &UiEventSender, turn: &mut Option<Turn>, outcome: TurnOutcome) {
    let Some(mut ending) = turn.take() else {
        return;
    };
    if matches!(outcome, TurnOutcome::Interrupted) {
        for (call_id, name) in ending.open_tools.drain(..) {
            events.send(UiEvent::ToolFinished(ToolOutcome {
                call_id,
                name: Some(name),
                is_error: Some(true),
                result: Some(tool_result("cancelled: the turn was stopped")),
            }));
        }
        let mut changed = false;
        for child in &mut ending.children {
            if child.status == "running" {
                child.status = "cancelled".into();
                child.duration_ms = child
                    .started_ms
                    .map(|started| wall_clock_ms().saturating_sub(started));
                changed = true;
            }
        }
        if changed {
            events.send(UiEvent::SubagentsSnapshot(ending.children.clone()));
        }
    }
    events.send(UiEvent::AssistantFinished(None));
    events.send(UiEvent::TurnFinished(outcome));
}

pub(super) async fn run(
    commands: &Receiver<Command>,
    events: &UiEventSender,
    launch: &mut LaunchConfiguration,
) -> ClientExit {
    let session = launch.resume.clone().unwrap_or_else(session_id);
    ready(events, launch, &session);
    launch.resume = Some(session.clone());
    launch.fork = None;
    let mut turn: Option<Turn> = None;
    let mut turns = 0_u64;
    // A turn of the `omperor` scenario, which waits on the app rather than a
    // clock, runs as its own task; dropping the sender stops it.
    let mut omperor: Option<(tokio::task::JoinHandle<()>, flume::Sender<()>)> = None;
    loop {
        // With no turn, sleep on the channel: no idle polling. When AppKit
        // is behind, cap queued output at eight events (~32 KiB ASCII).
        let next = match turn.as_ref().and_then(Turn::deadline) {
            Some(deadline) => {
                let deadline = if events.0.len() >= 8 {
                    tokio::time::Instant::now() + Duration::from_millis(50)
                } else {
                    deadline
                };
                tokio::select! {
                    biased;
                    command = commands.recv_async() => Some(command),
                    _ = tokio::time::sleep_until(deadline) => None,
                }
            }
            None => Some(commands.recv_async().await),
        };
        let Some(command) = next else {
            if events.0.is_disconnected() {
                return ClientExit::Shutdown;
            }
            if events.0.len() >= 8 {
                continue;
            }
            if let Some(active) = &mut turn
                && let Some(step) = active.steps.pop_front()
            {
                match apply(events, active, step.action) {
                    Some(Outcome::Completed) => finish(events, &mut turn, TurnOutcome::Completed),
                    Some(Outcome::Errored(reason)) if reason.is_empty() => {
                        // A refused sign-in ended the turn on its own.
                        turn = None;
                    }
                    Some(Outcome::Errored(reason)) => {
                        finish(events, &mut turn, TurnOutcome::Errored(reason));
                    }
                    None => {}
                }
            }
            continue;
        };
        let command = match command {
            Ok(command) => match current_command(command, launch.generation) {
                Some(command) => Ok(command),
                None => continue,
            },
            Err(error) => Err(error),
        };
        match command {
            Err(_) | Ok(Command::Shutdown) => return ClientExit::Shutdown,
            Ok(Command::ReleaseSession) => {
                finish(events, &mut turn, TurnOutcome::Interrupted);
                return ClientExit::Released;
            }
            Ok(Command::Prompt(input)) => {
                if omperor.as_ref().is_some_and(|(task, _)| task.is_finished()) {
                    omperor = None;
                }
                if turn.is_some() || omperor.is_some() {
                    events.send(UiEvent::Notice(
                        "A turn is in progress: steer it, or stop it first.".into(),
                    ));
                    continue;
                }
                // A `file=` is read here, on the runtime's thread, against
                // the session's project.
                let request = Request::parse(&input.message).and_then(|mut request| {
                    request.load_file(&launch.project)?;
                    Ok(request)
                });
                match request {
                    Ok(request) if request.scenario == crate::mock_inference::Scenario::Omperor => {
                        turns += 1;
                        if launch.omperor_tools {
                            let (stop, cancel) = flume::bounded(1);
                            let task = tokio::spawn(omperor_turn(events.clone(), launch.host_calls.clone(), turns, cancel));
                            omperor = Some((task, stop));
                        } else {
                            events.send(UiEvent::Running);
                            events.send(UiEvent::AssistantDelta(
                                "Omperor's tools are off in this session. Send /omperor first, then ask again.".into(),
                            ));
                            events.send(UiEvent::AssistantFinished(None));
                            events.send(UiEvent::TurnFinished(TurnOutcome::Completed));
                        }
                    }
                    Ok(request) => {
                        turns += 1;
                        turn = Some(Turn::new(&request, turns));
                    }
                    Err(error) => {
                        events.send(UiEvent::TurnFinished(TurnOutcome::Errored(error)));
                    }
                }
            }
            Ok(Command::Abort) => {
                // Dropping the stop signal ends an `omperor` turn as
                // interrupted, withdrawing any question it is waiting on.
                omperor = None;
                finish(events, &mut turn, TurnOutcome::Interrupted);
            }
            Ok(Command::Steer(text)) => match &mut turn {
                Some(active) => active.steer(events, &text.message),
                None => events.send_notice("Nothing is running to steer."),
            },
            Ok(Command::RefreshSubagents) => {
                let board = turn
                    .as_ref()
                    .map(|active| active.children.clone())
                    .unwrap_or_default();
                events.send(UiEvent::SubagentsSnapshot(board));
            }
            Ok(Command::RefreshAccountUsage) => {
                events.send(UiEvent::AccountUsage(Vec::new()));
            }
            Ok(Command::RefreshModels) => return ClientExit::Restart,
            Ok(Command::SetThinkingLevel(level)) => {
                launch.thinking_level = Some(level.clone());
                events.send(UiEvent::ThinkingLevelChanged(level));
            }
            Ok(Command::SetFastMode(enabled)) => {
                launch.fast_mode = enabled;
                events.send(UiEvent::FastModeChanged(enabled));
            }
            Ok(Command::SetOmperorTools(enabled)) => {
                launch.omperor_tools = enabled;
                events.send(UiEvent::OmperorToolsChanged(enabled));
            }
            Ok(Command::Login(_, _, generation)) => {
                events.send(UiEvent::AuthenticationFailed {
                    session_id: None,
                    generation: Some(generation),
                    error: "Select OMP in Settings → General to manage sign-in.".into(),
                });
            }
            Ok(_) => {
                events.send(UiEvent::Notice(
                    "This action requires OMP. Change the backend in Settings → General.".into(),
                ));
            }
        }
    }
}

impl UiEventSender {
    fn send_notice(&self, text: &str) {
        self.send(UiEvent::Notice(text.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> LaunchConfiguration {
        LaunchConfiguration {
            helper: None,
            generation: 0,
            backend: Backend::Mock,
            project: PathBuf::from("/mock-unused"),
            model: "untouched".into(),
            thinking_level: None,
            fast_mode: false,
            omperor_tools: false,
            computer_use: false,
            host_calls: crate::omp_runtime::HostCalls::default(),
            account: None,
            data_dir: PathBuf::from("/mock-unused/data"),
            session_dir: PathBuf::from("/mock-unused/sessions"),
            environment: vec![],
            resume: None,
            fork: None,
        }
    }

    fn spawn_worker() -> (
        flume::Sender<Command>,
        flume::Receiver<ArrivedEvent>,
        thread::JoinHandle<ClientExit>,
    ) {
        let (tx, commands) = flume::unbounded();
        let (events, rx) = flume::unbounded();
        let worker = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(run(&commands, &UiEventSender(events), &mut launch()))
        });
        (tx, rx, worker)
    }

    #[test]
    fn startup_settings_and_login_failure_keep_their_identity() {
        let (tx, commands) = flume::unbounded();
        let (events, rx) = flume::unbounded();
        let mut config = launch();
        config.thinking_level = Some("high".into());
        config.fast_mode = true;
        tx.send(Command::Login(
            AuthProvider::OpenAi,
            AuthMethod::Browser,
            42,
        ))
        .unwrap();
        tx.send(Command::Shutdown).unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            executor.block_on(run(&commands, &UiEventSender(events), &mut config)),
            ClientExit::Shutdown
        );
        assert!(
            matches!(rx.recv().unwrap().1, UiEvent::Ready { thinking_level, fast_mode: true, .. } if thinking_level == "high")
        );
        assert!(matches!(
            rx.recv().unwrap().1,
            UiEvent::SubagentsSnapshot(_)
        ));
        assert!(matches!(
            rx.recv().unwrap().1,
            UiEvent::AuthenticationFailed {
                session_id: None,
                generation: Some(42),
                ..
            }
        ));
    }

    #[test]
    fn the_omperor_scenario_asks_the_app_and_acts_on_its_answers() {
        let (tx, commands) = flume::unbounded();
        let (events, rx) = flume::unbounded();
        let mut config = launch();
        config.omperor_tools = true;
        let calls = config.host_calls.clone();
        let worker = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(run(&commands, &UiEventSender(events), &mut config))
        });
        let receive = || rx.recv_timeout(Duration::from_secs(20)).unwrap().1;
        let next_call = || loop {
            if let UiEvent::HostToolCall { call, tool, arguments } = receive() {
                return (call, tool, arguments);
            }
        };
        tx.send(Command::Prompt("tidy up my workspace".into())).unwrap();
        let (call, tool, _) = next_call();
        assert_eq!(tool, "omperor_list_sessions");
        assert!(calls.answer(
            call,
            Ok(serde_json::json!({ "sessions": [
                { "id": "mock-1", "kind": "mock" },
                { "id": "mock-2", "kind": "mock", "current": true },
                { "id": "01JOURNAL", "kind": "conversation" },
                { "id": "draft-7", "kind": "draft" },
            ] })),
        ));
        let (call, tool, arguments) = next_call();
        assert_eq!(tool, "omperor_delete_sessions");
        // Never the session asking, never a conversation.
        assert_eq!(arguments["session_ids"], serde_json::json!(["mock-1", "draft-7"]));
        assert!(calls.answer(call, Ok(serde_json::json!({ "summary": "Deleted 2 sessions" }))));
        let answer = loop {
            match receive() {
                UiEvent::AssistantDelta(text) => break text,
                _ => continue,
            }
        };
        assert!(answer.contains("Deleted 2 sessions"), "{answer}");
        assert!(matches!(receive(), UiEvent::AssistantFinished(None)));
        assert!(matches!(receive(), UiEvent::TurnFinished(TurnOutcome::Completed)));

        // A stop while the app is still asking the user withdraws the
        // question and ends the turn as interrupted.
        tx.send(Command::Prompt("/mock scenario=omperor".into())).unwrap();
        let (call, _, _) = next_call();
        tx.send(Command::Abort).unwrap();
        loop {
            match receive() {
                UiEvent::HostToolCancelled { call: cancelled } => {
                    assert_eq!(cancelled, call);
                    break;
                }
                _ => continue,
            }
        }
        loop {
            if matches!(receive(), UiEvent::TurnFinished(TurnOutcome::Interrupted)) {
                break;
            }
        }
        assert!(!calls.answer(call, Ok(Value::Null)));
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(worker.join().unwrap(), ClientExit::Shutdown);
    }

    #[test]
    fn a_plain_prompt_plays_a_whole_agent_turn_through_the_real_event_stream() {
        let (tx, rx, worker) = spawn_worker();
        let receive = || rx.recv_timeout(Duration::from_secs(20)).unwrap().1;
        // The mock has a session of its own now, so the sidebar and the
        // Agents panel have something to show.
        assert!(
            matches!(receive(), UiEvent::Ready { session_id: Some(ref id), session_file: None, .. } if id.starts_with("mock-"))
        );
        assert!(matches!(receive(), UiEvent::SubagentsSnapshot(_)));
        tx.send(Command::Prompt(
            "/mock scenario=agent ttft=0 rate=0 size=200".into(),
        ))
        .unwrap();
        let mut kinds = Vec::new();
        let mut answer = String::new();
        let mut receipt = None;
        loop {
            let event = receive();
            kinds.push(event.kind());
            match event {
                UiEvent::AssistantDelta(delta) => answer.push_str(&delta),
                UiEvent::InferenceReceipt(r) => receipt = Some(r),
                UiEvent::TurnFinished(TurnOutcome::Completed) => break,
                UiEvent::TurnFinished(other) => panic!("{other:?}"),
                _ => {}
            }
        }
        let mut order = Vec::new();
        for kind in &kinds {
            if order.last() != Some(kind) {
                order.push(*kind);
            }
        }
        assert_eq!(
            order,
            [
                "running",
                "assistant_started",
                "thinking_delta",
                "tool_started",
                "tool_finished",
                "tool_started",
                "tool_finished",
                "tool_started",
                "tool_updated",
                "tool_finished",
                "thinking_delta",
                "assistant_delta",
                "inference_receipt",
                "assistant_finished",
                "turn_finished",
            ]
        );
        assert_eq!(answer.chars().count(), 200);
        let receipt = receipt.unwrap();
        // A second turn's calls are new tools, not the first turn's again.
        tx.send(Command::Prompt(
            "/mock scenario=agent ttft=0 rate=0 size=20".into(),
        ))
        .unwrap();
        let mut second_ids = Vec::new();
        loop {
            match receive() {
                UiEvent::ToolStarted { call_id, .. } => second_ids.push(call_id),
                UiEvent::TurnFinished(_) => break,
                _ => {}
            }
        }
        assert_eq!(second_ids.len(), 3);
        assert!(
            second_ids.iter().all(|id| id.starts_with("mock-t2-")),
            "{second_ids:?}"
        );
        assert_eq!(receipt.provider.as_deref(), Some(PROVIDER));
        assert_eq!(receipt.model.as_deref(), Some(MODEL));
        assert!(receipt.tokens_out > 0 && receipt.tokens_reasoning > 0);
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(worker.join().unwrap(), ClientExit::Shutdown);
    }

    #[test]
    fn the_markdown_turn_delivers_the_stress_document_exactly_and_a_missing_file_errs() {
        let (tx, rx, worker) = spawn_worker();
        let receive = || rx.recv_timeout(Duration::from_secs(20)).unwrap().1;
        assert!(matches!(receive(), UiEvent::Ready { .. }));
        assert!(matches!(receive(), UiEvent::SubagentsSnapshot(_)));
        tx.send(Command::Prompt("/mock scenario=markdown ttft=0 rate=0".into()))
            .unwrap();
        let mut answer = String::new();
        let mut deltas = 0;
        loop {
            match receive() {
                UiEvent::AssistantDelta(delta) => {
                    deltas += 1;
                    answer.push_str(&delta);
                }
                UiEvent::ToolStarted { .. } => panic!("the markdown turn uses no tools"),
                UiEvent::TurnFinished(TurnOutcome::Completed) => break,
                UiEvent::TurnFinished(other) => panic!("{other:?}"),
                _ => {}
            }
        }
        assert!(deltas >= 1);
        assert_eq!(answer, crate::mock_inference::STRESS_DOCUMENT);
        // The test launch's project does not exist; the turn says so.
        tx.send(Command::Prompt("/mock scenario=markdown file=absent.md".into()))
            .unwrap();
        loop {
            match receive() {
                UiEvent::TurnFinished(TurnOutcome::Errored(reason)) => {
                    assert!(reason.contains("absent.md"), "{reason}");
                    break;
                }
                UiEvent::TurnFinished(other) => panic!("{other:?}"),
                _ => {}
            }
        }
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(worker.join().unwrap(), ClientExit::Shutdown);
    }

    #[test]
    fn a_stop_settles_open_tools_and_children_as_cancelled_and_a_steer_lands_as_reasoning() {
        let (tx, rx, worker) = spawn_worker();
        let receive = || rx.recv_timeout(Duration::from_secs(20)).unwrap().1;
        assert!(matches!(receive(), UiEvent::Ready { .. }));
        assert!(matches!(receive(), UiEvent::SubagentsSnapshot(_)));
        tx.send(Command::Prompt(
            "/mock scenario=subagents ttft=0 rate=0".into(),
        ))
        .unwrap();
        // Run until the children are on the board and the wait has started.
        loop {
            match receive() {
                UiEvent::ToolStarted { name, .. } if name == "hub" => break,
                UiEvent::TurnFinished(outcome) => panic!("ended early: {outcome:?}"),
                _ => {}
            }
        }
        tx.send(Command::Steer("focus on the tests".into()))
            .unwrap();
        tx.send(Command::Abort).unwrap();
        let mut saw_steer = false;
        let mut hub_cancelled = false;
        let mut children_cancelled = false;
        loop {
            match receive() {
                UiEvent::ThinkingDelta(text) if text.contains("focus on the tests") => {
                    saw_steer = true
                }
                UiEvent::ToolFinished(outcome) if outcome.call_id.ends_with("-call-hub") => {
                    hub_cancelled = outcome.is_error == Some(true);
                }
                UiEvent::SubagentsSnapshot(board) if !board.is_empty() => {
                    children_cancelled = board.iter().all(|child| child.status == "cancelled");
                }
                UiEvent::TurnFinished(TurnOutcome::Interrupted) => break,
                UiEvent::TurnFinished(other) => panic!("{other:?}"),
                _ => {}
            }
        }
        assert!(saw_steer, "the steer was played as reasoning");
        assert!(hub_cancelled, "the open wait settled as cancelled");
        assert!(children_cancelled, "running children settled as cancelled");
        // The board is empty again once the turn is over.
        tx.send(Command::RefreshSubagents).unwrap();
        assert!(matches!(receive(), UiEvent::SubagentsSnapshot(board) if board.is_empty()));
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(worker.join().unwrap(), ClientExit::Shutdown);
    }

    #[test]
    fn invalid_input_a_refused_sign_in_and_a_stress_stream_all_settle() {
        let (tx, rx, worker) = spawn_worker();
        let receive = || rx.recv_timeout(Duration::from_secs(20)).unwrap().1;
        assert!(matches!(receive(), UiEvent::Ready { .. }));
        assert!(matches!(receive(), UiEvent::SubagentsSnapshot(_)));
        tx.send(Command::Prompt("/mock scenario=other".into()))
            .unwrap();
        assert!(matches!(
            receive(),
            UiEvent::TurnFinished(TurnOutcome::Errored(_))
        ));
        tx.send(Command::Prompt("/mock scenario=auth ttft=0 rate=0".into()))
            .unwrap();
        loop {
            match receive() {
                UiEvent::AuthenticationRequired { provider, error } => {
                    assert_eq!(provider, AuthProvider::OpenAi);
                    assert!(error.contains("token_revoked"));
                    break;
                }
                UiEvent::TurnFinished(outcome) => panic!("{outcome:?}"),
                _ => {}
            }
        }
        // The turn is over: a new prompt is accepted at once.
        tx.send(Command::Prompt(
            "/mock scenario=stress rate=0 size=12000".into(),
        ))
        .unwrap();
        let mut text = String::new();
        loop {
            match receive() {
                UiEvent::AssistantDelta(delta) => text.push_str(&delta),
                UiEvent::TurnFinished(TurnOutcome::Completed) => break,
                UiEvent::Notice(notice) => panic!("{notice}"),
                _ => {}
            }
        }
        assert_eq!(
            text,
            Request::parse("/mock scenario=stress rate=0 size=12000")
                .unwrap()
                .output()
        );
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(worker.join().unwrap(), ClientExit::Shutdown);
    }

    #[test]
    fn fast_stream_is_bounded_and_shutdown_does_not_wait_for_the_reader() {
        let (tx, rx, worker) = spawn_worker();
        tx.send(Command::Prompt(
            "/mock scenario=stress rate=0 size=1000000".into(),
        ))
        .unwrap();
        // Leave the output undrained, then terminate a blocked producer.
        thread::sleep(Duration::from_millis(100));
        assert!(rx.len() <= 8);
        tx.send(Command::Shutdown).unwrap();
        assert_eq!(worker.join().unwrap(), ClientExit::Shutdown);
    }
}
