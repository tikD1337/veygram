//! Bounded JSON-to-domain conversion, independent of the native library.

use base64::Engine as _;
use serde_json::{Value, json};
use tg_core::{
    AuthState, ChatId, CommandError, CommandKind, EntityKind, Event, Message, MessageId, SendState,
    TextEntity,
};
use tg_core::{MAX_REACTIONS, MediaFile, Photo, Reaction, Video};

use crate::chats::Chats;

/// Maximum transient JSON input (including unsupported fields): 8 MiB.
pub(crate) const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_LINK_BYTES: usize = 4096;
pub(crate) const PAGE_SIZE: usize = 50;
const MAX_ENTITIES: usize = 1024;

pub(crate) fn error(code: i32, message: &'static str) -> CommandError {
    CommandError {
        code,
        message: message.into(),
    }
}

pub(crate) fn malformed() -> CommandError {
    error(-1, "Invalid or oversized Telegram response.")
}

pub(crate) fn integer(value: &Value) -> Result<i64, CommandError> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or_else(malformed)
}

pub(crate) fn string(value: &Value, limit: usize) -> Result<String, CommandError> {
    value
        .as_str()
        .filter(|text| text.len() <= limit)
        .map(str::to_owned)
        .ok_or_else(malformed)
}

pub(crate) fn td_error(value: &Value) -> CommandError {
    let code = integer(&value["code"])
        .ok()
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(-1);
    // Never echo arbitrary TDLib error strings, which can contain request data.
    let message = match value["message"].as_str() {
        Some("PHONE_NUMBER_INVALID") => "The phone number is invalid.",
        Some("PHONE_CODE_INVALID") => "The verification code is invalid.",
        Some("PHONE_CODE_EXPIRED") => "The verification code has expired.",
        Some("PASSWORD_HASH_INVALID") => "The password is invalid.",
        Some("REACTION_INVALID") => "This reaction is not available here.",
        _ => match code {
            401 => "Authorization is required.",
            429 => "Too many requests. Try again later.",
            500.. => "Telegram is temporarily unavailable.",
            _ => "Telegram rejected the request.",
        },
    };
    error(code, message)
}

pub(crate) fn command_kind(value: &Value) -> CommandKind {
    match value.as_str() {
        Some("open_content") => CommandKind::OpenMessageContent,
        Some("download") => CommandKind::DownloadFile,
        Some("cancel_download") => CommandKind::CancelDownload,
        Some("qr") => CommandKind::RequestQrSignIn,
        Some("phone") => CommandKind::SubmitPhoneNumber,
        Some("code") => CommandKind::SubmitCode,
        Some("password") => CommandKind::SubmitPassword,
        Some("chats") => CommandKind::LoadMoreChats,
        Some("open" | "open_around") => CommandKind::OpenChat,
        Some("view") => CommandKind::ViewMessages,
        Some("close_chat") => CommandKind::CloseChat,
        Some("history" | "history_around") => CommandKind::LoadOlderMessages,
        Some("history_newer") => CommandKind::LoadNewerMessages,
        Some("send_text") => CommandKind::SendText,
        Some("send_photo") => CommandKind::SendPhoto,
        Some("send_document") => CommandKind::SendDocument,
        Some("edit_text") => CommandKind::EditText,
        Some("toggle_reaction" | "reaction") => CommandKind::ToggleReaction,
        Some("delete") => CommandKind::DeleteMessages,
        Some("forward") => CommandKind::ForwardMessages,
        Some("replied") => CommandKind::GetRepliedMessage,
        Some("properties") => CommandKind::GetMessageProperties,
        Some("logout") => CommandKind::LogOut,
        Some("shutdown") => CommandKind::ShutDown,
        _ => CommandKind::Initialize,
    }
}

pub(crate) fn auth(value: &Value) -> Result<AuthState, CommandError> {
    Ok(match value["@type"].as_str() {
        Some("authorizationStateWaitTdlibParameters") => AuthState::WaitParameters,
        Some("authorizationStateWaitPhoneNumber") => AuthState::WaitPhoneNumber,
        Some("authorizationStateWaitCode") => AuthState::WaitCode,
        Some("authorizationStateWaitPassword") => AuthState::WaitPassword {
            hint: string(&value["password_hint"], MAX_LINK_BYTES)?,
        },
        Some("authorizationStateWaitOtherDeviceConfirmation") => AuthState::WaitOtherDevice {
            link: string(&value["link"], MAX_LINK_BYTES)?,
        },
        Some("authorizationStateReady") => AuthState::Ready,
        Some("authorizationStateLoggingOut") => AuthState::LoggingOut,
        Some("authorizationStateClosing") => AuthState::Closing,
        Some("authorizationStateClosed") => AuthState::Closed,
        _ => AuthState::Unsupported,
    })
}

