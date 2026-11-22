//! The open chat only: at most 300 messages plus one transient page per direction.
use tg_core::{ChatId, Message, MessageId};
pub(crate) const MAX_MESSAGES: usize = 300;
#[derive(Default)]
pub(crate) struct History {
    pub chat: Option<ChatId>,
    pub messages: Vec<Message>,
    pub older: Option<MessageId>,
    pub newer: Option<MessageId>,
    pub pending_older: Option<MessageId>,
    pub pending_newer: Option<MessageId>,
    pub pending_around: Option<MessageId>,
    pub latest_requested: bool,
}
impl History {
    pub fn open(&mut self, chat: ChatId) {
        *self = Self {
            chat: Some(chat),
            pending_older: Some(MessageId(0)),
            ..Self::default()
        };
    }
    pub fn open_around(&mut self, chat: ChatId, anchor: MessageId) {
        self.open(chat);
        self.pending_older = None;
        self.pending_around = Some(anchor);
    }
    pub fn around(&mut self, chat: ChatId, messages: Vec<Message>, anchor: MessageId) -> bool {
        if self.chat != Some(chat)
            || self.pending_around != Some(anchor)
            || !self.messages.is_empty()
        {
            return false;
        }
        self.pending_around = None;
        self.messages = messages
            .into_iter()
            .filter(|m| m.chat == chat)
            .take(50)
            .collect();
        self.messages.sort_by_key(|m| (m.date, m.id));
        self.older = self.messages.first().map(|m| m.id);
        self.newer = self
            .messages
            .last()
            .map(|m| m.id)
            .or((anchor.0 > 0).then_some(anchor));
        self.pending_older = None;
        self.pending_newer = None;
        true
    }
    pub fn remove(&mut self, chat: ChatId, ids: &[MessageId]) {
        if self.chat == Some(chat) {
            self.messages.retain(|m| !ids.contains(&m.id));
        }
    }
    pub fn at_tail(&self) -> bool {
        self.newer.is_none()
    }
    pub fn request_older(&mut self) -> Option<MessageId> {
        if self.pending_older.is_some() {
            return None;
        }
        let cursor = if self.latest_requested {
            MessageId(0)
        } else {
            self.older?
        };
        self.pending_older = Some(cursor);
        Some(cursor)
    }
    pub fn request_newer(&mut self) -> Option<MessageId> {
        if self.pending_newer.is_some() || self.latest_requested {
            return None;
        }
        let cursor = self.newer?;
        self.pending_newer = Some(cursor);
        Some(cursor)
    }
    /// Reject responses that no longer belong to the requested edge. Never derive IDs by arithmetic.
    pub fn page(
        &mut self,
        chat: ChatId,
        cursor: MessageId,
        next: Option<MessageId>,
        messages: Vec<Message>,
        newer: bool,
    ) -> bool {
        let pending = if newer {
            self.pending_newer
        } else {
            self.pending_older
        };
        if self.chat != Some(chat) || pending != Some(cursor) {
            return false;
        }
        if newer {
            self.pending_newer = None;
        } else {
            self.pending_older = None;
        }
        if !newer && cursor == MessageId(0) {
            self.messages.clear();
            self.newer = None;
            self.pending_newer = None;
            self.latest_requested = false;
        }
        let next = next.filter(|id| {
            if newer {
                *id > cursor
            } else {
                cursor.0 == 0 || *id < cursor
            }
        });
        if newer {
            self.newer = next;
        } else {
            self.older = next;
        }
        for message in messages.into_iter().take(50) {
            if message.chat != chat {
                continue;
            }
            if let Some(index) = self.messages.iter().position(|m| m.id == message.id) {
                self.messages[index] = message;
            } else {
                self.messages.push(message);
            }
        }
        self.messages.sort_by_key(|m| (m.date, m.id));
        self.trim(newer);
        true
    }
    fn trim(&mut self, newer: bool) {
        if self.messages.len() <= MAX_MESSAGES {
            return;
        }
        if newer {
            self.messages.drain(..self.messages.len() - MAX_MESSAGES);
            self.older = self.messages.first().map(|m| m.id);
        } else {
            self.messages.truncate(MAX_MESSAGES);
            self.newer = self.messages.last().map(|m| m.id);
        }
    }
    pub fn upsert(&mut self, old: Option<MessageId>, message: Message, follow_tail: bool) {
        if self.chat != Some(message.chat) {
            return;
        }
        if let Some(index) = self
            .messages
            .iter()
            .position(|m| Some(m.id) == old || m.id == message.id)
        {
            self.messages[index] = message;
        } else if self.at_tail() {
            // A send result can arrive before its pending message was shown; append it then.
            if self.messages.len() == MAX_MESSAGES && !follow_tail {
                self.newer = self.messages.last().map(|m| m.id);
                return;
            }
            self.messages.push(message);
        }
        self.messages.sort_by_key(|m| (m.date, m.id));
        self.trim(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tg_core::SendState;
    fn id(n: i64) -> MessageId {
        MessageId(n * 1048576)
    }
    fn message(n: i64) -> Message {
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
            id: id(n),
            chat: ChatId(1),
            date: n as i32,
            outgoing: false,
            text: "synthetic".into(),
            entities: vec![],
            send_state: SendState::Sent,
            album: 0,
            sender_id: 0,
            sender: None,
            reactions: Vec::new(),
        }
    }
    fn page(h: &mut History, start: i64, end: i64, newer: bool) {
        let cursor = if newer {
            h.request_newer()
        } else {
            h.request_older()
        }
        .unwrap();
        let next = Some(id(if newer { end } else { start }));
        assert!(h.page(
            ChatId(1),
            cursor,
            next,
            (start..=end).rev().map(message).collect(),
            newer
        ));
    }
    #[test]
    fn around_first_unread_keeps_both_real_cursors_and_rejects_stale_chat() {
        let mut h = History::default();
        h.open_around(ChatId(1), id(19980));
        assert!(!h.around(ChatId(2), vec![message(20000)], id(19980)));
        assert!(!h.around(ChatId(1), vec![message(20000)], id(19800)));
        assert!(h.around(ChatId(1), (19955..=20004).map(message).collect(), id(19980)));
        assert_eq!(h.request_older(), Some(id(19955)));
        assert_eq!(h.request_newer(), Some(id(20004)));
        assert!(!h.at_tail());
        assert_eq!(h.messages.len(), 50);
    }
    #[test]
    fn long_scroll_both_directions_is_bounded_and_reopens_evicted_edges() {
        let mut h = History::default();
        h.open(ChatId(1));
        assert!(h.page(
            ChatId(1),
            id(0),
            Some(id(19951)),
            (19951..=20000).map(message).collect(),
            false
        ));
        for end in (350..19951).rev().step_by(50) {
            page(&mut h, end - 49, end, false);
            assert!(h.messages.len() <= 300);
        }
        assert!(h.newer.is_some());
        assert_eq!(h.messages.first().unwrap().id, id(301));
        for _ in 0..390 {
            let start = h.newer.unwrap().0 / 1048576 + 1;
            page(&mut h, start, start + 49, true);
            assert_eq!(h.messages.len(), 300);
        }
        assert!(h.older.is_some());
    }
    #[test]
    fn one_in_flight_each_direction_stale_pages_and_tail_reload() {
        let mut h = History::default();
        h.open(ChatId(1));
        assert_eq!(h.request_older(), None);
        h.page(ChatId(1), id(0), Some(id(100)), vec![message(100)], false);
        assert_eq!(h.request_older(), Some(id(100)));
        assert_eq!(h.request_older(), None);
        assert!(!h.page(ChatId(2), id(100), None, vec![], false));
        assert!(!h.page(ChatId(1), id(99), None, vec![], false));
        h.page(
            ChatId(1),
            id(100),
            Some(id(50)),
            (50..100).map(message).collect(),
            false,
        );
        h.latest_requested = true;
        assert_eq!(h.request_older(), Some(id(0)));
        assert_eq!(h.messages.len(), 51); // Existing feed remains during latency.
        h.page(ChatId(1), id(0), Some(id(200)), vec![message(200)], false);
        assert_eq!(h.messages.len(), 1);
        assert!(h.at_tail());
    }
    #[test]
    fn incoming_does_not_evict_reading_anchor_or_pollute_a_historical_window() {
        let mut h = History::default();
        h.open(ChatId(1));
        h.page(
            ChatId(1),
            id(0),
            Some(id(251)),
            (251..=300).map(message).collect(),
            false,
        );
        for end in [250, 200, 150, 100, 50] {
            page(&mut h, end - 49, end, false);
        }
        h.upsert(None, message(301), false);
        assert_eq!(h.messages[0].id, id(1));
        assert_eq!(h.newer, Some(id(300)));
        h.upsert(None, message(302), true);
        assert_eq!(h.messages.len(), 300);
        assert_eq!(h.messages.last().unwrap().id, id(300));
        assert_eq!(h.request_newer(), Some(id(300)));
        assert_eq!(h.request_newer(), None);
        h.page(
            ChatId(1),
            id(300),
            Some(id(302)),
            vec![message(301), message(302)],
            true,
        );
        assert_eq!(h.messages[0].id, id(3));
        h.request_newer();
        h.page(ChatId(1), id(302), None, vec![], true);
        assert!(h.at_tail());
        h.upsert(None, message(303), true);
        assert_eq!(h.messages.last().unwrap().id, id(303));
    }
    #[test]
    fn raw_cursors_empty_pages_and_deduplication() {
        let mut h = History::default();
        h.open(ChatId(1));
        h.page(ChatId(1), id(0), Some(id(100)), vec![], false);
        assert_eq!(h.request_older(), Some(id(100)));
        h.page(
            ChatId(1),
            id(100),
            Some(id(50)),
            vec![message(60), message(60)],
            false,
        );
        assert_eq!(h.messages.len(), 1);
        h.upsert(Some(id(60)), message(61), false);
        assert_eq!(h.messages.len(), 1);
        assert_eq!(h.messages[0].id, id(61));
        h.request_older();
        h.page(ChatId(1), id(50), None, vec![], false);
        assert_eq!(h.older, None);
    }
    #[test]
    fn send_result_without_a_shown_pending_message_is_appended_at_the_tail() {
        let mut h = History::default();
        h.open(ChatId(1));
        h.page(ChatId(1), id(0), None, vec![message(10)], false);
        h.upsert(Some(MessageId(-5)), message(11), false);
        assert_eq!(
            h.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            [id(10), id(11)]
        );
        // Away from the tail the newer page is loaded later instead.
        h.newer = Some(id(11));
        h.upsert(Some(MessageId(-6)), message(12), false);
        assert_eq!(h.messages.len(), 2);
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    #[test]
    fn delete_does_not_reset_paging_or_remove_another_chat() {
        let mut h = History::default();
        h.open(ChatId(7));
        let cursor = MessageId(20971520000);
        h.older = Some(cursor);
        h.newer = Some(cursor);
        h.remove(ChatId(8), &[cursor]);
        h.remove(ChatId(7), &[cursor]);
        assert_eq!(h.older, Some(cursor));
        assert_eq!(h.newer, Some(cursor));
    }
}
