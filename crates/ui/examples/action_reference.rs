// Review harness uses the real private action wiring and synthetic domain events only.
// cargo run -p tg-ui --example action_reference -- <action> <png>
// reply, edit, delete, forward, quote, reply-send, edit-save, delete-confirm, forward-send,
// edit-escape, delete-escape, forward-search. Manual-only menu mode needs Esc in the native menu.
extern crate self as tg_ui;
include!("../src/lib.rs");
#[path = "../../app/src/demo.rs"]
#[allow(dead_code)] // Only the local action backend is needed in this review executable.
mod demo_backend;

fn fixture(n: i64) -> tg_core::Message {
    tg_core::Message {
        attachment: None,
        video: None,
        id: MessageId(n * 1048576),
        chat: tg_core::ChatId(7),
        date: 1793620800 + n as i32 * 60,
        outgoing: n == 19998,
        text: if n == 19998 {
            "Own text ready to edit"
        } else {
            "A synthetic message with a compact quote"
        }
        .into(),
        entities: vec![],
        send_state: tg_core::SendState::Sent,
        photo: None,
        album: 0,
        sender_id: 1000,
        sender: (n != 19998).then(|| "Anna Petrova".into()),
        is_text: true,
        service: false,
        permissions: None,
        edited: n == 19998,
        forwarded: (n == 19997).then(|| "Design review".into()),
        reply: (n == 19997).then(|| tg_core::Reply {
            resolved: true,
            chat: tg_core::ChatId(7),
            message: MessageId(19996 * 1048576),
            sender_id: 1000,
            sender: "Anna Petrova".into(),
            preview: "A synthetic message with a compact quote".into(),
        }),
        reactions: Vec::new(),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let action = args.next().ok_or("Supply an action")?;
    let path = args.next().ok_or("Supply a PNG path")?;
    let media = demo_backend::prepare_media()?;
    let backend = Rc::new(RefCell::new(demo_backend::Demo::new(media.clone(), true)));
    let commands = backend.clone();
    let ui = Ui::new(
        Options {
            demo: true,
            snapshot: Some(path.into()),
            ..Default::default()
        },
        move |command| commands.borrow_mut().command(command),
    )?;
    let sink = ui.event_sink();
    backend.borrow_mut().sink = Some(sink.clone());
    sink.post(Event::AuthStateChanged(AuthState::Ready));
    for (position, title) in ["Action review", "Saved Messages", "Design review"]
        .into_iter()
        .enumerate()
    {
        sink.post(Event::ChatListChanged(tg_core::ChatListChange::Insert {
            position,
            chat: tg_core::Chat {
                id: tg_core::ChatId(7 + position as i64),
                title: title.into(),
                last_message: None,
                avatar: None,
                unread_count: 0,
                unread_mentions: 0,
                muted_until: 0,
                show_notification_preview: true,
                last_read_inbox: MessageId(0),
                last_read_outbox: MessageId(19998 * 1048576),
            },
        }));
    }
    sink.post(Event::MessagesLoaded {
        chat: tg_core::ChatId(7),
        before: MessageId(0),
        next_before: None,
        messages: (19996..=19998).map(fixture).collect(),
    });
    let verified_action = action.clone();
    let observed = ui.window.clone_strong();
    let verified_state = ui.state.clone();
    let w = ui.window.as_weak();
    let state = ui.state.clone();
    slint::Timer::single_shot(Duration::from_millis(700), move || {
        if let Some(w) = w.upgrade() {
            let (key, id) = match action.as_str() {
                "reply" | "reply-send" => (19996, 0),
                "edit" | "edit-save" | "edit-escape" => (19998, 1),
                "delete" | "delete-confirm" | "delete-escape" => (19996, 4),
                "forward" | "forward-send" | "forward-search" => (19996, 3),
                "quote" => (19997, 5),
                _ => (0, -1),
            };
            if id >= 0 {
                state.message_action(&w, &(key * 1048576_i64).to_string(), id);
            }
            match action.as_str() {
                "reply-send" | "edit-save" => {
                    w.set_draft("Saved through the real composer action".into());
                    w.invoke_send();
                }
                "delete-confirm" => w.invoke_confirm_delete(),
                "forward-send" => w.invoke_forward_to(0),
                "forward-search" => w.set_forward_query("saved".into()),
                "edit-escape" | "delete-escape" => {
                    w.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                            text: slint::platform::Key::Escape.into(),
                        });
                    w.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                            text: slint::platform::Key::Escape.into(),
                        });
                }
                "menu" => {
                    if let Some(row) = state
                        .feed
                        .row_data(state.feed.row_count().saturating_sub(1))
                    {
                        use slint::platform::{PointerEventButton, WindowEvent};
                        let x = 1040.0 - row.bubble_width + 20.0;
                        let y = 94.0
                            + row.row_y
                            + w.get_feed_y()
                            + (w.get_feed_height() - w.get_feed_content_height()).max(0.0)
                            + row.bubble_y
                            + 10.0;
                        for event in [
                            WindowEvent::PointerPressed {
                                position: slint::LogicalPosition::new(x, y),
                                button: PointerEventButton::Right,
                            },
                            WindowEvent::PointerReleased {
                                position: slint::LogicalPosition::new(x, y),
                                button: PointerEventButton::Right,
                            },
                        ] {
                            w.window().dispatch_event(event);
                        }
                    }
                }
                _ => {}
            }
        }
    });
    let result = ui.run();
    result?;
    if !observed.get_error().is_empty() {
        return Err(observed.get_error().to_string().into());
    }
    let history = verified_state.feed.history.borrow();
    let valid = match verified_action.as_str() {
        "reply" => !observed.get_composer_title().is_empty() && !observed.get_editing(),
        "edit" => observed.get_editing(),
        "delete" => observed.get_action_dialog() == 1,
        "forward" => observed.get_action_dialog() == 2,
        "forward-search" => verified_state.actions.destinations.row_count() == 1,
        "edit-save" => {
            history.messages.iter().any(|m| {
                m.id == MessageId(19998 * 1048576)
                    && m.edited
                    && m.text == "Saved through the real composer action"
            }) && !observed.get_editing()
        }
        "reply-send" => history.messages.iter().any(|m| {
            m.id.0 > 20000 * 1048576
                && m.reply
                    .as_ref()
                    .is_some_and(|r| r.message == MessageId(19996 * 1048576))
        }),
        "delete-confirm" => !history
            .messages
            .iter()
            .any(|m| m.id == MessageId(19996 * 1048576)),
        "forward-send" => history
            .messages
            .iter()
            .any(|m| m.id.0 > 20000 * 1048576 && m.forwarded.is_some()),
        "edit-escape" => !observed.get_editing() && observed.get_composer_title().is_empty(),
        "delete-escape" => observed.get_action_dialog() == 0,
        "quote" => verified_state.feed.highlight.get() == Some(MessageId(19996 * 1048576)),
        "menu" => true,
        _ => false,
    };
    if !valid {
        return Err("Action review state verification failed".into());
    }
    let _ = std::fs::remove_file(media);
    Ok(())
}
use slint::Model;
