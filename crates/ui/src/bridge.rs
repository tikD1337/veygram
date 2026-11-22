//! Event-driven bounded bridge. One Slint wakeup drains a batch; no timer or polling.
use crate::AppWindow;
use slint::ComponentHandle;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread::ThreadId,
};
use tg_core::Event;
const MAX_EVENTS: usize = 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default)]
struct Queue {
    events: VecDeque<Event>,
    bytes: usize,
    scheduled: bool,
    failed: bool,
    paused: bool,
    /// The UI is gone: events are dropped and waiting producers are released.
    closed: bool,
}
impl Queue {
    fn full(&self, bytes: usize) -> bool {
        self.events.len() >= MAX_EVENTS || self.bytes.saturating_add(bytes) > MAX_BYTES
    }
    fn enqueue(&mut self, event: Event) -> bool {
        if self.failed || self.closed {
            return false;
        }
        if matches!(event, Event::RoundFrame(_))
            && self
                .events
                .iter()
                .any(|e| matches!(e, Event::RoundFrame(_)))
        {
            if let Some(e) = self
                .events
                .iter_mut()
                .find(|e| matches!(e, Event::RoundFrame(_)))
            {
                *e = event;
            }
            return false;
        }
        let bytes = event_bytes(&event);
        if self.full(bytes) {
            self.failed = true;
            self.events.clear();
            self.bytes = 0;
        } else {
            self.bytes += bytes;
            self.events.push_back(event);
        }
        if self.scheduled || self.paused {
            false
        } else {
            self.scheduled = true;
            true
        }
    }
}
#[derive(Clone)]
pub struct EventSink {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    window: slint::Weak<AppWindow>,
    ui_thread: ThreadId,
}
impl std::fmt::Debug for EventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventSink(<bounded>)")
    }
}
impl EventSink {
    pub(crate) fn new(window: &AppWindow) -> Self {
        Self {
            queue: Arc::default(),
            window: window.as_weak(),
            ui_thread: std::thread::current().id(),
        }
    }
    /// A full queue makes background producers (the engine, the player) wait until the UI drains
    /// it, so a modal Windows loop (a system dialog, moving the window) only delays updates. The
    /// UI thread never waits; overflow there, or during startup, fails closed instead of
    /// dropping incremental updates silently.
    pub fn post(&self, event: Event) {
        let (lock, drained) = &*self.queue;
        let Ok(mut queue) = lock.lock() else {
            return;
        };
        if std::thread::current().id() != self.ui_thread {
            let bytes = event_bytes(&event);
            while queue.full(bytes) && !queue.closed && !queue.failed && !queue.paused {
                // Woken by every drain and by close; not a timed poll.
                queue = match drained.wait(queue) {
                    Ok(queue) => queue,
                    Err(_) => return,
                };
            }
        }
        if !queue.enqueue(event) {
            return;
        }
        drop(queue);
        self.wake();
    }
    /// Hold delivery during engine startup; the same count/byte limits still apply.
    /// Call before starting the engine, so authorization callbacks cannot race handle publication.
    pub fn pause(&self) {
        if let Ok(mut queue) = self.queue.0.lock() {
            queue.paused = true;
        }
    }
    /// Release queued startup events after the command target has been installed.
    pub fn resume(&self) {
        let Ok(mut queue) = self.queue.0.lock() else {
            return;
        };
        queue.paused = false;
        if queue.scheduled || (queue.events.is_empty() && !queue.failed) {
            return;
        }
        queue.scheduled = true;
        drop(queue);
        self.wake();
    }
    fn wake(&self) {
        // upgrade_in_event_loop uses slint::invoke_from_event_loop internally.
        let _ = self
            .window
            .upgrade_in_event_loop(|window| window.invoke_drain_events());
    }
    pub(crate) fn drain(&self) -> Result<VecDeque<Event>, ()> {
        let (lock, drained) = &*self.queue;
        let mut queue = lock.lock().map_err(|_| ())?;
        queue.scheduled = false;
        queue.bytes = 0;
        drained.notify_all();
        if queue.failed {
            return Err(());
        }
        Ok(std::mem::take(&mut queue.events))
    }
    /// Called when the UI stops: later events are dropped and waiting producers return.
    pub(crate) fn close(&self) {
        let (lock, drained) = &*self.queue;
        if let Ok(mut queue) = lock.lock() {
            queue.closed = true;
            queue.events.clear();
            queue.bytes = 0;
        }
        drained.notify_all();
    }
}
fn message_bytes(m: &tg_core::Message) -> usize {
    m.video.as_ref().map_or(0, |v| file_bytes(&v.file) + 64)
        + m.photo.as_ref().map_or(0, photo_bytes)
        + 256
        + match &m.send_state {
            tg_core::SendState::Failed(error) => error.message.capacity(),
            _ => 0,
        }
        + m.reply
            .as_ref()
            .map_or(0, |r| r.sender.capacity() + r.preview.capacity() + 128)
        + m.forwarded.as_ref().map_or(0, String::capacity)
        + attachment_bytes(&m.attachment)
        + reactions_bytes(&m.reactions)
        + m.text.capacity()
        + m.entities.capacity() * std::mem::size_of::<tg_core::TextEntity>()
        + m.entities
            .iter()
            .map(|e| match &e.kind {
                tg_core::EntityKind::TextUrl { target } => target.capacity(),
                _ => 0,
            })
            .sum::<usize>()
}
fn event_bytes(e: &Event) -> usize {
    256 + match e {
        Event::MessagesAroundLoaded { messages, .. }
        | Event::MessagesLoaded { messages, .. }
        | Event::NewerMessagesLoaded { messages, .. } => messages.iter().map(message_bytes).sum(),
        Event::IncomingMessage(m)
        | Event::NewMessage(m)
        | Event::MessageUpdated { message: m, .. } => message_bytes(m),
        Event::ChatListChanged(
            tg_core::ChatListChange::Insert { chat, .. }
            | tg_core::ChatListChange::Update { chat, .. },
        ) => {
            chat.avatar.as_ref().map_or(0, photo_bytes)
                + chat.title.capacity()
                + chat.last_message.as_ref().map_or(0, |m| m.text.capacity())
        }
        Event::AuthStateChanged(tg_core::AuthState::WaitOtherDevice { link }) => link.capacity(),
        Event::AuthStateChanged(tg_core::AuthState::WaitPassword { hint }) => hint.capacity(),
        Event::FileUpdated(file) => file_bytes(file),
        Event::LocalFileSendFinished(path) => path.as_os_str().len() * 2,
        Event::CommandFailed { error, .. } => error.message.capacity(),
        Event::ReplyResolved { reply, .. } => reply
            .as_ref()
            .map_or(0, |r| r.sender.capacity() + r.preview.capacity() + 128),
        Event::MessageReactionsChanged { reactions, .. } => reactions_bytes(reactions),
        Event::MessagesDeleted { messages, .. } => {
            messages.capacity() * std::mem::size_of::<tg_core::MessageId>()
        }
        Event::MessageContentChanged {
            text,
            entities,
            photo,
            attachment,
            ..
        } => {
            text.capacity()
                + entities.capacity() * std::mem::size_of::<tg_core::TextEntity>()
                + entities
                    .iter()
                    .map(|e| match &e.kind {
                        tg_core::EntityKind::TextUrl { target } => target.capacity(),
                        _ => 0,
                    })
                    .sum::<usize>()
                + photo.as_ref().map_or(0, photo_bytes)
                + attachment_bytes(attachment)
        }
        _ => 0,
    }
}

