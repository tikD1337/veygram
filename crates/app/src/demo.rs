//! Synthetic local backend. No TDLib, credentials, data-directory or key access.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tg_core::{
    AuthState, Chat, ChatId, ChatListChange, Command, EntityKind, Event, Message, MessageId,
    MessagePreview, SendState, TextEntity,
};
use tg_ui::EventSink;
/// Synthetic history starts on 2026-11-02 and has one message per minute.
const BASE_DATE: i32 = 1_793_620_800;
const NAMES: [&str; 3] = ["Anna Petrova", "Boris", "Clara Weber"];
pub(crate) struct Demo {
    pub sink: Option<EventSink>,
    media_path: std::path::PathBuf,
    text_only: bool,
    sent: i64,
    changes: Arc<std::sync::Mutex<Changes>>,
    // Four synthetic metadata rows; no retained full message history.
    chats: Arc<std::sync::Mutex<Vec<Chat>>>,
    // At most two delayed page workers, one per direction; no retained full history.
    pending: Arc<AtomicUsize>,
    video_workers: Arc<AtomicUsize>,
    downloads: Arc<std::sync::Mutex<std::collections::BTreeMap<i32, Arc<AtomicBool>>>>,
    windows: Arc<std::sync::Mutex<std::collections::BTreeMap<i32, DemoWindow>>>,
}
/// A streaming download as TDLib runs it: from `offset` up to `end`, extended in place when the
/// same offset is requested again with a larger limit.
struct DemoWindow {
    offset: u64,
    cursor: u64,
    end: u64,
    running: bool,
    cancelled: Arc<AtomicBool>,
}
impl Demo {
    pub fn new(media_path: std::path::PathBuf, text_only: bool) -> Self {
        Self {
            sink: None,
            media_path,
            text_only,
            sent: 20000,
            changes: Arc::default(),
            chats: Arc::default(),
            pending: Arc::default(),
            downloads: Arc::default(),
            windows: Arc::default(),
            video_workers: Arc::default(),
        }
    }
    pub fn seed(&mut self) {
        let Some(sink) = &self.sink else { return };
        sink.post(Event::AuthStateChanged(AuthState::Ready));
        let chats = [
            (
                "History / 20,000 messages",
                "Synthetic paged history / 150 ms latency",
                14,
            ),
            ("Saved Messages", "cargo build --release -p tg-app", 0),
            ("Design review", "Albums now show as one bubble", 3),
            ("Weekend trip", "Photo", 12),
            ("Sticker gallery", "Sticker", 0),
        ];
        for (position, (title, text, unread)) in chats.into_iter().enumerate() {
            let id = position as i64 + 1;
            let chat = Chat {
                avatar: (!self.text_only && id == 1).then(|| demo_photo(1)),
                id: ChatId(id),
                title: title.into(),
                last_message: Some(MessagePreview {
                    id: MessageId(20000 * 1048576),
                    text: text.into(),
                    date: BASE_DATE + 20000 * 60 - id as i32 * 7200,
                    outgoing: false,
                }),
                unread_count: unread,
                unread_mentions: if id == 3 { 2 } else { 0 },
                muted_until: if id == 4 { i64::MAX } else { 0 },
                show_notification_preview: true,
                last_read_inbox: MessageId(
                    match id {
                        1 => 19980,
                        3 => 19996,
                        4 => 19982,
                        _ => 20000,
                    } * 1048576,
                ),
                last_read_outbox: MessageId(19990 * 1048576),
            };
            if let Ok(mut chats) = self.chats.lock() {
                chats.push(chat.clone());
            }
            sink.post(Event::ChatListChanged(ChatListChange::Insert {
                position,
                chat,
            }));
        }
        sink.post(Event::ChatsLoaded { has_more: false });
    }
    pub fn command(&mut self, command: Command) -> Result<(), tg_core::CommandError> {
        let Some(sink) = &self.sink else {
            return Ok(());
        };
        match command {
            Command::OpenMessageContent { chat, message } => {
                if let Some(mut m) = self.lookup(chat, message) {
                    match &mut m.attachment {
                        Some(tg_core::Attachment::Voice(v)) => v.listened = true,
                        Some(tg_core::Attachment::Round(v)) => v.viewed = true,
                        _ => {}
                    }
                    self.remember(m)?;
                }
                sink.post(Event::MessageContentOpened { chat, message });
            }
            Command::DownloadFileUrgent(id) => self.download(id)?,
            Command::GetMessageProperties { chat, message } => {
                let m = self.lookup(chat, message);
                sink.post(Event::MessageProperties {
                    chat,
                    message,
                    permissions: m.map_or_else(Default::default, |m| tg_core::MessagePermissions {
                        edit: m.outgoing && m.is_text,
                        delete_self: true,
                        delete_all: true,
                        forward: true,
                        reply: true,
                    }),
                });
            }
            Command::GetRepliedMessage { chat, message } => {
                let reply = self.lookup(chat, message).and_then(|m| m.reply);
                sink.post(Event::ReplyResolved {
                    chat,
                    message,
                    reply,
                });
            }
            Command::ReplyText { chat, reply, text } => {
                let original = self.lookup(chat, reply);
                self.sent += 1;
                let mut m = local_message(chat, self.sent, text);
                m.reply = original.map(|m| quote(&m));
                self.remember(m.clone())?;
                sink.post(Event::NewMessage(m));
            }
            Command::EditText {
                chat,
                message,
                text,
            } => {
                let Some(mut m) = self
                    .lookup(chat, message)
                    .filter(|m| m.outgoing && m.is_text)
                else {
                    return Err(action_error());
                };
                m.text = text;
                m.entities.clear();
                m.edited = true;
                self.remember(m.clone())?;
                sink.post(Event::MessageContentChanged {
                    chat,
                    message,
                    text: m.text,
                    entities: vec![],
                    photo: None,
                    attachment: None,
                    is_text: true,
                });
                sink.post(Event::MessageEdited {
                    chat,
                    message,
                    edited: true,
                });
            }
            Command::ToggleReaction {
                chat,
                message,
                reaction,
            } => {
                let Some(mut m) = self.lookup(chat, message) else {
                    return Err(action_error());
                };
                toggle_reaction(&mut m.reactions, &reaction);
                self.remember(m.clone())?;
                sink.post(Event::MessageReactionsChanged {
                    chat,
                    message,
                    reactions: m.reactions,
                });
            }
            Command::DeleteMessages { chat, messages, .. } => {
                if messages.is_empty() || messages.len() > 10 {
                    return Err(action_error());
                }
                let mut changes = self.changes.lock().map_err(|_| action_error())?;
                if changes.entries.len() + messages.len() > 512 {
                    return Err(action_error());
                }
                for id in &messages {
                    changes.entries.insert((chat, *id), None);
                }
                sink.post(Event::MessagesDeleted { chat, messages });
            }
            Command::ForwardMessages { from, to, messages } => {
                if messages.is_empty() || messages.len() > 10 {
                    return Err(action_error());
                }
                let originals = messages
                    .iter()
                    .map(|id| self.lookup(from, *id).ok_or_else(action_error))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut forwarded = Vec::new();
                self.sent += 1;
                let album = self.sent;
                for mut m in originals {
                    self.sent += 1;
                    m.chat = to;
                    m.id = MessageId(self.sent * 1048576);
                    m.forwarded = Some(m.sender.clone().unwrap_or_else(|| {
                        if m.outgoing {
                            "You".into()
                        } else {
                            "Message".into()
                        }
                    }));
                    m.reply = None;
                    m.outgoing = true;
                    m.permissions = None;
                    m.album = if messages.len() > 1 { album } else { 0 };
                    m.date = BASE_DATE + self.sent as i32 * 60;
                    m.sender = None;
                    m.sender_id = 0;
                    forwarded.push(m);
                }
                let mut changes = self.changes.lock().map_err(|_| action_error())?;
                if changes.entries.len() + forwarded.len() > 512 {
                    return Err(action_error());
                }
                for m in forwarded {
                    changes.entries.insert((to, m.id), Some(m.clone()));
                    sink.post(Event::NewMessage(m));
                }
            }
            Command::DownloadRange {
                file,
                offset,
                limit,
            } => self.download_video(file, offset, limit)?,
            Command::DownloadFile(id) if (VIDEO_FILE_ID..2_000_000).contains(&id) => {
                self.download_video(id, 0, 0)?
            }
            Command::DownloadFile(id) => {
                let (path, size) = demo_file(&self.media_path, id);
                sink.post(Event::FileUpdated(tg_core::MediaFile {
                    partial: None,
                    id,
                    path: Some(path),
                    size,
                    downloaded: size,
                    downloading: false,
                }));
            }
            Command::CancelDownload(id) => {
                if let Ok(mut downloads) = self.downloads.lock()
                    && let Some(cancelled) = downloads.remove(&id)
                {
                    cancelled.store(true, Ordering::Release);
                }
                let (_, size) = demo_file(&self.media_path, id);
                sink.post(Event::FileUpdated(tg_core::MediaFile {
                    partial: None,
                    id,
                    path: None,
                    size,
                    downloaded: 0,
                    downloading: false,
                }));
            }
            Command::ViewMessages { chat, messages } => {
                let valid = !messages.is_empty()
                    && messages.len() <= 50
                    && messages.iter().all(|id| id.0 > 0);
                if valid
                    && let Ok(mut chats) = self.chats.lock()
                    && let Some(entry) = chats.iter_mut().find(|c| c.id == chat)
                    && let Some(last) = messages.iter().max().copied()
                {
                    let old = entry.last_read_inbox.0 / 1048576;
                    let new = last.0 / 1048576;
                    let incoming = |n: i64| n - n / 3;
                    let read = incoming(new).saturating_sub(incoming(old)).max(0) as u32;
                    entry.unread_count = entry.unread_count.saturating_sub(read);
                    entry.last_read_inbox = entry.last_read_inbox.max(last);
                    if entry.unread_count == 0 {
                        entry.unread_mentions = 0;
                    }
                    let position = (chat.0 - 1) as usize;
                    sink.post(Event::ChatListChanged(ChatListChange::Update {
                        position,
                        chat: entry.clone(),
                    }));
                }
                sink.post(Event::MessagesViewed {
                    chat,
                    success: valid,
                });
            }
            Command::OpenChatAt { chat, anchor } => {
                let mut event = media_page(chat, anchor, false, self.text_only);
                if let Event::MessagesLoaded { messages, .. } = &mut event {
                    let mut newer = media_page(chat, anchor, true, self.text_only);
                    if let Event::NewerMessagesLoaded { messages: tail, .. } = &mut newer {
                        messages.extend(tail.drain(..).take(25));
                    }
                    // Both paged halves exclude their cursor. An around response must
                    // include the anchor itself, including a round we return to from a float.
                    if let Some(message) = self.lookup(chat, anchor) {
                        messages.push(message);
                    }
                    messages.sort_by_key(|m| m.id);
                    while messages.len() > 50 {
                        messages.remove(0);
                    }
                }
                if let Ok(changes) = self.changes.lock() {
                    changes.apply(&mut event);
                }
                if let Event::MessagesLoaded { messages, .. } = event {
                    sink.post(Event::MessagesAroundLoaded {
                        chat,
                        anchor,
                        messages,
                    });
                }
            }
            Command::OpenChat(chat) => self.page(chat, MessageId(0), false)?,
            Command::LoadOlderMessages { chat, before } => self.page(chat, before, false)?,
            Command::LoadNewerMessages { chat, after } => self.page(chat, after, true)?,
            Command::RequestQrSignIn => {
                sink.post(Event::AuthStateChanged(AuthState::WaitOtherDevice {
                    link: "https://example.org/synthetic-sign-in".into(),
                }))
            }
            Command::SubmitPhoneNumber(_) => {
                sink.post(Event::AuthStateChanged(AuthState::WaitCode))
            }
            Command::SubmitCode(_) => sink.post(Event::AuthStateChanged(AuthState::WaitPassword {
                hint: String::new(),
            })),
            Command::SubmitPassword(_) => sink.post(Event::AuthStateChanged(AuthState::Ready)),
            Command::LoadMoreChats => sink.post(Event::ChatsLoaded { has_more: false }),
            Command::SendText { chat, text } => {
                self.sent += 1;
                let message = local_message(chat, self.sent, text);
                self.remember(message.clone())?;
                sink.post(Event::NewMessage(message));
            }
            // Demo never uploads: release the source file (and any pasted temporary) at once.
            Command::SendPhoto { path, .. } | Command::SendDocument { path, .. } => {
                sink.post(Event::LocalFileSendFinished(path));
            }
            _ => {}
        }
        Ok(())
    }
    fn download_video(
        &self,
        id: i32,
        offset: u64,
        limit: u64,
    ) -> Result<(), tg_core::CommandError> {
        use std::io::{Seek, SeekFrom, Write};
        let Some(sink) = self.sink.clone() else {
            return Ok(());
        };
        let bytes: &'static [u8] = include_bytes!("../assets/demo-video.mp4");
        let size = bytes.len() as u64;
        let offset = (offset / tg_core::STREAM_PART * tg_core::STREAM_PART).min(size);
        let end = if limit == 0 {
            size
        } else {
            offset.saturating_add(limit).min(size)
        };
        let mut windows = self.windows.lock().map_err(|_| action_error())?;
        let resume = match windows.get_mut(&id) {
            Some(w) if w.offset == offset && !w.cancelled.load(Ordering::Acquire) => {
                // The same offset again extends the download, as TDLib does.
                w.end = w.end.max(end);
                if w.running {
                    return Ok(());
                }
                Some((w.cancelled.clone(), w.cursor))
            }
            _ => None,
        };
        if self
            .video_workers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 4).then_some(n + 1)
            })
            .is_err()
        {
            return Err(action_error());
        }
        let slot = VideoSlot(self.video_workers.clone());
        let (cancelled, start) = match resume {
            Some(resume) => resume,
            None => {
                let cancelled = Arc::new(AtomicBool::new(false));
                let mut downloads = self.downloads.lock().map_err(|_| action_error())?;
                if let Some(old) = downloads.remove(&id) {
                    old.store(true, Ordering::Release);
                }
                if downloads.len() >= 8 {
                    return Err(action_error());
                }
                downloads.insert(id, cancelled.clone());
                windows.insert(
                    id,
                    DemoWindow {
                        offset,
                        cursor: offset,
                        end,
                        running: false,
                        cancelled: cancelled.clone(),
                    },
                );
                (cancelled, offset)
            }
        };
        if let Some(w) = windows.get_mut(&id) {
            w.running = true;
        }
        drop(windows);
        let path = self.media_path.with_extension("stream.mp4");
        let downloads = self.downloads.clone();
        let windows = self.windows.clone();
        std::thread::Builder::new()
            .name("tg-demo-video-range".into())
            .spawn(move || {
                let _slot = slot;
                let mut output = match std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .open(&path)
                {
                    Ok(f) => f,
                    Err(_) => {
                        sink.post(Event::FileDownloadFailed(id));
                        return;
                    }
                };
                let mut cursor = start;
                while !cancelled.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_secs(2));
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    // Stop at the window's end under the lock, so an extension cannot be lost.
                    let stop = match windows.lock() {
                        Ok(mut windows) => match windows.get_mut(&id) {
                            Some(w) if Arc::ptr_eq(&w.cancelled, &cancelled) => {
                                let stop = cursor >= w.end;
                                w.running = !stop;
                                stop
                            }
                            _ => true,
                        },
                        Err(_) => true,
                    };
                    if stop {
                        break;
                    }
                    let end = (cursor + tg_core::STREAM_PART).min(size);
                    if output
                        .seek(SeekFrom::Start(cursor))
                        .and_then(|_| output.write_all(&bytes[cursor as usize..end as usize]))
                        .is_err()
                    {
                        sink.post(Event::FileDownloadFailed(id));
                        break;
                    }
                    cursor = end;
                    if let Ok(mut windows) = windows.lock()
                        && let Some(w) = windows.get_mut(&id)
                        && Arc::ptr_eq(&w.cancelled, &cancelled)
                    {
                        w.cursor = cursor;
                    }
                    let completed = offset == 0 && cursor == size;
                    sink.post(Event::FileUpdated(tg_core::MediaFile {
                        partial: Some(tg_core::StreamFile {
                            id,
                            size,
                            path: Some(path.clone()),
                            offset,
                            prefix: cursor - offset,
                            completed,
                        }),
                        id,
                        size,
                        downloaded: cursor - offset,
                        downloading: !completed,
                        path: completed.then(|| path.clone()),
                    }));
                }
                if let Ok(mut windows) = windows.lock()
                    && let Some(w) = windows.get_mut(&id)
                    && Arc::ptr_eq(&w.cancelled, &cancelled)
                {
                    w.running = false;
                }
                if let Ok(mut downloads) = downloads.lock()
                    && downloads
                        .get(&id)
                        .is_some_and(|flag| Arc::ptr_eq(flag, &cancelled))
                {
                    downloads.remove(&id);
                }
            })
            .map_err(|_| action_error())?;
        Ok(())
    }
    fn download(&self, id: i32) -> Result<(), tg_core::CommandError> {
        let Some(sink) = self.sink.clone() else {
            return Ok(());
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut downloads = self.downloads.lock().map_err(|_| action_error())?;
            if downloads.contains_key(&id) {
                return Ok(());
            }
            if downloads.len() >= 8 {
                return Err(action_error());
            }
            downloads.insert(id, cancelled.clone());
        }
        let (path, size) = demo_file(&self.media_path, id);
        sink.post(Event::FileUpdated(tg_core::MediaFile {
            partial: None,
            id,
            path: None,
            size,
            downloaded: size / 2,
            downloading: true,
        }));
        let downloads = self.downloads.clone();
        std::thread::Builder::new()
            .name("tg-demo-download".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                if !cancelled.load(Ordering::Acquire) {
                    sink.post(Event::FileUpdated(tg_core::MediaFile {
                        partial: None,
                        id,
                        path: Some(path),
                        size,
                        downloaded: size,
                        downloading: false,
                    }));
                }
                if let Ok(mut downloads) = downloads.lock() {
                    downloads.remove(&id);
                }
            })
            .map_err(|_| action_error())?;
        Ok(())
    }
    fn lookup(&self, chat: ChatId, id: MessageId) -> Option<Message> {
        if let Ok(changes) = self.changes.lock()
            && let Some(m) = changes.entries.get(&(chat, id))
        {
            return m.clone();
        }
        synthetic(chat, id, self.text_only)
    }
    fn remember(&self, m: Message) -> Result<(), tg_core::CommandError> {
        let mut changes = self.changes.lock().map_err(|_| action_error())?;
        if changes.entries.len() >= 512 && !changes.entries.contains_key(&(m.chat, m.id)) {
            return Err(action_error());
        }
        changes.entries.insert((m.chat, m.id), Some(m));
        Ok(())
    }
    pub fn schedule_incoming(&mut self) {
        let Some(sink) = self.sink.clone() else {
            return;
        };
        // Reserved from the local-message sequence now, so a text sent before the arrival
        // gets the next ID instead of this one.
        self.sent += 1;
        let sequence = self.sent;
        let chats = self.chats.clone();
        let changes = self.changes.clone();
        // One demo-only arrival, three seconds after startup; no periodic notification timer.
        let _ = std::thread::Builder::new()
            .name("tg-demo-incoming".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                let message = Message {
                    attachment: None,
                    reply: None,
                    forwarded: None,
                    edited: false,
                    is_text: true,
                    service: false,
                    permissions: None,
                    video: None,
                    chat: ChatId(3),
                    id: MessageId(sequence * 1048576),
                    date: BASE_DATE + sequence as i32 * 60,
                    outgoing: false,
                    text: "A synthetic desktop notification has arrived.".into(),
                    sender: Some("Anna Petrova".into()),
                    sender_id: 1000,
                    album: 0,
                    photo: None,
                    entities: vec![],
                    send_state: SendState::Sent,
                    reactions: Vec::new(),
                };
                // Stored like a sent message, so actions such as reactions can find it.
                if let Ok(mut changes) = changes.lock()
                    && changes.entries.len() < 512
                {
                    changes
                        .entries
                        .insert((message.chat, message.id), Some(message.clone()));
                }
                if let Ok(mut chats) = chats.lock()
                    && let Some(chat) = chats.iter_mut().find(|c| c.id == message.chat)
                {
                    chat.unread_count = chat.unread_count.saturating_add(1);
                    chat.last_message = Some(MessagePreview {
                        id: message.id,
                        date: message.date,
                        outgoing: false,
                        text: message.text.clone(),
                    });
                    sink.post(Event::ChatListChanged(ChatListChange::Update {
                        position: 2,
                        chat: chat.clone(),
                    }));
                }
                sink.post(Event::IncomingMessage(message));
            });
    }
    fn page(
        &self,
        chat: ChatId,
        cursor: MessageId,
        newer: bool,
    ) -> Result<(), tg_core::CommandError> {
        let error = || tg_core::CommandError {
            code: -4,
            message: "Demo page capacity exceeded.".into(),
        };
        let Some(sink) = self.sink.clone() else {
            return Ok(());
        };
        let pending = self.pending.clone();
        if pending
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .is_err()
        {
            return Err(error());
        }
        let release = pending.clone();
        let text_only = self.text_only;
        let changes = self.changes.clone();
        if std::thread::Builder::new()
            .name("tg-demo-page".into())
            .spawn(move || {
                // A single artificial latency delay per request, never a sleeping loop or UI wait.
                std::thread::sleep(Duration::from_millis(150));
                let mut event = media_page(chat, cursor, newer, text_only);
                if let Ok(changes) = changes.lock() {
                    changes.apply(&mut event);
                }

                release.fetch_sub(1, Ordering::AcqRel);
                sink.post(event);
            })
            .is_err()
        {
            pending.fetch_sub(1, Ordering::AcqRel);
            return Err(error());
        }
        Ok(())
    }
}
fn media_page(chat: ChatId, before: MessageId, newer: bool, text_only: bool) -> Event {
    let mut event = page(chat, before, newer);
    if (!text_only || chat.0 == 5)
        && let Event::MessagesLoaded { messages, .. } | Event::NewerMessagesLoaded { messages, .. } =
            &mut event
    {
        for message in messages {
            let n = message.id.0 / 1048576;
            // Service messages never carry media, as in Telegram.
            if message.service {
                continue;
            }
            if chat.0 == 5 {
                demo_attachment(message, 9);
                continue;
            }
            if matches!(n % 20, 4 | 8 | 9 | 14 | 17 | 18 | 15 | 16 | 19) {
                demo_attachment(message, n % 20);
                continue;
            }
            match n % 20 {
                0 => {
                    message.is_text = false;
                    // A synthetic 4-second clip with the demo photo as its preview frame.
                    let mut preview = demo_photo((100000 + n) as i32);
                    (preview.width, preview.height) = (426, 240);
                    message.photo = Some(preview);
                    message.video = Some(tg_core::Video {
                        supports_streaming: true,
                        file: tg_core::MediaFile {
                            partial: None,
                            size: include_bytes!("../assets/demo-video.mp4").len() as u64,
                            downloaded: 0,
                            downloading: false,
                            id: VIDEO_FILE_ID + n as i32,
                            path: None,
                        },
                        duration: 4,
                        width: 426,
                        height: 240,
                    });
                    message.text = String::new();
                    message.entities.clear();
                }
                // Albums of 2, 3 and 4 photos with the caption on the first item, like Telegram.
                1..=2 | 5..=7 | 10..=13 => {
                    let first = match n % 20 {
                        1..=2 => 1,
                        5..=7 => 5,
                        _ => 10,
                    };
                    let (width, height) = [(1008, 640), (640, 800), (800, 800)][(n % 3) as usize];
                    let mut photo = demo_photo((100000 + n) as i32);
                    photo.width = width;
                    photo.height = height;
                    message.photo = Some(photo);
                    message.is_text = false;
                    message.album = n - n % 20 + first;
                    message.text = if n % 20 == first {
                        format!("#{n}  Album / synthetic photos")
                    } else {
                        String::new()
                    };
                    message.entities.clear();
                }
                15 => {
                    message.photo = Some(demo_photo((100000 + n) as i32));
                    message.is_text = false;
                    message.text = String::new();
                    message.entities.clear();
                }
                16 => {
                    message.photo = Some(demo_photo((100000 + n) as i32));
                    message.is_text = false;
                    message.text = format!("#{n}  Mountain lake / synthetic photo caption");
                    message.entities.clear();
                }
                _ => {}
            }
        }
    }
    event
}
fn demo_photo(id: i32) -> tg_core::Photo {
    tg_core::Photo {
        file: tg_core::MediaFile {
            partial: None,
            size: 0,
            downloaded: 0,
            downloading: false,
            id,
            path: None,
        },
        width: 1008,
        height: 640,
        minithumbnail: include_bytes!("../assets/demo-mini.jpg").to_vec(),
    }
}
/// First file ID of synthetic videos; photo IDs stay below it.
const VIDEO_FILE_ID: i32 = 1_000_000;
/// Called by the startup worker. One synthetic JPEG and one synthetic MP4 (the same name with
/// another extension) per process, no account data.
pub(crate) fn prepare_media() -> std::io::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!("veygram-demo-{}.jpg", std::process::id()));
    std::fs::write(&path, include_bytes!("../assets/demo-photo.jpg"))?;
    std::fs::write(
        path.with_extension("mp4"),
        include_bytes!("../assets/demo-video.mp4"),
    )?;
    for (extension, bytes) in [
        (
            "webp",
            include_bytes!("../assets/demo-sticker.webp").as_slice(),
        ),
        (
            "round.mp4",
            include_bytes!("../assets/demo-round.mp4").as_slice(),
        ),
        ("ogg", include_bytes!("../assets/demo-voice.ogg").as_slice()),
        ("txt", include_bytes!("../assets/demo-file.txt").as_slice()),
        ("pdf", include_bytes!("../assets/demo-file.pdf").as_slice()),
        (
            "ps1",
            include_bytes!("../assets/demo-script.ps1").as_slice(),
        ),
    ] {
        std::fs::write(path.with_extension(extension), bytes)?;
    }
    Ok(path)
}
pub(crate) fn cleanup_media(path: &std::path::Path) -> std::io::Result<()> {
    for ext in [
        "stream.mp4",
        "round.mp4",
        "mp4",
        "webp",
        "ogg",
        "txt",
        "pdf",
        "ps1",
    ] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    std::fs::remove_file(path)
}
fn page(chat: ChatId, before: MessageId, newer: bool) -> Event {
    let sequence = before.0 / 1048576;
    let end = if newer {
        (sequence + 49).min(20000)
    } else if before.0 == 0 {
        20000
    } else {
        sequence - 1
    };
    let start = if newer {
        sequence + 1
    } else {
        (end - 49).max(1)
    };
    let messages = (start..=end).rev().map(|id| {
        let (text,mut entities): (String,Vec<TextEntity>) = match id % 6 {
            0 => ("Welcome to the demo. These messages are generated locally; no Telegram account is connected.".into(), vec![TextEntity { offset: 0, length: 19, kind: EntityKind::Bold }]),
            1 => ("\u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}! \u{041f}\u{0440}\u{043e}\u{0432}\u{0435}\u{0440}\u{044f}\u{0435}\u{043c} \u{043f}\u{0435}\u{0440}\u{0435}\u{043d}\u{043e}\u{0441} \u{0441}\u{0442}\u{0440}\u{043e}\u{043a} \u{0438} emoji \u{1f642} \u{1f525} \u{1f44d}".into(), vec![]),
            2 => ("\u{05e9}\u{05dc}\u{05d5}\u{05dd} \u{05e2}\u{05d5}\u{05dc}\u{05dd} - \u{0645}\u{0631}\u{062d}\u{0628}\u{0627} \u{0628}\u{0627}\u{0644}\u{0639}\u{0627}\u{0644}\u{0645}".into(), vec![]),
            3 => ("cargo build --release -p tg-app".into(), vec![TextEntity { offset: 0, length: 30, kind: EntityKind::Code }]),
            4 => ("Formatting: bold, italic, underline, strike, spoiler".into(), vec![TextEntity { offset: 12, length: 4, kind: EntityKind::Bold }, TextEntity { offset: 18, length: 6, kind: EntityKind::Italic }, TextEntity { offset: 26, length: 9, kind: EntityKind::Underline }, TextEntity { offset: 37, length: 6, kind: EntityKind::Strikethrough }, TextEntity { offset: 45, length: 7, kind: EntityKind::Spoiler }]),
            _ => ("You can select another chat, load earlier messages, resize the window, and send a local message. Enter sends; Shift+Enter inserts a new line.\nhttps://example.org".into(), vec![TextEntity { offset: 142, length: 19, kind: EntityKind::Url }]),
        };
        let prefix=format!("#{id}  ");
        for entity in &mut entities { entity.offset+=prefix.len() as u32; }
        let outgoing = id % 3 == 0;
        let sender_id = if outgoing { 0 } else { 1000 + (id / 4) % 3 };
        let mut message = Message { attachment: None, video: None, reply: (id%6==4).then(||tg_core::Reply {resolved:true,chat,message:MessageId((id-3).max(1)*1048576),sender_id:1000,sender:"Anna Petrova".into(),preview:"Earlier synthetic message".into()}), forwarded: (id%9==0).then(||"Design review".into()), edited: outgoing && id%15==0, is_text: true, service: false, permissions: None, album: 0, sender_id, sender: (!outgoing).then(|| NAMES[(sender_id - 1000) as usize].to_owned()), photo: None, id: MessageId(id*1048576), chat, date: BASE_DATE + id as i32 * 60, outgoing, text: format!("{prefix}{text}{}", if id % 11 == 0 { " Long messages wrap at the current bubble width. We keep reading the same line while pages arrive above the viewport.".repeat(8) } else { String::new() }), entities, send_state: SendState::Sent, reactions: demo_reactions(id) };
        // A few service messages, drawn as centred pills.
        // Slot 3 of every 20 has no demo attachment (see media_page), so pills never displace one.
        if id % 20 == 3 {
            let name = if outgoing { "You" } else { NAMES[(sender_id - 1000) as usize] };
            message.text = if id % 2 == 0 { format!("{name} pinned a message") } else { format!("{name} joined the group via invite link") };
            message.entities.clear();
            message.reply = None;
            message.forwarded = None;
            message.is_text = false;
            message.service = true;
            message.reactions.clear();
        }
        message
    }).collect();
    if newer {
        Event::NewerMessagesLoaded {
            chat,
            after: before,
            next_after: (start <= end).then_some(MessageId(end * 1048576)),
            messages,
        }
    } else {
        Event::MessagesLoaded {
            chat,
            before,
            next_before: (start <= end).then_some(MessageId(start * 1048576)),
            messages,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_history_has_twenty_thousand_messages_and_realistic_paged_cursors() {
        let Event::MessagesLoaded {
            messages,
            next_before,
            ..
        } = page(ChatId(1), MessageId(0), false)
        else {
            panic!("older page");
        };
        assert_eq!(messages.len(), 50);
        assert_eq!(messages[0].id, MessageId(20971520000));
        assert_eq!(next_before, Some(MessageId(20920139776)));
        let Event::MessagesLoaded {
            messages,
            next_before,
            ..
        } = page(ChatId(1), MessageId(20920139776), false)
        else {
            panic!("older page");
        };
        assert_eq!(messages[0].id, MessageId(20919091200));
        assert_eq!(messages.len(), 50);
        assert!(next_before.is_some());
        let Event::NewerMessagesLoaded {
            messages,
            next_after,
            ..
        } = page(ChatId(1), MessageId(20920139776), true)
        else {
            panic!("newer page");
        };
        assert_eq!(messages.len(), 49);
        assert_eq!(next_after, Some(MessageId(20971520000)));
        let Event::MessagesLoaded {
            messages,
            next_before,
            ..
        } = page(ChatId(1), MessageId(1048576), false)
        else {
            panic!("empty page");
        };
        assert!(messages.is_empty());
        assert_eq!(next_before, None);
        let Event::NewerMessagesLoaded {
            messages,
            next_after,
            ..
        } = page(ChatId(1), MessageId(20971520000), true)
        else {
            panic!("empty page");
        };
        assert!(messages.is_empty());
        assert_eq!(next_after, None);
    }
}

#[derive(Default)]
struct Changes {
    // Demo-only mutation overlay; 512 edited/sent/deleted IDs, never the full history.
    entries: std::collections::BTreeMap<(ChatId, MessageId), Option<Message>>,
}
impl Changes {
    fn apply(&self, event: &mut Event) {
        let (chat, cursor, newer, messages, next) = match event {
            Event::MessagesLoaded {
                chat,
                before,
                messages,
                next_before,
            } => (*chat, *before, false, messages, next_before),
            Event::NewerMessagesLoaded {
                chat,
                after,
                messages,
                next_after,
            } => (*chat, *after, true, messages, next_after),
            _ => return,
        };
        messages.retain(|m| m.id.0 <= 20000 * 1048576);
        for m in messages.iter_mut() {
            if let Some(Some(replacement)) = self.entries.get(&(chat, m.id)) {
                *m = replacement.clone();
            }
        }
        messages.retain(|m| !matches!(self.entries.get(&(chat, m.id)), Some(None)));
        for ((c, id), m) in &self.entries {
            if *c != chat || id.0 <= 20000 * 1048576 {
                continue;
            }
            let in_page = if newer {
                *id > cursor && id.0 <= cursor.0 + 49 * 1048576
            } else {
                cursor.0 == 0 || (*id < cursor && id.0 >= cursor.0 - 50 * 1048576)
            };
            if in_page && let Some(m) = m {
                messages.push(m.clone());
            }
        }
        messages.sort_by_key(|m| m.id);
        if messages.len() > 50 {
            if newer {
                messages.truncate(50)
            } else {
                messages.drain(..messages.len() - 50);
            }
        }
        if let Some(id) = if newer {
            messages.last()
        } else {
            messages.first()
        }
        .map(|m| m.id)
        {
            *next = Some(id);
        }
        messages.reverse();
    }
}
/// Synthetic reactions on some messages: one chip, a chosen pair, or a full row that wraps.
fn demo_reactions(id: i64) -> Vec<tg_core::Reaction> {
    let r = |emoji: &str, count: i32, chosen: bool| tg_core::Reaction {
        emoji: emoji.into(),
        count,
        chosen,
    };
    match id % 12 {
        2 => vec![r("👍", (id % 7 + 1) as i32, false)],
        5 => vec![r("\u{2764}", 2, true), r("🔥", 1, false)],
        9 if id % 24 == 9 => vec![
            r("👍", 1234, false),
            r("😂", 3, false),
            r("👏", 1, true),
            r("🎉", 12, false),
            r("🔥", 25_000, false),
        ],
        _ => Vec::new(),
    }
}

/// Local toggle with the non-Premium limit of one chosen reaction, as TDLib applies it.
fn toggle_reaction(reactions: &mut Vec<tg_core::Reaction>, wanted: &str) {
    let was_chosen = reactions
        .iter()
        .any(|r| r.chosen && tg_core::same_reaction(&r.emoji, wanted));
    for r in reactions.iter_mut().filter(|r| r.chosen) {
        r.chosen = false;
        r.count -= 1;
    }
    if !was_chosen {
        if let Some(r) = reactions
            .iter_mut()
            .find(|r| tg_core::same_reaction(&r.emoji, wanted))
        {
            r.chosen = true;
            r.count += 1;
        } else if reactions.len() < tg_core::MAX_REACTIONS {
            reactions.push(tg_core::Reaction {
                emoji: wanted.into(),
                count: 1,
                chosen: true,
            });
        }
    }
    reactions.retain(|r| r.count > 0);
}

fn action_error() -> tg_core::CommandError {
    tg_core::CommandError {
        code: -4,
        message: "Demo action unavailable or 512 local changes exceeded.".into(),
    }
}
fn quote(m: &Message) -> tg_core::Reply {
    tg_core::Reply {
        resolved: true,
        chat: m.chat,
        message: m.id,
        sender_id: m.sender_id,
        sender: m.sender.clone().unwrap_or_else(|| {
            if m.outgoing {
                "You".into()
            } else {
                "Message".into()
            }
        }),
        preview: bounded_preview(&m.text),
    }
}
fn bounded_preview(text: &str) -> String {
    let mut s = text.replace(['\r', '\n'], " ");
    let mut end = s.len().min(256);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}
fn synthetic(chat: ChatId, id: MessageId, text_only: bool) -> Option<Message> {
    if id.0 <= 0 || id.0 > 20000 * 1048576 {
        return None;
    }
    if let Event::MessagesLoaded { messages, .. } =
        media_page(chat, MessageId(id.0 + 1048576), false, text_only)
    {
        messages.into_iter().find(|m| m.id == id)
    } else {
        None
    }
}
fn local_message(chat: ChatId, sequence: i64, text: String) -> Message {
    Message {
        attachment: None,
        video: None,
        reply: None,
        forwarded: None,
        edited: false,
        is_text: true,
        service: false,
        permissions: None,
        photo: None,
        id: MessageId(sequence * 1048576),
        chat,
        date: BASE_DATE + sequence as i32 * 60,
        outgoing: true,
        text,
        entities: vec![],
        send_state: SendState::Sent,
        album: 0,
        sender_id: 0,
        sender: None,
        reactions: Vec::new(),
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    #[test]
    fn demo_reactions_toggle_with_one_chosen_reaction() {
        let mut reactions = demo_reactions(5);
        // The chosen heart is taken back; the picker's spelling carries U+FE0F.
        toggle_reaction(&mut reactions, "\u{2764}\u{FE0F}");
        assert!(reactions.iter().all(|r| !r.chosen));
        assert_eq!(reactions[0].count, 1);
        // A new reaction replaces nothing chosen and appears with one vote.
        toggle_reaction(&mut reactions, "🎉");
        assert!(
            reactions
                .iter()
                .any(|r| r.emoji == "🎉" && r.chosen && r.count == 1)
        );
        // Choosing another one drops the previous choice; an emptied chip disappears.
        toggle_reaction(&mut reactions, "🔥");
        assert!(reactions.iter().all(|r| r.emoji != "🎉"));
        assert!(
            reactions
                .iter()
                .any(|r| r.emoji == "🔥" && r.chosen && r.count == 2)
        );
        assert!(demo_reactions(1).is_empty());
    }
    #[test]
    fn local_changes_survive_reopening_and_deleted_rows_stay_deleted() {
        let mut changes = Changes::default();
        let id = MessageId(19998 * 1048576);
        let mut m = synthetic(ChatId(1), id, true).unwrap();
        m.text = "Edited locally".into();
        m.edited = true;
        changes.entries.insert((ChatId(1), id), Some(m));
        changes
            .entries
            .insert((ChatId(1), MessageId(19999 * 1048576)), None);
        let mut event = media_page(ChatId(1), MessageId(0), false, true);
        changes.apply(&mut event);
        let Event::MessagesLoaded { messages, .. } = event else {
            panic!("page")
        };
        assert!(
            messages
                .iter()
                .any(|m| m.id == id && m.edited && m.text == "Edited locally")
        );
        assert!(!messages.iter().any(|m| m.id == MessageId(19999 * 1048576)));
    }
}

fn demo_attachment(m: &mut Message, kind: i64) {
    use tg_core::{Attachment, Document, MediaFile, Photo, Sticker, Voice};
    let n = m.id.0 / 1048576;
    m.is_text = false;
    m.album = 0;
    m.photo = None;
    m.video = None;
    m.text.clear();
    m.entities.clear();
    let file = |base| MediaFile {
        partial: None,
        id: base + n as i32,
        path: None,
        size: 0,
        downloaded: 0,
        downloading: false,
    };
    m.attachment = Some(match kind {
        15 | 16 => {
            m.photo = Some(demo_photo((100000 + n) as i32));
            let mut f = file(VIDEO_FILE_ID);
            f.size = include_bytes!("../assets/demo-round.mp4").len() as u64;
            Attachment::Round(tg_core::Round {
                file: f,
                duration: 12,
                length: 240,
                viewed: kind == 15,
            })
        }
        19 => Attachment::Emoji(Sticker {
            emoji: "\u{1f642}".into(),
            width: 128,
            height: 128,
            animated: true,
        }),
        4 => {
            let mut f = file(2_000_000);
            f.size = include_bytes!("../assets/demo-voice.ogg").len() as u64;
            Attachment::Voice(Voice {
                file: f,
                duration: 12,
                waveform: (0..64).map(|n| ((n * 13 + 7) % 32) as u8).collect(),
                listened: n % 40 == 4,
            })
        }
        8 | 17 | 18 => {
            let name = match kind {
                17 => "demo-script.ps1",
                18 => "demo-file.pdf",
                _ => "demo-file.txt",
            };
            let mut f = file(3_000_000);
            let (_, size) = demo_file(std::path::Path::new("unused.jpg"), f.id);
            f.size = size;
            if kind == 8 {
                m.text = "A locally generated document with a caption.".into();
            }
            Attachment::Document(Document {
                file: f,
                name: name.into(),
            })
        }
        _ => {
            if kind != 14 {
                m.photo = Some(Photo {
                    file: file(4_000_000),
                    width: 192,
                    height: 192,
                    minithumbnail: vec![],
                });
            }
            Attachment::Sticker(Sticker {
                emoji: "\u{1f916}".into(),
                width: 192,
                height: 192,
                animated: kind == 14,
            })
        }
    });
}
fn demo_file(base: &std::path::Path, id: i32) -> (std::path::PathBuf, u64) {
    let (extension, size) = if id >= 4_000_000 {
        ("webp", include_bytes!("../assets/demo-sticker.webp").len())
    } else if id >= 3_000_000 {
        match (id - 3_000_000) % 20 {
            17 => ("ps1", include_bytes!("../assets/demo-script.ps1").len()),
            18 => ("pdf", include_bytes!("../assets/demo-file.pdf").len()),
            _ => ("txt", include_bytes!("../assets/demo-file.txt").len()),
        }
    } else if id >= 2_000_000 {
        ("ogg", include_bytes!("../assets/demo-voice.ogg").len())
    } else if id >= VIDEO_FILE_ID && matches!((id - VIDEO_FILE_ID) % 20, 15 | 16) {
        (
            "round.mp4",
            include_bytes!("../assets/demo-round.mp4").len(),
        )
    } else if id >= VIDEO_FILE_ID {
        ("mp4", include_bytes!("../assets/demo-video.mp4").len())
    } else {
        ("jpg", include_bytes!("../assets/demo-photo.jpg").len())
    };
    (base.with_extension(extension), size as u64)
}

#[cfg(test)]
mod round_tests {
    use super::*;
    #[test]
    fn round_messages_preserve_every_member_of_existing_demo_albums() {
        let Event::MessagesLoaded { messages, .. } =
            media_page(ChatId(1), MessageId(20000 * 1048576), false, false)
        else {
            panic!("page")
        };
        for (first, last) in [(19981, 19982), (19985, 19987), (19990, 19993)] {
            for n in first..=last {
                let m = messages
                    .iter()
                    .find(|m| m.id == MessageId(n * 1048576))
                    .unwrap();
                assert_eq!(
                    m.album, first,
                    "round messages must not split synthetic albums"
                );
                assert!(m.photo.is_some() && m.attachment.is_none());
            }
        }
    }
}

struct VideoSlot(Arc<AtomicUsize>);
impl Drop for VideoSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
