//! Visible-row text images: Parley shaping and Swash color/mask rasterization.
//! No rendered-image cache: the feed owns images only for its visible Flickable rows.
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, GlyphRun, Layout,
    LayoutContext, PositionedLayoutItem, StyleProperty,
};
use std::{borrow::Cow, collections::HashMap, ops::Range};
use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    zeno::{Format, Vector},
};
use tg_core::{EntityKind, Message};
type Color = [u8; 4];
const WHITE: Color = [255, 255, 255, 255];
const LINK: Color = [106, 179, 243, 255];
const CACHE_BYTES: usize = 2 * 1024 * 1024;
// Includes negative entries and metadata, so missing glyphs cannot grow the cache forever.
const CACHE_ENTRIES: usize = 8192;
const ENTRY_COST: usize = 128;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_LINK_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub(crate) struct Links {
    entries: Vec<Link>,
}
struct Link {
    target: LinkTarget,
    rects: Vec<[f32; 4]>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkTarget {
    pub url: String,
    pub hidden: bool,
}
impl Links {
    fn from_layout(
        message: &Message,
        layout: &Layout<Color>,
        placements: &[LinePlacement],
        padding: f32,
        width: u32,
        height: u32,
    ) -> Self {
        use parley::{Affinity, Cursor, Selection};
        let spoilers: Vec<_> = message
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Spoiler)
            .filter_map(|e| byte_range(&message.text, e.offset, e.length))
            .collect();
        let mut links = Self::default();
        let mut bytes = 0;
        for entity in &message.entities {
            if !matches!(
                entity.kind,
                EntityKind::Url | EntityKind::TextUrl { .. } | EntityKind::Mention
            ) {
                continue;
            }
            let Some(range) = byte_range(&message.text, entity.offset, entity.length) else {
                continue;
            };
            if range.is_empty()
                || spoilers
                    .iter()
                    .any(|s| s.start < range.end && range.start < s.end)
            {
                continue;
            }
            let label = &message.text[range.clone()];
            let target = match &entity.kind {
                EntityKind::Url => tg_core::external_url(label),
                EntityKind::TextUrl { target } => tg_core::external_url(target),
                EntityKind::Mention => label
                    .strip_prefix('@')
                    .filter(|name| {
                        !name.is_empty()
                            && name.len() <= 32
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    })
                    .map(|name| format!("https://t.me/{name}")),
                _ => None,
            };
            let Some(url) = target else {
                continue;
            };
            let hidden = matches!(entity.kind, EntityKind::TextUrl { .. })
                && tg_core::external_url(label).as_deref() != Some(url.as_str());
            let start = Cursor::from_byte_index(layout, range.start, Affinity::Downstream);
            let end = Cursor::from_byte_index(layout, range.end, Affinity::Upstream);
            let rects: Vec<_> = Selection::new(start, end)
                .geometry(layout)
                .into_iter()
                .filter_map(|(rect, line)| {
                    let p = placements.get(line)?;
                    let x0 = (rect.x0 as f32 + padding).max(0.0);
                    let x1 = (rect.x1 as f32 + padding).min(width as f32);
                    let y0 = p.top.max(0.0);
                    let y1 = p.bottom.min(height as f32);
                    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
                })
                .collect();
            // Charge double entry size for Vec growth; targets and geometry are byte-bounded.
            let cost = 2 * std::mem::size_of::<Link>()
                + url.capacity()
                + rects.capacity() * std::mem::size_of::<[f32; 4]>();
            if bytes + cost > MAX_LINK_BYTES {
                break;
            }
            if !rects.is_empty() {
                bytes += cost;
                links.entries.push(Link {
                    target: LinkTarget { url, hidden },
                    rects,
                });
            }
        }
        links
    }
    /// Physical image coordinates, with half-open bounds so blank space never snaps to text.
    pub fn link_at(&self, (x, y): (f32, f32)) -> Option<&LinkTarget> {
        self.entries
            .iter()
            .find(|link| {
                link.rects
                    .iter()
                    .any(|&[x0, y0, x1, y1]| x >= x0 && x < x1 && y >= y0 && y < y1)
            })
            .map(|link| &link.target)
    }
    pub fn bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Link>()
            + self
                .entries
                .iter()
                .map(|link| {
                    link.target.url.capacity()
                        + link.rects.capacity() * std::mem::size_of::<[f32; 4]>()
                })
                .sum::<usize>()
    }
}

