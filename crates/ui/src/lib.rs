// The sole Slint boundary. tg-app sees domain commands/events and plain options only.
mod actions;
mod attachments;
mod bridge;
mod check;
mod clock;
mod history;
mod media;
mod models;
mod outgoing;
mod reactions;
pub use outgoing::AttachmentSources;
mod scale;
mod streaming;
mod text;
mod viewport;
pub use bridge::EventSink;
use models::{CHAT_AVATAR, CHAT_ROW, Chats, Feed};
use slint::winit_030::{WinitWindowAccessor, winit};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, SharedPixelBuffer};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};
use tg_core::{
    AuthState, Command, CommandError, CommandKind, Event, MessageId, Notification, ReadBatches,
    Secret, VideoRect, VideoRequest,
};
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
// Slint-generated code contains unwrap/todo and handles without Debug.
// Exceptions apply only to the generated module, never hand-written code.
mod generated {
    #![allow(missing_debug_implementations, clippy::unwrap_used, clippy::todo)]
    slint::include_modules!();
}
use generated::{
    AppWindow, ChatRow, MediaViewer, MessageRow, PhotoCell, QuickReaction, ReactionChip, Theme,
};

#[derive(Debug, Default)]
pub struct Options {
    pub oled: bool,
    pub demo: bool,
    pub snapshot: Option<PathBuf>,
    pub demo_scroll: Option<u32>,
    /// Demo only: open the newest synthetic video after startup and exit after this many seconds.
    pub demo_video: Option<u32>,
    pub demo_voice: Option<u32>,
    pub demo_round: Option<u32>,
    pub demo_stickers: bool,
    /// Write a JSON account of the run here at exit (smoke tests). See `check.rs`.
    pub check: Option<PathBuf>,
    /// Loaded off-thread by tg-app. At most 32 MiB, never bundled.
    pub emoji_font: Option<Vec<u8>>,
    /// The stored interface scale (`tg_core::SCALE_AUTO` or a percentage), shown in its dialog.
    pub interface_scale: u16,
}
type NotifyFn = Box<dyn Fn(Notification) -> Result<(), CommandError>>;
type VideoFn = Box<dyn Fn(VideoRequest) -> Result<(), CommandError>>;
const OPEN_LINK_DIALOG: i32 = 5;

fn close_link_confirmation(window: &AppWindow) {
    window.set_link_target(Default::default());
    window.set_action_dialog(0);
}

