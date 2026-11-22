use tg_core::{AuthState, ChatId, ChatListChange, EntityKind, Event, MessageId};
use tg_telegram::Decoder;

fn decode(decoder: &mut Decoder, json: &str) -> Vec<Event> {
    // Fixtures produce at most three events per update.
    let mut events = Vec::new();
    let result = decoder.decode(json, |event| events.push(event));
    assert_eq!(result, Ok(()));
    assert!(events.len() <= 3);
    events
}

#[test]
fn outgoing_documents_are_displayed_and_release_sources_only_after_terminal_updates() {
    use serde_json::json;
    let mut decoder = Decoder::default();
    let pending = json!({"@type":"message","id":-100,"chat_id":7,"date":1,"is_outgoing":true,
        "sending_state":{"@type":"messageSendingStatePending"},
        "@extra":{"command":"send_document","local_path":"C:/source.zip"},
        "content":{"@type":"messageDocument","caption":{"text":"Caption","entities":[]},
            "document":{"file_name":"source.zip","document":{"id":42,"size":4096,"local":{}}}}});
    let events = decode(&mut decoder, &pending.to_string());
    assert!(
        matches!(&events[..], [Event::NewMessage(m)] if m.send_state == tg_core::SendState::Pending && m.text == "Caption")
    );
    let done = json!({"@type":"updateMessageSendSucceeded","old_message_id":-100,"message":{
        "id":1048576,"chat_id":7,"date":1,"is_outgoing":true,"content":pending["content"]}});
    let events = decode(&mut decoder, &done.to_string());
    assert!(events.contains(&Event::LocalFileSendFinished("C:/source.zip".into())));
    assert!(
        events
            .iter()
            .any(|e| matches!(e,Event::MessageUpdated {old_id,..} if old_id.0 == -100))
    );
}

#[test]
fn upload_errors_keep_command_identity_and_release_the_source() {
    for (tag, kind) in [
        ("send_photo", tg_core::CommandKind::SendPhoto),
        ("send_document", tg_core::CommandKind::SendDocument),
    ] {
        let events = decode(
            &mut Decoder::default(),
            &serde_json::json!({"@type":"error","code":400,"message":"private path",
            "@extra":{"command":tag,"local_path":"C:/paste.png"}})
            .to_string(),
        );
        assert_eq!(
            events[0],
            Event::LocalFileSendFinished("C:/paste.png".into())
        );
        assert!(
            matches!(&events[1],Event::CommandFailed{command,error} if *command == kind && !error.message.contains("private path"))
        );
    }
}

#[test]
fn upload_completion_can_arrive_before_the_tagged_pending_response() {
    use serde_json::json;
    let mut decoder = Decoder::default();
    let content = json!({"@type":"messageText","text":{"text":"caption","entities":[]}});
    decode(
        &mut decoder,
        &json!({"@type":"updateMessageSendSucceeded","old_message_id":-1,
        "message":{"id":1048576,"chat_id":7,"date":1,"is_outgoing":true,"content":content}})
        .to_string(),
    );
    let events = decode(
        &mut decoder,
        &json!({"@type":"message","id":-1,"chat_id":7,"date":1,"is_outgoing":true,
        "sending_state":{"@type":"messageSendingStatePending"},"content":content,
        "@extra":{"command":"send_photo","local_path":"C:/paste.png"}})
        .to_string(),
    );
    assert!(events.contains(&Event::LocalFileSendFinished("C:/paste.png".into())));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::NewMessage(_)))
    );
}

#[test]
fn qr_confirmation_exposes_a_link_without_leaking_it_in_debug() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateWaitOtherDeviceConfirmation","link":"tg://synthetic"}}"#,
    );
    assert_eq!(
        events,
        [Event::AuthStateChanged(AuthState::WaitOtherDevice {
            link: "tg://synthetic".into()
        })]
    );
    assert!(!format!("{:?}", events[0]).contains("tg://synthetic"));
}

#[test]
fn non_bmp_text_keeps_utf16_entity_offsets() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewMessage","message":{"id":12,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"A😀bold","entities":[{"offset":3,"length":4,"type":{"@type":"textEntityTypeBold"}}]}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("expected message")
    };
    assert_eq!(message.chat, ChatId(7));
    assert_eq!(message.id, MessageId(12));
    assert_eq!(message.text, "A😀bold");
    assert_eq!(message.entities[0].offset, 3);
    assert_eq!(message.entities[0].length, 4);
    assert_eq!(message.entities[0].kind, EntityKind::Bold);
}

#[test]
fn main_list_orders_by_int64_then_chat_id_and_emits_positions() {
    let mut decoder = Decoder::default();
    let first = decode(
        &mut decoder,
        r#"{"@type":"updateNewChat","chat":{"id":1,"title":"One","positions":[{"list":{"@type":"chatListMain"},"order":"9007199254740993"}]}}"#,
    );
    assert!(
        matches!(&first[0], Event::ChatListChanged(ChatListChange::Insert { position: 0, chat }) if chat.id == ChatId(1))
    );
    let second = decode(
        &mut decoder,
        r#"{"@type":"updateNewChat","chat":{"id":2,"title":"Two","positions":[{"list":{"@type":"chatListMain"},"order":"9007199254740993"}]}}"#,
    );
    assert!(
        matches!(&second[0], Event::ChatListChanged(ChatListChange::Insert { position: 0, chat }) if chat.id == ChatId(2))
    );
    let moved = decode(
        &mut decoder,
        r#"{"@type":"updateChatPosition","chat_id":1,"position":{"list":{"@type":"chatListMain"},"order":"9007199254740994"}}"#,
    );
    assert_eq!(
        moved,
        [Event::ChatListChanged(ChatListChange::Move {
            from: 1,
            to: 0
        })]
    );
    let removed = decode(
        &mut decoder,
        r#"{"@type":"updateChatPosition","chat_id":1,"position":{"list":{"@type":"chatListMain"},"order":"0"}}"#,
    );
    assert_eq!(
        removed,
        [Event::ChatListChanged(ChatListChange::Remove {
            position: 0
        })]
    );
    let archived = decode(
        &mut decoder,
        r#"{"@type":"updateChatPosition","chat_id":2,"position":{"list":{"@type":"chatListArchive"},"order":"0"}}"#,
    );
    assert!(archived.is_empty());
}