pub(crate) fn byte_range(text: &str, offset: u32, length: u32) -> Option<Range<usize>> {
    let end = offset.checked_add(length)?;
    let mut units = 0;
    let mut start = None;
    for (byte, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        if units == offset {
            start = Some(byte);
        }
        if units == end {
            return start.map(|start| start..byte);
        }
        units += ch.len_utf16() as u32;
    }
    None
}

#[derive(Hash, PartialEq, Eq)]
struct GlyphKey {
    font: u64,
    index: u32,
    size: u32,
    coords: u64,
    id: u16,
    bin: u8,
}
struct Glyph {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    color: bool,
    bytes: Vec<u8>,
}
pub(crate) struct Rendered {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub clipped: bool,
    /// Advance of the last line in pixels; the full width for RTL text.
    pub last_line: u32,
    pub links: Links,
}
// Parley 0.12 adds a root-font strut and rounds normal line heights. Preserve the
// measured 0.11 message geometry: actual run metrics, fractional line accumulation,
// and pixel-aligned baselines. Shaping, wrapping and bidi remain entirely Parley's.
struct LinePlacement {
    shift: f32,
    top: f32,
    bottom: f32,
    content_top: f32,
}
fn message_lines(layout: &Layout<Color>, text: &str) -> (Vec<LinePlacement>, f32) {
    // At most one line per input byte plus an empty trailing line (engine limit: 64 KiB).
    let mut lines = Vec::new();
    let mut y = 0.0_f64;
    for line in layout.lines().take(65537) {
        let range = line.text_range();
        let non_space_end =
            range.start + text.get(range.clone()).unwrap_or_default().trim_end().len();
        let (mut ascent, mut descent, mut height) = (0.0_f32, 0.0_f32, 0.0_f32);
        for run in line.runs() {
            let metrics = run.font_metrics();
            height = height.max(metrics.ascent + metrics.descent + metrics.leading);
            if run.text_range().start < non_space_end || non_space_end == range.start {
                ascent = ascent.max(metrics.ascent);
                descent = descent.max(metrics.descent);
            }
        }
        let (a, d) = (ascent.round(), descent.round());
        let leading = height - a - d;
        let above = (leading * 0.5).floor();
        let baseline = y.round() as f32 + a + above;
        lines.push(LinePlacement {
            shift: baseline - line.metrics().baseline,
            top: baseline - a - above.max(0.0),
            bottom: baseline + d + (leading.round() - above).max(0.0),
            content_top: baseline - ascent,
        });
        y += f64::from(height);
    }
    (lines, y as f32)
}

