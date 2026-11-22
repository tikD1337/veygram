# Architecture

Status: **accepted 2026-11-15.** The Slint prototype confirmed the memory and CPU case,
but left open questions about message text (section 3). The choice of TDLib is confirmed by
measurement: 28.7 MB of private memory after sign-in and synchronization (section 2).
Anything below marked “estimate” has not been measured.

## 1. What we are building and why

A native Telegram client for Windows 10/11 x64 that does the same things as Telegram
Desktop but uses significantly less RAM and CPU — especially when idle. Rust is a
means, not an end: where a proven library gives a better result, we use it.

Not goals for the first stage: Linux/macOS, calls, a custom MTProto implementation.

Failure, even if everything “works”: the client is lighter than the official one only because it
supports less; or savings come at the expense of protocol correctness or security.

## 2. Protocol layer: TDLib versus grammers

| Criterion | A. TDLib (C++, via `tdjson`) | B. grammers (pure Rust) |
|---|---|---|
| Status | Official Telegram library, used by Telegram X and Unigram | v0.12.0 dated 2026-11-08, actively developed, one main author |
| Completeness | All client logic: updates and gap recovery, local database, file manager, secret chats, folders, reactions, multiple accounts | Authentication (including 2FA), updates, messages, media — the high-level API is “gradually taking shape”; everything else uses raw TL calls |
| Missing features | Calls (requires separate tgcalls/WebRTC) | Local database, chat list model, secret chats, calls, client-level file manager |
| Audit | Years of use in official clients | The author explicitly states that the code has not been audited |
| RAM | Own chat/user cache + SQLite; **estimate 30–80 MB**, not measured | Only what we store ourselves; **estimate 10–20 MB**, not measured |
| CPU | JSON serialization of every event; own threads | Typed TL without JSON; one tokio runtime |
| Startup | DLL loading + database opening | Faster (estimate) |
| Encryption of data on disk | Built in: `database_encryption_key` | Must implement ourselves |
| Maintenance | Update the version to get new features | Implement and maintain every Telegram feature ourselves |
| Build | C++ dependency: prebuilt DLL or build from source (CMake, OpenSSL, zlib, gperf) | `cargo build` |
| License | BSL-1.0 | MIT / Apache-2.0 |

### Decision: TDLib

In Telegram Desktop, the UI and media consume memory, rather than the protocol. A measurement on the
development machine (see [BENCHMARKS.md](BENCHMARKS.md)) shows hundreds of megabytes for a client
based on tdesktop; the protocol engine is a small part of that total. grammers would save
an estimated 20–60 MB, at the cost of implementing everything that makes a client
a client: years of work and a constant risk of diverging from the server. The project's priorities
of “protocol correctness” and “feature completeness” would be unattainable in a reasonable time with this choice.

TDLib covers the functionality immediately and provides built-in database encryption, which
session protection relies on ([SECURITY.md](SECURITY.md)).

**Decision validation (stage 0):** headless TDLib measurement after authentication and synchronization
on a real account. If the engine's private memory exceeds ~100 MB and cannot be reduced
through settings, reconsider the decision in favor of grammers.

**Result (2026-11-15): confirmed.** TDLib 1.8.70 built from source, signed in to a real
account, main chat list loaded, 10 minutes idle: 28.7 MB private bytes on average
(29.6 MB peak), 7 threads, 0.16 % of one core. That is well under the 100 MB threshold and
at the low end of the 30–80 MB estimate. Details and caveats are in
[BENCHMARKS.md](BENCHMARKS.md). The measurement used a direct `tdjson` binding (four
functions) rather than `tdlib-rs`, which also shows the wrapper is optional.

Wrapper: initially `tdlib-rs` 1.4.0 (TDLib 1.8.61, prebuilt DLLs for Windows x64).
The `tdjson` interface consists of four functions, so the wrapper can be replaced
with our own if necessary: with selective JSON parsing and no reliance on someone else's release schedule.
Do this only if profiling shows that JSON is a bottleneck.

## 3. Graphical interface