#[test]
fn unknown_error_text_is_not_forwarded() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"error","code":400,"message":"private server detail","@extra":{"command":"send_text"}}"#,
    );
    let Event::CommandFailed { error, .. } = &events[0] else {
        panic!("expected failure")
    };
    assert_eq!(error.code, 400);
    assert!(!error.message.contains("private server detail"));
}

#[test]
fn older_page_excludes_the_anchor_and_advances_across_non_text_messages() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"messages","@extra":{"command":"history","chat":7,"before":31457280},"messages":[{"id":31457280,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"anchor","entities":[]}}},{"id":20971520,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messagePhoto"}},{"id":10485760,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messagePhoto"}}]}"#,
    );
    let Event::MessagesLoaded {
        messages,
        next_before,
        ..
    } = &events[0]
    else {
        panic!("expected page")
    };
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.text == "Photo"));
    assert_eq!(*next_before, Some(MessageId(10485760)));
}

#[test]
fn invalid_utf16_ranges_are_rejected_without_splitting_a_surrogate_pair() {
    let mut decoder = Decoder::default();
    assert!(decoder.decode(r#"{"@type":"updateNewMessage","message":{"id":12,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"A😀bold","entities":[{"offset":2,"length":1,"type":{"@type":"textEntityTypeBold"}}]}}}}"#, |_| {}).is_err());
}

#[test]
fn title_read_status_and_last_message_update_the_current_row() {
    let mut decoder = Decoder::default();
    decode(
        &mut decoder,
        r#"{"@type":"updateNewChat","chat":{"id":1,"title":"One","positions":[{"list":{"@type":"chatListMain"},"order":"100"}]}}"#,
    );
    let title = decode(
        &mut decoder,
        r#"{"@type":"updateChatTitle","chat_id":1,"title":"Renamed"}"#,
    );
    assert!(
        matches!(&title[0], Event::ChatListChanged(ChatListChange::Update { position: 0, chat }) if chat.title == "Renamed")
    );
    let read = decode(
        &mut decoder,
        r#"{"@type":"updateChatReadInbox","chat_id":1,"unread_count":4,"last_read_inbox_message_id":42}"#,
    );
    assert!(
        matches!(&read[0], Event::ChatListChanged(ChatListChange::Update { position: 0, chat }) if chat.unread_count == 4 && chat.last_read_inbox == MessageId(42))
    );
    let last = decode(
        &mut decoder,
        r#"{"@type":"updateChatLastMessage","chat_id":1,"last_message":{"id":50,"content":{"@type":"messageText","text":{"text":"preview"}}},"positions":[{"list":{"@type":"chatListMain"},"order":"200"}]}"#,
    );
    assert!(
        matches!(&last[0], Event::ChatListChanged(ChatListChange::Update { position: 0, chat }) if chat.last_message.as_ref().is_some_and(|last| last.id == MessageId(50) && last.text == "preview"))
    );
    let removed = decode(
        &mut decoder,
        r#"{"@type":"updateChatLastMessage","chat_id":1,"last_message":null,"positions":[]}"#,
    );
    assert_eq!(
        removed,
        [Event::ChatListChanged(ChatListChange::Remove {
            position: 0
        })]
    );
    let inserted = decode(
        &mut decoder,
        r#"{"@type":"updateChatPosition","chat_id":1,"position":{"list":{"@type":"chatListMain"},"order":"100"}}"#,
    );
    assert!(
        matches!(&inserted[0], Event::ChatListChanged(ChatListChange::Insert { position: 0, chat }) if chat.title == "Renamed" && chat.unread_count == 4 && chat.last_message.is_none())
    );
}

#[test]
fn send_completion_replaces_the_temporary_id_and_sanitizes_failure() {
    let mut decoder = Decoder::default();
    let success = decode(
        &mut decoder,
        r#"{"@type":"updateMessageSendSucceeded","old_message_id":-1,"message":{"id":12,"chat_id":7,"date":1,"is_outgoing":true,"content":{"@type":"messageText","text":{"text":"sent","entities":[]}}}}"#,
    );
    assert!(
        matches!(&success[0], Event::MessageUpdated { old_id: MessageId(-1), message } if message.id == MessageId(12) && message.send_state == tg_core::SendState::Sent)
    );
    let failed = decode(
        &mut decoder,
        r#"{"@type":"updateMessageSendFailed","old_message_id":-2,"error":{"code":400,"message":"private server detail"},"message":{"id":-3,"chat_id":7,"date":1,"is_outgoing":true,"content":{"@type":"messageText","text":{"text":"unsent","entities":[]}}}}"#,
    );
    assert!(
        matches!(&failed[0], Event::MessageUpdated { old_id: MessageId(-2), message } if matches!(&message.send_state, tg_core::SendState::Failed(error) if error.code == 400 && !error.message.contains("private server detail")))
    );
}

#[test]
fn every_required_entity_kind_is_mapped_without_changing_utf16_units() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewMessage","message":{"id":12,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"😀x","entities":[{"offset":0,"length":2,"type":{"@type":"textEntityTypeBold"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeItalic"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeUnderline"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeStrikethrough"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeCode"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypePre"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeSpoiler"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeUrl"}},{"offset":0,"length":2,"type":{"@type":"textEntityTypeTextUrl","url":"https://example.invalid/"}},{"offset":2,"length":1,"type":{"@type":"textEntityTypeMention"}}]}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("expected message")
    };
    // Fixed list of ten expected entity kinds; no dynamically growing expected data.
    let expected = [
        EntityKind::Bold,
        EntityKind::Italic,
        EntityKind::Underline,
        EntityKind::Strikethrough,
        EntityKind::Code,
        EntityKind::Pre,
        EntityKind::Spoiler,
        EntityKind::Url,
        EntityKind::TextUrl {
            target: "https://example.invalid/".into(),
        },
        EntityKind::Mention,
    ];
    assert_eq!(message.entities.len(), expected.len());
    for (entity, expected) in message.entities.iter().zip(expected) {
        assert_eq!(entity.kind, expected);
    }
    assert_eq!(message.entities[9].offset, 2);
}