pub(crate) struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<Color>,
    scaler: ScaleContext,
    // Bounded by CACHE_BYTES including a conservative per-entry bookkeeping charge.
    glyphs: HashMap<GlyphKey, Option<Glyph>>,
    bytes: usize,
    // At most 128 font identities (~8 KiB); stable keys let Swash reuse its 8 font entries.
    font_keys: HashMap<(u64, u32), (u32, swash::CacheKey)>,
    emoji_family: String,
}
impl TextEngine {
    pub fn new(font: Option<Vec<u8>>) -> Self {
        let mut fonts = FontContext::new();
        let mut emoji_family = "Segoe UI Emoji".to_owned();
        if let Some(data) = font {
            let families = fonts
                .collection
                .register_fonts(parley::fontique::Blob::new(std::sync::Arc::new(data)), None);
            if let Some((id, _)) = families.first()
                && let Some(name) = fonts.collection.family_name(*id)
            {
                emoji_family = name.to_owned();
            }
        }
        Self {
            fonts,
            layouts: LayoutContext::new(),
            scaler: ScaleContext::new(),
            glyphs: HashMap::new(),
            bytes: 0,
            font_keys: HashMap::new(),
            emoji_family,
        }
    }
    pub fn render_emoji(&mut self, message: &Message, scale: f32, width: f32) -> Rendered {
        let rendered = self.render_sized(message, scale, width, 120.0);
        trim(rendered)
    }
    /// A short plain string (a reaction emoji) at `font_size`, trimmed to its painted bounds.
    /// At most MAX_REACTION_BYTES of text; the raster is a few kilobytes.
    pub fn render_symbol(&mut self, text: &str, scale: f32, font_size: f32) -> Rendered {
        let message = Message {
            attachment: None,
            reply: None,
            forwarded: None,
            edited: false,
            is_text: true,
            service: false,
            permissions: None,
            video: None,
            photo: None,
            id: tg_core::MessageId(0),
            chat: tg_core::ChatId(0),
            date: 0,
            outgoing: false,
            text: text.chars().take(tg_core::MAX_REACTION_BYTES).collect(),
            entities: Vec::new(),
            send_state: tg_core::SendState::Sent,
            album: 0,
            sender_id: 0,
            sender: None,
            reactions: Vec::new(),
        };
        let rendered = self.render_sized(&message, scale, font_size * 4.0 * scale, font_size);
        trim(rendered)
    }
    pub fn render(&mut self, message: &Message, scale: f32, width: f32) -> Rendered {
        self.render_sized(message, scale, width, 14.0)
    }
    /// Shape at the requested font size; callers display the resulting pixels 1:1 at the DPI.
    pub fn render_sized(
        &mut self,
        message: &Message,
        scale: f32,
        width: f32,
        font_size: f32,
    ) -> Rendered {
        let family = format!("Segoe UI, {}, sans-serif", self.emoji_family);
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, &message.text, scale, true);
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(
            Cow::Borrowed(&family),
        )));
        builder.push_default(StyleProperty::Brush(WHITE));
        for entity in &message.entities {
            let Some(range) = byte_range(&message.text, entity.offset, entity.length) else {
                continue;
            };
            match entity.kind {
                EntityKind::Bold => {
                    builder.push(StyleProperty::FontWeight(FontWeight::BOLD), range)
                }
                EntityKind::Italic => {
                    builder.push(StyleProperty::FontStyle(FontStyle::Italic), range)
                }
                EntityKind::Underline => builder.push(StyleProperty::Underline(true), range),
                EntityKind::Strikethrough => {
                    builder.push(StyleProperty::Strikethrough(true), range)
                }
                EntityKind::Code | EntityKind::Pre => builder.push(
                    StyleProperty::FontFamily(FontFamily::Source(Cow::Borrowed(
                        "Consolas, monospace",
                    ))),
                    range,
                ),
                EntityKind::Url | EntityKind::TextUrl { .. } | EntityKind::Mention => {
                    builder.push(StyleProperty::Brush(LINK), range.clone());
                    builder.push(StyleProperty::Underline(true), range);
                }
                EntityKind::Spoiler => {}
            }
        }
        let mut layout: Layout<Color> = builder.build(&message.text);
        layout.break_all_lines(Some(width));
        // As in the measured spike, RTL must align within the image's actual full width.
        if layout.is_rtl() {
            layout.break_all_lines(Some(layout.full_width().ceil()));
        }
        layout.align(Alignment::Start, AlignmentOptions::default());
        let image_width = if layout.is_rtl() {
            layout.full_width()
        } else {
            layout.width()
        }
        .ceil()
        .clamp(1.0, width) as u32;
        let (mut placements, legacy_height) = message_lines(&layout, &message.text);
        // Large colour strikes can extend beyond the font's line metrics. Leave raster room
        // above/below the baseline; render_emoji trims transparent padding afterwards.
        let padding = if font_size > 14.0 {
            (font_size * scale * 0.3).ceil() as u32
        } else {
            0
        };
        for p in &mut placements {
            p.shift += padding as f32;
            p.top += padding as f32;
            p.bottom += padding as f32;
            p.content_top += padding as f32;
        }
        let image_width = image_width + 2 * padding;
        let full_height = legacy_height.ceil().max(1.0) as u32 + 2 * padding;
        let height = full_height.min((MAX_IMAGE_BYTES / (image_width as usize * 4)) as u32);
        let mut canvas = Canvas {
            width: image_width,
            height,
            pixels: vec![0; image_width as usize * height as usize * 4],
        };
        // Code backgrounds are composited beneath glyphs, unlike opaque spoiler covers.
        for entity in message
            .entities
            .iter()
            .filter(|e| matches!(e.kind, EntityKind::Code | EntityKind::Pre))
        {
            let Some(range) = byte_range(&message.text, entity.offset, entity.length) else {
                continue;
            };
            use parley::{Affinity, Cursor, Selection};
            let a = Cursor::from_byte_index(&layout, range.start, Affinity::Downstream);
            let b = Cursor::from_byte_index(&layout, range.end, Affinity::Upstream);
            for (rect, line) in Selection::new(a, b).geometry(&layout) {
                let Some(placement) = placements.get(line) else {
                    continue;
                };
                canvas.rect(
                    rect.x0 as f32,
                    placement.top,
                    rect.x1 as f32,
                    placement.bottom,
                    [255, 255, 255, 28],
                );
            }
        }
        for (line, placement) in layout.lines().zip(&placements) {
            if placement.content_top > height as f32 {
                break;
            }
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    self.draw(&mut canvas, &run, placement.shift, padding as f32);
                }
            }
        }
        for entity in message
            .entities
            .iter()
            .filter(|entity| entity.kind == EntityKind::Spoiler)
        {
            let Some(range) = byte_range(&message.text, entity.offset, entity.length) else {
                continue;
            };
            use parley::{Affinity, Cursor, Selection};
            let a = Cursor::from_byte_index(&layout, range.start, Affinity::Downstream);
            let b = Cursor::from_byte_index(&layout, range.end, Affinity::Upstream);
            for (rect, line) in Selection::new(a, b).geometry(&layout) {
                let Some(placement) = placements.get(line) else {
                    continue;
                };
                canvas.rect(
                    rect.x0 as f32,
                    placement.top,
                    rect.x1 as f32,
                    placement.bottom,
                    [120, 134, 148, 255],
                );
            }
        }
        let last_line = if layout.is_rtl() {
            image_width
        } else {
            layout
                .lines()
                .last()
                .map_or(0.0, |line| line.metrics().advance)
                .ceil() as u32
        };
        Rendered {
            pixels: canvas.pixels,
            width: image_width,
            height,
            clipped: height < full_height,
            last_line,
            links: Links::from_layout(
                message,
                &layout,
                &placements,
                padding as f32,
                image_width,
                height,
            ),
        }
    }
    fn draw(&mut self, canvas: &mut Canvas, run: &GlyphRun<'_, Color>, shift: f32, shift_x: f32) {
        let font = run.run().font();
        if self.font_keys.len() >= 128 {
            self.font_keys.clear();
        }
        let identity = (font.data.id(), font.index);
        let (offset, key) = if let Some(cached) = self.font_keys.get(&identity) {
            *cached
        } else {
            let Some(reference) = swash::FontRef::from_index(font.data.data(), font.index as usize)
            else {
                return;
            };
            let value = (reference.offset, reference.key);
            self.font_keys.insert(identity, value);
            value
        };
        let font_ref = swash::FontRef {
            data: font.data.data(),
            offset,
            key,
        };
        let coords = run.run().normalized_coords();
        let coords_hash = coords.iter().fold(0u64, |hash, x| {
            hash.wrapping_mul(16777619) ^ (x.to_bits() as u16 as u64)
        });
        for placed in run.positioned_glyphs() {
            let floor = placed.x.floor();
            let bin = ((placed.x - floor) * 4.0) as u8;
            let key = GlyphKey {
                font: font.data.id(),
                index: font.index,
                size: run.run().font_size().to_bits(),
                coords: coords_hash,
                id: placed.id as u16,
                bin,
            };
            if !self.glyphs.contains_key(&key) {
                let mut scaler = self
                    .scaler
                    .builder(font_ref)
                    .size(run.run().font_size())
                    .normalized_coords(coords.iter().map(|coord| coord.to_bits()))
                    .build();
                let image = Render::new(&[
                    Source::ColorOutline(0),
                    Source::ColorBitmap(StrikeWith::BestFit),
                    Source::Outline,
                ])
                .format(Format::Alpha)
                .offset(Vector::new(f32::from(bin) / 4.0, 0.0))
                .render(&mut scaler, key.id);
                let glyph = image.map(|image| Glyph {
                    left: image.placement.left,
                    top: image.placement.top,
                    width: image.placement.width,
                    height: image.placement.height,
                    color: image.content == Content::Color,
                    bytes: image.data,
                });
                let cost = ENTRY_COST + glyph.as_ref().map_or(0, |g| g.bytes.len());
                if self.bytes + cost > CACHE_BYTES || self.glyphs.len() == CACHE_ENTRIES {
                    self.glyphs.clear();
                    self.bytes = 0;
                }
                if cost <= CACHE_BYTES {
                    self.bytes += cost;
                    self.glyphs.insert(key, glyph);
                }
            }
            if let Some(Some(glyph)) = self.glyphs.get(&GlyphKey {
                font: font.data.id(),
                index: font.index,
                size: run.run().font_size().to_bits(),
                coords: coords_hash,
                id: placed.id as u16,
                bin,
            }) {
                for y in 0..glyph.height {
                    for x in 0..glyph.width {
                        let i = (y * glyph.width + x) as usize;
                        let (color, coverage) = if glyph.color {
                            (
                                glyph.bytes[i * 4..i * 4 + 4].try_into().unwrap_or(WHITE),
                                255,
                            )
                        } else {
                            (run.style().brush, u32::from(glyph.bytes[i]))
                        };
                        canvas.blend(
                            floor as i32 + glyph.left + x as i32 + shift_x as i32,
                            (placed.y + shift).round() as i32 - glyph.top + y as i32,
                            color,
                            coverage,
                        );
                    }
                }
            }
        }
        let metrics = run.run().font_metrics();
        if let Some(line) = &run.style().underline {
            let y = run.baseline() + shift - line.offset.unwrap_or(metrics.underline_offset);
            canvas.rect(
                run.offset(),
                y,
                run.offset() + run.advance(),
                y + line.size.unwrap_or(metrics.underline_size).max(1.0),
                line.brush,
            );
        }
        if let Some(line) = &run.style().strikethrough {
            let y = run.baseline() + shift - line.offset.unwrap_or(metrics.strikethrough_offset);
            canvas.rect(
                run.offset(),
                y,
                run.offset() + run.advance(),
                y + line.size.unwrap_or(metrics.strikethrough_size).max(1.0),
                line.brush,
            );
        }
    }
}
struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}
impl Canvas {
    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha = u32::from(color[3]) * coverage / 255;
        let i = (y as usize * self.width as usize + x as usize) * 4;
        for (channel, value) in color.iter().take(3).enumerate() {
            self.pixels[i + channel] = (u32::from(*value) * alpha / 255
                + u32::from(self.pixels[i + channel]) * (255 - alpha) / 255)
                .min(255) as u8;
        }
        self.pixels[i + 3] =
            (alpha + u32::from(self.pixels[i + 3]) * (255 - alpha) / 255).min(255) as u8;
    }
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
        for y in (y0.round() as i32).max(0)..(y1.round() as i32).min(self.height as i32) {
            for x in (x0.round() as i32).max(0)..(x1.round() as i32).min(self.width as i32) {
                self.blend(x, y, color, 255);
            }
        }
    }
}
/// Emoji rasters are trimmed to painted bounds and are not interactive message text.
fn trim(mut rendered: Rendered) -> Rendered {
    rendered.links = Links::default();
    let (mut left, mut top) = (rendered.width, rendered.height);
    let (mut right, mut bottom) = (0, 0);
    for (y, row) in rendered
        .pixels
        .chunks_exact(rendered.width as usize * 4)
        .enumerate()
    {
        for (x, pixel) in row.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] != 0 {
                left = left.min(x as u32);
                right = right.max(x as u32 + 1);
                top = top.min(y as u32);
                bottom = bottom.max(y as u32 + 1);
            }
        }
    }
    if bottom > top && right > left {
        let mut pixels = Vec::with_capacity(((right - left) * (bottom - top) * 4) as usize);
        for row in rendered
            .pixels
            .chunks_exact(rendered.width as usize * 4)
            .skip(top as usize)
            .take((bottom - top) as usize)
        {
            pixels.extend_from_slice(&row[(left * 4) as usize..(right * 4) as usize]);
        }
        rendered.pixels = pixels;
        rendered.width = right - left;
        rendered.height = bottom - top;
    }
    rendered
}
#[cfg(test)]
mod tests {
    use super::*;
    fn linked_message(text: &str, range: Range<usize>, kind: EntityKind) -> Message {
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
            id: tg_core::MessageId(1),
            chat: tg_core::ChatId(1),
            date: 0,
            outgoing: false,
            text: text.into(),
            entities: vec![tg_core::TextEntity {
                offset: text[..range.start].encode_utf16().count() as u32,
                length: text[range].encode_utf16().count() as u32,
                kind,
            }],
            send_state: tg_core::SendState::Sent,
            album: 0,
            sender_id: 0,
            sender: None,
            reactions: Vec::new(),
        }
    }
    #[test]
    fn link_geometry_tracks_utf16_wrapping_rtl_and_scale() {
        let mut engine = TextEngine::new(None);
        for text in [
            "🙂 before example.org/long/path/that/wraps after",
            "שלום example.org/long/path/that/wraps סוף",
        ] {
            let start = text.find("example").unwrap();
            let end = text[start..].find(' ').map_or(text.len(), |n| start + n);
            let message = linked_message(text, start..end, EntityKind::Url);
            for scale in [1.0, 1.5, 2.0] {
                let rendered = engine.render(&message, scale, 90.0 * scale);
                assert!(!rendered.links.entries.is_empty());
                let link = &rendered.links.entries[0];
                assert!(link.rects.len() > 1);
                for &[x0, y0, x1, y1] in &link.rects {
                    assert_eq!(
                        rendered
                            .links
                            .link_at(((x0 + x1) / 2.0, (y0 + y1) / 2.0))
                            .map(|target| target.url.as_str()),
                        Some("https://example.org/long/path/that/wraps")
                    );
                    assert!(
                        x0 >= 0.0
                            && y0 >= 0.0
                            && x1 <= rendered.width as f32
                            && y1 <= rendered.height as f32
                    );
                }
                for point in [
                    (-1.0, 0.0),
                    (0.0, -1.0),
                    (rendered.width as f32, 0.0),
                    (0.0, rendered.height as f32),
                    (f32::NAN, 0.0),
                ] {
                    assert_eq!(rendered.links.link_at(point), None);
                }
            }
        }
    }
    #[test]
    fn hidden_links_invalid_ranges_mentions_and_text_urls() {
        let mut engine = TextEngine::new(None);
        for (text, kind, target, hidden) in [
            (
                "@telegram",
                EntityKind::Mention,
                "https://t.me/telegram",
                false,
            ),
            (
                "label",
                EntityKind::TextUrl {
                    target: "http://example.org/actual".into(),
                },
                "http://example.org/actual",
                true,
            ),
            (
                "example.org",
                EntityKind::TextUrl {
                    target: "HTTPS://example.org".into(),
                },
                "https://example.org/",
                false,
            ),
        ] {
            let mut message = linked_message(text, 0..text.len(), kind);
            let rendered = engine.render(&message, 1.0, 200.0);
            assert_eq!(
                rendered.links.entries[0].target,
                LinkTarget {
                    url: target.into(),
                    hidden
                }
            );
            message.entities.push(tg_core::TextEntity {
                offset: 1,
                length: 2,
                kind: EntityKind::Spoiler,
            });
            assert!(engine.render(&message, 1.0, 200.0).links.entries.is_empty());
            message.entities.pop();
            message.entities[0].offset = u32::MAX;
            assert!(engine.render(&message, 1.0, 200.0).links.entries.is_empty());
        }
        let message = linked_message(
            "label",
            0..5,
            EntityKind::TextUrl {
                target: "file:///C:/a.exe".into(),
            },
        );
        assert!(engine.render(&message, 1.0, 200.0).links.entries.is_empty());
    }
    #[test]
    fn multilingual_images_are_bounded_and_use_cached_glyphs() {
        let mut engine = TextEngine::new(None);
        let mut message = Message {
            attachment: None,
            reply: None,
            forwarded: None,
            edited: false,
            is_text: true,
            service: false,
            permissions: None,
            video: None,
            photo: None,
            id: tg_core::MessageId(1),
            chat: tg_core::ChatId(1),
            date: 0,
            outgoing: false,
            text: "Hello \u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{1f642}".into(),
            entities: vec![],
            send_state: tg_core::SendState::Sent,
            album: 0,
            sender_id: 0,
            sender: None,
            reactions: Vec::new(),
        };
        let rendered = engine.render(&message, 1.0, 180.0);
        assert!(rendered.width <= 180 && rendered.height > 0);
        assert!(
            rendered
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        );
        let cached = engine.glyphs.len();
        let bytes = engine.bytes;
        engine.render(&message, 1.0, 180.0);
        assert_eq!(engine.glyphs.len(), cached);
        assert_eq!(engine.bytes, bytes);
        message.text = "\u{1f642}".into();
        for scale in [1.0, 2.0] {
            let emoji = engine.render_emoji(&message, scale, 128.0 * scale);
            assert!(
                emoji.width <= (128.0 * scale) as u32 && emoji.height <= (128.0 * scale) as u32
            );
            assert!(emoji.height >= (110.0 * scale) as u32);
            let edge = emoji
                .pixels
                .chunks_exact(emoji.width as usize * 4)
                .filter(|row| row[row.len() - 1] != 0)
                .count();
            assert!(
                edge < emoji.height as usize / 2,
                "Round emoji glyph was clipped at the right edge"
            );
            assert!(
                emoji
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 0 && p[0] != p[2])
            );
        }
        message.text =
            "\u{05e9}\u{05dc}\u{05d5}\u{05dd} \u{0645}\u{0631}\u{062d}\u{0628}\u{0627}".into();
        let rtl = engine.render(&message, 1.0, 180.0);
        assert!(
            rtl.width <= 180
                && rtl
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] != 0)
        );
        assert!(engine.bytes <= CACHE_BYTES && engine.glyphs.len() <= CACHE_ENTRIES);
        assert!(engine.font_keys.len() <= 128);
    }
    #[test]
    fn utf16_ranges_reject_split_surrogates_and_overflow() {
        let text = "A\u{1f642}\u{05e9}\u{05dc}\u{05d5}\u{05dd}";
        assert_eq!(byte_range(text, 1, 2), Some(1..5));
        assert_eq!(byte_range(text, 3, 4), Some(5..13));
        assert_eq!(byte_range(text, 2, 1), None);
        assert_eq!(byte_range(text, 1, 1), None);
        assert_eq!(byte_range(text, u32::MAX, 2), None);
    }
    #[test]
    fn link_metadata_is_byte_bounded_and_empty_entities_do_not_hit() {
        let mut engine = TextEngine::new(None);
        let mut message = linked_message(
            "label",
            0..5,
            EntityKind::TextUrl {
                target: format!("https://example.org/{}", "x".repeat(4000)),
            },
        );
        message.entities = vec![message.entities[0].clone(); 1000];
        let rendered = engine.render(&message, 1.0, 200.0);
        assert!(!rendered.links.entries.is_empty() && rendered.links.entries.len() < 1000);
        assert!(rendered.links.bytes() <= MAX_LINK_BYTES);
        message.entities.truncate(1);
        message.entities[0].length = 0;
        assert!(engine.render(&message, 1.0, 200.0).links.entries.is_empty());
    }
}