struct State {
    outgoing: outgoing::Outgoing,
    check: check::Check,
    attachments: attachments::Attachments,
    actions: actions::Actions,
    quick: reactions::Quick,
    media: Rc<RefCell<media::Media>>,
    chats: Rc<Chats>,
    feed: Rc<Feed>,
    command: Box<dyn Fn(Command) -> Result<(), CommandError>>,
    loading_chats: Cell<bool>,
    refreshing: Cell<bool>,
    focused: Cell<bool>,
    reads: RefCell<ReadBatches>,
    notifications: RefCell<NotifyFn>,
    video: RefCell<VideoFn>,
    /// The video in the player: its TDLib file ID and the local path once downloaded.
    playing: RefCell<Option<(i32, Option<PathBuf>)>>,
    streaming: RefCell<Option<streaming::ViewerStream>>,
    stream_serial: Cell<u64>,
    viewer: MediaViewer,
    /// Feed position (from the top) that wheel scrolling eases towards, if any.
    scroll_target: Cell<Option<f32>>,
    scroll_timer: slint::Timer,
    total_unread: Cell<Option<u32>>,
    // Seek the actual oldest edge for an entirely unread chat (TDLib has no oldest sentinel).
    seek_start: Cell<bool>,
    demo_stickers: Cell<bool>,
    /// Stores a new interface scale; set by tg-app with `Ui::on_interface_scale`.
    scale: RefCell<Option<scale::ScaleFn>>,
    scale_applied: Cell<u16>,
}
impl State {
    /// Physical-pixel area of the media viewer above its control bar.
    fn video_rect(viewer: &MediaViewer) -> VideoRect {
        let size = viewer.window().size();
        let bar = (viewer.get_bar() * viewer.window().scale_factor()).round() as i32;
        VideoRect {
            x: 0,
            y: 0,
            width: i32::try_from(size.width).unwrap_or(i32::MAX).max(1),
            height: (i32::try_from(size.height).unwrap_or(i32::MAX) - bar).max(1),
        }
    }
    fn video(&self, window: &AppWindow, request: VideoRequest) {
        let streaming_request = matches!(
            &request,
            VideoRequest::OpenStream { .. } | VideoRequest::Availability { .. }
        );
        let inline = matches!(
            &request,
            VideoRequest::OpenRound { .. }
                | VideoRequest::RoundSurface { .. }
                | VideoRequest::Move(_)
        );
        if let Err(error) = (self.video.borrow())(request) {
            if streaming_request {
                self.close_video(window);
            }
            if inline {
                self.stop_voice(window);
            }
            self.viewer.set_loading(false);
            self.viewer.set_failed(true);
            window.set_error(error.message.into());
        }
    }
    fn start_video(&self, window: &AppWindow, path: PathBuf) {
        self.viewer.set_loading(false);
        let Some(handle) = native_window(self.viewer.window()) else {
            self.viewer.set_failed(true);
            return;
        };
        let rect = Self::video_rect(&self.viewer);
        self.video(
            window,
            VideoRequest::Open {
                path,
                window: handle,
                rect,
            },
        );
    }
    fn open_video(&self, window: &AppWindow, row: usize, cell: usize) {
        if let Some(video) = self.feed.video_at(row, cell) {
            self.play(window, video);
        }
    }
    fn play(&self, window: &AppWindow, video: tg_core::Video) {
        self.stop_voice(window);
        self.close_video(window);
        self.viewer.set_failed(false);
        self.viewer.set_playing(false);
        self.viewer.set_progress(0.0);
        self.viewer.set_download_start(0.0);
        self.viewer.set_download_end(0.0);
        self.viewer
            .set_time(format!("0:00 / {}", clock::duration(video.duration)).into());
        // The native window must exist before the player can attach its child window.
        if self.viewer.show().is_err() {
            window.set_error("Could not open the media viewer.".into());
            return;
        }
        if let Some(viewer) = native_window(self.viewer.window()) {
            let bar = (self.viewer.get_bar() * self.viewer.window().scale_factor()).round() as i32;
            self.video(
                window,
                VideoRequest::Present {
                    viewer,
                    width: u32::from(video.width),
                    height: u32::from(video.height),
                    bar,
                },
            );
        }
        *self.playing.borrow_mut() = Some((video.file.id, video.file.path.clone()));
        if video.supports_streaming && video.file.path.is_none() && video.file.size > 0 {
            self.start_stream(window, &video);
        } else if let Some(path) = video.file.path {
            self.start_video(window, path);
        } else {
            self.viewer.set_loading(true);
            // Downloaded completely before playback; streaming is a later step.
            if !self.send(window, Command::DownloadFile(video.file.id)) {
                self.viewer.set_loading(false);
                self.viewer.set_failed(true);
            }
        }
    }
    fn close_video(&self, window: &AppWindow) {
        let streaming = self.streaming.borrow_mut().take().is_some();
        if let Some((id, path)) = self.playing.borrow_mut().take() {
            if streaming || path.is_none() {
                self.send(window, Command::CancelDownload(id));
            }
            self.video(window, VideoRequest::Close);
        }
        let _ = self.viewer.hide();
        self.viewer.set_loading(false);
    }
    /// Wheel and touchpad scrolling: the target position eases in on a short-lived timer, which
    /// runs only while a scroll is in progress. `refresh` moves the target together with the
    /// content when pages are added or dropped, so the feed never overshoots.
    fn wheel(self: &Rc<Self>, window: &AppWindow, delta: f32) {
        let max = (window.get_feed_content_height() - window.get_feed_height()).max(0.0);
        let current = -window.get_feed_y();
        let target = (self.scroll_target.get().unwrap_or(current) - delta).clamp(0.0, max);
        self.scroll_target.set(Some(target));
        if self.scroll_timer.running() {
            return;
        }
        let weak = window.as_weak();
        let state = Rc::downgrade(self);
        self.scroll_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let (Some(w), Some(s)) = (weak.upgrade(), state.upgrade()) else {
                    return;
                };
                let Some(target) = s.scroll_target.get() else {
                    s.scroll_timer.stop();
                    return;
                };
                let current = -w.get_feed_y();
                let distance = target - current;
                if distance.abs() < 0.5 {
                    s.scroll_target.set(None);
                    s.scroll_timer.stop();
                    w.set_feed_y(-target);
                } else {
                    // About 120 ms to cover most of the distance, as smooth wheel scrolling does.
                    w.set_feed_y(-(current + distance * 0.35));
                }
            },
        );
    }
    fn send(&self, window: &AppWindow, command: Command) -> bool {
        if window.get_blocked() {
            return false;
        }
        match (self.command)(command) {
            Ok(()) => {
                window.set_error("".into());
                true
            }
            Err(error) => {
                window.set_error(error.message.into());
                false
            }
        }
    }
    fn refresh(&self, window: &AppWindow) {
        if self.refreshing.replace(true) {
            return;
        }
        if self.feed.jump_target.get().is_some_and(|id| {
            self.feed
                .history
                .borrow()
                .messages
                .iter()
                .any(|m| m.id == id)
        }) {
            self.start_highlight(window);
        }
        let height = window.get_feed_height().max(1.0);
        let requested = (-window.get_feed_y()).max(0.0);
        let (top, total) = self
            .feed
            .layout(requested, height, window.get_follow_tail());
        if let Some(target) = self.scroll_target.get() {
            // Rows added or dropped above moved the content: move the wheel target with it.
            let max = (total - height).max(0.0);
            self.scroll_target
                .set(Some((target + top - requested).clamp(0.0, max)));
        }
        window.set_feed_content_height(total);
        window.set_feed_y(-top);
        window.set_at_newest(self.feed.history.borrow().at_tail());
        self.refreshing.set(false);
        self.prefetch(window, top, height, total);
        self.sync_media(window);
        self.sync_round(window);
        self.view_visible(window);
        self.query_properties(window);
    }
    fn view_visible(&self, window: &AppWindow) {
        if !window.get_ready()
            || !window.get_chat_open()
            || !self.focused.get()
            || self.seek_start.get()
        {
            return;
        }
        let history = self.feed.history.borrow();
        let Some(chat) = history.chat else {
            return;
        };
        let read = self
            .chats
            .rows
            .borrow()
            .iter()
            .find(|c| c.id == chat)
            .map_or(MessageId(0), |c| c.last_read_inbox);
        drop(history);
        let top = (-window.get_feed_y()).max(0.0)
            - (window.get_feed_height() - window.get_feed_content_height()).max(0.0);
        let ids = self.feed.visible_incoming(top, window.get_feed_height());
        let batch = self.reads.borrow_mut().begin(chat, read, ids);
        if let Some(messages) = batch
            && !self.send(window, Command::ViewMessages { chat, messages })
        {
            self.reads.borrow_mut().complete(chat, false);
        }
    }
    fn unread_title(&self, window: &AppWindow) {
        let rows = self.chats.rows.borrow();
        let total = self.total_unread.get().unwrap_or_else(|| {
            rows.iter()
                .fold(0u32, |n, c| n.saturating_add(c.unread_count))
        });
        window.set_window_title(if total == 0 {
            "Veygram".into()
        } else {
            format!("Veygram ({total})").into()
        });
        let unread = rows
            .iter()
            .find(|c| Some(c.id) == self.chats.selected.get())
            .map_or(0, |c| c.unread_count);
        window.set_open_unread(
            if unread == 0 {
                String::new()
            } else {
                unread.to_string()
            }
            .into(),
        );
    }
    fn sync_media(&self, window: &AppWindow) {
        let mut wanted = if window.get_ready() {
            self.feed.media_requests()
        } else {
            Vec::new()
        };
        let rows = self.chats.rows.borrow();
        let first = ((-window.get_chat_y()).max(0.0) / CHAT_ROW).floor() as usize;
        let count = (window.get_chat_height().max(1.0) / CHAT_ROW).ceil() as usize + 2;
        self.chats.scale.set(self.feed.scale.get());
        for chat in rows
            .iter()
            // Three spare rows on each side: avatars are small and appear before they scroll in.
            .skip(first.saturating_sub(3))
            .take((count + 4).min(40))
        {
            if window.get_ready()
                && let Some(photo) = &chat.avatar
            {
                wanted.push((
                    media::Key::new(photo, CHAT_AVATAR, CHAT_AVATAR, self.feed.scale.get()),
                    photo.clone(),
                ));
            }
        }
        if window.get_action_dialog() == 2 {
            let first = ((-window.get_destination_y()).max(0.0) / 52.0).floor() as usize;
            for index in first.saturating_sub(1)..first.saturating_add(9) {
                if let Some(i) = self.actions.destinations.index(index)
                    && let Some(photo) = rows.get(i).and_then(|c| c.avatar.as_ref())
                {
                    wanted.push((
                        media::Key::new(photo, CHAT_AVATAR, CHAT_AVATAR, self.feed.scale.get()),
                        photo.clone(),
                    ));
                }
            }
        }
        let selected = rows
            .iter()
            .find(|c| window.get_ready() && Some(c.id) == self.chats.selected.get());
        if let Some(photo) = selected.and_then(|c| c.avatar.as_ref()) {
            wanted.push((
                media::Key::new(photo, HEADER_AVATAR, HEADER_AVATAR, self.feed.scale.get()),
                photo.clone(),
            ));
        }
        let commands = self.media.borrow_mut().sync(wanted);
        let avatar = selected
            .and_then(|c| c.avatar.as_ref())
            .map(|p| {
                self.media.borrow_mut().image(media::Key::new(
                    p,
                    HEADER_AVATAR,
                    HEADER_AVATAR,
                    self.feed.scale.get(),
                ))
            })
            .unwrap_or_default();
        window.set_chat_avatar(avatar);
        window.set_chat_initials(
            selected
                .map(|c| models::initials(&c.title))
                .unwrap_or_default()
                .into(),
        );
        if let Some(chat) = selected {
            window.set_chat_color(models::peer_color(chat.id.0));
        }
        drop(rows);
        for command in commands {
            self.send(window, command);
        }
    }
    fn prefetch(&self, window: &AppWindow, top: f32, height: f32, total: f32) {
        let (older, newer, chat) = {
            let mut h = self.feed.history.borrow_mut();
            let Some(chat) = h.chat else {
                return;
            };
            let geometry = self.feed.geometry.borrow();
            let first = geometry
                .rows
                .iter()
                .scan(0.0, |y, row| {
                    *y += row.1;
                    Some(*y)
                })
                .position(|y| y > top)
                .unwrap_or(0);
            // Never evict a visible row, including with both directions outstanding.
            let older_safe = h.messages.len() < 250 || first + 100 < h.messages.len();
            let newer_safe = h.messages.len() < 250 || first >= 100;
            let older = if h.latest_requested
                || self.seek_start.get()
                || (top <= 2.0 * height && older_safe)
            {
                h.request_older()
            } else {
                None
            };
            let newer = if total - top - height <= 2.0 * height && newer_safe {
                h.request_newer()
            } else {
                None
            };
            (older, newer, chat)
        };
        if let Some(before) = older
            && !self.send(window, Command::LoadOlderMessages { chat, before })
        {
            self.feed.history.borrow_mut().pending_older = None;
        }
        if let Some(after) = newer
            && !self.send(window, Command::LoadNewerMessages { chat, after })
        {
            self.feed.history.borrow_mut().pending_newer = None;
        }
    }

    fn open(&self, window: &AppWindow, index: usize) {
        let Some(chat) = self.chats.rows.borrow().get(index).cloned() else {
            return;
        };
        if self.chats.selected.get() == Some(chat.id) {
            return;
        }
        if let Some(old) = self.chats.selected.get() {
            self.send(window, Command::CloseChat(old));
        }
        let unread = chat.unread_count > 0;
        let command = if unread {
            Command::OpenChatAt {
                chat: chat.id,
                anchor: chat.last_read_inbox,
            }
        } else {
            Command::OpenChat(chat.id)
        };
        if !self.send(window, command) {
            return;
        }
        if self.round_active() {
            self.feed.audio.set(None);
        } else {
            self.stop_voice(window);
        }
        self.cancel_action(window);
        self.cancel_outgoing(window);
        self.feed.jump_target.set(None);
        self.feed.highlight.set(None);
        self.seek_start.set(unread && chat.last_read_inbox.0 == 0);
        self.chats.selected.set(Some(chat.id));
        self.chats.notify.reset();
        if unread {
            self.feed
                .history
                .borrow_mut()
                .open_around(chat.id, chat.last_read_inbox);
        } else {
            self.feed.history.borrow_mut().open(chat.id);
        }
        self.feed
            .unread_after
            .set(unread.then_some(chat.last_read_inbox));
        self.feed.jump_unread.set(unread && !self.seek_start.get());
        self.feed.read_outbox.set(chat.last_read_outbox);
        self.feed.invalidate();
        *self.feed.geometry.borrow_mut() = Default::default();
        window.set_follow_tail(!unread);
        window.set_chat_open(true);
        self.refresh(window);
        self.unread_title(window);
        window.set_chat_title(chat.title.into());
        window.set_draft("".into());
    }
    fn event(&self, window: &AppWindow, event: Event) {
        if let Event::VideoState(state) = &event {
            self.check.record(state);
        }
        match event {
            Event::LocalFileSendFinished(path) => self.upload_finished(&path),
            Event::RoundFrame(frame) => self.round_frame(window, frame),
            Event::MessageReactionsChanged {
                chat,
                message,
                reactions,
            } => self.reactions_changed(window, chat, message, reactions),
            event @ (Event::MessageProperties { .. }
            | Event::MessageContentChanged { .. }
            | Event::MessageEdited { .. }
            | Event::MessagesDeleted { .. }
            | Event::ReplyResolved { .. }) => self.action_event(window, event),
            Event::NotificationActivated(chat) => {
                let index = self.chats.rows.borrow().iter().position(|c| c.id == chat);
                if let Some(index) = index {
                    self.open(window, index);
                    let _ = window.show();
                    window.window().with_winit_window(|w| {
                        w.set_minimized(false);
                        w.focus_window();
                    });
                }
            }
            Event::TotalUnreadCount(total) => {
                self.total_unread.set(Some(total));
                self.unread_title(window);
            }
            Event::MessagesViewed { chat, success } => {
                self.reads.borrow_mut().complete(chat, success);
                if success {
                    self.view_visible(window);
                } else {
                    window.set_error(
                        "Could not update read state. Scroll or refocus to retry.".into(),
                    );
                }
            }
            Event::MessagesAroundLoaded {
                chat,
                anchor,
                messages,
            } => {
                if self
                    .feed
                    .history
                    .borrow_mut()
                    .around(chat, messages, anchor)
                {
                    if let Some(target) = self.feed.jump_target.get()
                        && !self
                            .feed
                            .history
                            .borrow()
                            .messages
                            .iter()
                            .any(|m| m.id == target)
                    {
                        self.feed.jump_target.set(None);
                        self.feed.highlight.set(None);
                        window.set_error("The original message is unavailable.".into());
                    }
                    self.feed.invalidate();
                    self.refresh(window);
                }
            }
            Event::IncomingMessage(message) => {
                if let Some(chat) = self
                    .chats
                    .rows
                    .borrow()
                    .iter()
                    .find(|c| c.id == message.chat)
                    && let Some(notification) = Notification::incoming(
                        chat,
                        &message,
                        self.chats.selected.get(),
                        self.focused.get(),
                        unix_now(),
                    )
                    && let Err(error) = (self.notifications.borrow())(notification)
                {
                    window.set_error(error.message.into());
                }
                self.event(window, Event::NewMessage(message));
            }
            Event::AuthStateChanged(auth) => {
                if auth == AuthState::Closed { self.outgoing.clear_uploads(); }
                window.set_auth_input("".into());
                window.set_error("".into());
                if auth != AuthState::Ready {
                    window.set_ready(false);
                    self.sync_media(window);
                }
                match auth {
                    // Phone entry first. A QR request moves TDLib to the other-device state, where
                    // it rejects phone numbers (AuthManager::set_phone_number) until a logOut, so
                    // the QR code is only requested when the user asks for it.
                    AuthState::WaitPhoneNumber => {
                        window.set_auth_step(2);
                        window.set_auth_title("Sign in to Telegram".into());
                    }
                    AuthState::WaitOtherDevice { link } => {
                        window.set_auth_step(1);
                        window.set_auth_title("Scan to sign in".into());
                        match qr_image(&link) {
                            Ok(image) => window.set_qr(image),
                            Err(()) => {
                                window.set_error("Could not generate the sign-in QR code.".into())
                            }
                        }
                    }
                    AuthState::WaitCode => {
                        window.set_auth_step(3);
                        window.set_auth_title("Enter your verification code".into());
                        window.set_qr(Default::default());
                    }
                    AuthState::WaitPassword { .. } => {
                        window.set_auth_step(4);
                        window.set_auth_title("Two-step verification".into());
                    }
                    AuthState::Ready => {
                        window.set_ready(true);
                        window.set_qr(Default::default());
                        if !self.loading_chats.replace(true)
                            && !self.send(window, Command::LoadMoreChats)
                        {
                            self.loading_chats.set(false);
                        }
                    }
                    AuthState::LoggingOut | AuthState::Closed => {
                        self.cancel_outgoing(window);
                        self.stop_voice(window);
                        self.cancel_action(window);
                        *self.reads.borrow_mut() = ReadBatches::default();
                        self.total_unread.set(None);
                        self.chats.rows.borrow_mut().clear();
                        self.chats.selected.set(None);
                        self.chats.notify.reset();
                        *self.feed.history.borrow_mut() = Default::default();
                        self.feed.invalidate();
                        *self.feed.geometry.borrow_mut() = Default::default();
                        self.feed.notify.reset();
                        window.set_chat_open(false);
                        self.unread_title(window);
                        window.set_draft("".into());
                        window.set_qr(Default::default());
                        window.set_auth_step(0);
                        window.set_auth_title("Telegram session closed".into());
                    }
                    AuthState::Unsupported => {
                        window.set_auth_step(0);
                        window.set_error("This sign-in step is not supported.".into());
                    }
                    _ => {
                        window.set_auth_step(0);
                        window.set_auth_title("Connecting to Telegram...".into());
                    }
                }
            }
            Event::FileUpdated(file) => {
                self.attachment_file(window,&file);
                if self.stream_file(window,&file){return;}
                let ready = match self.playing.borrow_mut().as_mut() {
                    Some((id, path)) if *id == file.id && path.is_none() && file.path.is_some() => {
                        path.clone_from(&file.path);
                        path.clone()
                    }
                    _ => None,
                };
                self.media.borrow_mut().file(file);
                if let Some(path) = ready
                    && self.viewer.window().is_visible()
                {
                    self.start_video(window, path);
                }
            }
            Event::MessageContentOpened {chat,message}=>self.content_opened(chat,message),
            Event::FileDownloadFailed(id)=>{
                let streaming=self.streaming.borrow().as_ref().is_some_and(|s|s.file.id==id);
                if streaming {
                    self.close_video(window);
                    window.set_error("Could not download the video.".into());
                } else { self.download_failed(window,id); }
            },
            Event::FileOperationFailed=>window.set_error("Could not open the link or file, or locate the file. Downloaded files require internet-zone marking.".into()),
            Event::VideoState(state) if state.stream.is_some()=>self.stream_status(window,state.stream.unwrap_or_default()),
            Event::VideoState(state) if state.audio_token.is_some()=>self.voice_state(window,state),
            Event::VideoState(state) if state.dismissed => self.close_video(window),
            Event::VideoState(state) => {
                if state.failed && self.streaming.borrow().as_ref().is_some_and(|s|Some(s.token)==state.stream_token) {
                    self.close_video(window);window.set_error("This video cannot be streamed.".into());return;
                }
                if self.viewer.window().is_visible() && self.stream_playback(window, state) {
                    let duration = state.duration_ms.max(1);
                    self.viewer.set_playing(state.playing);
                    self.viewer.set_failed(state.failed);
                    if state.duration_ms>0 {
                    self.viewer
                        .set_progress(state.position_ms as f32 / duration as f32);
                    self.viewer.set_time(
                        format!(
                            "{} / {}",
                            clock::duration(state.position_ms / 1000),
                            clock::duration(state.duration_ms / 1000)
                        )
                        .into(),
                    );
                    }
                }
            }
            Event::ChatListChanged(change) => {
                self.chats.apply(change);
                self.actions.destinations.notify.reset();
                self.unread_title(window);
                if self.demo_stickers.get() && self.chats.rows.borrow().iter().any(|c|c.id.0==5) {
                    self.demo_stickers.set(false);self.open(window,4);
                }
                if window.get_demo() && self.chats.selected.get().is_none() {
                    self.open(window, 0);
                }
                let mut read_changed = false;
                if let Some(id) = self.chats.selected.get()
                    && let Some(chat) = self.chats.rows.borrow().iter().find(|chat| chat.id == id)
                {
                    window.set_chat_title(chat.title.as_str().into());
                    read_changed = self.feed.read_outbox.replace(chat.last_read_outbox)
                        != chat.last_read_outbox;
                }
                if read_changed {
                    // Rows compare their context, so only rows whose read state changed redraw.
                    self.refresh(window);
                }
                self.sync_media(window);
            }
            Event::ChatsLoaded { has_more } => {
                self.loading_chats.set(false);
                window.set_more_chats(has_more);
                if has_more && window.get_chat_near_end() {
                    window.invoke_load_chats();
                }
            }
            Event::MessagesLoaded {
                chat,
                before,
                next_before,
                messages,
            } => {
                let accepted =
                    self.feed
                        .history
                        .borrow_mut()
                        .page(chat, before, next_before, messages, false);
                if accepted {
                    if self.seek_start.get() && next_before.is_none() {
                        self.seek_start.set(false);
                        self.feed.jump_unread.set(true);
                    }
                    if before == MessageId(0) && self.feed.unread_after.get().is_none() {
                        window.set_follow_tail(true);
                        self.feed.invalidate();
                    }
                    self.refresh(window);
                }
            }
            Event::NewerMessagesLoaded {
                chat,
                after,
                next_after,
                messages,
            } => {
                if self
                    .feed
                    .history
                    .borrow_mut()
                    .page(chat, after, next_after, messages, true)
                {
                    if next_after.is_none()
                        && -window.get_feed_y() + window.get_feed_height()
                            >= window.get_feed_content_height() - 2.0
                    {
                        window.set_follow_tail(true);
                    }
                    self.refresh(window);
                }
            }
            Event::NewMessage(message) => {
                self.finish_edit(window, message.chat, message.id);
                self.feed.invalidate_message(message.id);
                self.feed
                    .history
                    .borrow_mut()
                    .upsert(None, message, window.get_follow_tail());
                self.refresh(window);
            }
            Event::MessageUpdated { old_id, message } => {
                self.feed.invalidate_message(old_id);
                self.feed.invalidate_message(message.id);
                self.feed.history.borrow_mut().upsert(
                    Some(old_id),
                    message,
                    window.get_follow_tail(),
                );
                self.refresh(window);
            }
            Event::CommandFailed { command, error } => {
                if command == CommandKind::EditText {
                    self.edit_failed(window);
                }
                if command == CommandKind::LoadMoreChats {
                    self.loading_chats.set(false);
                }
                if matches!(
                    command,
                    CommandKind::OpenChat
                        | CommandKind::LoadOlderMessages
                        | CommandKind::LoadNewerMessages
                ) {
                    let mut history = self.feed.history.borrow_mut();
                    if command == CommandKind::LoadNewerMessages {
                        history.pending_newer = None;
                    } else {
                        history.pending_older = None;
                    }
                }
                // Command kind and numeric code carry no user data and make reports useful.
                window.set_error(
                    format!("{} ({command:?}, code {})", error.message, error.code).into(),
                );
            }
            Event::SecurityWarning(_) => window.set_security_warning(true),
            Event::LocalDataLocked => {
                window.set_ready(false);
                window.set_auth_step(5);
                window.set_auth_title("Local data is locked".into());
            }
            _ => {}
        }
    }
}
/// Header avatar size in logical pixels; app.slint uses the same value.
const HEADER_AVATAR: f32 = 38.0;
/// The Win32 handle of a window, for the player's child window.
fn native_window(window: &slint::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
        _ => None,
    }
}
fn qr_image(link: &str) -> Result<slint::Image, ()> {
    // QR input bounded by the engine to 4 KiB. QR version 40 caps the matrix at 177?.
    let code = qrcode::QrCode::new(link).map_err(|_| ())?;
    let side = (code.width() + 8) as u32;
    let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(side, side);
    for y in 0..side {
        for x in 0..side {
            let black = x >= 4
                && y >= 4
                && x < side - 4
                && y < side - 4
                && code[((x - 4) as usize, (y - 4) as usize)] == qrcode::Color::Dark;
            let value = if black { 0 } else { 255 };
            pixels.make_mut_slice()[(y * side + x) as usize] = Rgb8Pixel::new(value, value, value);
        }
    }
    Ok(slint::Image::from_rgb8(pixels))
}
/// Owns the complete UI on its event-loop thread. No Slint types escape this crate.
pub struct Ui {
    window: AppWindow,
    sink: EventSink,
    snapshot: Option<PathBuf>,
    check: Option<PathBuf>,
    demo_scroll: Option<u32>,
    demo_video: Option<u32>,
    demo_voice: Option<u32>,
    demo_round: Option<u32>,
    state: Rc<State>,
}
impl std::fmt::Debug for Ui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ui")
    }
}
impl Ui {
    pub fn new(
        options: Options,
        command: impl Fn(Command) -> Result<(), CommandError> + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if std::env::var_os("SLINT_BACKEND").is_none() {
            slint::BackendSelector::new()
                .backend_name("winit".into())
                .renderer_name("software".into())
                .select()?;
        }
        let window = AppWindow::new()?;
        window.global::<Theme>().set_oled(options.oled);
        window.set_demo(options.demo);
        let chats = Rc::new(Chats::default());
        let feed = Feed::new(options.emoji_font);
        feed.scale.set(window.window().scale_factor());
        window.set_chats(ModelRc::from(chats.clone()));
        window.set_messages(ModelRc::from(feed.clone()));
        let media = Rc::new(RefCell::new(media::Media::new(&window)?));
        *chats.media.borrow_mut() = Some(media.clone());
        *feed.media.borrow_mut() = Some(media.clone());
        let state = Rc::new(State {
            outgoing: Default::default(),
            check: Default::default(),
            attachments: Default::default(),
            actions: actions::Actions::new(chats.clone()),
            quick: Default::default(),
            media,
            chats,
            feed,
            command: Box::new(command),
            loading_chats: Cell::new(false),
            refreshing: Cell::new(false),
            focused: Cell::new(false),
            reads: RefCell::default(),
            notifications: RefCell::new(Box::new(|_| Ok(()))),
            video: RefCell::new(Box::new(|_| {
                Err(CommandError {
                    code: -4,
                    message: "Video playback is not available.".into(),
                })
            })),
            playing: RefCell::default(),
            streaming: RefCell::default(),
            stream_serial: Cell::new(0),
            viewer: MediaViewer::new()?,
            scroll_target: Cell::new(None),
            scroll_timer: slint::Timer::default(),
            total_unread: Cell::new(None),
            seek_start: Cell::new(false),
            demo_stickers: Cell::new(options.demo_stickers),
            scale: RefCell::new(None),
            scale_applied: Cell::new(tg_core::check_scale(options.interface_scale)),
        });
        actions::bind(&window, &state);
        reactions::bind(&window, &state);
        attachments::bind(&window, &state);
        outgoing::bind(&window, &state);
        scale::bind(&window, &state);
        let s = state.clone();
        window.on_text_link(move |key, x, y| {
            s.feed
                .link_at(&key, x, y)
                .map_or_else(Default::default, |target| target.url.into())
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_text_link_clicked(move |key, x, y| {
            if let Some(w) = weak.upgrade()
                && !w.get_blocked()
                && w.get_action_dialog() == 0
                && let Some(target) = s.feed.link_at(&key, x, y)
            {
                if target.hidden {
                    w.set_link_target(target.url.into());
                    w.set_action_dialog(OPEN_LINK_DIALOG);
                } else {
                    s.file_request(&w, tg_core::FileRequest::OpenUrl(target.url));
                }
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_confirm_link(move || {
            if let Some(w) = weak.upgrade()
                && !w.get_blocked()
                && w.get_action_dialog() == OPEN_LINK_DIALOG
            {
                let target = w.get_link_target();
                close_link_confirmation(&w);
                if !target.is_empty() {
                    s.file_request(&w, tg_core::FileRequest::OpenUrl(target.to_string()));
                }
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_media_ready(move || {
            if let Some(w) = weak.upgrade() {
                s.media.borrow_mut().drain();
                s.chats.notify.reset();
                s.actions.destinations.notify.reset();
                s.feed.notify.reset();
                s.sync_media(&w);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_chat_viewport_changed(move || {
            if let Some(w) = weak.upgrade() {
                s.sync_media(&w);
                if w.get_chat_near_end() && w.get_more_chats() {
                    w.invoke_load_chats();
                }
            }
        });
        let sink = EventSink::new(&window);
        let weak = window.as_weak();
        let s = state.clone();
        let drain = sink.clone();
        window.on_drain_events(move || {
            if let Some(w) = weak.upgrade() {
                match drain.drain() {
                    Ok(events) => {
                        for event in events {
                            s.event(&w, event);
                        }
                    }
                    Err(()) => {
                        let _ = (s.command)(Command::ShutDown);
                        w.set_blocked(true);
                        w.set_error(
                            "UI event capacity exceeded. Please close and restart the application."
                                .into(),
                        );
                    }
                }
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_width_changed(move || {
            if let Some(w) = weak.upgrade() {
                let width = w.get_feed_width();
                let scale = w.window().scale_factor();
                let changed_width = s.feed.width.replace(width) != width;
                let changed_scale = s.feed.scale.replace(scale) != scale;
                if changed_width || changed_scale {
                    s.feed.invalidate();
                    s.refresh(&w);
                }
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_open_video(move |row, cell| {
            if let Some(w) = weak.upgrade() {
                s.open_video(&w, row.max(0) as usize, cell.max(0) as usize);
            }
        });
        let viewer = state.viewer.as_weak();
        let s = state.clone();
        let main = window.as_weak();
        state.viewer.on_toggle(move || {
            if let Some(w) = main.upgrade() {
                s.video(&w, VideoRequest::TogglePause);
            }
        });
        let s = state.clone();
        let main = window.as_weak();
        state.viewer.on_seek(move |fraction| {
            if let Some(w) = main.upgrade() {
                s.video(&w, VideoRequest::Seek(f64::from(fraction)));
            }
        });
        let s = state.clone();
        let main = window.as_weak();
        state.viewer.on_dismiss(move || {
            if let Some(w) = main.upgrade() {
                s.close_video(&w);
            }
        });
        let s = state.clone();
        let main = window.as_weak();
        state.viewer.window().on_close_requested(move || {
            if let Some(w) = main.upgrade() {
                s.close_video(&w);
            }
            slint::CloseRequestResponse::HideWindow
        });
        let s = state.clone();
        let main = window.as_weak();
        state.viewer.on_area_changed(move || {
            if let (Some(w), Some(v)) = (main.upgrade(), viewer.upgrade())
                && v.window().is_visible()
                && (s.streaming.borrow().is_some()
                    || s.playing.borrow().as_ref().is_some_and(|p| p.1.is_some()))
            {
                s.video(&w, VideoRequest::Move(State::video_rect(&v)));
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_wheel(move |delta| {
            if let Some(w) = weak.upgrade() {
                s.wheel(&w, delta);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_open_chat(move |index| {
            if let Some(w) = weak.upgrade() {
                s.open(&w, index.max(0) as usize);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_auth_submit(move || {
            if let Some(w) = weak.upgrade() {
                let input = w.get_auth_input().to_string();
                if input.is_empty() || input.len() > 4096 {
                    return;
                }
                let secret = Secret::new(input);
                let command = match w.get_auth_step() {
                    2 => Command::SubmitPhoneNumber(secret),
                    3 => Command::SubmitCode(secret),
                    4 => Command::SubmitPassword(secret),
                    _ => return,
                };
                w.set_auth_input("".into());
                s.send(&w, command);
            }
        });
        let local: [(bool, fn() -> Command); 2] = [
            (false, || Command::RetryLocalData),
            (true, || Command::ResetLocalData),
        ];
        for (reset, command) in local {
            let weak = window.as_weak();
            let s = state.clone();
            let handler = move || {
                if let Some(w) = weak.upgrade() {
                    // The screen stays until the application reports the next auth state.
                    w.set_error("".into());
                    w.set_auth_title(if reset { "Starting fresh" } else { "Unlocking" }.into());
                    s.send(&w, command());
                }
            };
            if reset {
                window.on_local_reset(handler);
            } else {
                window.on_local_retry(handler);
            }
        }
        let weak = window.as_weak();
        let s = state.clone();
        window.on_request_qr(move || {
            if let Some(w) = weak.upgrade() {
                w.set_auth_step(1);
                s.send(&w, Command::RequestQrSignIn);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_load_chats(move || {
            if let Some(w) = weak.upgrade()
                && s.chats.rows.borrow().len() < 2048
                && !s.loading_chats.replace(true)
                && !s.send(&w, Command::LoadMoreChats)
            {
                s.loading_chats.set(false);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_viewport_changed(move || {
            if let Some(w) = weak.upgrade()
                && !s.refreshing.get()
            {
                let at_bottom =
                    (-w.get_feed_y() + w.get_feed_height()) >= w.get_feed_content_height() - 2.0;
                // Returning to a floating round can briefly leave an empty around page.
                // Its layout is not a user scroll to the newest message.
                if s.feed.history.borrow().pending_around.is_none()
                    && s.feed.jump_target.get().is_none()
                {
                    w.set_follow_tail(at_bottom && s.feed.history.borrow().at_tail());
                }
                s.refresh(&w);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_latest(move || {
            if let Some(w) = weak.upgrade() {
                s.scroll_target.set(None);
                s.seek_start.set(false);
                s.feed.jump_unread.set(false);
                if s.feed.history.borrow().at_tail() {
                    w.set_follow_tail(true);
                    s.refresh(&w);
                } else {
                    s.feed.history.borrow_mut().latest_requested = true;
                    s.refresh(&w);
                }
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_send(move || {
            if let Some(w) = weak.upgrade() {
                s.send_composer(&w);
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_demo_toggle(move || {
            if let Some(w) = weak.upgrade()
                && w.get_demo()
            {
                if w.get_ready() {
                    s.event(&w, Event::AuthStateChanged(AuthState::WaitPhoneNumber));
                } else {
                    s.event(&w, Event::AuthStateChanged(AuthState::Ready));
                }
            }
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.window().on_winit_window_event(move |_, event| {
            if let winit::event::WindowEvent::Focused(focused) = event {
                s.focused.set(*focused);
                if *focused && let Some(w) = weak.upgrade() {
                    s.view_visible(&w);
                }
            }
            if let winit::event::WindowEvent::Occluded(occluded) = event
                && let Some(w) = weak.upgrade()
            {
                s.round_occluded(&w, *occluded);
            }
            // Windows does not emit Occluded; WM_SIZE is delivered as Resized(0, 0)
            // when minimized. Restore arrives as a positive size.
            if let winit::event::WindowEvent::Resized(size) = event
                && let Some(w) = weak.upgrade()
            {
                s.round_occluded(&w, size.width == 0 || size.height == 0);
            }
            slint::winit_030::EventResult::Propagate
        });
        let weak = window.as_weak();
        let s = state.clone();
        window.on_logout(move || {
            if let Some(w) = weak.upgrade() {
                s.send(&w, Command::LogOut);
            }
        });
        Ok(Self {
            window,
            sink,
            snapshot: options.snapshot,
            check: options.check,
            demo_scroll: options.demo_scroll,
            demo_video: options.demo_video,
            demo_voice: options.demo_voice,
            demo_round: options.demo_round,
            state,
        })
    }
    pub fn on_notification(
        &self,
        handler: impl Fn(Notification) -> Result<(), CommandError> + 'static,
    ) {
        *self.state.notifications.borrow_mut() = Box::new(handler);
    }
    /// Video requests run on the UI thread, which owns the main window.
    pub fn on_file(
        &self,
        handler: impl Fn(tg_core::FileRequest) -> Result<(), CommandError> + 'static,
    ) {
        *self.state.attachments.files.borrow_mut() = Some(Box::new(handler));
    }
    /// The open-file dialog and clipboard used by the attach button and Ctrl+V.
    pub fn on_attachment_sources(&self, sources: AttachmentSources) {
        self.state.outgoing.sources.set(Some(sources));
    }
    pub fn on_video(&self, handler: impl Fn(VideoRequest) -> Result<(), CommandError> + 'static) {
        *self.state.video.borrow_mut() = Box::new(handler);
    }
    /// Stores a newly chosen interface scale. After it returns `Ok`, `run` returns so the
    /// caller can restart the client; without a handler the dialog only closes.
    pub fn on_interface_scale(&self, handler: impl Fn(u16) -> Result<(), CommandError> + 'static) {
        *self.state.scale.borrow_mut() = Some(Box::new(handler));
    }
    pub fn event_sink(&self) -> EventSink {
        self.sink.clone()
    }
    /// Keep the UI owner alive until the Telegram engine has joined: pending clipboard
    /// upload files must outlive the event loop, since TDLib reads them asynchronously.
    pub fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let result = Rc::new(RefCell::new(None));
        let snapshot_requested = self.snapshot.is_some();
        let demo_timer = slint::Timer::default();
        if let Some(seconds) = self.demo_scroll {
            let weak = self.window.as_weak();
            let start = std::time::Instant::now();
            let previous = Cell::new(start);
            // Demo-only animation clock; normal operation has no repeating timer.
            demo_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(16),
                move || {
                    let now = std::time::Instant::now();
                    let dt = now.duration_since(previous.replace(now)).as_secs_f32();
                    if let Some(w) = weak.upgrade()
                        && start.elapsed() > Duration::from_millis(500)
                    {
                        w.set_feed_y((w.get_feed_y() + 900.0 * dt).min(0.0));
                    }
                    if start.elapsed() >= Duration::from_secs(u64::from(seconds))
                        && !snapshot_requested
                    {
                        let _ = slint::quit_event_loop();
                    }
                },
            );
        }
        if let Some(seconds) = self.demo_round {
            let weak = self.window.as_weak();
            let state = self.state.clone();
            slint::Timer::single_shot(Duration::from_millis(1500), move || {
                let round = state
                    .feed
                    .history
                    .borrow()
                    .messages
                    .iter()
                    .rev()
                    .find(|m| matches!(m.attachment, Some(tg_core::Attachment::Round(_))))
                    .cloned();
                if let Some(w) = weak.upgrade()
                    && let Some(round) = round
                {
                    w.set_follow_tail(false);
                    state.feed.jump_target.set(Some(round.id));
                    state.refresh(&w);
                    state.demo_play_voice(&w, &round);
                }
            });
            slint::Timer::single_shot(Duration::from_secs(u64::from(seconds)), || {
                let _ = slint::quit_event_loop();
            });
        }
        if let Some(seconds) = self.demo_voice {
            let weak = self.window.as_weak();
            let state = self.state.clone();
            slint::Timer::single_shot(Duration::from_millis(1500), move || {
                let visible = state.feed.visible_ids();
                let voice = state
                    .feed
                    .history
                    .borrow()
                    .messages
                    .iter()
                    .find(|m| {
                        visible.contains(&m.id)
                            && matches!(m.attachment, Some(tg_core::Attachment::Voice(_)))
                    })
                    .cloned();
                if let Some(w) = weak.upgrade()
                    && let Some(voice) = voice
                {
                    state.demo_play_voice(&w, &voice);
                }
            });
            slint::Timer::single_shot(Duration::from_secs(u64::from(seconds)), || {
                let _ = slint::quit_event_loop();
            });
        }
        if let Some(seconds) = self.demo_video {
            let weak = self.window.as_weak();
            let state = self.state.clone();
            // Demo-only single shots: let the first page arrive, then open the newest video.
            slint::Timer::single_shot(Duration::from_millis(1500), move || {
                if let Some(w) = weak.upgrade()
                    && let Some(video) = state.feed.newest_video()
                {
                    state.play(&w, video);
                }
            });
            slint::Timer::single_shot(Duration::from_secs(u64::from(seconds)), || {
                let _ = slint::quit_event_loop();
            });
        }
        if let Some(path) = self.snapshot.clone() {
            let weak = self.window.as_weak();
            let result = result.clone();
            // Single-shot render settling delay, not polling. File encoding/writing runs off-thread.
            slint::Timer::single_shot(
                Duration::from_secs(u64::from(
                    self.demo_voice
                        .or(self.demo_round)
                        .map(|s| s.saturating_sub(1).clamp(2, 5))
                        .unwrap_or(self.demo_scroll.unwrap_or(2)),
                )),
                move || {
                    let saved = weak
                        .upgrade()
                        .ok_or_else(|| "Snapshot window was closed".to_owned())
                        .and_then(|w| {
                            // Slint 1.18.1's vector-text cache can leave missing glyphs on the
                            // first render into snapshot pixels (reproduced on the password
                            // screen). Prime that pixel format, then capture the complete frame.
                            drop(
                                w.window()
                                    .take_snapshot()
                                    .map_err(|_| "Could not capture the window".to_owned())?,
                            );
                            let pixels = w
                                .window()
                                .take_snapshot()
                                .map_err(|_| "Could not capture the window".to_owned())?;
                            Ok(std::thread::spawn(move || {
                                image::save_buffer(
                                    path,
                                    pixels.as_bytes(),
                                    pixels.width(),
                                    pixels.height(),
                                    image::ColorType::Rgba8,
                                )
                                .map_err(|_| "Could not save the snapshot".to_owned())
                            }))
                        });
                    *result.borrow_mut() = Some(saved);
                    let _ = slint::quit_event_loop();
                },
            );
        }
        let ran = self.window.run();
        // Release producers that may wait for a drain that will never come again.
        self.sink.close();
        self.state.close_video(&self.window);
        self.state.stop_voice(&self.window);
        if let Some(path) = &self.check {
            let window = &self.window;
            self.state
                .check
                .write(
                    path,
                    &check::Screen {
                        ready: window.get_ready(),
                        auth_step: window.get_auth_step(),
                        chats: self.state.chats.rows.borrow().len(),
                        messages: self.state.feed.history.borrow().messages.len(),
                        error: &window.get_error(),
                    },
                )
                .map_err(|_| "Could not write the check report")?;
        }
        ran?;
        if let Some(saved) = result.borrow_mut().take() {
            saved?.join().map_err(|_| "Snapshot worker failed")??;
        } else if snapshot_requested {
            return Err("Window closed before snapshot capture".into());
        }
        Ok(())
    }
}