#[test]
fn only_an_explicit_load_chats_response_reports_list_exhaustion() {
    let mut decoder = Decoder::default();
    assert_eq!(
        decode(
            &mut decoder,
            r#"{"@type":"error","code":404,"message":"synthetic","@extra":{"command":"chats"}}"#
        ),
        [Event::ChatsLoaded { has_more: false }]
    );
    assert_eq!(
        decode(
            &mut decoder,
            r#"{"@type":"ok","@extra":{"command":"chats"}}"#
        ),
        [Event::ChatsLoaded { has_more: true }]
    );
    assert!(matches!(
        &decode(
            &mut decoder,
            r#"{"@type":"error","code":404,"message":"synthetic","@extra":{"command":"open"}}"#
        )[0],
        Event::CommandFailed {
            command: tg_core::CommandKind::OpenChat,
            ..
        }
    ));
}

#[test]
fn oversized_pages_and_malformed_json_return_sanitized_errors() {
    let mut decoder = Decoder::default();
    assert!(decoder.decode("not json", |_| {}).is_err());
    // Exactly 51 tiny synthetic values, bounded below 1 KiB total.
    let source = serde_json::json!({"@type":"messages", "@extra":{"command":"history", "chat":7, "before":0}, "messages":vec![serde_json::Value::Null; 51]});
    assert!(decoder.decode(&source.to_string(), |_| {}).is_err());
}

#[test]
fn chat_metadata_limit_is_explicit_and_reported() {
    let mut decoder = Decoder::default();
    // Fixed 2,049 synthetic rows; no collected events or unbounded fixture storage.
    for id in 1..=2048 {
        let source = serde_json::json!({"@type":"updateNewChat", "chat":{"id":id, "title":"row", "positions":[]}});
        assert_eq!(decoder.decode(&source.to_string(), |_| {}), Ok(()));
    }
    assert!(matches!(
        decoder.decode(
            r#"{"@type":"updateNewChat","chat":{"id":2049,"title":"overflow","positions":[]}}"#,
            |_| {}
        ),
        Err(tg_core::CommandError { code: -3, .. })
    ));
}

#[test]
fn newer_page_filters_anchor_and_uses_maximum_raw_id_for_cursor() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"messages","@extra":{"command":"history_newer","chat":7,"after":104857600},"messages":[{"id":157286400,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messagePhoto"}},{"id":125829120,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"newer","entities":[]}}},{"id":104857600,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messagePhoto"}}]}"#,
    );
    let Event::NewerMessagesLoaded {
        after,
        next_after,
        messages,
        ..
    } = &events[0]
    else {
        panic!("expected newer history");
    };
    assert_eq!(*after, MessageId(104857600));
    assert_eq!(*next_after, Some(MessageId(157286400)));
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].id, MessageId(125829120));
}

#[test]
fn unsupported_content_is_a_visible_row() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewMessage","message":{"id":104857600,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageAnimation"}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("placeholder row")
    };
    assert_eq!(message.text, "Animation");
}

#[test]
fn video_maps_file_duration_caption_and_jpeg_preview() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewMessage","message":{"id":104857600,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageVideo","cover":null,"caption":{"text":"Clip","entities":[]},"video":{"duration":75,"width":1280,"height":720,"minithumbnail":null,"thumbnail":{"format":{"@type":"thumbnailFormatJpeg"},"width":320,"height":180,"file":{"id":11,"local":{"is_downloading_completed":false}}},"video":{"id":12,"local":{"is_downloading_completed":false}}}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("video row")
    };
    let video = message.video.as_ref().expect("video");
    assert_eq!((video.file.id, video.duration), (12, 75));
    let preview = message.photo.as_ref().expect("preview");
    assert_eq!(
        (preview.file.id, preview.width, preview.height),
        (11, 1280, 720)
    );
    assert_eq!(message.text, "Clip");
    // An MPEG-4 thumbnail cannot be decoded: no preview frame, the video stays playable.
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewMessage","message":{"id":105906176,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageVideo","cover":null,"caption":{"text":"","entities":[]},"video":{"duration":5,"width":640,"height":360,"minithumbnail":null,"thumbnail":{"format":{"@type":"thumbnailFormatMpeg4"},"width":320,"height":180,"file":{"id":13,"local":{"is_downloading_completed":false}}},"video":{"id":14,"local":{"is_downloading_completed":false}}}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("video row")
    };
    assert!(message.photo.is_none());
    assert_eq!(message.video.as_ref().map(|v| v.file.id), Some(14));
}