fn utf16_boundary(text: &str, index: u32) -> bool {
    let mut units = 0;
    for ch in text.chars() {
        if units == index {
            return true;
        }
        units += ch.len_utf16() as u32;
        if units > index {
            return false;
        }
    }
    units == index
}

pub(crate) const MAX_NAME_BYTES: usize = 256;
const MAX_USERS: usize = 8192;

/// Display names of users, at most MAX_USERS entries of at most MAX_NAME_BYTES each (<= 2 MiB).
#[derive(Debug, Default)]
pub(crate) struct Users {
    names: std::collections::HashMap<i64, String>,
}

impl Users {
    pub(crate) fn apply(&mut self, user: &Value) -> Result<(), CommandError> {
        let id = integer(&user["id"])?;
        let first = user["first_name"].as_str().unwrap_or("");
        let last = user["last_name"].as_str().unwrap_or("");
        let name = truncate(
            if last.is_empty() {
                first.to_owned()
            } else {
                format!("{first} {last}")
            },
            MAX_NAME_BYTES,
        );
        if !self.names.contains_key(&id)
            && self.names.len() >= MAX_USERS
            && let Some(evict) = self.names.keys().next().copied()
        {
            // Arbitrary eviction keeps the bound; TDLib resends users it needs again.
            self.names.remove(&evict);
        }
        self.names.insert(id, name);
        Ok(())
    }
    pub(crate) fn name(&self, id: i64) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }
}

