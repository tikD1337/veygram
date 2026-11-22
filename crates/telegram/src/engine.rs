//! One receiving thread, lock-free command dispatch, explicit close-and-join lifecycle.

use crate::ffi::TdJson;
use crate::mapping::{Decoder, MAX_LINK_BYTES, auth, command_kind, error, integer, parse};
use crate::request;
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use tg_core::{AuthState, ChatId, Command, CommandError, CommandKind, Event, MessageId, Secret};

static ENGINE_ACTIVE: AtomicBool = AtomicBool::new(false);
const MAX_IN_FLIGHT: usize = 128;
/// Messages with a reaction change in progress, and later changes held per message.
const MAX_TOGGLED_MESSAGES: usize = 32;
const MAX_HELD_TOGGLES: usize = 8;

/// Parameters supplied by tg-app. No environment, credential or key files are read here.
/// Key generation/protection and selection of the data directory belong to milestone 2.
pub struct EngineParameters {
    pub api_id: i32,
    pub api_hash: Secret,
    pub database_encryption_key: [u8; 32],
    pub data_directory: PathBuf,
}

impl fmt::Debug for EngineParameters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EngineParameters(<redacted>)")
    }
}

impl EngineParameters {
    fn request(self) -> Result<Value, CommandError> {
        let directory = self
            .data_directory
            .to_str()
            .ok_or_else(|| error(-2, "The data directory is not UTF-8."))?;
        if self.api_id <= 0
            || self.api_hash.expose().is_empty()
            || self.api_hash.expose().len() > MAX_LINK_BYTES
            || directory.len() > MAX_LINK_BYTES
            || !self.data_directory.is_absolute()
            || self
                .data_directory
                .components()
                .any(|part| part.as_os_str().eq_ignore_ascii_case("tdlib-data"))
        {
            return Err(error(-2, "Invalid engine parameters or data directory."));
        }
        Ok(json!({"@type":"setTdlibParameters",
            "database_directory":directory, "files_directory":self.data_directory.join("files"),
            "database_encryption_key":base64::engine::general_purpose::STANDARD.encode(self.database_encryption_key),
            "use_file_database":true, "use_chat_info_database":true, "use_message_database":true,
            "use_secret_chats":false, "api_id":self.api_id, "api_hash":self.api_hash.expose(),
            "system_language_code":"en", "device_model":"Desktop", "system_version":"Windows",
            "application_version":env!("CARGO_PKG_VERSION"), "@extra":{"command":"initialize"}}))
    }
}

pub(crate) trait Transport: Send + Sync {
    fn send(&self, client: i32, request: &CStr);
    /// Only the receiving thread may call this; it blocks in the native implementation.
    fn receive(&self) -> Result<Option<String>, CommandError>;
}

pub(crate) struct Shared {
    transport: Arc<dyn Transport>,
    client: i32,
    closing: AtomicBool,
    closed: AtomicBool,
    // At most MAX_IN_FLIGHT requests outstanding in TDLib; no Rust request queue/map.
    in_flight: AtomicUsize,
    // One view request per chat, at most the 128 globally tracked requests.
    views: Mutex<std::collections::HashSet<ChatId>>,
    // Reaction toggles: one getMessage-then-mutation in flight per (chat, message); later
    // toggles of that message wait here as encoded getMessage requests, so each reads the state
    // the previous mutation left. Bounded by MAX_TOGGLED_MESSAGES x MAX_HELD_TOGGLES.
    toggles: Mutex<HashMap<(i64, i64), VecDeque<Value>>>,
}

impl Shared {
    fn new(transport: Arc<dyn Transport>, client: i32) -> Self {
        Self {
            transport,
            client,
            closing: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            views: Mutex::default(),
            toggles: Mutex::default(),
        }
    }