#[test]
fn photo_caption_and_display_variant_survive_mapping() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewMessage","message":{"id":104857600,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messagePhoto","caption":{"text":"A caption","entities":[{"offset":2,"length":7,"type":{"@type":"textEntityTypeBold"}}]},"photo":{"minithumbnail":{"data":"AQID"},"sizes":[{"width":90,"height":60,"photo":{"id":1}},{"width":800,"height":600,"photo":{"id":2}},{"width":4000,"height":3000,"photo":{"id":3}}]}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("photo row")
    };
    assert_eq!(message.text, "A caption");
    assert_eq!(message.entities.len(), 1);
    let photo = message.photo.as_ref().unwrap();
    assert_eq!(photo.file.id, 2);
    assert_eq!((photo.width, photo.height), (800, 600));
    assert_eq!(photo.minithumbnail, [1, 2, 3]);
}
#[test]
fn avatars_change_and_file_completion_uses_domain_metadata() {
    let mut decoder = Decoder::default();
    let events = decode(
        &mut decoder,
        r#"{"@type":"updateNewChat","chat":{"id":7,"title":"Synthetic","photo":{"small":{"id":9}},"positions":[{"list":{"@type":"chatListMain"},"order":"100"}]}}"#,
    );
    let Event::ChatListChanged(ChatListChange::Insert { chat, .. }) = &events[0] else {
        panic!("chat")
    };
    assert_eq!(chat.avatar.as_ref().unwrap().file.id, 9);
    let events = decode(
        &mut decoder,
        r#"{"@type":"updateChatPhoto","chat_id":7,"photo":null}"#,
    );
    assert!(
        matches!(&events[0],Event::ChatListChanged(ChatListChange::Update {chat,..}) if chat.avatar.is_none())
    );
    let events = decode(
        &mut decoder,
        r#"{"@type":"updateFile","file":{"id":9,"local":{"is_downloading_completed":true,"path":"C:/synthetic/photo.jpg"}}}"#,
    );
    let Event::FileUpdated(file) = &events[0] else {
        panic!("file")
    };
    assert_eq!(file.id, 9);
    assert!(file.path.is_some());
    assert!(!format!("{file:?}").contains("photo.jpg"));
}
#[test]
fn oversized_minithumbnails_and_paths_are_rejected() {
    let oversized = "A".repeat(12289);
    let json = format!(
        r#"{{"@type":"updateNewChat","chat":{{"id":7,"title":"Synthetic","photo":{{"small":{{"id":9}},"minithumbnail":{{"data":"{oversized}"}}}},"positions":[]}}}}"#
    );
    assert!(Decoder::default().decode(&json, |_| {}).is_err());
    let json = format!(
        r#"{{"@type":"updateFile","file":{{"id":9,"local":{{"is_downloading_completed":true,"path":"{}"}}}}}}"#,
        "x".repeat(4097)
    );
    assert!(Decoder::default().decode(&json, |_| {}).is_err());
}

#[test]
fn group_sender_name_album_and_private_chat_sender() {
    let mut decoder = Decoder::default();
    assert!(
        decode(
            &mut decoder,
            r#"{"@type":"updateUser","user":{"id":5,"first_name":"Anna","last_name":"Petrova"}}"#
        )
        .is_empty()
    );
    let events = decode(
        &mut decoder,
        r#"{"@type":"updateNewMessage","message":{"id":1048576,"chat_id":-1001,"date":1,"is_outgoing":false,"media_album_id":"13887604000000001","sender_id":{"@type":"messageSenderUser","user_id":5},"content":{"@type":"messageText","text":{"text":"hi","entities":[]}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("expected message")
    };
    assert_eq!(message.sender.as_deref(), Some("Anna Petrova"));
    assert_eq!((message.sender_id, message.album), (5, 13887604000000001));
    // In a private chat the sender is the chat itself: no name above the bubble.
    let events = decode(
        &mut decoder,
        r#"{"@type":"updateNewMessage","message":{"id":2097152,"chat_id":5,"date":1,"is_outgoing":false,"sender_id":{"@type":"messageSenderUser","user_id":5},"content":{"@type":"messageText","text":{"text":"hi","entities":[]}}}}"#,
    );
    let (Event::NewMessage(message) | Event::IncomingMessage(message)) = &events[0] else {
        panic!("expected message")
    };
    assert_eq!((message.sender.as_deref(), message.album), (None, 0));
}

