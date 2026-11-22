//! Account-free pointer regression: production UI and link dispatch, shell boundary captured.
extern crate self as tg_ui;
include!("../src/lib.rs");
use slint::{
    Model,
    platform::{PointerEventButton, WindowEvent},
};
const LINK_LABEL: &str = "https://mybank.example";

fn pointer(w: &AppWindow, event: WindowEvent) {
    w.window().dispatch_event(event);
}
fn click(w: &AppWindow, p: slint::LogicalPosition, button: PointerEventButton) {
    pointer(w, WindowEvent::PointerMoved { position: p });
    pointer(
        w,
        WindowEvent::PointerPressed {
            position: p,
            button,
        },
    );
    pointer(
        w,
        WindowEvent::PointerReleased {
            position: p,
            button,
        },
    );
}
fn enter(w: &AppWindow) {
    let text: slint::SharedString = slint::platform::Key::Return.into();
    pointer(w, WindowEvent::KeyPressed { text: text.clone() });
    pointer(w, WindowEvent::KeyReleased { text });
}
fn verify(
    w: &AppWindow,
    state: &State,
    requests: &RefCell<Vec<tg_core::FileRequest>>,
) -> Result<(), String> {
    let row = state.feed.row_data(0).ok_or("No visible message")?;
    let key = row.key.to_string();
    let mut hit = None;
    for y in 0..row.text_height as u32 {
        for x in 0..row.text_width as u32 {
            if state.feed.link_at(&key, x as f32, y as f32).is_some() {
                hit = Some((x as f32 + 1.0, y as f32 + 3.0));
                break;
            }
        }
        if hit.is_some() {
            break;
        }
    }
    let (x, y) = hit.ok_or("No link geometry")?;
    let left = w.get_feed_left()
        + if row.outgoing {
            w.get_feed_view_width() - row.bubble_width - 14.0
        } else {
            14.0
        };
    let top = w.get_feed_top()
        + row.row_y
        + w.get_feed_y()
        + (w.get_feed_height() - w.get_feed_content_height()).max(0.0)
        + row.bubble_y;
    let p = slint::LogicalPosition::new(left + 10.0 + x, top + row.text_y + y);
    pointer(w, WindowEvent::PointerMoved { position: p });
    let expected = "http://example.org/actual";
    if w.invoke_text_link(key.clone().into(), x, y).as_str() != expected {
        return Err("Hover resolved the wrong target".into());
    }
    click(w, p, PointerEventButton::Left);
    if !requests.borrow().is_empty() {
        return Err("Disguised link opened without confirmation".into());
    }
    if w.get_action_dialog() != OPEN_LINK_DIALOG || w.get_link_target().as_str() != expected {
        return Err("Confirmation did not show the real normalized target".into());
    }
    w.invoke_cancel_action();
    w.invoke_confirm_link();
    if w.get_action_dialog() != 0
        || !w.get_link_target().is_empty()
        || !requests.borrow().is_empty()
    {
        return Err("Cancelling did not discard the pending link".into());
    }
    click(w, p, PointerEventButton::Left);
    w.set_blocked(true);
    enter(w);
    if w.get_action_dialog() != OPEN_LINK_DIALOG || !requests.borrow().is_empty() {
        return Err("Enter confirmed the link while the UI was blocked".into());
    }
    w.set_blocked(false);
    enter(w);
    if w.get_action_dialog() != 0 || requests.borrow().len() != 1 {
        return Err("Enter did not confirm the link dialog".into());
    }
    w.invoke_confirm_link();
    if w.get_action_dialog() != 0
        || !w.get_link_target().is_empty()
        || requests.borrow().as_slice() != [tg_core::FileRequest::OpenUrl(expected.into())]
    {
        return Err("Confirmation did not dispatch the displayed URL exactly once".into());
    }
    requests.borrow_mut().clear();
    pointer(
        w,
        WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Left,
        },
    );
    pointer(
        w,
        WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(p.x + 20.0, p.y),
        },
    );
    pointer(w, WindowEvent::PointerMoved { position: p });
    pointer(
        w,
        WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        },
    );
    if !requests.borrow().is_empty() {
        return Err("Dragging activated a link".into());
    }
    // A fresh press is required even when release lands on a link.
    let plain = slint::LogicalPosition::new(left + 11.0, p.y);
    pointer(
        w,
        WindowEvent::PointerPressed {
            position: plain,
            button: PointerEventButton::Left,
        },
    );
    pointer(
        w,
        WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        },
    );
    if !requests.borrow().is_empty() {
        return Err("Pressing plain text activated a link".into());
    }
    click(w, p, PointerEventButton::Right);
    if !requests.borrow().is_empty() || !w.get_menu_open() {
        return Err("Right click did not preserve the context menu".into());
    }
    w.set_menu_open(false);
    w.set_blocked(true);
    click(w, p, PointerEventButton::Left);
    if !requests.borrow().is_empty() {
        return Err("Blocked UI activated a link".into());
    }
    w.set_blocked(false);
    state.feed.invalidate();
    if state.feed.link_at(&key, x, y).is_some() {
        return Err("Invalidated row retained its link".into());
    }
    state.refresh(w);
    state.feed.history.borrow_mut().messages[0].entities[0].kind = tg_core::EntityKind::TextUrl {
        target: "HTTPS://mybank.example/".into(),
    };
    state.feed.invalidate();
    state.refresh(w);
    click(w, p, PointerEventButton::Left);
    if w.get_action_dialog() != 0
        || requests.borrow().as_slice() != [tg_core::FileRequest::OpenUrl(format!("{LINK_LABEL}/"))]
    {
        return Err(
            "A text link with a matching normalized label prompted or failed to open".into(),
        );
    }
    pointer(w, WindowEvent::PointerMoved { position: plain });
    pointer(w, WindowEvent::PointerMoved { position: p });
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Keep the real shell worker asleep during idle measurements; dispatch remains captured.
    let _shell_worker = tg_platform::Files::start(|_| {})?;
    let ui = Ui::new(
        Options {
            demo: true,
            ..Default::default()
        },
        |_| Ok(()),
    )?;
    ui.window.set_ready(true);
    ui.window.set_chat_open(true);
    let text = format!("plain text before {LINK_LABEL} and after");
    ui.state.feed.history.borrow_mut().messages = vec![tg_core::Message {
        id: MessageId(1048576),
        chat: tg_core::ChatId(1),
        date: 0,
        outgoing: false,
        text,
        entities: vec![tg_core::TextEntity {
            offset: 18,
            length: LINK_LABEL.len() as u32,
            kind: tg_core::EntityKind::TextUrl {
                target: "HTTP://example.org/actual".into(),
            },
        }],
        photo: None,
        video: None,
        attachment: None,
        reply: None,
        forwarded: None,
        edited: false,
        is_text: true,
        service: false,
        permissions: None,
        send_state: tg_core::SendState::Sent,
        album: 0,
        sender_id: 0,
        sender: None,
        reactions: Vec::new(),
    }];
    let requests: Rc<RefCell<Vec<tg_core::FileRequest>>> = Rc::default();
    let captured = requests.clone();
    ui.on_file(move |request| {
        captured.borrow_mut().push(request);
        Ok(())
    });
    let result: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
    let checked = result.clone();
    let weak = ui.window.as_weak();
    let state = ui.state.clone();
    let idle = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    slint::Timer::single_shot(Duration::from_millis(800), move || {
        if let Some(w) = weak.upgrade() {
            state.refresh(&w);
            *checked.borrow_mut() = Some(verify(&w, &state, &requests));
            println!("pointer verification: {:?}", checked.borrow());
        }
        slint::Timer::single_shot(Duration::from_secs(idle), || {
            let _ = slint::quit_event_loop();
        });
    });
    ui.window.run()?;
    result
        .borrow_mut()
        .take()
        .ok_or("Pointer verification did not run")?
        .map_err(Into::into)
}