| Criterion | Slint 1.18 | Iced 0.14 | egui/eframe | GPUI 0.2 | Win32 + Direct2D |
|---|---|---|---|---|---|
| Model | Retained, reactive properties | Elm, reactive rendering since 0.14 | Immediate mode | Retained hybrid | Custom toolkit |
| Idle redraws | Only on changes | Only on changes | Event-driven, but redraws the entire frame | Only on changes | Full control |
| RAM | Third-party measurement: 30 MB with the software renderer (Linux) | No reliable data | Same measurement: 135 MB | No data | Minimal (estimate) |
| List virtualization | `ListView` creates only visible items | None built in | `show_rows` only for equal heights | `list` with variable heights and bottom anchoring | Must implement ourselves |
| Complex text | parley/swash; `StyledText` with Markdown since 1.16–1.17 | cosmic-text, `rich_text` | Custom layout, without full shaping and RTL | DirectWrite | DirectWrite |
| Text selection outside input fields | No | No | Yes | Not included by default | Must implement ourselves |
| Tray | `SystemTrayIcon` since 1.17 | No | No | No | Win32 API |
| Maturity | 1.x, stable API, backed by a company | Pre-1.0 | Pre-1.0, changing API | Pre-1.0; latest release on crates.io — October 2025 | — |
| License | GPLv3 / free Royalty-free / commercial | MIT | MIT / Apache-2.0 | Apache-2.0 | — |

The native interface excludes Electron, Tauri and WebViews.

### Decision: WebView2 for web content (2026-11-19)

To reach Telegram Desktop feature parity, WebView2 is approved only for features that require a
web runtime, including Mini Apps and web-rendered bot content such as HTML widgets.

- Start the runtime only when the user opens content that needs it. Release its views, processes
  and workers after the last view closes or the client unloads to the tray.
- Isolate storage and authorization data per account. Prompt before granting access to sensitive
  capabilities such as the camera, microphone or clipboard.
- Review security during implementation. Measure RAM and CPU across the whole process tree at
  startup, while the runtime is in use and after it closes or unloads to the tray.