#[test]
fn notification_scope_defaults_overrides_and_mentions_are_mapped() {
    use serde_json::json;
    let mut decoder = Decoder::default();
    let mut chat = json!({"id":7,"title":"Synthetic group","type":{"@type":"chatTypeSupergroup","is_channel":false},"last_message":null,"positions":[{"list":{"@type":"chatListMain"},"order":"100"}],"unread_count":3,"unread_mention_count":2,"notification_settings":{"use_default_mute_for":true,"use_default_show_preview":true}});
    let inserted = decode(
        &mut decoder,
        &json!({"@type":"updateNewChat","chat":chat}).to_string(),
    );
    assert!(
        matches!(&inserted[0],Event::ChatListChanged(ChatListChange::Insert {chat,..}) if chat.unread_mentions==2)
    );
    let changed = decode(
        &mut decoder,
        r#"{"@type":"updateScopeNotificationSettings","scope":{"@type":"notificationSettingsScopeGroupChats"},"notification_settings":{"mute_for":3600,"show_preview":false}}"#,
    );
    assert!(
        matches!(&changed[0],Event::ChatListChanged(ChatListChange::Update {chat,..}) if chat.muted_until>0 && !chat.show_notification_preview)
    );
    let changed = decode(
        &mut decoder,
        r#"{"@type":"updateChatNotificationSettings","chat_id":7,"notification_settings":{"use_default_mute_for":false,"mute_for":0,"use_default_show_preview":false,"show_preview":true}}"#,
    );
    assert!(
        matches!(&changed[0],Event::ChatListChanged(ChatListChange::Update {chat,..}) if chat.muted_until==0 && chat.show_notification_preview)
    );
    assert!(decode(&mut decoder,r#"{"@type":"updateScopeNotificationSettings","scope":{"@type":"notificationSettingsScopeGroupChats"},"notification_settings":{"mute_for":0,"show_preview":false}}"#).is_empty());
    let changed = decode(
        &mut decoder,
        r#"{"@type":"updateChatUnreadMentionCount","chat_id":7,"unread_mention_count":1}"#,
    );
    assert!(
        matches!(&changed[0],Event::ChatListChanged(ChatListChange::Update {chat,..}) if chat.unread_mentions==1)
    );
    chat["id"] = 8.into();
    chat["type"] = json!({"@type":"chatTypeSupergroup","is_channel":true});
    decode(
        &mut decoder,
        &json!({"@type":"updateNewChat","chat":chat}).to_string(),
    );
    assert!(decode(&mut decoder,r#"{"@type":"updateScopeNotificationSettings","scope":{"@type":"notificationSettingsScopePrivateChats"},"notification_settings":{"mute_for":0,"show_preview":true}}"#).is_empty());
    let changed = decode(
        &mut decoder,
        r#"{"@type":"updateScopeNotificationSettings","scope":{"@type":"notificationSettingsScopeChannelChats"},"notification_settings":{"mute_for":0,"show_preview":true}}"#,
    );
    assert!(
        matches!(&changed[0],Event::ChatListChanged(ChatListChange::Update {chat,..}) if chat.id==ChatId(8) && chat.muted_until==0)
    );
}
#[test]
fn around_history_live_origin_read_completion_and_global_unread_are_distinct() {
    let mut decoder = Decoder::default();
    let message = serde_json::json!({"id":2097152,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"Synthetic","entities":[]}}});
    let live = decode(
        &mut decoder,
        &serde_json::json!({"@type":"updateNewMessage","message":message}).to_string(),
    );
    assert!(matches!(&live[0], Event::IncomingMessage(_)));
    let page=decode(&mut decoder,&serde_json::json!({"@type":"messages","messages":[message],"@extra":{"command":"history_around","chat":7,"anchor":1048576}}).to_string());
    assert!(
        matches!(&page[0],Event::MessagesAroundLoaded {anchor:MessageId(1048576),messages,..} if messages.len()==1)
    );
    for (kind, success) in [("ok", true), ("error", false)] {
        assert_eq!(
            decode(
                &mut decoder,
                &serde_json::json!({"@type":kind,"@extra":{"command":"view","chat":7}}).to_string()
            ),
            vec![Event::MessagesViewed {
                chat: ChatId(7),
                success
            }]
        );
    }
    assert_eq!(
        decode(
            &mut decoder,
            r#"{"@type":"updateUnreadMessageCount","chat_list":{"@type":"chatListMain"},"unread_count":123}"#
        ),
        vec![Event::TotalUnreadCount(123)]
    );
}

#[test]
fn actions_map_reply_forward_edit_and_permanent_deletions() {
    use serde_json::json;
    let mut d = Decoder::default();
    let value = json!({"@type":"updateNewMessage","message": {
        "id":20971520000_i64,"chat_id":7,"date":10,"edit_date":11,"is_outgoing":true,
        "reply_to":{"@type":"messageReplyToMessage","chat_id":7,"message_id":20968374272_i64,
            "origin":{"@type":"messageOriginHiddenUser","sender_name":"\u{754c}".repeat(100)},
            "quote":{"text":{"text":"\u{1f642}".repeat(100)}}},
        "forward_info":{"origin":{"@type":"messageOriginHiddenUser","sender_name":"e".repeat(200)}},
        "content":{"@type":"messageText","text":{"text":"updated","entities":[]}}
    }});
    let events = decode(&mut d, &value.to_string());
    let Event::IncomingMessage(m) = &events[0] else {
        panic!("message")
    };
    assert!(m.edited && m.is_text);
    let r = m.reply.as_ref().unwrap();
    assert_eq!(r.message, MessageId(20968374272));
    assert!(
        r.sender.len() <= 256
            && r.preview.len() <= 256
            && m.forwarded.as_ref().unwrap().len() <= 256
    );
    let events = decode(
        &mut d,
        &json!({"@type":"updateMessageContent","chat_id":7,"message_id":20971520000_i64,
        "new_content":{"@type":"messageText","text":{"text":"changed","entities":[]}}})
        .to_string(),
    );
    assert!(matches!(&events[0],Event::MessageContentChanged {text,..} if text=="changed"));
    let events=decode(&mut d,&json!({"@type":"updateMessageEdited","chat_id":7,"message_id":20971520000_i64,"edit_date":12}).to_string());
    assert!(matches!(
        events[0],
        Event::MessageEdited { edited: true, .. }
    ));
    let mut deletion = json!({"@type":"updateDeleteMessages","chat_id":7,"message_ids":[20971520000_i64],"is_permanent":false});
    assert!(decode(&mut d, &deletion.to_string()).is_empty());
    deletion["is_permanent"] = json!(true);
    let events = decode(&mut d, &deletion.to_string());
    assert!(
        matches!(&events[0],Event::MessagesDeleted {messages,..} if messages==&[MessageId(20971520000)])
    );
}
#[test]
fn replied_message_resolves_names_without_a_retained_engine_history() {
    use serde_json::json;
    let mut d = Decoder::default();
    let value = json!({"@type":"message","id":20968374272_i64,"chat_id":7,"date":10,"is_outgoing":true,
        "content":{"@type":"messageText","text":{"text":"Original\nmessage","entities":[]}},
        "@extra":{"command":"replied","chat":7,"message":20971520000_i64}});
    let events = decode(&mut d, &value.to_string());
    assert!(
        matches!(&events[0],Event::ReplyResolved {reply:Some(r),..} if r.sender=="You" && r.preview=="Original message" && r.resolved)
    );
}

#[test]
fn replies_to_captionless_attachments_keep_a_description() {
    use serde_json::json;
    let file = json!({"id":42,"size":4096,"local":{"is_downloading_completed":false}});
    let cases = [
        (
            json!({"@type":"messageDocument","caption":{"text":"","entities":[]},
                "document":{"file_name":"report.pdf","mime_type":"application/pdf","document":file}}),
            "report.pdf",
        ),
        (
            json!({"@type":"messageVoiceNote","caption":{"text":"","entities":[]},"is_listened":true,
                "voice_note":{"duration":3,"waveform":"","mime_type":"audio/ogg","voice":file}}),
            "Voice message",
        ),
        (
            json!({"@type":"messageDocument","caption":{"text":"Quarterly\nnumbers","entities":[]},
                "document":{"file_name":"report.pdf","mime_type":"application/pdf","document":file}}),
            "Quarterly numbers",
        ),
    ];
    for (content, expected) in cases {
        let value = json!({"@type":"message","id":20968374272_i64,"chat_id":7,"date":10,
            "is_outgoing":true,"content":content,
            "@extra":{"command":"replied","chat":7,"message":20971520000_i64}});
        let events = decode(&mut Decoder::default(), &value.to_string());
        assert!(
            matches!(&events[0], Event::ReplyResolved {reply:Some(r),..} if r.preview == expected),
            "{expected}: {events:?}"
        );
    }
}

#[test]
fn media_without_caption_previews_its_label_not_an_empty_line() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"updateNewChat","chat":{"id":1,"title":"Channel","last_message":{"id":1048576,"date":1,"is_outgoing":false,"content":{"@type":"messagePhoto","caption":{"text":"","entities":[]}}},"positions":[{"list":{"@type":"chatListMain"},"order":"100"}]}}"#,
    );
    let Event::ChatListChanged(ChatListChange::Insert { chat, .. }) = &events[0] else {
        panic!("chat row")
    };
    assert_eq!(
        chat.last_message.as_ref().map(|m| m.text.as_str()),
        Some("Photo")
    );
}
#[test]
fn voice_documents_stickers_and_file_progress_use_bounded_domain_metadata() {
    use base64::Engine as _;
    use serde_json::json;
    use tg_core::Attachment;
    let file = json!({"id":42,"size":4096,"local":{"is_downloading_completed":false,"is_downloading_active":true,"downloaded_size":1024}});
    let mut decoder = Decoder::default();
    let mut m = json!({"id":20971520000_i64,"chat_id":7,"date":10,"is_outgoing":false,
        "content":{"@type":"messageVoiceNote","is_listened":false,"caption":{"text":"Voice caption","entities":[]},
            "voice_note":{"duration":12,"waveform":"IIhBivk=","mime_type":"audio/ogg","voice":file}}});
    let events = decode(
        &mut decoder,
        &json!({"@type":"updateNewMessage","message":m}).to_string(),
    );
    let Event::IncomingMessage(message) = &events[0] else {
        panic!("message")
    };
    let Some(Attachment::Voice(v)) = &message.attachment else {
        panic!("voice")
    };
    assert_eq!(v.waveform, [0, 1, 2, 3, 4, 5, 6, 31]);
    assert!(!v.listened);
    assert_eq!(v.file.downloaded, 1024);
    assert!(v.file.downloading);
    assert_eq!(message.text, "Voice caption");
    m["content"]["voice_note"]["waveform"] =
        json!(base64::engine::general_purpose::STANDARD.encode([0u8; 257]));
    assert!(
        decoder
            .decode(
                &json!({"@type":"updateNewMessage","message":m}).to_string(),
                |_| {}
            )
            .is_err()
    );
    m["content"] = json!({"@type":"messageDocument","document":{"file_name":"\u{754c}".repeat(200),"document":file},"caption":{"text":"Document caption","entities":[]}});
    let events = decode(
        &mut decoder,
        &json!({"@type":"updateNewMessage","message":m}).to_string(),
    );
    let Event::IncomingMessage(message) = &events[0] else {
        panic!("message")
    };
    let Some(Attachment::Document(d)) = &message.attachment else {
        panic!("document")
    };
    assert!(d.name.len() <= 256 && d.name.len() % 3 == 0);
    assert_eq!(message.text, "Document caption");
    for format in ["stickerFormatWebp", "stickerFormatTgs", "stickerFormatWebm"] {
        m["content"] = json!({"@type":"messageSticker","sticker":{"width":512,"height":256,"emoji":"\u{1f916}".repeat(100),"format":{"@type":format},"sticker":file}});
        let events = decode(
            &mut decoder,
            &json!({"@type":"updateNewMessage","message":m}).to_string(),
        );
        let Event::IncomingMessage(message) = &events[0] else {
            panic!("message")
        };
        let Some(Attachment::Sticker(s)) = &message.attachment else {
            panic!("sticker")
        };
        assert!(s.emoji.len() <= 64);
        assert_eq!(s.animated, format != "stickerFormatWebp");
        assert_eq!(message.photo.is_some(), format == "stickerFormatWebp");
        assert_eq!(message.text, "");
    }
    let events = decode(
        &mut decoder,
        r#"{"@type":"updateMessageContentOpened","chat_id":7,"message_id":20971520000}"#,
    );
    assert_eq!(
        events,
        [Event::MessageContentOpened {
            chat: ChatId(7),
            message: MessageId(20971520000)
        }]
    );
}
#[test]
fn explicit_file_failure_is_retryable_and_does_not_expose_server_text() {
    let events = decode(
        &mut Decoder::default(),
        r#"{"@type":"error","code":400,"message":"sensitive arbitrary text","@extra":{"command":"download","file":42}}"#,
    );
    assert_eq!(events, [Event::FileDownloadFailed(42)]);
}

