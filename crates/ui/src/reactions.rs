//! Reaction chips under a bubble and the quick-reaction strip above the message menu.
//!
//! As in Telegram Desktop, chips sit inside the bubble below the text or media, wrap to the
//! bubble width, and the time joins the last chip row when it fits. A click toggles the
//! reaction; the engine decides between adding and removing from TDLib's current state.
use super::*;
use crate::text::TextEngine;
use slint::{Image, Model, Rgba8Pixel, VecModel};
use tg_core::{ChatId, Message, Reaction};

/// Telegram's spelling: the red heart has no variation selector on the server.
pub(crate) const QUICK: [&str; 6] = ["👍", "\u{2764}", "🔥", "😂", "👏", "🎉"];

pub(crate) const CHIP_HEIGHT: f32 = 26.0;
/// Space between chips, horizontally and between rows.
pub(crate) const CHIP_GAP: f32 = 4.0;
const CHIP_PAD_LEFT: f32 = 8.0;
const CHIP_PAD_RIGHT: f32 = 10.0;
const EMOJI_BOX: f32 = 18.0;
const EMOJI_SKIP: f32 = 4.0;
/// Upper estimate of one 12 px semibold Segoe UI digit (also covers "K", "M" and ".").
const DIGIT: f32 = 7.5;
const CHIP_FONT: f32 = 15.0;
const QUICK_FONT: f32 = 22.0;

/// Short counter as in Telegram Desktop: 999, 1.2K, 12K, 1.2M.
pub(crate) fn count_label(count: i32) -> String {
    let count = count.max(0);
    let short = |value: i32, unit: i32, suffix: &str| {
        let tenths = value / (unit / 10);
        if tenths < 100 && tenths % 10 != 0 {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        } else {
            format!("{}{suffix}", value / unit)
        }
    };
    match count {
        0..1000 => count.to_string(),
        1000..1_000_000 => short(count, 1000, "K"),
        _ => short(count, 1_000_000, "M"),
    }
}

pub(crate) fn chip_width(label: &str) -> f32 {
    CHIP_PAD_LEFT + EMOJI_BOX + EMOJI_SKIP + label.chars().count() as f32 * DIGIT + CHIP_PAD_RIGHT
}

/// Chip positions relative to the top-left of the reaction block.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Placement {
    /// (x, y) per chip, in input order.
    pub chips: Vec<(f32, f32)>,
    /// Width of the widest row.
    pub width: f32,
    pub height: f32,
    /// Right edge and top of the last row, where the time may follow.
    pub last_right: f32,
    pub last_y: f32,
}

/// Rows fill left to right and wrap at `available`; a chip wider than a row gets its own row.
pub(crate) fn place(widths: &[f32], available: f32) -> Placement {
    let mut placement = Placement::default();
    if widths.is_empty() {
        return placement;
    }
    let (mut x, mut y) = (0.0_f32, 0.0_f32);
    for &width in widths {
        if x > 0.0 && x + width > available {
            x = 0.0;
            y += CHIP_HEIGHT + CHIP_GAP;
        }
        placement.chips.push((x, y));
        x += width;
        placement.width = placement.width.max(x);
        x += CHIP_GAP;
    }
    placement.last_right = x - CHIP_GAP;
    placement.last_y = y;
    placement.height = y + CHIP_HEIGHT;
    placement
}

/// The member of a row that owns its reactions: the captioned album item, otherwise the first.
pub(crate) fn owner(group: &[Message]) -> Option<&Message> {
    group
        .iter()
        .find(|m| group.len() > 1 && !m.text.is_empty())
        .or_else(|| group.first())
}

/// Messages still being sent, or that failed, cannot get reactions.
pub(crate) fn can_react(group: &[Message]) -> bool {
    !group.is_empty()
        && !group[0].service
        && group
            .iter()
            .all(|m| m.id.0 > 0 && m.send_state == tg_core::SendState::Sent)
}

/// Asks the emoji font for its colour presentation of a single symbol like the bare heart.
fn display(emoji: &str) -> std::borrow::Cow<'_, str> {
    if emoji.chars().count() == 1 {
        format!("{emoji}\u{FE0F}").into()
    } else {
        emoji.into()
    }
}

/// A colour emoji raster and its size in bytes.
pub(crate) fn emoji_image(
    engine: &mut TextEngine,
    emoji: &str,
    scale: f32,
    font: f32,
) -> (Image, usize) {
    let rendered = engine.render_symbol(&display(emoji), scale, font);
    let bytes = rendered.pixels.len();
    (
        Image::from_rgba8_premultiplied(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &rendered.pixels,
            rendered.width,
            rendered.height,
        )),
        bytes,
    )
}

