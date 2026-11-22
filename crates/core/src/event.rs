//! Events: from the engine to the UI.

use std::fmt;

use crate::{ChatId, CommandKind, MessageId};

/// Authentication step. Debug hides QR links and password hints.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthState {
    WaitParameters,
    WaitPhoneNumber,
    WaitCode,
    WaitPassword { hint: String },
    WaitOtherDevice { link: String },
    Ready,
    LoggingOut,
    Closing,
    Closed,
    Unsupported,
}

impl fmt::Debug for AuthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WaitParameters => "WaitParameters",
            Self::WaitPhoneNumber => "WaitPhoneNumber",
            Self::WaitCode => "WaitCode",
            Self::WaitPassword { .. } => "WaitPassword(<redacted>)",
            Self::WaitOtherDevice { .. } => "WaitOtherDevice(<redacted>)",
            Self::Ready => "Ready",
            Self::LoggingOut => "LoggingOut",
            Self::Closing => "Closing",
            Self::Closed => "Closed",
            Self::Unsupported => "Unsupported",
        })
    }
}

/// Fixed application text, never the raw TDLib error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityKind {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Code,
    Pre,
    Spoiler,
    Url,
    TextUrl { target: String },
    Mention,
}

/// Offsets are UTF-16 code units, not UTF-8 bytes or Unicode scalar values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEntity {
    pub offset: u32,
    pub length: u32,
    pub kind: EntityKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendState {
    Sent,
    Pending,
    Failed(CommandError),
}

/// Compact reply metadata. Strings are each bounded to 256 UTF-8 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub resolved: bool,
    pub chat: ChatId,
    pub message: MessageId,
    pub sender_id: i64,
    pub sender: String,
    pub preview: String,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessagePermissions {
    pub edit: bool,
    pub delete_self: bool,
    pub delete_all: bool,
    pub forward: bool,
    pub reply: bool,
}
/// Engine output is bounded to 64 KiB text and 1,024 entities per message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub attachment: Option<crate::Attachment>,
    pub reply: Option<Reply>,
    /// Forward origin display name, at most 256 UTF-8 bytes.
    pub forwarded: Option<String>,
    pub edited: bool,
    pub is_text: bool,
    /// A service message ("Anna joined the group"): `text` holds its wording and it is drawn as a
    /// centred pill, not a bubble.
    pub service: bool,
    /// Populated on demand, never inferred from ownership alone.
    pub permissions: Option<MessagePermissions>,
    pub photo: Option<Photo>,
    pub id: MessageId,
    pub chat: ChatId,
    pub date: i32,
    pub outgoing: bool,
    pub text: String,
    /// At most 1,024 entries; each text-URL target is at most 4 KiB.
    pub entities: Vec<TextEntity>,
    pub send_state: SendState,
    /// TDLib `media_album_id`; non-zero values group consecutive messages into one album.
    pub album: i64,
    /// Sender user or chat ID, 0 when unknown. Used for grouping and name colours.
    pub sender_id: i64,
    /// Sender display name, only for messages from someone other than the chat itself
    /// (group members); at most 256 bytes.
    pub sender: Option<String>,
    /// Video file; its preview frame is `photo`.
    pub video: Option<crate::Video>,
    /// Emoji reactions in Telegram's order, at most [`crate::MAX_REACTIONS`].
    pub reactions: Vec<crate::Reaction>,
}

/// Bounded chat-row preview, not a retained message or history cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePreview {
    pub id: MessageId,
    /// At most 512 UTF-8 bytes, truncated at a character boundary.
    pub text: String,
    /// Unix time of the message.
    pub date: i32,
    pub outgoing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chat {
    pub avatar: Option<Photo>,
    pub id: ChatId,
    /// At most 1 KiB from engine output.
    pub title: String,
    pub last_message: Option<MessagePreview>,
    pub unread_count: u32,
    pub unread_mentions: u32,
    /// Absolute Unix deadline; scope defaults are resolved by tg-telegram.
    pub muted_until: i64,
    pub show_notification_preview: bool,
    pub last_read_inbox: MessageId,
    /// Outgoing messages up to this ID were read by the other side.
    pub last_read_outbox: MessageId,
}