See [ROADMAP.md](ROADMAP.md#44-bots--mini-apps).

### Decision: Slint, with a validation prototype

Slint is the only candidate with a stable API, event-driven rendering, built-in
virtualization, a tray and confirmed low memory usage. egui is ruled out because of
text: a messenger needs shaping and RTL. GPUI is the strongest in text and lists,
but its API is unstable, and current code is available only as a git dependency on the Zed monorepo.
A custom Direct2D toolkit would minimize memory, but would be a separate project lasting several months.

Slint's weak point is the message feed: formatted text with links and inline
emoji, text selection, a list with variable heights anchored to the bottom. Therefore, the first
step of stage 2 is a feed prototype with 10 000 synthetic messages, measuring memory, idle CPU
and frame time while scrolling. If Slint cannot handle it, the fallback is
GPUI. Replacement affects only the `ui` crate.

Choose the renderer (software, FemtoVG or Skia) based on the same measurement.

### Prototype results (2026-11-15)

The `spikes/slint-feed` prototype was measured with four renderers; numbers are in
[BENCHMARKS.md](BENCHMARKS.md). The memory and CPU hypothesis is **confirmed**:

- Slint software renderer: 7.2 MB of private memory when idle, 0 ms CPU, 17 % of one core
  during fast scrolling. This is the primary candidate.
- FemtoVG — 87 MB, hardware-accelerated Skia — 277 MB. Hardware renderers in this scenario
  provide no CPU improvement and cost an order of magnitude more memory.
- Variable heights, line wrapping, bottom anchoring, mixed writing directions and
  formatting (bold, italic, strikethrough, code, links with a click handler) work.

**Unresolved issues — functionality, rather than speed:**

| Problem | Details | Options |
|---|---|---|
| Color emoji | Slint's software renderer draws them in monochrome; its source has no color glyph support | Software Skia (54 MB, supports color); extend Slint's software renderer; custom text rendering |
| Emoji like Telegram's | Telegram draws its own set as images, and custom emoji are stickers inside text. Slint does not insert images into a line | Embedded color font for regular emoji; custom text rendering for custom emoji |
| Text selection | Neither `Text` nor `StyledText` has it | Read-only `TextInput` (without formatting); custom text rendering |
| Telegram markup | `StyledText` accepts only Markdown: no spoilers, quotes, code blocks or mentions | Convert Telegram entities to Markdown for simple cases; custom rendering for the rest |
| First letter clipped | The first character is clipped in a narrow bubble with mixed text directions | Find the cause; report to Slint developers if necessary |

Three of the five problems converge on one solution — **a custom message text
component** on top of Slint: layout through parley (already used by Slint itself), rendering
to a buffer, our own emoji images and selection. The rest of the UI stays on standard
Slint components. This is the next subject of research; no decision has been made yet.

### Custom rendering research results (2026-11-15)

The approach **works** and costs almost nothing: 7.7 MB versus 7.2 MB when idle, 8.7 MB
versus 8.3 MB and the same CPU usage while scrolling (numbers are in [BENCHMARKS.md](BENCHMARKS.md)). It uses the same parley and
swash as Slint internally — no new dependencies. The software renderer now provides:
color emoji, inline images, spoilers, code backgrounds, text selection with
extraction of the selected text, and accurate link hit-testing.

The cause of text clipping in narrow messages was found: parley right-aligns a paragraph starting with Arabic or
Hebrew across the full available width, rather than the text width.
Custom rendering fixes this by breaking the lines again at the actual width.

Not implemented in the prototype: selection across multiple messages, copying to the clipboard,
redrawing when the window width changes, display scaling other than 100 %, a real emoji set
(currently the system font), animated emoji, screen reader accessibility —
the image is opaque to screen readers, so text will need to be provided separately. Recommendation: adopt
the custom message text component as the solution; awaiting approval.

## 4. Application structure

```
┌──────────── UI thread ────────────┐       ┌────────── engine thread ─────────┐
│ Slint: windows, lists, input      │◄──────│ blocks in td_receive             │
│ view models for visible items     │ events│ JSON → domain events             │
│ only                              │       │ state: chats, message window     │
│ user commands ────────────────────┼──────►│ td_send                          │
└───────────────────────────────────┘       └──────────────┬───────────────────┘
                                                           │ FFI
        ┌─ on-demand worker threads ──┐          ┌─────────▼───────────┐
        │ image decoding              │          │ TDLib: network,     │
        │ exit when idle              │          │ MTProto, encrypted DB│
        └─────────────────────────────┘          └─────────────────────┘
```

- **No polling.** The engine thread sleeps inside `td_receive` and wakes only on
  an event. The UI redraws only when displayed data changes.
- **Events are coalesced.** A batch of TDLib updates becomes one model update
  per frame, rather than a redraw for each one.
- **No Tokio by default:** TDLib handles networking. If needed, use
  a single-threaded runtime rather than a pool.
- **The `core` boundary.** The UI and engine communicate only through domain commands and events from
  `core`. This isolates the UI thread from heavy work and allows both Slint and TDLib to be replaced.

### Crates

| Crate | Purpose | Depends on |
|---|---|---|
| `app` | Entry point, lifecycle, assembling the parts | all |
| `core` | Domain types, commands and events, state, cache budgets. No FFI or UI | — |
| `telegram` | TDLib wrapper, engine thread | `core` |
| `ui` | Slint UI and view models | `core` |
| `security` | Database key protection: TPM and DPAPI, the key on a locked page; master password planned | — |
| `platform` | Windows: notifications, video and audio (Media Foundation), opening and picking files, clipboard images, startup error box, process hardening; tray planned | — |

`media` and `storage` will appear when there is code for them. With TDLib, separate message
storage is unnecessary — it is inside the library. Measurements live in `scripts/` and `bench/`.

## 5. Where we save resources compared with Telegram Desktop

These are hypotheses. Each is validated by before and after measurements.

**Memory**

1. **Tray mode.** Minimizing to the tray destroys the entire UI: the component
   tree, textures, font and image caches. The engine remains. The official client
   keeps everything.
2. **Strict budgets.** The decoded image cache is bounded in bytes (LRU), rather than
   item count. Only visible content plus a small margin is decoded at a time.
3. **History does not live in memory.** Memory holds a window of messages around the viewport.
   The rest is loaded by page from the TDLib database and unloaded when it leaves that window.
4. **Animations.** Sticker and emoji frames are not cached in advance. Animation runs only for
   visible items in the active window.
5. **Lazy subsystems.** Audio, video and other heavy libraries are loaded on
   first use, rather than at startup.

**CPU**

1. Zero frames when nothing on screen changes. Zero animation timers in an inactive window.
2. Incoming events are processed without the UI when the window is hidden.
3. Heavy work (decoding, parsing large responses) stays off the UI thread.

## 6. Main risks

| Risk | What we do |
|---|---|
| TDLib memory exceeds the estimate and cannot be controlled | Measure at stage 0; reconsideration threshold ~100 MB; fallback — grammers |
| Slint cannot handle the message feed | Prototype as the first step of stage 2; fallback — GPUI |
| Prebuilt `tdjson.dll` is downloaded from a third-party repository | Done: TDLib is built from official source at a pinned commit (`scripts/build-tdlib.ps1`, SHA-256 in BENCHMARKS.md) and shipped next to the executable in the release zip |
| Own `api_id`/`api_hash` required; Telegram API terms are mandatory for third-party clients | Registered at my.telegram.org; the release carries Veygram's own pair, built in (section 8, SECURITY.md) |
| Malware under the same account while the client is running | Full protection is impossible; see [SECURITY.md](SECURITY.md) |
| Media brings native dependencies (rlottie, VP9, Opus) | Introduce them one at a time, each with measurements and lazy loading |
| Calls require WebRTC | Outside the upcoming stages |
| Slint's license constrains the project's license | Resolved: GPL-3.0-or-later matches Slint's GPL option (section 7) |
| Scope: a full client is a very large project | Stages with exit criteria; no feature is declared ready without tests |

## Sources

- [grammers on Codeberg](https://codeberg.org/Lonami/grammers), [grammers-client on docs.rs](https://docs.rs/grammers-client/latest/grammers_client/)
- [tdlib-rs on docs.rs](https://docs.rs/crate/tdlib-rs/latest), [TDLib parameters](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1set_tdlib_parameters.html)
- [Slint 1.18](https://slint.dev/blog/slint-1.18-released), [Slint 1.17](https://slint.dev/blog/slint-1.17-released), [Slint 1.16 changelog](https://cdn.jsdelivr.net/gh/slint-ui/slint@1.16.0/CHANGELOG.md)
- [egui → Slint measurement: 135 MB → 30 MB](https://dev.to/trystan_sarrade/how-i-took-my-rust-gui-from-135-mb-to-30-mb-by-ditching-the-gpu-2i7d)
- [gpui](https://crates.io/crates/gpui), [iced](https://crates.io/crates/iced) on crates.io

## 7. Decisions of 2026-11-16

- **Message text: custom element — accepted.** The requirement is that messages look and
  behave as in Telegram, which the standard Slint text elements cannot do.
- **Emoji: Apple set**, as in the official client. The artwork is Apple's and may not be
  redistributed, so it is not committed to this repository: the client loads the set from
  a local directory. How a public release obtains it is an open question; until it is
  answered a public build needs a freely licensed fallback set.
- **Sign-in must offer QR code.** Login codes were not delivered for the new `api_id`.
- **OLED theme.** A true-black theme is required: background `#000000` so unused pixels
  stay off. Tried in the prototype (`--theme oled`): incoming bubbles stay black with a
  hairline outline, outgoing ones get a very dark accent tint; text and accent colours are
  unchanged. Still to design: chat list, panels, hover and selection states, and avoiding
  large static bright areas that could burn in.
- **License: GPL-3.0-or-later.** Free software, the same license family as Telegram
  Desktop, and it matches the GPL option of Slint, so no separate Slint license is needed.

## 8. Decisions of 2026-11-17 and 2026-11-18

- **Media through the Windows Media Foundation Media Engine**, no bundled codecs. The media viewer
  presents into a native child window; voice messages use the audio-only mode; round videos use
  frame-server mode, and the interface draws their frames clipped to the circle. A child window
  with an elliptic region dropped out of the composed screen, so rounds have no native window.
- **Video streaming** reads TDLib's partial file through a read-only `IMFByteStream`, with a
  bounded download window (ROADMAP.md, BENCHMARKS.md).
- **Interface scale** as Telegram Desktop's (50-300 %, 5 % steps), applied by restarting, because
  Slint reads its scale factor once at startup.
- **Name: Veygram** (until 2026-11-18 Ferrogram, before that tg-rust). Storage identifiers keep
  `tg-rust`, so existing sessions and keys stay valid.
- **Public release: a portable zip** (`scripts/package-release.ps1`): the executable, TDLib with
  OpenSSL and zlib, the Visual C++ runtime and license texts in one folder. The executable loads
  `tdjson.dll` from its own folder, otherwise from the TDLib installation it was built against.
  Veygram's own `api_id`/`api_hash` are built into release executables only; development builds
  read `.env`. Release v0.1.0 (2026-11-19) was published as an installer,
  `Veygram-Setup-<version>-x64.exe`, built outside this repository; a reproducible installer
  build, an uninstaller, code signing and automatic updates are later steps.

## 9. Decisions of 2026-11-20

- **Sending photos and documents** (`Command::SendPhoto`, `Command::SendDocument`) passes only a
  local path and an optional plain caption (at most 1024 characters). `tg-telegram` maps them to
  `sendMessage` with `inputMessagePhoto` or `inputMessageDocument` and `inputFileLocal`, so TDLib
  streams the file from disk; no file bytes cross the `tg-core` boundary.
- **The open-file dialog and the clipboard bitmap** are Win32 calls in `tg-platform`
  (`pick_file`, `clipboard_has_image`, `read_clipboard_image`). `tg-app` injects them into `tg-ui`
  as `AttachmentSources`, so `tg-ui` has no `unsafe` code. Both run on a short-lived worker
  started by the attach button or Ctrl+V; nothing runs while idle. Ctrl+V attaches only a
  bitmap without text (copied spreadsheet cells carry both and paste as text), and falls
  through to the normal text paste whenever an attachment cannot start.
- **Previews are bounded**: PNG/JPEG only, at most 32 MiB encoded and 8 megapixels, scaled to
  320x200 before reaching the UI. Larger or other images are still sent unchanged, as files.
- **Pasted images** are decoded while the clipboard memory is lent out (no raw copy), written as a
  PNG into the user's temporary folder and deleted once TDLib reports the send as finished
  (`Event::LocalFileSendFinished`), on cancel, on logout, or at exit after the engine has closed.
  At most eight uploads are pending at a time.

## 10. Decisions of 2026-11-22

- **Reactions** (`tg_core::Reaction { emoji, count, chosen }`, `Message::reactions`,
  `Command::ToggleReaction`, `Event::MessageReactionsChanged`). Only emoji reactions are mapped;
  custom-emoji and paid reactions are skipped. At most 32 per message, each emoji at most 64 bytes.
  Clients receive reactions inside `updateMessageInteractionInfo`; `updateMessageReactions` is the
  bot-only variant and maps to the same event.
- **Toggling reads TDLib's state, not the UI's.** The engine keeps no message copy, so
  `ToggleReaction` first sends `getMessage` and, from that answer, `removeMessageReaction` when the
  user has chosen the reaction, otherwise `addMessageReaction`. An existing reaction is sent in
  Telegram's spelling (the heart has no variation selector). TDLib applies the change locally at once,
  drops older choices beyond the account's limit and sends `updateMessageInteractionInfo`.
  Toggles of one message run one at a time: later clicks wait (at most 8 per message, 32 messages)
  until the previous mutation is answered, so each reads the state the previous one left; an error
  releases the next waiting toggle.
- **Chips are laid out in Rust and drawn by Slint**, inside the bubble below the text or media; the
  time joins the last chip row when it fits, as in Telegram Desktop. Their emoji are coloured rasters
  from the message text engine (a few KiB each), counted in the 96 MiB viewport budget and freed with
  their row. Interaction updates that leave the reaction list unchanged (view counts) cause no relayout.