#[test]
fn round_and_single_emoji_are_media_instead_of_placeholders() {
    use serde_json::json;
    let file = json!({"id":42,"size":4096,"local":{}});
    for content in [
        json!({"@type":"messageVideoNote","is_viewed":false,"video_note":{"duration":9,"length":9999,"video":file,"thumbnail":{"format":{"@type":"thumbnailFormatJpeg"},"width":100,"height":100,"file":file}}}),
        json!({"@type":"messageAnimatedEmoji","emoji":"\u{1f642}","animated_emoji":{"sticker":null}}),
        json!({"@type":"messageAnimatedEmoji","emoji":"\u{1f642}","animated_emoji":{"sticker":{"format":{"@type":"stickerFormatTgs"},"thumbnail":{"format":{"@type":"thumbnailFormatJpeg"},"file":file}}}}),
        json!({"@type":"messageAnimatedEmoji","emoji":"\u{1f642}","animated_emoji":{"sticker":{"format":{"@type":"stickerFormatWebp"},"width":512,"height":512,"sticker":file}}}),
    ] {
        let mut d = Decoder::default();
        let events = decode(&mut d, &json!({"@type":"updateNewMessage","message":{"id":20971520000_i64,"chat_id":7,"date":10,"is_outgoing":false,"content":content}}).to_string());
        let Event::IncomingMessage(m) = &events[0] else {
            panic!("message")
        };
        assert!(m.attachment.is_some(), "missing media metadata");
        assert!(m.text.is_empty());
        if content["@type"] == "messageAnimatedEmoji" {
            let Some(tg_core::Attachment::Emoji(emoji)) = &m.attachment else {
                panic!("emoji")
            };
            assert_eq!(emoji.emoji, "\u{1f642}");
            assert_eq!(
                m.photo.is_some(),
                content["animated_emoji"]["sticker"]["format"]["@type"] == "stickerFormatWebp"
            );
        }
        let events = decode(&mut d, &json!({"@type":"updateNewChat","chat":{"id":7,"title":"Test","last_message":{"id":20971520000_i64,"date":10,"content":content},"positions":[{"list":{"@type":"chatListMain"},"order":"100"}]}}).to_string());
        let Event::ChatListChanged(ChatListChange::Insert { chat, .. }) = &events[0] else {
            panic!("chat")
        };
        assert_eq!(
            chat.last_message.as_ref().unwrap().text,
            if content["@type"] == "messageVideoNote" {
                "Video message"
            } else {
                "\u{1f642}"
            }
        );
    }
}

