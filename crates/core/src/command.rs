//! Commands: from the UI to the engine.

use crate::{ChatId, MessageId, Secret};

/// Non-secret command identity for associating an error with a UI action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    DownloadFileUrgent,
    OpenMessageContent,
    ReplyText,
    EditText,
    ToggleReaction,
    DeleteMessages,
    ForwardMessages,
    GetMessageProperties,
    GetRepliedMessage,
    ViewMessages,
    DownloadFile,
    CancelDownload,
    RequestQrSignIn,
    SubmitPhoneNumber,
    SubmitCode,
    SubmitPassword,
    LoadMoreChats,
    OpenChat,
    CloseChat,
    LoadOlderMessages,
    LoadNewerMessages,
    SendText,
    SendPhoto,
    SendDocument,
    LogOut,
    ShutDown,
    Initialize,
}

/// A user action for the engine to perform.
///
/// Commands are moved rather than copied: `Clone` is deliberately absent so that secrets
/// inside them exist in a single instance.
#[derive(Debug)]
#[non_exhaustive]
pub enum Command {
    /// TDLib reads the local file asynchronously; no file bytes cross this boundary.
    SendPhoto {
        chat: ChatId,
        path: std::path::PathBuf,
        caption: Option<String>,
    },
    SendDocument {
        chat: ChatId,
        path: std::path::PathBuf,
        caption: Option<String>,
    },
    /// Streaming download of `limit` bytes from `offset`; repeating it with the same offset and
    /// a larger limit extends the running download instead of restarting it.
    DownloadRange {
        file: i32,
        offset: u64,
        limit: u64,
    },
    /// A user-requested voice/document download, above viewport photo prefetch priority.
    DownloadFileUrgent(i32),
    OpenMessageContent {
        chat: ChatId,
        message: MessageId,
    },
    ReplyText {
        chat: ChatId,
        reply: MessageId,
        text: String,
    },
    EditText {
        chat: ChatId,
        message: MessageId,
        text: String,
    },
    /// Add the reaction if the current user has not chosen it, otherwise remove it. The engine
    /// decides from TDLib's current message state, not from a possibly stale UI copy.
    ToggleReaction {
        chat: ChatId,
        message: MessageId,
        reaction: String,
    },
    /// One album at most (10 IDs), ascending order.
    DeleteMessages {
        chat: ChatId,
        messages: Vec<MessageId>,
        revoke: bool,
    },
    ForwardMessages {
        from: ChatId,
        to: ChatId,
        messages: Vec<MessageId>,
    },
    GetRepliedMessage {
        chat: ChatId,
        message: MessageId,
    },
    GetMessageProperties {
        chat: ChatId,
        message: MessageId,
    },
    /// Report only actually visible incoming IDs; at most 50 per request.
    ViewMessages {
        chat: ChatId,
        messages: Vec<MessageId>,
    },
    /// Open around a real last-read ID; zero is valid for an entirely unread chat.
    OpenChatAt {
        chat: ChatId,
        anchor: MessageId,
    },
    /// Only the viewport media controller requests files.
    DownloadFile(i32),
    CancelDownload(i32),
    /// Request a QR link to confirm on another signed-in device.
    RequestQrSignIn,
    /// Phone number for sign-in.
    SubmitPhoneNumber(Secret),
    /// Verification code from Telegram or SMS.
    SubmitCode(Secret),
    /// Two-factor authentication password.
    SubmitPassword(Secret),
    /// Load the next 50 main-list chats. No automatic prefetching.
    LoadMoreChats,
    /// Open a chat and request its latest page of messages.
    OpenChat(ChatId),
    CloseChat(ChatId),
    /// Load up to 50 messages before this ID (zero starts at the newest message).
    LoadOlderMessages {
        chat: ChatId,
        before: MessageId,
    },
    /// Load messages strictly newer than a real, positive message ID.
    LoadNewerMessages {
        chat: ChatId,
        after: MessageId,
    },
    /// Send a text message.
    SendText {
        chat: ChatId,
        text: String,
    },
    LogOut,
    /// Start closing; joining is a separate, blocking engine operation.
    ShutDown,
    /// After `Event::LocalDataLocked`: try to unlock the database key again. Handled by the
    /// application, never sent to the engine.
    RetryLocalData,
    /// After `Event::LocalDataLocked`: move the unreadable local data to a dated backup folder
    /// next to it and start with a new key and an empty database. Nothing is deleted. Handled by
    /// the application, never sent to the engine.
    ResetLocalData,
}