impl Chat {
    pub fn is_muted(&self, now: i64) -> bool {
        self.muted_until > now
    }
}

/// Apply sequentially. `to` is the index after removing `from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatListChange {
    Insert { position: usize, chat: Chat },
    Update { position: usize, chat: Chat },
    Move { from: usize, to: usize },
    Remove { position: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityWarning {
    /// Milestone 2 can report that the key is protected only by DPAPI.
    TpmUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    AuthStateChanged(AuthState),
    /// The local database key exists but cannot be unlocked on this computer and account (the
    /// TPM or DPAPI refused it). The engine is not running; the user chooses
    /// `Command::RetryLocalData` or `Command::ResetLocalData`.
    LocalDataLocked,
    /// Playback state of the open video, sent on player events.
    VideoState(crate::VideoState),
    RoundFrame(crate::RoundFrame),
    FileUpdated(MediaFile),
    MessageContentOpened {
        chat: ChatId,
        message: MessageId,
    },
    FileOperationFailed,
    FileDownloadFailed(i32),

    ChatListChanged(ChatListChange),
    /// False means TDLib reported that the main list is exhausted.
    ChatsLoaded {
        has_more: bool,
    },
    MessagesLoaded {
        chat: ChatId,
        before: MessageId,
        /// Cursor from the raw page, including unsupported media. None means no progress.
        next_before: Option<MessageId>,
        /// At most 50 messages, newest first; never cached by the engine.
        messages: Vec<Message>,
    },
    NewerMessagesLoaded {
        chat: ChatId,
        after: MessageId,
        /// Maximum raw ID, including unsupported content. None means no progress.
        next_after: Option<MessageId>,
        /// At most 50 messages, newest first.
        messages: Vec<Message>,
    },
    /// Live incoming update, eligible for notification filtering (never a history page).
    IncomingMessage(Message),
    /// Open a chat in response to the native notification callback.
    NotificationActivated(ChatId),
    TotalUnreadCount(u32),
    MessagesViewed {
        chat: ChatId,
        success: bool,
    },
    MessagesAroundLoaded {
        chat: ChatId,
        anchor: MessageId,
        messages: Vec<Message>,
    },
    NewMessage(Message),
    /// TDLib no longer reads the submitted local file (success or failure).
    LocalFileSendFinished(std::path::PathBuf),
    ReplyResolved {
        chat: ChatId,
        message: MessageId,
        reply: Option<Reply>,
    },
    MessageProperties {
        chat: ChatId,
        message: MessageId,
        permissions: MessagePermissions,
    },
    MessageContentChanged {
        chat: ChatId,
        message: MessageId,
        text: String,
        entities: Vec<TextEntity>,
        photo: Option<Photo>,
        attachment: Option<crate::Attachment>,
        is_text: bool,
    },
    MessageEdited {
        chat: ChatId,
        message: MessageId,
        edited: bool,
    },
    /// The full new reaction list of a message (possibly empty), at most
    /// [`crate::MAX_REACTIONS`] entries.
    MessageReactionsChanged {
        chat: ChatId,
        message: MessageId,
        reactions: Vec<crate::Reaction>,
    },
    MessagesDeleted {
        chat: ChatId,
        messages: Vec<MessageId>,
    },
    MessageUpdated {
        old_id: MessageId,
        message: Message,
    },
    CommandFailed {
        command: CommandKind,
        error: CommandError,
    },
    SecurityWarning(SecurityWarning),
}

/// Encoded JPEG minithumbnail, at most 8 KiB. No decoded pixels cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    pub file: MediaFile,
    pub width: u16,
    pub height: u16,
    pub minithumbnail: Vec<u8>,
}
/// TDLib-managed local file; paths are at most 4 KiB and are never logged.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaFile {
    pub partial: Option<crate::StreamFile>,
    pub size: u64,
    pub downloaded: u64,
    pub downloading: bool,
    pub id: i32,
    pub path: Option<std::path::PathBuf>,
}
impl fmt::Debug for MediaFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaFile")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