pub(crate) fn truncate(mut text: String, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

pub(crate) fn message(
    value: &Value,
    users: &Users,
    chats: &Chats,
) -> Result<Option<Message>, CommandError> {
    let content = &value["content"];
    let (photo, video) = match content["@type"].as_str() {
        Some("messagePhoto") => (photo(&content["photo"])?, None),
        Some("messageVideo") => video(content)?,
        _ => (None, None),
    };
    let attachment = attachment(content)?;
    let photo = match attachment.as_ref() {
        Some(tg_core::Attachment::Document(document)) => {
            thumbnail(&content["document"], Some(document.file.id))?
        }
        Some(tg_core::Attachment::Round(round)) => {
            thumbnail(&content["video_note"], Some(round.file.id))?
        }
        Some(tg_core::Attachment::Sticker(sticker) | tg_core::Attachment::Emoji(sticker)) => {
            let s = if content["@type"] == "messageAnimatedEmoji" {
                &content["animated_emoji"]["sticker"]
            } else {
                &content["sticker"]
            };
            if !sticker.animated {
                Some(Photo {
                    file: media_file(&s["sticker"])?,
                    width: sticker.width,
                    height: sticker.height,
                    minithumbnail: vec![],
                })
            } else if matches!(attachment, Some(tg_core::Attachment::Emoji(_))) {
                None
            } else {
                thumbnail(s, None)?
            }
        }
        _ => photo,
    };
    let chat = ChatId(integer(&value["chat_id"])?);
    let outgoing = value["is_outgoing"].as_bool().ok_or_else(malformed)?;
    let sender_value = &value["sender_id"];
    let (sender_id, name) = match sender_value["@type"].as_str() {
        Some("messageSenderUser") => {
            let id = integer(&sender_value["user_id"])?;
            (id, users.name(id))
        }
        Some("messageSenderChat") => {
            let id = integer(&sender_value["chat_id"])?;
            (id, chats.title(ChatId(id)))
        }
        _ => (0, None),
    };
    // Service messages ("Anna joined the group") are drawn as pills; their text is built here.
    let actor = if outgoing && sender_id != chat.0 {
        "You"
    } else if sender_id > 0 {
        crate::service::name(users, sender_id)
    } else {
        name.unwrap_or("Unknown chat")
    };
    let service = crate::service::text(content, actor, chats.is_channel(chat), sender_id, users);
    let placeholder = serde_json::json!({
        "text": service.as_deref().unwrap_or_else(|| content_label(content)),
        "entities": [],
    });
    let formatted = match content["@type"].as_str() {
        Some("messageText") => &content["text"],
        Some("messagePhoto" | "messageVideo" | "messageDocument" | "messageVoiceNote")
            if !content["caption"].is_null() =>
        {
            &content["caption"]
        }
        Some(
            "messageSticker"
            | "messageVoiceNote"
            | "messageDocument"
            | "messageVideoNote"
            | "messageAnimatedEmoji",
        ) => &json!({"text":"","entities":[]}),
        _ => &placeholder,
    };
    let text = string(&formatted["text"], MAX_TEXT_BYTES)?;
    let source = formatted["entities"].as_array().ok_or_else(malformed)?;
    if source.len() > MAX_ENTITIES {
        return Err(malformed());
    }
    // At most MAX_ENTITIES; temporary entity storage dies with the emitted event.
    let mut entities = Vec::with_capacity(source.len());
    for entity in source {
        let kind = match entity["type"]["@type"].as_str() {
            Some("textEntityTypeBold") => EntityKind::Bold,
            Some("textEntityTypeItalic") => EntityKind::Italic,
            Some("textEntityTypeUnderline") => EntityKind::Underline,
            Some("textEntityTypeStrikethrough") => EntityKind::Strikethrough,
            Some("textEntityTypeCode") => EntityKind::Code,
            Some("textEntityTypePre" | "textEntityTypePreCode") => EntityKind::Pre,
            Some("textEntityTypeSpoiler") => EntityKind::Spoiler,
            Some("textEntityTypeUrl") => EntityKind::Url,
            Some("textEntityTypeTextUrl") => EntityKind::TextUrl {
                target: string(&entity["type"]["url"], MAX_LINK_BYTES)?,
            },
            Some("textEntityTypeMention" | "textEntityTypeMentionName") => EntityKind::Mention,
            _ => continue,
        };
        let offset = u32::try_from(integer(&entity["offset"])?).map_err(|_| malformed())?;
        let length = u32::try_from(integer(&entity["length"])?).map_err(|_| malformed())?;
        let end = offset.checked_add(length).ok_or_else(malformed)?;
        if length == 0 || !utf16_boundary(&text, offset) || !utf16_boundary(&text, end) {
            return Err(malformed());
        }
        entities.push(TextEntity {
            offset,
            length,
            kind,
        });
    }
    let send_state = match value["sending_state"]["@type"].as_str() {
        Some("messageSendingStatePending") => SendState::Pending,
        Some("messageSendingStateFailed") => {
            SendState::Failed(td_error(&value["sending_state"]["error"]))
        }
        _ => SendState::Sent,
    };
    // Private chats and channels have the chat itself as the sender: no name is shown.
    let sender = name
        .filter(|_| !outgoing && sender_id != 0 && sender_id != chat.0)
        .map(|name| truncate(name.to_owned(), MAX_NAME_BYTES));
    let album = if value["media_album_id"].is_null() {
        0
    } else {
        integer(&value["media_album_id"])?
    };
    let reply = &value["reply_to"];
    let reply = if reply["@type"] == "messageReplyToMessage" {
        let (sender_id, sender) = origin(&reply["origin"], users, chats);
        let preview = reply["quote"]["text"]["text"]
            .as_str()
            .or_else(|| reply["content"]["text"]["text"].as_str())
            .or_else(|| reply["content"]["caption"]["text"].as_str())
            .unwrap_or_else(|| content_label(&reply["content"]));
        Some(tg_core::Reply {
            resolved: false,
            chat: ChatId(integer(&reply["chat_id"])?),
            message: MessageId(integer(&reply["message_id"])?),
            sender_id,
            sender,
            preview: truncate(preview.replace(['\r', '\n'], " "), 256),
        })
    } else {
        None
    };
    Ok(Some(Message {
        attachment,
        reply,
        forwarded: (!value["forward_info"].is_null())
            .then(|| origin(&value["forward_info"]["origin"], users, chats).1),
        edited: integer(&value["edit_date"]).unwrap_or(0) > 0,
        is_text: content["@type"] == "messageText",
        service: service.is_some(),
        permissions: None,
        video,
        photo,
        id: MessageId(integer(&value["id"])?),
        chat,
        date: i32::try_from(integer(&value["date"])?).map_err(|_| malformed())?,
        outgoing,
        text,
        entities,
        send_state,
        album,
        sender_id,
        sender,
        reactions: reactions(&value["interaction_info"]["reactions"]["reactions"])?,
    }))
}

/// Emoji reactions from a `vector<messageReaction>` (null means none), in Telegram's order.
/// Custom-emoji and paid reactions are skipped, as are oversized or empty entries; at most
/// MAX_REACTIONS are kept.
pub(crate) fn reactions(value: &Value) -> Result<Vec<Reaction>, CommandError> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let source = value.as_array().ok_or_else(malformed)?;
    let mut reactions = Vec::with_capacity(source.len().min(MAX_REACTIONS));
    for item in source {
        if reactions.len() == MAX_REACTIONS {
            break;
        }
        if item["type"]["@type"] != "reactionTypeEmoji" {
            continue;
        }
        let Some(emoji) = item["type"]["emoji"]
            .as_str()
            .filter(|e| tg_core::valid_reaction(e))
        else {
            continue;
        };
        let count = i32::try_from(integer(&item["total_count"])?)
            .map_err(|_| malformed())?
            .max(0);
        let chosen = item["is_chosen"] == true;
        if count == 0 && !chosen {
            continue;
        }
        reactions.push(Reaction {
            emoji: emoji.to_owned(),
            count,
            chosen,
        });
    }
    Ok(reactions)
}

