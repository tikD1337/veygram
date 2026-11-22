//! Message reactions: the chips under a bubble.

/// At most this many reactions are kept per message; the rest are dropped by the engine.
pub const MAX_REACTIONS: usize = 32;
/// Longest accepted reaction emoji in UTF-8 bytes (ZWJ sequences stay well below it).
pub const MAX_REACTION_BYTES: usize = 64;

/// One emoji reaction on a message. Custom-emoji and paid reactions are not mapped yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    /// The emoji as Telegram reports it, at most [`MAX_REACTION_BYTES`].
    pub emoji: String,
    /// Number of users who added it; never negative.
    pub count: i32,
    /// The current user has added this reaction.
    pub chosen: bool,
}

/// Whether two reaction emoji are the same, ignoring variation selectors.
///
/// Telegram reports the red heart as `U+2764` without `U+FE0F`, while emoji pickers and
/// keyboards usually add the selector.
pub fn same_reaction(a: &str, b: &str) -> bool {
    let plain = |c: &char| !matches!(c, '\u{FE0E}' | '\u{FE0F}');
    a.chars().filter(plain).eq(b.chars().filter(plain))
}

/// Whether a reaction string is acceptable to send: non-empty, bounded, without control
/// characters.
pub fn valid_reaction(emoji: &str) -> bool {
    !emoji.trim().is_empty()
        && emoji.len() <= MAX_REACTION_BYTES
        && !emoji.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variation_selectors_do_not_change_identity() {
        assert!(same_reaction("\u{2764}", "\u{2764}\u{FE0F}"));
        assert!(same_reaction("👍", "👍"));
        assert!(!same_reaction("👍", "👎"));
        assert!(!same_reaction("👍", "👍👍"));
    }

    #[test]
    fn reactions_to_send_are_bounded() {
        assert!(valid_reaction("🔥"));
        assert!(!valid_reaction(""));
        assert!(!valid_reaction(" "));
        assert!(!valid_reaction("a\u{0}"));
        assert!(!valid_reaction(&"👍".repeat(17)));
    }
}