    /// Starts a reaction toggle, or holds it while an earlier one for the same message runs.
    /// Admission, the first send and its rollback are one nonblocking step: the lock is only
    /// tried, and it stays held across `send_value` (which takes no other lock for a read and
    /// never waits), so the entry inserted here is still empty if the send fails.
    fn toggle(&self, request: Value) -> Result<(), CommandError> {
        let key = toggle_key(&request["@extra"])?;
        let mut toggles = self
            .toggles
            .try_lock()
            .map_err(|_| error(-3, "A reaction change is being processed. Try again."))?;
        if let Some(held) = toggles.get_mut(&key) {
            if held.len() >= MAX_HELD_TOGGLES {
                return Err(error(
                    -3,
                    "Too many reaction changes are pending for this message.",
                ));
            }
            held.push_back(request);
            return Ok(());
        }
        if toggles.len() >= MAX_TOGGLED_MESSAGES {
            return Err(error(-3, "Too many reaction changes are pending."));
        }
        self.send_value(request)?;
        toggles.insert(key, VecDeque::new());
        Ok(())
    }

    /// The toggle of `key` finished (mutation answered, or failed before it): send the next
    /// held toggle, skipping ones that cannot be sent, or free the message. Returns the last
    /// send failure, if any.
    fn advance_toggle(&self, key: (i64, i64)) -> Option<CommandError> {
        let mut failure = None;
        loop {
            let next = match self.toggles.lock() {
                Ok(mut toggles) => match toggles.get_mut(&key).map(VecDeque::pop_front) {
                    Some(Some(request)) => request,
                    _ => {
                        toggles.remove(&key);
                        return failure;
                    }
                },
                Err(_) => return failure,
            };
            match self.send_value(next) {
                Ok(()) => return failure,
                Err(error) => failure = Some(error),
            }
        }
    }

    fn send_value(&self, mut request: Value) -> Result<(), CommandError> {
        if self.closing.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
            return Err(error(-2, "The engine is closing or closed."));
        }
        request["@extra"]["tracked"] = Value::Bool(true);
        let text = request::serialize(&request)?;
        self.in_flight
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_IN_FLIGHT).then_some(count + 1)
            })
            .map_err(|_| error(-3, "Too many commands are in flight."))?;
        if request["@extra"]["command"] == "view" {
            let chat = ChatId(integer(&request["@extra"]["chat"])?);
            let admitted = self
                .views
                .try_lock()
                .is_ok_and(|mut views| views.insert(chat));
            if !admitted {
                self.in_flight.fetch_sub(1, Ordering::AcqRel);
                return Err(error(
                    -3,
                    "A read receipt is already pending for this chat.",
                ));
            }
        }
        self.transport.send(self.client, &text);
        Ok(())
    }

    fn close(&self) {
        if !self.closed.load(Ordering::Acquire) && !self.closing.swap(true, Ordering::AcqRel) {
            // Shutdown has a reserved slot: saturation can never prevent close.
            self.transport.send(
                self.client,
                c"{\"@type\":\"close\",\"@extra\":{\"command\":\"shutdown\"}}",
            );
        }
    }

    fn complete(&self, value: &Value) {
        if value["@extra"]["command"] == "view"
            && let Ok(chat) = integer(&value["@extra"]["chat"])
            && let Ok(mut views) = self.views.lock()
        {
            views.remove(&ChatId(chat));
        }
        if value["@extra"]["tracked"] == true {
            let _ = self
                .in_flight
                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_sub(1))
                });
        }
    }
}

struct EngineLease;
impl Drop for EngineLease {
    fn drop(&mut self) {
        ENGINE_ACTIVE.store(false, Ordering::Release);
    }
}