fn origin(value: &Value, users: &Users, chats: &Chats) -> (i64, String) {
    let (id, name) = match value["@type"].as_str() {
        Some("messageOriginUser") => {
            let id = integer(&value["sender_user_id"]).unwrap_or(0);
            (id, users.name(id).unwrap_or("Unknown sender"))
        }
        Some("messageOriginChat" | "messageOriginChannel") => {
            let id = integer(
                &value[if value["@type"] == "messageOriginChat" {
                    "sender_chat_id"
                } else {
                    "chat_id"
                }],
            )
            .unwrap_or(0);
            (id, chats.title(ChatId(id)).unwrap_or("Unknown chat"))
        }
        Some("messageOriginHiddenUser") => {
            (0, value["sender_name"].as_str().unwrap_or("Hidden sender"))
        }
        _ => (0, "Message"),
    };
    (id, truncate(name.replace(['\r', '\n'], " "), 256))
}
/// Reusable decoder with a bounded main-chat-list model and no message cache.
#[derive(Debug, Default)]
pub struct Decoder {
    pub(crate) chats: Chats,
    pub(crate) users: Users,
    // Only local paths, never file bytes. The UI admits at most eight active uploads.
    uploads: std::collections::BTreeMap<(ChatId, MessageId), std::path::PathBuf>,
    finished_uploads: std::collections::VecDeque<(ChatId, MessageId)>,
}

impl Decoder {
    /// Decode one update or tagged response. The callback runs synchronously.
    /// JSON and all transient JSON collections are bounded by MAX_JSON_BYTES.
    pub fn decode(&mut self, json: &str, emit: impl FnMut(Event)) -> Result<(), CommandError> {
        self.apply(&parse(json)?, emit)
    }

