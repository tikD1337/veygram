//! Deterministic synthetic text assets for comparing a text-stack dependency migration.
//! Run with an output directory; metrics include the bubble footer's last-line advance.
#[path = "../src/text.rs"]
mod text;
use tg_core::{ChatId, EntityKind, Message, MessageId, SendState, TextEntity};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Supply an output directory")?,
    );
    std::fs::create_dir_all(&directory)?;
    let mut engine = text::TextEngine::new(None);
    let fixtures = [
        ("single-emoji", "\u{1f642}", vec![]),
        // A reaction chip emoji: the bare heart as Telegram spells it, drawn in colour.
        ("reaction", "\u{2764}\u{FE0F}", vec![]),
        (
            "styled",
            "Bold, italic, underline, strike. cargo build\nA hidden spoiler and a link.",
            vec![
                ("Bold", EntityKind::Bold),
                ("italic", EntityKind::Italic),
                ("underline", EntityKind::Underline),
                ("strike", EntityKind::Strikethrough),
                ("cargo build", EntityKind::Code),
                ("hidden spoiler", EntityKind::Spoiler),
                ("link", EntityKind::Url),
            ],
        ),
        (
            "rtl",
            "\u{05e9}\u{05dc}\u{05d5}\u{05dd} \u{05e2}\u{05d5}\u{05dc}\u{05dd} - \u{0645}\u{0631}\u{062d}\u{0628}\u{0627} \u{0628}\u{0627}\u{0644}\u{0639}\u{0627}\u{0644}\u{0645}",
            vec![],
        ),
        (
            "emoji",
            "Color emoji: \u{1f642} \u{1f525} \u{1f44d} \u{1f30d}",
            vec![],
        ),
        (
            "wrapping",
            "A longer synthetic paragraph tests wrapping at a fixed width. The last line must leave the same amount of room for the time and read checks.",
            vec![],
        ),
        (
            "pre",
            "fn main() {\n    println!(\"demo\");\n}",
            vec![("fn main() {\n    println!(\"demo\");\n}", EntityKind::Pre)],
        ),
    ];
    let mut metrics = String::new();
    for (name, body, spans) in fixtures {
        let entities = spans
            .into_iter()
            .filter_map(|(needle, kind)| {
                body.find(needle).map(|start| TextEntity {
                    offset: body[..start].encode_utf16().count() as u32,
                    length: needle.encode_utf16().count() as u32,
                    kind,
                })
            })
            .collect();
        let message = Message {
            attachment: None,
            reply: None,
            forwarded: None,
            edited: false,
            is_text: true,
            service: false,
            permissions: None,
            video: None,
            id: MessageId(1048576),
            chat: ChatId(1),
            date: 0,
            outgoing: false,
            text: body.into(),
            entities,
            photo: None,
            album: 0,
            sender_id: 0,
            sender: None,
            send_state: SendState::Sent,
            reactions: Vec::new(),
        };
        for (suffix, scale, width) in [("1x", 1.0, 300.0), ("2x", 2.0, 600.0)] {
            let rendered = if name == "single-emoji" {
                engine.render_emoji(&message, scale, 128.0 * scale)
            } else if name == "reaction" {
                engine.render_symbol(&message.text, scale, 15.0)
            } else {
                engine.render(&message, scale, width)
            };
            assert!(rendered.links.link_at((-1.0, -1.0)).is_none());
            assert!(rendered.links.bytes() <= 256 * 1024);
            metrics.push_str(&format!(
                "{name}-{suffix}: {}x{}, last_line={}, clipped={}\n",
                rendered.width, rendered.height, rendered.last_line, rendered.clipped
            ));
            // Composite premultiplied text onto the same incoming dark surface for PNG review.
            let mut pixels = rendered.pixels;
            for pixel in pixels.as_chunks_mut::<4>().0 {
                let alpha = u32::from(pixel[3]);
                for (channel, background) in [24u32, 37, 51].into_iter().enumerate() {
                    pixel[channel] = (u32::from(pixel[channel]) + background * (255 - alpha) / 255)
                        .min(255) as u8;
                }
                pixel[3] = 255;
            }
            image::save_buffer(
                directory.join(format!("{name}-{suffix}.png")),
                &pixels,
                rendered.width,
                rendered.height,
                image::ColorType::Rgba8,
            )?;
        }
    }
    std::fs::write(directory.join("metrics.txt"), metrics)?;
    Ok(())
}