/// Chips of one row: Slint items, block placement and raster bytes.
pub(crate) fn chips(
    engine: &mut TextEngine,
    reactions: &[Reaction],
    scale: f32,
    available: f32,
) -> (Vec<ReactionChip>, Placement, usize) {
    let labels: Vec<String> = reactions.iter().map(|r| count_label(r.count)).collect();
    let widths: Vec<f32> = labels.iter().map(|l| chip_width(l)).collect();
    let placement = place(&widths, available);
    let mut bytes = 0;
    let items = reactions
        .iter()
        .zip(labels)
        .zip(&widths)
        .zip(&placement.chips)
        .map(|(((reaction, label), &width), &(x, y))| {
            let (emoji, cost) = emoji_image(engine, &reaction.emoji, scale, CHIP_FONT);
            bytes += cost;
            ReactionChip {
                x,
                y,
                width,
                emoji,
                count: label.into(),
                chosen: reaction.chosen,
                reaction: reaction.emoji.as_str().into(),
            }
        })
        .collect();
    (items, placement, bytes)
}

/// The quick-reaction strip. Its six rasters are drawn once per display scale.
#[derive(Default)]
pub(crate) struct Quick {
    pub model: Rc<VecModel<QuickReaction>>,
    scale: Cell<f32>,
}
impl Quick {
    /// Prepares the strip for the message row `key`: rasters at the current scale and the
    /// chosen marks of that message. Returns whether the message can get reactions.
    /// After a reaction update: when the open menu's row `key` belongs to `message`, redraws
    /// the chosen marks from the new list and returns whether the row can still react.
    /// Otherwise the strip stays as it is and `None` is returned.
    pub fn refresh(
        &self,
        feed: &Feed,
        key: MessageId,
        chat: ChatId,
        message: MessageId,
    ) -> Option<bool> {
        let owner = feed.reaction_state(key).map(|(c, m, _)| (c, m));
        (owner == Some((chat, message))).then(|| self.open(feed, key))
    }
    pub fn open(&self, feed: &Feed, key: MessageId) -> bool {
        let scale = feed.scale.get();
        if self.scale.replace(scale) != scale || self.model.row_count() != QUICK.len() {
            let mut engine = feed.engine.borrow_mut();
            let items: Vec<QuickReaction> = QUICK
                .iter()
                .map(|&emoji| QuickReaction {
                    emoji: emoji_image(&mut engine, emoji, scale, QUICK_FONT).0,
                    reaction: emoji.into(),
                    chosen: false,
                })
                .collect();
            self.model.set_vec(items);
        }
        let Some((_, _, reactions)) = feed.reaction_state(key) else {
            return false;
        };
        for (index, &emoji) in QUICK.iter().enumerate() {
            let chosen = reactions
                .iter()
                .any(|r| r.chosen && tg_core::same_reaction(&r.emoji, emoji));
            if let Some(mut item) = self.model.row_data(index)
                && item.chosen != chosen
            {
                item.chosen = chosen;
                self.model.set_row_data(index, item);
            }
        }
        true
    }
}

impl State {
    pub(super) fn react(&self, w: &AppWindow, key: &str, reaction: &str) {
        let Some(key) = key.parse().ok().map(MessageId) else {
            return;
        };
        if !tg_core::valid_reaction(reaction) {
            return;
        }
        if let Some((chat, message, _)) = self.feed.reaction_state(key) {
            self.send(
                w,
                Command::ToggleReaction {
                    chat,
                    message,
                    reaction: reaction.to_owned(),
                },
            );
        }
    }
    /// Applies a reaction update. Interaction updates also carry view and forward counts, so
    /// an unchanged list is dropped without a relayout.
    pub(super) fn reactions_changed(
        &self,
        w: &AppWindow,
        chat: ChatId,
        message: MessageId,
        reactions: Vec<Reaction>,
    ) {
        {
            let mut history = self.feed.history.borrow_mut();
            let Some(m) = history
                .messages
                .iter_mut()
                .find(|m| m.chat == chat && m.id == message)
            else {
                return;
            };
            if m.reactions == reactions {
                return;
            }
            m.reactions = reactions;
        }
        self.feed.invalidate_message(message);
        self.refresh(w);
        // An open menu must not keep its opening snapshot: a click on a mark that is out of
        // date would toggle the reaction the other way.
        if w.get_menu_open()
            && let Ok(key) = w.get_menu_key().parse::<i64>()
            && let Some(can) = self
                .quick
                .refresh(&self.feed, MessageId(key), chat, message)
        {
            w.set_menu_can_react(can);
        }
    }
}