    pub(crate) fn apply(
        &mut self,
        value: &Value,
        mut emit: impl FnMut(Event),
    ) -> Result<(), CommandError> {
        let extra = &value["@extra"];
        match value["@type"].as_str() {
            Some("updateAuthorizationState") => {
                let state = auth(&value["authorization_state"])?;
                if state == AuthState::Closed {
                    self.uploads.clear();
                    self.finished_uploads.clear();
                }
                emit(Event::AuthStateChanged(state));
            }
            Some(
                "updateNewChat"
                | "updateChatPosition"
                | "updateChatLastMessage"
                | "updateChatTitle"
                | "updateChatReadInbox"
                | "updateChatUnreadMentionCount"
                | "updateChatNotificationSettings"
                | "updateChatReadOutbox"
                | "updateChatPhoto",
            ) => self.chats.apply(value, &self.users, emit)?,
            Some("updateUnreadMessageCount") if value["chat_list"]["@type"] == "chatListMain" => {
                emit(Event::TotalUnreadCount(
                    u32::try_from(integer(&value["unread_count"])?).map_err(|_| malformed())?,
                ))
            }
            Some("updateScopeNotificationSettings") => self.chats.scope(value, &mut emit)?,
            Some("scopeNotificationSettings") => self.chats.scope(
                &json!({"scope":{"number":extra["scope"]},"notification_settings":value}),
                &mut emit,
            )?,
            Some("ok" | "error") if extra["command"] == "view" => {
                emit(Event::MessagesViewed {
                    chat: ChatId(integer(&extra["chat"])?),
                    success: value["@type"] == "ok",
                });
            }
            Some("messages") if extra["command"] == "history_around" => {
                let source = value["messages"].as_array().ok_or_else(malformed)?;
                if source.len() > PAGE_SIZE {
                    return Err(malformed());
                }
                let chat = ChatId(integer(&extra["chat"])?);
                let anchor = MessageId(integer(&extra["anchor"])?);
                let mut messages = Vec::with_capacity(source.len());
                for item in source {
                    if integer(&item["chat_id"])? != chat.0 {
                        return Err(malformed());
                    }
                    if let Some(message) = message(item, &self.users, &self.chats)? {
                        messages.push(message);
                    }
                }
                emit(Event::MessagesAroundLoaded {
                    chat,
                    anchor,
                    messages,
                });
            }
            Some("message" | "error") if extra["command"] == "replied" => {
                let reply = if value["@type"] == "message" {
                    message(value, &self.users, &self.chats)?.map(|m| tg_core::Reply {
                        resolved: true,
                        chat: m.chat,
                        message: m.id,
                        sender_id: m.sender_id,
                        sender: truncate(
                            if m.outgoing {
                                "You".into()
                            } else {
                                m.sender.unwrap_or_else(|| {
                                    self.users
                                        .name(m.sender_id)
                                        .or_else(|| self.chats.title(m.chat))
                                        .unwrap_or("Message")
                                        .into()
                                })
                            },
                            256,
                        ),
                        // A caption when there is one; otherwise what the attachment is
                        // ("Sticker", "Voice message", the file name), never an empty quote.
                        preview: truncate(
                            {
                                let caption = m.text.replace(['\r', '\n'], " ");
                                if caption.trim().is_empty()
                                    || matches!(m.attachment, Some(tg_core::Attachment::Emoji(_)))
                                {
                                    attachment_preview(&value["content"]).unwrap_or_else(|| {
                                        content_label(&value["content"]).to_owned()
                                    })
                                } else {
                                    caption
                                }
                            },
                            256,
                        ),
                    })
                } else {
                    None
                };
                emit(Event::ReplyResolved {
                    chat: ChatId(integer(&extra["chat"])?),
                    message: MessageId(integer(&extra["message"])?),
                    reply,
                });
            }
            Some("messageProperties" | "error") if extra["command"] == "properties" => {
                emit(Event::MessageProperties {
                    chat: ChatId(integer(&extra["chat"])?),
                    message: MessageId(integer(&extra["message"])?),
                    permissions: tg_core::MessagePermissions {
                        edit: value["can_be_edited"] == true,
                        delete_self: value["can_be_deleted_only_for_self"] == true,
                        delete_all: value["can_be_deleted_for_all_users"] == true,
                        forward: value["can_be_forwarded"] == true,
                        reply: value["can_be_replied"] == true,
                    },
                });
            }
            Some("updateMessageContent") => {
                // Reuse the bounded content/entity decoder, without retaining a second history.
                let mapped = json!({"id":value["message_id"],"chat_id":value["chat_id"],
                    "date":0,"is_outgoing":false,"content":value["new_content"]});
                if let Some(m) = message(&mapped, &self.users, &self.chats)? {
                    emit(Event::MessageContentChanged {
                        chat: m.chat,
                        message: m.id,
                        text: m.text,
                        entities: m.entities,
                        photo: m.photo,
                        attachment: m.attachment,
                        is_text: m.is_text,
                    });
                }
            }
            // Clients receive reactions with the interaction info (views, forwards, replies);
            // updateMessageReactions is the bot-only variant with the same reaction list.
            Some("updateMessageInteractionInfo") => emit(Event::MessageReactionsChanged {
                chat: ChatId(integer(&value["chat_id"])?),
                message: MessageId(integer(&value["message_id"])?),
                reactions: reactions(&value["interaction_info"]["reactions"]["reactions"])?,
            }),
            Some("updateMessageReactions") => emit(Event::MessageReactionsChanged {
                chat: ChatId(integer(&value["chat_id"])?),
                message: MessageId(integer(&value["message_id"])?),
                reactions: reactions(&value["reactions"])?,
            }),
            Some("updateMessageEdited") => emit(Event::MessageEdited {
                chat: ChatId(integer(&value["chat_id"])?),
                message: MessageId(integer(&value["message_id"])?),
                edited: integer(&value["edit_date"])? > 0,
            }),
            Some("updateDeleteMessages") if value["is_permanent"] == true => {
                let ids = value["message_ids"].as_array().ok_or_else(malformed)?;
                // Emit bounded chunks; the UI bridge enforces the total queued byte budget.
                for chunk in ids.chunks(300) {
                    emit(Event::MessagesDeleted {
                        chat: ChatId(integer(&value["chat_id"])?),
                        messages: chunk
                            .iter()
                            .map(|id| integer(id).map(MessageId))
                            .collect::<Result<_, _>>()?,
                    });
                }
            }
            Some("updateUser") => self.users.apply(&value["user"])?,
            Some("error")
                if extra["command"] == "download"
                    && extra["file"]
                        .as_i64()
                        .is_some_and(|id| id > 0 && id <= i32::MAX as i64) =>
            {
                emit(Event::FileDownloadFailed(integer(&extra["file"])? as i32))
            }
            Some("updateMessageContentOpened") => emit(Event::MessageContentOpened {
                chat: ChatId(integer(&value["chat_id"])?),
                message: MessageId(integer(&value["message_id"])?),
            }),
            Some("updateFile") => emit(Event::FileUpdated(media_file(&value["file"])?)),
            Some("file") if extra["command"] == "download" => {
                emit(Event::FileUpdated(media_file(value)?))
            }
            Some("updateNewMessage") => {
                if let Some(message) = message(&value["message"], &self.users, &self.chats)? {
                    emit(Event::IncomingMessage(message));
                }
            }
            Some("updateMessageSendSucceeded" | "updateMessageSendFailed") => {
                let key = (
                    ChatId(integer(&value["message"]["chat_id"])?),
                    MessageId(integer(&value["old_message_id"])?),
                );
                if let Some(path) = self.uploads.remove(&key) {
                    emit(Event::LocalFileSendFinished(path));
                } else {
                    if self.finished_uploads.len() == 128 {
                        self.finished_uploads.pop_front();
                    }
                    self.finished_uploads.push_back(key);
                }
                if let Some(mut message) = message(&value["message"], &self.users, &self.chats)? {
                    message.send_state = if value["@type"] == "updateMessageSendFailed" {
                        SendState::Failed(td_error(&value["error"]))
                    } else {
                        SendState::Sent
                    };
                    emit(Event::MessageUpdated {
                        old_id: MessageId(integer(&value["old_message_id"])?),
                        message,
                    });
                }
            }
            Some("message")
                if matches!(
                    extra["command"].as_str(),
                    Some("send_text" | "edit_text" | "send_photo" | "send_document")
                ) =>
            {
                if matches!(
                    extra["command"].as_str(),
                    Some("send_photo" | "send_document")
                ) {
                    let path = std::path::PathBuf::from(string(&extra["local_path"], 4096)?);
                    let key = (
                        ChatId(integer(&value["chat_id"])?),
                        MessageId(integer(&value["id"])?),
                    );
                    let already_finished = self
                        .finished_uploads
                        .iter()
                        .position(|k| *k == key)
                        .and_then(|i| self.finished_uploads.remove(i))
                        .is_some();
                    if already_finished {
                        emit(Event::LocalFileSendFinished(path));
                        // The terminal update already delivered the final message (the UI
                        // appends it when the pending one was never shown); a pending copy
                        // here would reintroduce a stale ID.
                        return Ok(());
                    }
                    if value["sending_state"]["@type"] == "messageSendingStatePending" {
                        // A saturated tracker retains source files in the UI until shutdown.
                        if self.uploads.len() < 128 {
                            self.uploads.insert(key, path);
                        }
                    } else {
                        emit(Event::LocalFileSendFinished(path));
                    }
                }
                if let Some(message) = message(value, &self.users, &self.chats)? {
                    emit(Event::NewMessage(message));
                }
            }
            Some("messages")
                if extra["command"] == "history" || extra["command"] == "history_newer" =>
            {
                let source = value["messages"].as_array().ok_or_else(malformed)?;
                if source.len() > PAGE_SIZE {
                    return Err(malformed());
                }
                let chat = ChatId(integer(&extra["chat"])?);
                let newer = extra["command"] == "history_newer";
                let before = MessageId(integer(&extra[if newer { "after" } else { "before" }])?);
                let mut next_before = None;
                // One page only, at most PAGE_SIZE messages. No retained message storage.
                let mut messages = Vec::with_capacity(source.len());
                for item in source {
                    let id = MessageId(integer(&item["id"])?);
                    if integer(&item["chat_id"])? != chat.0 {
                        return Err(malformed());
                    }
                    if (newer && id <= before) || (!newer && before.0 != 0 && id >= before) {
                        continue;
                    }
                    next_before = Some(next_before.map_or(id, |previous: MessageId| {
                        if newer {
                            previous.max(id)
                        } else {
                            previous.min(id)
                        }
                    }));
                    if let Some(message) = message(item, &self.users, &self.chats)? {
                        if message.chat != chat {
                            return Err(malformed());
                        }
                        messages.push(message);
                    }
                }
                if newer {
                    emit(Event::NewerMessagesLoaded {
                        chat,
                        after: before,
                        next_after: next_before,
                        messages,
                    });
                } else {
                    emit(Event::MessagesLoaded {
                        chat,
                        before,
                        next_before,
                        messages,
                    });
                }
            }
            Some("ok") if extra["command"] == "chats" => {
                emit(Event::ChatsLoaded { has_more: true })
            }
            Some("error") if extra["command"] == "chats" && value["code"] == 404 => {
                emit(Event::ChatsLoaded { has_more: false })
            }
            Some("error") => {
                if matches!(
                    extra["command"].as_str(),
                    Some("send_photo" | "send_document")
                ) {
                    emit(Event::LocalFileSendFinished(std::path::PathBuf::from(
                        string(&extra["local_path"], 4096)?,
                    )));
                }
                emit(Event::CommandFailed {
                    command: command_kind(&extra["command"]),
                    error: td_error(value),
                });
            }
            _ => {}
        }
        Ok(())
    }
}

