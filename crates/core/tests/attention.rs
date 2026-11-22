use tg_core::{ChatId, MessageId, ReadBatches};
#[test]
fn visible_receipts_are_bounded_and_serialized_per_chat() {
    let mut batches = ReadBatches::default();
    let chat = ChatId(7);
    let ids = (1..=100).map(|n| MessageId(n * 1048576));
    let first = batches.begin(chat, MessageId(0), ids).unwrap();
    assert_eq!(first.len(), 50);
    assert!(
        batches
            .begin(chat, MessageId(0), [MessageId(200 * 1048576)])
            .is_none()
    );
    batches.complete(chat, true);
    assert!(
        batches
            .begin(chat, MessageId(0), [MessageId(1048576)])
            .is_none()
    );
    assert_eq!(
        batches
            .begin(chat, MessageId(0), [MessageId(51 * 1048576)])
            .unwrap(),
        vec![MessageId(51 * 1048576)]
    );
    batches.complete(chat, false);
    assert!(
        batches
            .begin(chat, MessageId(0), [MessageId(51 * 1048576)])
            .is_some()
    );
}
#[test]
fn empty_or_already_read_view_does_not_send_and_state_is_bounded() {
    let mut batches = ReadBatches::default();
    assert!(
        batches
            .begin(ChatId(1), MessageId(1048576), [MessageId(1048576)])
            .is_none()
    );
    for chat in 1..=128 {
        assert!(
            batches
                .begin(ChatId(chat), MessageId(0), [MessageId(1048576)])
                .is_some()
        );
    }
    assert!(
        batches
            .begin(ChatId(129), MessageId(0), [MessageId(1048576)])
            .is_none()
    );
}

fn chat() -> tg_core::Chat {
    tg_core::Chat {
        id: ChatId(7),
        title: "Demo group".into(),
        avatar: None,
        last_message: None,
        unread_count: 2,
        unread_mentions: 1,
        muted_until: 0,
        show_notification_preview: true,
        last_read_inbox: MessageId(1048576),
        last_read_outbox: MessageId(0),
    }
}
fn message() -> tg_core::Message {
    tg_core::Message {
        attachment: None,
        reply: None,
        forwarded: None,
        edited: false,
        is_text: true,
        service: false,
        permissions: None,
        video: None,
        id: MessageId(2097152),
        chat: ChatId(7),
        date: 1,
        outgoing: false,
        text: "Synthetic message".into(),
        sender: Some("Sender".into()),
        sender_id: 1,
        album: 0,
        photo: None,
        entities: vec![],
        send_state: tg_core::SendState::Sent,
        reactions: Vec::new(),
    }
}
#[test]
fn notifications_filter_focus_mute_outgoing_and_already_read_messages() {
    let mut chat = chat();
    let mut m = message();
    assert!(tg_core::Notification::incoming(&chat, &m, Some(chat.id), true, 100).is_none());
    let n = tg_core::Notification::incoming(&chat, &m, Some(chat.id), false, 100).unwrap();
    assert_eq!(n.body, "Sender: Synthetic message");
    assert!(!format!("{n:?}").contains("Synthetic"));
    chat.muted_until = 101;
    assert!(tg_core::Notification::incoming(&chat, &m, None, true, 100).is_none());
    chat.muted_until = 100;
    m.outgoing = true;
    assert!(tg_core::Notification::incoming(&chat, &m, None, true, 100).is_none());
    m.outgoing = false;
    m.id = chat.last_read_inbox;
    assert!(tg_core::Notification::incoming(&chat, &m, None, true, 100).is_none());
}
#[test]
fn notification_preview_limits_and_privacy_are_applied_before_queueing() {
    let mut chat = chat();
    let mut m = message();
    chat.title = "long ".repeat(1000);
    m.text = "\u{1f642}".repeat(10000);
    let n = tg_core::Notification::incoming(&chat, &m, None, false, 1).unwrap();
    assert!(n.title.len() <= 192 && n.body.len() <= 768);
    chat.show_notification_preview = false;
    assert_eq!(
        tg_core::Notification::incoming(&chat, &m, None, false, 1)
            .unwrap()
            .body,
        "New message"
    );
}
