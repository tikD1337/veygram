//! Client domain model.
//!
//! Boundary between the UI and the Telegram engine: the UI sends [`Command`], the engine
//! responds with [`Event`]. No type here knows about TDLib or Slint — this
//! allows either side to be replaced and the logic to be tested without them.
//!
//! The set of commands and events is deliberately small and grows with the functionality.

mod attachments;
pub use attachments::{
    Attachment, Document, FileRequest, Round, Sticker, Voice, decode_waveform, is_executable_name,
    waveform_bars,
};
mod command;
mod event;
mod ids;
mod links;
mod reactions;
mod secret;
pub use links::external_url;
pub use reactions::{MAX_REACTION_BYTES, MAX_REACTIONS, Reaction, same_reaction, valid_reaction};

pub use command::{Command, CommandKind};
pub use event::{
    AuthState, Chat, ChatListChange, CommandError, EntityKind, Event, MediaFile, Message,
    MessagePermissions, MessagePreview, Photo, Reply, SecurityWarning, SendState, TextEntity,
};
pub use ids::{ChatId, MessageId, UserId};
pub use secret::Secret;

mod attention;
pub use attention::{Notification, ReadBatches};
mod video;
pub use video::{RoundFrame, RoundPixels, Video, VideoRect, VideoRequest, VideoState};

mod scale;
pub use scale::{
    SCALE_AUTO, SCALE_MAX, SCALE_MIN, SCALE_STEP, check_scale, parse_scale, scale_factor,
};

mod streaming;
pub use streaming::{
    LoadedRanges, MAX_LOADED_RANGES, STREAM_AHEAD_SECONDS, STREAM_PART, STREAM_PRELOAD, StreamFile,
    StreamRanges, StreamStatus, StreamWindow,
};