pub(crate) fn parse(json: &str) -> Result<Value, CommandError> {
    if json.len() > MAX_JSON_BYTES {
        return Err(malformed());
    }
    serde_json::from_str(json).map_err(|_| malformed())
}

pub(crate) fn content_label(content: &Value) -> &str {
    match content["@type"].as_str() {
        Some("messagePhoto") => "Photo",
        Some("messageVideo") => "Video",
        Some("messageAnimation") => "Animation",
        Some("messageSticker") => "Sticker",
        Some("messageDocument") => "File",
        Some("messageVoiceNote") => "Voice message",
        Some("messageAudio") => "Audio",
        Some("messageVideoNote") => "Video message",
        Some("messageAnimatedEmoji") => content["emoji"].as_str().unwrap_or(""),
        Some("messageLocation" | "messageVenue") => "Location",
        Some("messageContact") => "Contact",
        Some("messagePoll") => "Poll",
        _ => "Unsupported message",
    }
}
pub(crate) fn media_file(value: &Value) -> Result<MediaFile, CommandError> {
    let id = i32::try_from(integer(&value["id"])?).map_err(|_| malformed())?;
    if id <= 0 {
        return Err(malformed());
    }
    let path = if value["local"]["is_downloading_completed"] == true {
        Some(std::path::PathBuf::from(string(
            &value["local"]["path"],
            MAX_LINK_BYTES,
        )?))
    } else {
        None
    };
    let size = value["size"]
        .as_u64()
        .filter(|n| *n > 0)
        .or_else(|| value["expected_size"].as_u64())
        .unwrap_or(0)
        .min((1 << 53) - 1);
    let partial_path = value["local"]["path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() > MAX_LINK_BYTES {
                Err(malformed())
            } else {
                Ok(std::path::PathBuf::from(s))
            }
        })
        .transpose()?;
    Ok(MediaFile {
        partial: Some(tg_core::StreamFile {
            id,
            size,
            path: partial_path,
            offset: value["local"]["download_offset"]
                .as_u64()
                .unwrap_or(0)
                .min(size),
            prefix: value["local"]["downloaded_prefix_size"]
                .as_u64()
                .unwrap_or(0)
                .min(size),
            completed: value["local"]["is_downloading_completed"] == true,
        }),
        size: value["size"]
            .as_u64()
            .filter(|n| *n > 0)
            .or_else(|| value["expected_size"].as_u64())
            .unwrap_or(0)
            .min(1 << 53),
        downloaded: value["local"]["downloaded_size"]
            .as_u64()
            .unwrap_or(0)
            .min(1 << 53),
        downloading: value["local"]["is_downloading_active"] == true,
        id,
        path,
    })
}
fn mini(value: &Value) -> Result<Vec<u8>, CommandError> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let encoded = string(&value["data"], 12 * 1024)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| malformed())?;
    if bytes.len() > 8192 {
        return Err(malformed());
    }
    Ok(bytes)
}
pub(crate) fn avatar(value: &Value) -> Result<Option<Photo>, CommandError> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(Photo {
        file: media_file(&value["small"])?,
        width: 160,
        height: 160,
        minithumbnail: mini(&value["minithumbnail"])?,
    }))
}
/// Video file plus a JPEG preview frame: the custom cover if any, else the JPEG thumbnail.
/// The preview carries the video's own size so the feed lays it out at the right aspect ratio.
fn video(content: &Value) -> Result<(Option<Photo>, Option<Video>), CommandError> {
    let value = &content["video"];
    let size = |key: &str| {
        value[key]
            .as_u64()
            .filter(|n| (1..=8192).contains(n))
            .map_or(1, |n| n as u16)
    };
    let (width, height) = (size("width"), size("height"));
    let thumbnail = &value["thumbnail"];
    let preview = if let Some(cover) = photo(&content["cover"])? {
        Some(cover)
    } else if thumbnail["format"]["@type"] == "thumbnailFormatJpeg" {
        Some(Photo {
            file: media_file(&thumbnail["file"])?,
            width,
            height,
            minithumbnail: mini(&value["minithumbnail"])?,
        })
    } else {
        // No decodable preview: the feed draws a dark cell with a play button.
        None
    };
    let duration = value["duration"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    Ok((
        preview,
        Some(Video {
            supports_streaming: value["supports_streaming"] == true,
            file: media_file(&value["video"])?,
            duration,
            width,
            height,
        }),
    ))
}
fn photo(value: &Value) -> Result<Option<Photo>, CommandError> {
    let Some(sizes) = value["sizes"].as_array() else {
        return Ok(None);
    };
    if sizes.len() > 32 {
        return Err(malformed());
    }
    // Prefer TDLib's display-size variant, never request the original for a thumbnail.
    let valid = |v: &&Value| {
        v["width"].as_u64().is_some_and(|n| n > 0 && n <= 8192)
            && v["height"].as_u64().is_some_and(|n| n > 0 && n <= 8192)
    };
    let selected = sizes.iter().filter(valid).min_by_key(|v| {
        let side = v["width"]
            .as_u64()
            .unwrap_or(0)
            .max(v["height"].as_u64().unwrap_or(0));
        if side >= 504 { side } else { 100000 - side }
    });
    let Some(v) = selected else {
        return Ok(None);
    };
    Ok(Some(Photo {
        file: media_file(&v["photo"])?,
        width: v["width"].as_u64().unwrap_or(1) as u16,
        height: v["height"].as_u64().unwrap_or(1) as u16,
        minithumbnail: mini(&value["minithumbnail"])?,
    }))
}

fn attachment(content: &Value) -> Result<Option<tg_core::Attachment>, CommandError> {
    use tg_core::{Attachment, Document, Round, Sticker, Voice};
    Ok(match content["@type"].as_str() {
        Some("messageVideoNote") => {
            let v = &content["video_note"];
            Some(Attachment::Round(Round {
                file: media_file(&v["video"])?,
                duration: v["duration"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
                length: v["length"].as_u64().unwrap_or(240).clamp(1, 240) as u16,
                viewed: content["is_viewed"] == true,
            }))
        }
        Some("messageAnimatedEmoji") => {
            let s = &content["animated_emoji"]["sticker"];
            Some(Attachment::Emoji(Sticker {
                emoji: bounded_label(content["emoji"].as_str().unwrap_or(""), 64),
                width: s["width"].as_u64().unwrap_or(128).clamp(1, 2048) as u16,
                height: s["height"].as_u64().unwrap_or(128).clamp(1, 2048) as u16,
                animated: s["format"]["@type"] != "stickerFormatWebp",
            }))
        }
        Some("messageVoiceNote") => {
            let v = &content["voice_note"];
            let encoded = v["waveform"].as_str().unwrap_or("");
            if encoded.len() > 344 {
                return Err(malformed());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| malformed())?;
            Some(Attachment::Voice(Voice {
                file: media_file(&v["voice"])?,
                duration: v["duration"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
                waveform: tg_core::decode_waveform(&bytes).map_err(|_| malformed())?,
                listened: content["is_listened"] == true,
            }))
        }
        Some("messageDocument") => Some(Attachment::Document(Document {
            file: media_file(&content["document"]["document"])?,
            name: bounded_label(
                content["document"]["file_name"].as_str().unwrap_or("File"),
                256,
            ),
        })),
        Some("messageSticker") => {
            let s = &content["sticker"];
            Some(Attachment::Sticker(Sticker {
                emoji: bounded_label(
                    s["emoji"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("\u{1f5bc}"),
                    64,
                ),
                width: s["width"].as_u64().unwrap_or(192).clamp(1, 2048) as u16,
                height: s["height"].as_u64().unwrap_or(192).clamp(1, 2048) as u16,
                animated: s["format"]["@type"] != "stickerFormatWebp",
            }))
        }
        _ => None,
    })
}
fn thumbnail(value: &Value, mini_id: Option<i32>) -> Result<Option<Photo>, CommandError> {
    let t = &value["thumbnail"];
    let mini = mini(&value["minithumbnail"])?;
    if !t.is_null()
        && matches!(
            t["format"]["@type"].as_str(),
            Some("thumbnailFormatJpeg" | "thumbnailFormatWebp")
        )
    {
        Ok(Some(Photo {
            file: media_file(&t["file"])?,
            width: t["width"].as_u64().unwrap_or(48).clamp(1, 2048) as u16,
            height: t["height"].as_u64().unwrap_or(48).clamp(1, 2048) as u16,
            minithumbnail: mini,
        }))
    } else if let Some(id) = mini_id
        && !mini.is_empty()
    {
        // A minithumbnail-only key must never start a download of the document itself.
        Ok(Some(Photo {
            file: MediaFile {
                partial: None,
                id: -id,
                size: 0,
                downloaded: 0,
                downloading: false,
                path: None,
            },
            width: 48,
            height: 48,
            minithumbnail: mini,
        }))
    } else {
        Ok(None)
    }
}
/// Labels for non-text chat previews, separate from message captions and service contents.
pub(crate) fn attachment_preview(content: &Value) -> Option<String> {
    match content["@type"].as_str() {
        Some("messageSticker") => Some(format!(
            "Sticker {}",
            bounded_label(content["sticker"]["emoji"].as_str().unwrap_or(""), 64)
        )),
        Some("messageVoiceNote") => Some("Voice message".into()),
        Some("messageVideoNote") => Some("Video message".into()),
        Some("messageAnimatedEmoji") => {
            Some(bounded_label(content["emoji"].as_str().unwrap_or(""), 64))
        }
        Some("messageDocument") => Some(bounded_label(
            content["document"]["file_name"].as_str().unwrap_or("File"),
            256,
        )),
        _ => None,
    }
}
fn bounded_label(value: &str, limit: usize) -> String {
    let mut result = String::with_capacity(value.len().min(limit));
    for c in value.chars() {
        let c = if c == '\r' || c == '\n' { ' ' } else { c };
        if result.len() + c.len_utf8() > limit {
            break;
        }
        result.push(c);
    }
    result
}