/// The callback runs on the receiving thread and must return promptly.
/// Start, shutdown, join and Drop may block: call them off the UI thread.
/// Only send is suitable for UI callbacks and never waits on a channel, mutex or receive.
pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("closing", &self.shared.closing.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Engine {
    pub fn start(
        parameters: EngineParameters,
        callback: impl Fn(Event) + Send + 'static,
    ) -> Result<Self, CommandError> {
        if ENGINE_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(error(-2, "Only one TDLib engine may run in this process."));
        }
        let lease = EngineLease;
        // Exactly one bounded startup result; never used by Engine::send.
        let (started, result) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("tg-telegram".into())
            .spawn(move || {
                let _lease = lease;
                let initialized = initialize(parameters);
                match initialized {
                    Ok((shared, parameters)) => {
                        if started.send(Ok(shared.clone())).is_err() {
                            shared.close();
                        }
                        run(shared, Some(parameters), callback);
                    }
                    Err(error) => {
                        let _ = started.send(Err(error));
                    }
                }
            })
            .map_err(|_| error(-4, "Could not start the Telegram engine thread."))?;
        match result.recv() {
            Ok(Ok(shared)) => Ok(Self {
                shared,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(error(-4, "The Telegram engine failed during startup."))
            }
        }
    }

    /// Serialize a bounded command and call the thread-safe td_send immediately.
    /// Local validation/capacity failures are returned; TDLib failures arrive as events.
    pub fn send(&self, command: Command) -> Result<(), CommandError> {
        if matches!(command, Command::ShutDown) {
            self.shared.close();
            return Ok(());
        }
        let request = request::encode(command)?;
        if request["@extra"]["command"] == "toggle_reaction" {
            return self.shared.toggle(request);
        }
        self.shared.send_value(request)
    }

    /// Send close and join after authorizationStateClosed. Run off the UI thread.
    pub fn shutdown(mut self) -> Result<(), CommandError> {
        self.shared.close();
        self.join()
    }

    fn join(&mut self) -> Result<(), CommandError> {
        if let Some(thread) = self.thread.take() {
            // A caller may release its final Engine inside the callback. The receive thread
            // still owns Shared and finishes closing; joining itself would deadlock.
            if thread.thread().id() != thread::current().id() {
                thread
                    .join()
                    .map_err(|_| error(-4, "The Telegram engine thread panicked."))?;
            }
        }
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.close();
        let _ = self.join();
    }
}

fn toggle_key(extra: &Value) -> Result<(i64, i64), CommandError> {
    Ok((integer(&extra["chat"])?, integer(&extra["message"])?))
}

/// The message of a reaction toggle step: the read's answer or error, or the mutation's answer.
fn toggle_step(value: &Value) -> Option<(i64, i64)> {
    let extra = &value["@extra"];
    let step = matches!(
        (value["@type"].as_str(), extra["command"].as_str()),
        (Some("message" | "error"), Some("toggle_reaction")) | (Some(_), Some("reaction"))
    );
    step.then(|| toggle_key(extra).ok()).flatten()
}

fn initialize(parameters: EngineParameters) -> Result<(Arc<Shared>, Value), CommandError> {
    let parameters = parameters.request()?;
    let native = TdJson::load()?;
    let response = native
        .execute(c"{\"@type\":\"setLogVerbosityLevel\",\"new_verbosity_level\":1}")?
        .ok_or_else(|| error(-4, "TDLib did not confirm the log verbosity."))?;
    if parse(&response)?["@type"] != "ok" {
        return Err(error(-4, "Could not set TDLib log verbosity."));
    }
    let client = native.create();
    let shared = Arc::new(Shared::new(Arc::new(native), client));
    // The first request starts TDLib updates. Fixed input cannot fail serialization.
    shared.transport.send(
        client,
        c"{\"@type\":\"getAuthorizationState\",\"@extra\":{\"command\":\"initialize\"}}",
    );
    Ok((shared, parameters))
}

fn run(shared: Arc<Shared>, mut parameters: Option<Value>, callback: impl Fn(Event)) {
    let mut decoder = Decoder::default();
    let mut callback_alive = true;
    let mut emit = |event| {
        if callback_alive
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event))).is_err()
        {
            callback_alive = false;
            shared.close();
        }
    };
    loop {
        let received = shared.transport.receive();
        let value = match received {
            Ok(None) => continue, // Native 60-second blocking timeout, not a polling timer.
            Ok(Some(text)) => parse(&text),
            Err(error) => Err(error),
        };
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                emit(Event::CommandFailed {
                    command: CommandKind::Initialize,
                    error,
                });
                continue;
            }
        };
        if integer(&value["@client_id"]).ok() != Some(i64::from(shared.client)) {
            continue;
        }
        shared.complete(&value);
        let state = if value["@type"] == "updateAuthorizationState" {
            auth(&value["authorization_state"]).ok()
        } else {
            None
        };
        if state == Some(AuthState::Closed) {
            // Publish closure before invoking the callback so it cannot submit new work.
            shared.closed.store(true, Ordering::Release);
        }
        if state == Some(AuthState::WaitParameters)
            && !shared.closing.load(Ordering::Acquire)
            && let Some(parameters) = parameters.take()
            && let Err(error) = shared.send_value(parameters)
        {
            emit(Event::CommandFailed {
                command: CommandKind::Initialize,
                error,
            });
            shared.close();
        }
        if state == Some(AuthState::Ready) {
            for (scope, number) in [
                ("notificationSettingsScopePrivateChats", 0),
                ("notificationSettingsScopeGroupChats", 1),
                ("notificationSettingsScopeChannelChats", 2),
            ] {
                if let Err(error) = shared.send_value(json!({"@type":"getScopeNotificationSettings","scope":{"@type":scope},"@extra":{"command":"scope","scope":number}})) {
                    emit(Event::CommandFailed { command: CommandKind::Initialize,error });
                }
            }
        }
        if value["@type"] == "ok"
            && (value["@extra"]["command"] == "open" || value["@extra"]["command"] == "open_around")
            && let Ok(chat) = integer(&value["@extra"]["chat"])
        {
            let request = if value["@extra"]["command"] == "open_around" {
                request::around(
                    ChatId(chat),
                    MessageId(integer(&value["@extra"]["anchor"]).unwrap_or(0)),
                )
            } else {
                request::history(ChatId(chat), MessageId(0))
            };
            if let Err(error) = shared.send_value(request) {
                emit(Event::CommandFailed {
                    command: CommandKind::LoadOlderMessages,
                    error,
                });
            }
        }
        if let Some(key) = toggle_step(&value) {
            let mut failure = None;
            let finished =
                if value["@type"] == "message" && value["@extra"]["command"] == "toggle_reaction" {
                    // The read answered: send the mutation; the toggle finishes with its answer.
                    match request::toggle_reaction(&value).and_then(|r| shared.send_value(r)) {
                        Ok(()) => false,
                        Err(error) => {
                            failure = Some(error);
                            true
                        }
                    }
                } else {
                    // The mutation answered, or the read failed (the decoder reports the error).
                    true
                };
            if finished {
                failure = shared.advance_toggle(key).or(failure);
            }
            if let Some(error) = failure {
                emit(Event::CommandFailed {
                    command: CommandKind::ToggleReaction,
                    error,
                });
            }
        }
        if let Err(error) = decoder.apply(&value, &mut emit) {
            emit(Event::CommandFailed {
                command: command_kind(&value["@extra"]["command"]),
                error,
            });
        }
        if state == Some(AuthState::Closed) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::ffi::CStr;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, mpsc};
    use tg_core::{Command, CommandError};

    // Native-boundary double only: JSON serialization and engine lifecycle stay real.
    struct Fake {
        // Test-only channels hold at most 130 items, matching the in-flight test limit.
        sent: mpsc::SyncSender<String>,
        received: Mutex<mpsc::Receiver<String>>,
    }
    impl Transport for Fake {
        fn send(&self, _: i32, request: &CStr) {
            self.sent
                .send(request.to_string_lossy().into_owned())
                .unwrap();
        }
        fn receive(&self) -> Result<Option<String>, CommandError> {
            Ok(self.received.lock().unwrap().recv().ok())
        }
    }

    fn fake_shared() -> (
        Arc<Shared>,
        mpsc::Receiver<String>,
        mpsc::SyncSender<String>,
    ) {
        let (sent, requests) = mpsc::sync_channel(130);
        let (updates, received) = mpsc::sync_channel(130);
        let transport = Arc::new(Fake {
            sent,
            received: Mutex::new(received),
        });
        (Arc::new(Shared::new(transport, 7)), requests, updates)
    }

    #[test]
    fn receipts_reserve_one_slot_per_chat_until_actual_success_or_failure() {
        let (shared, requests, _) = fake_shared();
        let view = || {
            request::encode(Command::ViewMessages {
                chat: ChatId(7),
                messages: vec![MessageId(20401094656)],
            })
            .unwrap()
        };
        shared.send_value(view()).unwrap();
        let sent: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        assert!(shared.send_value(view()).is_err());
        shared.complete(&json!({"@type":"error","@extra":sent["@extra"]}));
        assert!(shared.send_value(view()).is_ok());
    }
    #[test]
    fn saturated_commands_do_not_block_and_shutdown_bypasses_the_limit() {
        let (shared, requests, _) = fake_shared();
        for _ in 0..128 {
            shared
                .send_value(request::encode(Command::LoadMoreChats).unwrap())
                .unwrap();
        }
        assert_eq!(
            shared
                .send_value(request::encode(Command::LoadMoreChats).unwrap())
                .unwrap_err()
                .code,
            -3
        );
        shared.close();
        for _ in 0..128 {
            requests.recv().unwrap();
        }
        let close: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        assert_eq!(close["@type"], "close");
    }

    #[test]
    fn receive_thread_finishes_only_on_closed_and_returns_capacity() {
        let (shared, requests, updates) = fake_shared();
        shared
            .send_value(request::encode(Command::LoadMoreChats).unwrap())
            .unwrap();
        let request: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        let callback_closed = Arc::new(AtomicBool::new(false));
        let flag = callback_closed.clone();
        let observed = shared.clone();
        let running = shared.clone();
        let thread = std::thread::spawn(move || {
            run(running, None, move |event| {
                if matches!(event, Event::AuthStateChanged(tg_core::AuthState::Closed)) {
                    assert!(observed.closed.load(Ordering::Acquire));
                    flag.store(true, Ordering::Release);
                }
            })
        });
        updates
            .send(
                serde_json::json!({"@type":"ok", "@client_id":7, "@extra":request["@extra"]})
                    .to_string(),
            )
            .unwrap();
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":99,"authorization_state":{"@type":"authorizationStateClosed"}}"#.into()).unwrap();
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":7,"authorization_state":{"@type":"authorizationStateClosing"}}"#.into()).unwrap();
        shared.close();
        assert_eq!(
            serde_json::from_str::<Value>(&requests.recv().unwrap()).unwrap()["@type"],
            "close"
        );
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":7,"authorization_state":{"@type":"authorizationStateClosed"}}"#.into()).unwrap();
        thread.join().unwrap();
        assert!(callback_closed.load(Ordering::Acquire));
        assert!(shared.closed.load(Ordering::Acquire));
        assert_eq!(shared.in_flight.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn opening_a_chat_loads_one_page_only_after_success() {
        let (shared, requests, updates) = fake_shared();
        let running = shared.clone();
        let thread = std::thread::spawn(move || run(running, None, |_| {}));
        let engine = Engine {
            shared,
            thread: Some(thread),
        };
        engine.send(Command::OpenChat(ChatId(7))).unwrap();
        let open: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        assert_eq!(open["@type"], "openChat");
        updates
            .send(json!({"@type":"ok", "@client_id":7, "@extra":open["@extra"]}).to_string())
            .unwrap();
        let history: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        assert_eq!(history["@type"], "getChatHistory");
        assert_eq!(history["limit"], 50);
        assert_eq!(history["from_message_id"], 0);
        assert_eq!(history["chat_id"], 7);
        updates.send(json!({"@type":"messages", "@client_id":7, "@extra":history["@extra"], "messages":[]}).to_string()).unwrap();
        engine.send(Command::CloseChat(ChatId(7))).unwrap();
        let close: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        assert_eq!(close["@type"], "closeChat");
        engine.send(Command::ShutDown).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&requests.recv().unwrap()).unwrap()["@type"],
            "close"
        );
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":7,"authorization_state":{"@type":"authorizationStateClosed"}}"#.into()).unwrap();
        engine.shutdown().unwrap();
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn toggling_a_reaction_reads_the_message_then_adds_or_removes_it() {
        let (shared, requests, updates) = fake_shared();
        let running = shared.clone();
        let thread = std::thread::spawn(move || run(running, None, |_| {}));
        let engine = Engine {
            shared,
            thread: Some(thread),
        };
        for (chosen, expected) in [
            (false, "addMessageReaction"),
            (true, "removeMessageReaction"),
        ] {
            engine
                .send(Command::ToggleReaction {
                    chat: ChatId(7),
                    message: MessageId(1048576),
                    reaction: "👍".into(),
                })
                .unwrap();
            let get: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
            assert_eq!(get["@type"], "getMessage");
            updates
                .send(
                    json!({"@type":"message","@client_id":7,"@extra":get["@extra"],
                    "id":1048576,"chat_id":7,"interaction_info":{"reactions":{"reactions":[
                        {"type":{"@type":"reactionTypeEmoji","emoji":"👍"},
                        "total_count":1,"is_chosen":chosen}]}}})
                    .to_string(),
                )
                .unwrap();
            let change: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
            assert_eq!(change["@type"], expected);
            assert_eq!(change["chat_id"], 7);
            assert_eq!(change["message_id"], 1048576);
            assert_eq!(change["reaction_type"]["emoji"], "👍");
            updates
                .send(json!({"@type":"ok","@client_id":7,"@extra":change["@extra"]}).to_string())
                .unwrap();
        }
        engine.send(Command::ShutDown).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&requests.recv().unwrap()).unwrap()["@type"],
            "close"
        );
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":7,"authorization_state":{"@type":"authorizationStateClosed"}}"#.into()).unwrap();
        engine.shutdown().unwrap();
    }

    fn toggle_command() -> Command {
        Command::ToggleReaction {
            chat: ChatId(7),
            message: MessageId(1048576),
            reaction: "👍".into(),
        }
    }

    fn answer_with(get: &Value, chosen: bool) -> String {
        json!({"@type":"message","@client_id":7,"@extra":get["@extra"],
            "id":1048576,"chat_id":7,"interaction_info":{"reactions":{"reactions":[
                {"type":{"@type":"reactionTypeEmoji","emoji":"👍"},
                "total_count":1,"is_chosen":chosen}]}}})
        .to_string()
    }

    #[test]
    fn toggle_admission_never_waits_and_leaves_nothing_behind_on_failure() {
        let (shared, requests, _) = fake_shared();
        let toggle = || request::encode(toggle_command()).unwrap();
        // TDLib request slots are full: the first send fails and no entry remains.
        shared.in_flight.store(MAX_IN_FLIGHT, Ordering::Release);
        assert_eq!(shared.toggle(toggle()).unwrap_err().code, -3);
        assert!(shared.toggles.lock().unwrap().is_empty());
        assert!(requests.try_recv().is_err());
        // The receive thread holds the lock: the UI gets an error at once instead of waiting.
        shared.in_flight.store(0, Ordering::Release);
        let guard = shared.toggles.lock().unwrap();
        assert_eq!(shared.toggle(toggle()).unwrap_err().code, -3);
        drop(guard);
        assert!(requests.try_recv().is_err());
        // With a free slot and lock the toggle starts and its message is tracked.
        shared.toggle(toggle()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&requests.recv().unwrap()).unwrap()["@type"],
            "getMessage"
        );
        assert_eq!(shared.toggles.lock().unwrap().len(), 1);
    }

    #[test]
    fn quick_toggles_of_one_message_run_one_after_another() {
        let (shared, requests, updates) = fake_shared();
        let running = shared.clone();
        let failures = Arc::new(Mutex::new(Vec::new()));
        let seen = failures.clone();
        let thread = std::thread::spawn(move || {
            run(running, None, move |event| {
                if let Event::CommandFailed { command, .. } = event {
                    seen.lock().unwrap().push(command);
                }
            })
        });
        let engine = Engine {
            shared,
            thread: Some(thread),
        };
        let next = || serde_json::from_str::<Value>(&requests.recv().unwrap()).unwrap();
        // Two clicks before TDLib answers: only the first read goes out.
        engine.send(toggle_command()).unwrap();
        engine.send(toggle_command()).unwrap();
        let get = next();
        assert_eq!(get["@type"], "getMessage");
        assert!(requests.try_recv().is_err());
        updates.send(answer_with(&get, false)).unwrap();
        let add = next();
        assert_eq!(add["@type"], "addMessageReaction");
        assert!(
            requests.try_recv().is_err(),
            "the held toggle waits for the mutation"
        );
        updates
            .send(json!({"@type":"ok","@client_id":7,"@extra":add["@extra"]}).to_string())
            .unwrap();
        // The held toggle now reads the state the add left and takes it back.
        let get = next();
        assert_eq!(get["@type"], "getMessage");
        updates.send(answer_with(&get, true)).unwrap();
        let remove = next();
        assert_eq!(remove["@type"], "removeMessageReaction");
        updates
            .send(
                json!({"@type":"error","@client_id":7,"code":400,"message":"x",
                "@extra":remove["@extra"]})
                .to_string(),
            )
            .unwrap();

        // A failed read releases the message: the next toggle starts at once.
        engine.send(toggle_command()).unwrap();
        let get = next();
        updates
            .send(
                json!({"@type":"error","@client_id":7,"code":400,"message":"x",
                "@extra":get["@extra"]})
                .to_string(),
            )
            .unwrap();
        engine.send(toggle_command()).unwrap();
        let get = next();
        assert_eq!(get["@type"], "getMessage");
        // Held toggles are bounded per message.
        for _ in 0..MAX_HELD_TOGGLES {
            engine.send(toggle_command()).unwrap();
        }
        assert_eq!(engine.send(toggle_command()).unwrap_err().code, -3);
        assert!(requests.try_recv().is_err());

        engine.send(Command::ShutDown).unwrap();
        assert_eq!(next()["@type"], "close");
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":7,"authorization_state":{"@type":"authorizationStateClosed"}}"#.into()).unwrap();
        engine.shutdown().unwrap();
        // Both TDLib errors reached the UI as toggle failures.
        assert_eq!(
            *failures.lock().unwrap(),
            vec![CommandKind::ToggleReaction, CommandKind::ToggleReaction]
        );
    }

    #[test]
    fn commands_can_be_sent_from_other_threads_and_are_rejected_after_close() {
        let (shared, requests, updates) = fake_shared();
        let running = shared.clone();
        let thread = std::thread::spawn(move || run(running, None, |_| {}));
        let engine = Arc::new(Engine {
            shared,
            thread: Some(thread),
        });
        let sender = engine.clone();
        std::thread::spawn(move || sender.send(Command::LoadMoreChats))
            .join()
            .unwrap()
            .unwrap();
        let sent: Value = serde_json::from_str(&requests.recv().unwrap()).unwrap();
        assert_eq!(sent["@type"], "loadChats");
        engine.send(Command::ShutDown).unwrap();
        requests.recv().unwrap();
        assert!(engine.send(Command::LoadMoreChats).is_err());
        updates.send(r#"{"@type":"updateAuthorizationState","@client_id":7,"authorization_state":{"@type":"authorizationStateClosed"}}"#.into()).unwrap();
        Arc::try_unwrap(engine).unwrap().shutdown().unwrap();
    }
}