fn reactions_bytes(reactions: &[tg_core::Reaction]) -> usize {
    reactions
        .iter()
        .map(|r| std::mem::size_of::<tg_core::Reaction>() + r.emoji.capacity())
        .sum()
}
fn attachment_bytes(value: &Option<tg_core::Attachment>) -> usize {
    match value {
        Some(tg_core::Attachment::Voice(v)) => v.waveform.capacity() + file_bytes(&v.file),
        Some(tg_core::Attachment::Document(d)) => d.name.capacity() + file_bytes(&d.file),
        Some(tg_core::Attachment::Sticker(s) | tg_core::Attachment::Emoji(s)) => {
            s.emoji.capacity() + 64
        }
        Some(tg_core::Attachment::Round(v)) => file_bytes(&v.file) + 32,
        None => 0,
    }
}
fn file_bytes(f: &tg_core::MediaFile) -> usize {
    128 + f.partial.as_ref().map_or(0, |s| {
        128 + s.path.as_ref().map_or(0, |p| p.as_os_str().len() * 2)
    }) + f.path.as_ref().map_or(0, |p| p.as_os_str().len() * 2)
}
fn photo_bytes(p: &tg_core::Photo) -> usize {
    p.minithumbnail.capacity() + file_bytes(&p.file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incremental_overflow_fails_closed_with_one_wakeup() {
        let mut queue = Queue::default();
        assert!(queue.enqueue(Event::ChatsLoaded { has_more: true }));
        for _ in 1..MAX_EVENTS {
            assert!(!queue.enqueue(Event::ChatsLoaded { has_more: true }));
        }
        assert_eq!(queue.events.len(), MAX_EVENTS);
        assert!(!queue.enqueue(Event::ChatsLoaded { has_more: true }));
        assert!(queue.failed && queue.events.is_empty());
        assert!(!queue.enqueue(Event::ChatsLoaded { has_more: false }));
    }
    #[test]
    fn round_frames_replace_one_pending_handle_even_across_playback_tokens() {
        let mut queue = Queue::default();
        for token in 0..4096 {
            queue.enqueue(Event::RoundFrame(tg_core::RoundFrame {
                token,
                pixels: Arc::new(Mutex::new(Default::default())),
            }));
        }
        assert!(!queue.failed);
        assert_eq!(queue.events.len(), 1);
        assert_eq!(queue.bytes, 256);
        assert!(
            matches!(queue.events.front(), Some(Event::RoundFrame(frame)) if frame.token == 4095)
        );
    }
    #[test]
    fn startup_hold_does_not_schedule_early_authentication() {
        let mut queue = Queue {
            paused: true,
            ..Default::default()
        };
        assert!(!queue.enqueue(Event::AuthStateChanged(tg_core::AuthState::Ready)));
        assert!(!queue.scheduled);
        assert_eq!(queue.events.len(), 1);
        queue.paused = false;
        assert!(queue.enqueue(Event::ChatsLoaded { has_more: true }));
        assert!(queue.scheduled);
    }
    #[test]
    fn byte_budget_also_bounds_large_events() {
        let mut queue = Queue::default();
        assert!(queue.enqueue(Event::CommandFailed {
            command: tg_core::CommandKind::Initialize,
            error: tg_core::CommandError {
                code: -1,
                message: "x".repeat(MAX_BYTES)
            }
        }));
        assert!(queue.failed && queue.events.is_empty() && queue.bytes == 0);
    }
    fn detached() -> EventSink {
        EventSink {
            queue: Arc::default(),
            window: slint::Weak::default(),
            ui_thread: std::thread::current().id(),
        }
    }
    fn flood(sink: &EventSink, done: std::sync::mpsc::Sender<()>) -> std::thread::JoinHandle<()> {
        let sink = sink.clone();
        std::thread::spawn(move || {
            for _ in 0..=MAX_EVENTS {
                sink.post(Event::ChatsLoaded { has_more: true });
            }
            let _ = done.send(());
        })
    }
    #[test]
    fn a_background_producer_waits_for_a_drain_instead_of_failing() {
        let sink = detached();
        let (done, finished) = std::sync::mpsc::channel();
        let producer = flood(&sink, done);
        // The UI is busy (as inside a modal Windows loop): the producer stays blocked.
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
        assert_eq!(sink.drain().unwrap().len(), MAX_EVENTS);
        finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        producer.join().unwrap();
        assert_eq!(sink.drain().unwrap().len(), 1);
    }
    #[test]
    fn closing_releases_a_blocked_producer() {
        let sink = detached();
        let (done, finished) = std::sync::mpsc::channel();
        let producer = flood(&sink, done);
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
        sink.close();
        finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        producer.join().unwrap();
        assert!(sink.drain().unwrap().is_empty());
    }
}