pub(super) fn bind(window: &AppWindow, state: &Rc<State>) {
    window.set_quick_reactions(ModelRc::from(state.quick.model.clone()));
    let s = state.clone();
    let weak = window.as_weak();
    window.on_react(move |key, reaction| {
        if let Some(w) = weak.upgrade() {
            s.react(&w, &key, &reaction);
        }
    });
    let s = state.clone();
    let weak = window.as_weak();
    window.on_menu_opened(move |key| {
        if let Some(w) = weak.upgrade() {
            let can = key
                .parse()
                .ok()
                .is_some_and(|key| s.quick.open(&s.feed, MessageId(key)));
            w.set_menu_can_react(can);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reacted(n: i64, chosen: bool) -> Message {
        Message {
            attachment: None,
            reply: None,
            forwarded: None,
            edited: false,
            is_text: true,
            service: false,
            permissions: None,
            video: None,
            photo: None,
            id: MessageId(n * 1048576),
            chat: ChatId(1),
            date: n as i32,
            outgoing: false,
            text: "Text".into(),
            entities: vec![],
            send_state: tg_core::SendState::Sent,
            album: 0,
            sender_id: 0,
            sender: None,
            reactions: vec![Reaction {
                emoji: "👍".into(),
                count: 1,
                chosen,
            }],
        }
    }

    #[test]
    fn an_open_strip_follows_updates_of_its_own_message_only() {
        let feed = Feed::new(None);
        feed.history.borrow_mut().messages = vec![reacted(1, false), reacted(2, false)];
        let quick = Quick::default();
        let key = MessageId(1048576);
        assert!(quick.open(&feed, key));
        let thumbs = |quick: &Quick| quick.model.row_data(0).is_some_and(|q| q.chosen);
        assert!(!thumbs(&quick));
        // The toggle comes back after the menu opened (or another device changed it).
        feed.history.borrow_mut().messages[0].reactions[0].chosen = true;
        assert_eq!(quick.refresh(&feed, key, ChatId(1), key), Some(true));
        assert!(thumbs(&quick));
        // An update of another message leaves the open strip alone.
        feed.history.borrow_mut().messages[0].reactions[0].chosen = false;
        let other = MessageId(2 * 1048576);
        assert_eq!(quick.refresh(&feed, key, ChatId(1), other), None);
        assert!(thumbs(&quick));
    }

    #[test]
    fn counters_are_shortened_like_telegram_desktop() {
        for (count, label) in [
            (0, "0"),
            (7, "7"),
            (999, "999"),
            (1000, "1K"),
            (1234, "1.2K"),
            (9999, "9.9K"),
            (12_345, "12K"),
            (999_999, "999K"),
            (1_250_000, "1.2M"),
            (25_000_000, "25M"),
            (-3, "0"),
        ] {
            assert_eq!(count_label(count), label, "{count}");
        }
    }

    #[test]
    fn chips_wrap_at_the_available_width_and_report_the_last_row() {
        let w = chip_width("1");
        assert_eq!(place(&[], 100.0), Placement::default());
        let one = place(&[w], 300.0);
        assert_eq!(one.chips, vec![(0.0, 0.0)]);
        assert_eq!(one.height, CHIP_HEIGHT);
        assert_eq!(one.last_right, w);

        // Two chips fit per row: the third wraps.
        let available = 2.0 * w + CHIP_GAP + 1.0;
        let three = place(&[w, w, w], available);
        let row = CHIP_HEIGHT + CHIP_GAP;
        assert_eq!(
            three.chips,
            vec![(0.0, 0.0), (w + CHIP_GAP, 0.0), (0.0, row)]
        );
        assert_eq!(three.width, 2.0 * w + CHIP_GAP);
        assert_eq!(three.height, row + CHIP_HEIGHT);
        assert_eq!((three.last_right, three.last_y), (w, row));

        // A chip wider than the row still gets a row of its own.
        let wide = place(&[w, 500.0], 100.0);
        assert_eq!(wide.chips[1], (0.0, row));
        assert!(wide.chips.iter().all(|c| c.0 >= 0.0));
    }

    #[test]
    fn the_quick_strip_uses_telegram_spelling() {
        assert_eq!(QUICK.len(), 6);
        assert!(QUICK.iter().all(|e| tg_core::valid_reaction(e)));
        assert!(QUICK.iter().all(|e| !e.contains('\u{FE0F}')));
        assert_eq!(display("\u{2764}"), "\u{2764}\u{FE0F}");
        assert_eq!(display("👍🏻"), "👍🏻");
    }
}
