//! Account-free visual review harness for the sign-in states and TPM warning.
//! Usage: cargo run -p tg-ui --example review -- qr|code|password|warning|media-loading|media-missing <png>
use tg_core::{AuthState, Event, SecurityWarning};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let screen = args.next().ok_or("Choose qr, code, password or warning")?;
    let snapshot = args.next().ok_or("Supply a PNG output path")?;
    let ui = tg_ui::Ui::new(
        tg_ui::Options {
            demo: true,
            oled: true,
            snapshot: Some(snapshot.into()),
            ..Default::default()
        },
        |_| Ok(()),
    )?;
    let events = ui.event_sink();
    match screen.as_str() {
        "qr" => events.post(Event::AuthStateChanged(AuthState::WaitOtherDevice {
            link: "https://example.org/synthetic-sign-in".into(),
        })),
        "code" => events.post(Event::AuthStateChanged(AuthState::WaitCode)),
        "password" => events.post(Event::AuthStateChanged(AuthState::WaitPassword {
            hint: String::new(),
        })),
        "media-loading" | "media-missing" => {
            let photo = tg_core::Photo {
                file: tg_core::MediaFile {
                    partial: None,
                    size: 0,
                    downloaded: 0,
                    downloading: false,
                    id: 42,
                    path: None,
                },
                width: 1008,
                height: 640,
                minithumbnail: if screen == "media-loading" {
                    include_bytes!("../../app/assets/demo-mini.jpg").to_vec()
                } else {
                    Vec::new()
                },
            };
            events.post(Event::AuthStateChanged(AuthState::Ready));
            events.post(Event::ChatListChanged(tg_core::ChatListChange::Insert {
                position: 0,
                chat: tg_core::Chat {
                    id: tg_core::ChatId(7),
                    title: "Synthetic loading preview".into(),
                    avatar: Some(photo.clone()),
                    last_message: None,
                    unread_count: 0,
                    unread_mentions: 0,
                    muted_until: 0,
                    show_notification_preview: true,
                    last_read_inbox: tg_core::MessageId(0),
                    last_read_outbox: tg_core::MessageId(0),
                },
            }));
            events.post(Event::MessagesLoaded {
                chat: tg_core::ChatId(7),
                before: tg_core::MessageId(0),
                next_before: None,
                messages: vec![tg_core::Message {
                    attachment: None,
                    reply: None,
                    forwarded: None,
                    edited: false,
                    is_text: true,
                    service: false,
                    permissions: None,
                    video: None,
                    id: tg_core::MessageId(104857600),
                    chat: tg_core::ChatId(7),
                    date: 43200,
                    outgoing: false,
                    text: "Photo caption stays visible while downloading".into(),
                    entities: vec![],
                    send_state: tg_core::SendState::Sent,
                    album: 0,
                    sender_id: 0,
                    sender: None,
                    photo: Some(photo),
                    reactions: Vec::new(),
                }],
            });
        }
        "warning" => {
            events.post(Event::AuthStateChanged(AuthState::Ready));
            events.post(Event::SecurityWarning(SecurityWarning::TpmUnavailable));
        }
        _ => return Err("Unknown review screen".into()),
    }
    ui.run()
}