#[test]
fn round_size_is_bounded_and_emoji_quotes_preserve_the_emoji() {
    use serde_json::json;
    let mut decoder = Decoder::default();
    for (length, expected) in [(0, 1), (240, 240), (i64::MAX, 240)] {
        let events=decode(&mut decoder,&json!({"@type":"updateNewMessage","message":{"id":20971520000_i64,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageVideoNote","is_viewed":true,"video_note":{"duration":4,"length":length,"video":{"id":42,"local":{}}}}}}).to_string());
        let Event::IncomingMessage(m) = &events[0] else {
            panic!("message")
        };
        let Some(tg_core::Attachment::Round(r)) = &m.attachment else {
            panic!("round")
        };
        assert_eq!(r.length, expected);
        assert!(r.viewed);
        assert_eq!(r.duration, 4);
    }
    let events=decode(&mut decoder,&json!({"@type":"updateNewMessage","message":{"id":20971520000_i64,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageText","text":{"text":"Reply","entities":[]}},"reply_to":{"@type":"messageReplyToMessage","chat_id":7,"message_id":20970471424_i64,"content":{"@type":"messageAnimatedEmoji","emoji":"\u{1f642}"}}}}).to_string());
    let Event::IncomingMessage(m) = &events[0] else {
        panic!("message")
    };
    assert_eq!(m.reply.as_ref().unwrap().preview, "\u{1f642}");
}

#[test]
fn streamable_video_maps_partial_path_and_contiguous_range() {
    use serde_json::json;
    let mut d = Decoder::default();
    for supports in [true, false] {
        let events=decode(&mut d,&json!({"@type":"updateNewMessage","message":{"id":20971520000_i64,"chat_id":7,"date":1,"is_outgoing":false,"content":{"@type":"messageVideo","video":{"duration":12,"width":426,"height":240,"supports_streaming":supports,"video":{"id":42,"size":8000000,"local":{"path":"C:/demo/partial.mp4","is_downloading_completed":false,"download_offset":2097152,"downloaded_prefix_size":131072,"downloaded_size":262144}}}}}}).to_string());
        let Event::IncomingMessage(m) = &events[0] else {
            panic!("video")
        };
        let v = m.video.as_ref().unwrap();
        assert_eq!(v.supports_streaming, supports);
        assert!(v.file.path.is_none());
        let partial = v.file.partial.as_ref().unwrap();
        assert_eq!(partial.offset, 2097152);
        assert_eq!(partial.prefix, 131072);
        assert!(partial.path.is_some());
        assert!(!partial.completed);
    }
}

#[test]
fn reactions_map_emoji_entries_and_skip_custom_paid_and_oversized_ones() {
    use serde_json::json;
    let reactions = json!({"reactions":[
        {"type":{"@type":"reactionTypeEmoji","emoji":"👍"},"total_count":5,"is_chosen":true},
        {"type":{"@type":"reactionTypeCustomEmoji","custom_emoji_id":"1"},"total_count":2,"is_chosen":false},
        {"type":{"@type":"reactionTypePaid"},"total_count":9,"is_chosen":false},
        {"type":{"@type":"reactionTypeEmoji","emoji":"x".repeat(65)},"total_count":1,"is_chosen":false},
        {"type":{"@type":"reactionTypeEmoji","emoji":"\u{2764}"},"total_count":1,"is_chosen":false}]});
    let page = json!({"@type":"messages","@extra":{"command":"history","chat":7,"before":0},
        "messages":[{"id":1048576,"chat_id":7,"date":1,"is_outgoing":false,
            "interaction_info":{"view_count":0,"forward_count":0,"reactions":reactions},
            "content":{"@type":"messageText","text":{"text":"hi","entities":[]}}}]});
    let events = decode(&mut Decoder::default(), &page.to_string());
    let Event::MessagesLoaded { messages, .. } = &events[0] else {
        panic!("expected page")
    };
    let expected = vec![
        tg_core::Reaction {
            emoji: "👍".into(),
            count: 5,
            chosen: true,
        },
        tg_core::Reaction {
            emoji: "\u{2764}".into(),
            count: 1,
            chosen: false,
        },
    ];
    assert_eq!(messages[0].reactions, expected);

    let update = json!({"@type":"updateMessageInteractionInfo","chat_id":7,"message_id":1048576,
        "interaction_info":{"view_count":3,"forward_count":0,"reactions":reactions}});
    assert_eq!(
        decode(&mut Decoder::default(), &update.to_string()),
        vec![Event::MessageReactionsChanged {
            chat: ChatId(7),
            message: MessageId(1048576),
            reactions: expected.clone(),
        }]
    );
    // Null interaction info means the reactions are gone.
    let cleared = json!({"@type":"updateMessageInteractionInfo","chat_id":7,
        "message_id":1048576,"interaction_info":null});
    assert_eq!(
        decode(&mut Decoder::default(), &cleared.to_string()),
        vec![Event::MessageReactionsChanged {
            chat: ChatId(7),
            message: MessageId(1048576),
            reactions: vec![],
        }]
    );
    let bots = json!({"@type":"updateMessageReactions","chat_id":7,"message_id":1048576,
        "date":1,"reactions":reactions["reactions"]});
    assert_eq!(
        decode(&mut Decoder::default(), &bots.to_string()),
        vec![Event::MessageReactionsChanged {
            chat: ChatId(7),
            message: MessageId(1048576),
            reactions: expected,
        }]
    );
}

#[test]
fn reaction_lists_are_bounded_and_failures_keep_the_command_identity() {
    use serde_json::json;
    let many: Vec<_> = (0..40)
        .map(|i| {
            json!({"type":{"@type":"reactionTypeEmoji","emoji":format!("e{i}")},
            "total_count":1,"is_chosen":false})
        })
        .collect();
    let update = json!({"@type":"updateMessageReactions","chat_id":7,"message_id":1048576,
        "date":1,"reactions":many});
    let events = decode(&mut Decoder::default(), &update.to_string());
    assert!(
        matches!(&events[..], [Event::MessageReactionsChanged { reactions, .. }] if reactions.len() == tg_core::MAX_REACTIONS)
    );
    for tag in ["toggle_reaction", "reaction"] {
        let events = decode(
            &mut Decoder::default(),
            &json!({"@type":"error","code":400,"message":"REACTION_INVALID","@extra":{"command":tag}})
                .to_string(),
        );
        assert!(
            matches!(&events[..], [Event::CommandFailed { command: tg_core::CommandKind::ToggleReaction, error }] if error.message == "This reaction is not available here.")
        );
    }
    // The tagged getMessage answer is consumed by the engine, not shown as a message.
    let answer = json!({"@type":"message","id":1048576,"chat_id":7,"date":1,"is_outgoing":false,
        "content":{"@type":"messageText","text":{"text":"hi","entities":[]}},
        "@extra":{"command":"toggle_reaction","chat":7,"message":1048576,"reaction":"👍"}});
    assert!(decode(&mut Decoder::default(), &answer.to_string()).is_empty());
}
