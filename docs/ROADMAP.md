# Implementation plan

A stage is complete only when its exit criterion is met. A feature is not declared
supported until it has been implemented and tested.

## Stage 0. Environment and hypothesis validation

- [x] Install Rust, verify that the skeleton builds — 2026-11-15: Rust 1.100.0, MSVC Build Tools 2026;
      `fmt`, `clippy -D warnings`, `test`, `doc` and the release build pass without warnings.
- [x] Obtain `api_id` / `api_hash` at my.telegram.org — 2026-11-15, stored in `.env` (outside git).
- [ ] Baseline measurement of the official Telegram Desktop on this machine following [BENCHMARKS.md](BENCHMARKS.md).
      Partly done 2026-11-16: 5 minutes idle, 656 MB private bytes, 124 threads, 0.88 % of one core;
      uncontrolled state, the scripted scenarios are still to be run.
- [x] Build TDLib from official source — 2026-11-15, `scripts/build-tdlib.ps1`: TDLib 1.8.70,
      commit `c8828d5` (rebuilt at `5aef7d4` on 2026-11-16), 34 minutes, `tdjson.dll` 28.7 MB (details in [BENCHMARKS.md](BENCHMARKS.md)).
- [x] Headless TDLib measurement: authentication, synchronization, 10 minutes of idle time —
      2026-11-15, `spikes/tdlib-probe`: 28.7 MB private bytes, 0.16 % of one core. TDLib confirmed.
- [x] Slint feed prototype: 10 000 synthetic messages, memory, CPU and scrolling measurements —
      2026-11-15, `spikes/slint-feed`. The memory and CPU hypothesis is confirmed; questions remain
      about color emoji and text selection (see [ARCHITECTURE.md](ARCHITECTURE.md), section 3).
- [x] Resolve the color emoji question: renderer or custom rendering — 2026-11-15: custom message
      text rendering with parley and swash (ARCHITECTURE.md, section 3).
- [x] Extend the prototype: load earlier history without jumps, images in messages — done in the
      product instead (2026-11-16). A 4K window has not been measured.

**Exit:** the choice of TDLib and Slint is confirmed by measurements or reconsidered.

## Stage 1. Research — completed 2026-11-15

Result: [ARCHITECTURE.md](ARCHITECTURE.md), [SECURITY.md](SECURITY.md).

## Stage 2. Minimal working client

- [x] Engine thread: start TDLib, `td_receive` loop, domain events.
- [x] Database key protection: TPM + DPAPI (after approval of [SECURITY.md](SECURITY.md)).
- [x] Authentication: phone number, code, 2FA password, and QR code. Phone sign-in confirmed on a
      live account 2026-11-17.
- [x] Virtualized chat list.
- [x] Message viewing: history window, loading by page.
- [x] Send text.
- [x] Network and authentication errors are shown to the user rather than lost.

Done 2026-11-16, used daily on a live account since then.

**Exit:** users can sign in, read and write text; all checks (`fmt`, `clippy`, `test`) pass.

## Stage 3. Performance

- [ ] Run all scenarios in [BENCHMARKS.md](BENCHMARKS.md) against the official client.
- [ ] Tray mode that unloads the UI.
- [ ] Cache budgets, event coalescing.
- [ ] Startup time: cold and warm.
- [ ] Memory growth test: 4 hours of operation.

**Exit:** every metric is better than the official client's, and results are recorded.

## Stage 4. Features

The target is feature parity with the stable Windows Telegram Desktop, with the same RAM and CPU
discipline as stages 0–3. Work is grouped below, roughly by frequency of use; the heavy subsystems
come after the everyday messaging flows. Completed foundations are recorded separately from the
remaining work. An unchecked item can include an existing prototype that still needs integration
or live-account verification.

Parity is checked against the [official Desktop changelog](https://telegramdesktop.github.io/tdesktop/changelog/)
and the corresponding stable release, initially 7.4.4 (2026-11-13). Record the compared version,
account tier and server availability when testing; refresh the checklist before a release. A
phone-only feature is not a Desktop parity requirement. A missing TDLib capability stays an
explicit gap, with an upgrade or integration task; it is not silently omitted.

**Rules for every subsection**

- Each feature comes with before and after measurements of memory and CPU usage, including
  first use, active use, closing it and returning to idle or the tray. Follow [BENCHMARKS.md](BENCHMARKS.md).
- No polling or busy-waiting. Coalesce incoming updates; redraw only changed visible content.
  Animation stops outside the viewport or in an inactive window; recording and playback run only
  while explicitly active. Heavy work stays off the UI thread.
- History, search results and member lists are paged and virtualized. Caches are bounded in bytes,
  queues and concurrent work are bounded, and closing a view cancels work that is no longer needed.
- Audio, video, calls and browser components start on demand and release their resources when
  finished. Count helper processes and GPU allocations as part of Veygram's cost.
- Protocol correctness, security, accessibility and usable interactions are part of completion.
  Lower usage obtained by dropping functionality does not meet the goal.

### 4.1. Media & Creation

Photos and files can be downloaded and viewed (2026-11-16/17); text sending works. Video streaming,
voice playback and round video playback are present; recording and most media sending remain.
Since 2026-11-20 a single photo or document can be sent from the file picker (paperclip) or pasted
from the clipboard (Ctrl+V), with a preview, an optional caption, "send as file" for photos and
cancellation before sending. TDLib reads the file from its local path; Veygram never loads it.

- [ ] Send photos, videos, documents and albums from the file picker, drag-and-drop and clipboard;
      preview, reorder, remove items and edit captions before sending. Preserve album grouping
      across page boundaries and verify receiving albums on a live account.
      Done: one photo or document from the picker or a pasted clipboard image, with preview,
      plain-text caption and cancel. Remaining: videos, albums, multiple files, drag-and-drop,
      reordering and formatted captions.
- [ ] Compressed photos and videos, HD where available, and uncompressed originals sent as files;
      respect server limits and avoid re-encoding an unchanged original.
- [ ] Image and video editing: crop, rotate, draw, add text or stickers, trim, mute, choose quality
      and cover; GIF creation and editing. Process large inputs incrementally with bounded buffers.
- [ ] Upload queue with progress, cancellation, retry, failed-send recovery and per-file captions;
      folders/file sets as archives, without reading whole files into RAM.
- [ ] Voice note recording: microphone choice, level/waveform, hold or lock, pause, preview, trim,
      discard and send. Video note recording: camera choice, preview and the same send/cancel flow.
      Release capture devices and encoding workers on completion, cancellation or failure.
- [ ] Scheduled and repeating messages, send when online where supported, Saved Messages reminders,
      scheduled-message list with edit/reschedule/delete/send now, and silent messages.
- [ ] Media and caption spoilers, self-destructing and view-once media where Desktop supports them;
      enforce viewing, saving and forwarding restrictions in previews, notifications and caches.
- [ ] Contact cards, locations, live-location viewing and other Desktop-supported attachments;
      unsupported content must explain itself rather than disappear.

**Exit:** all send paths, cancellation and reconnect recovery work on a live account; large files
and repeated recordings stay within measured memory and CPU budgets.

### 4.2. Chat Interactivity

Replies, reply quotes, forwarding, editing and deletion were added 2026-11-16.

- [ ] Polls and quizzes: create, vote, retract where allowed, show results/voters, stop a poll,
      anonymous or public voting, multiple answers and quiz explanations; current Desktop poll
      options, media and permissions, including clear read-only states.
- [ ] Reactions: picker, quick reaction, counts and reactors, custom/Premium and paid reactions
      where available; respect chat limits and animate only visible reactions.
      Done 2026-11-22: emoji reaction chips under the bubble (count, chosen state), click to
      toggle, a quick strip of six reactions above the message menu, live updates from
      `updateMessageInteractionInfo`. Still open: the full picker from the chat's available
      reactions, reactor lists, custom-emoji and paid reactions, reaction animations.
- [ ] Pin/unpin messages, notification choice, pinned-message bar and paged pinned-message list;
      navigate among pins in chats, topics and Saved Messages.
- [ ] Server drafts: text, formatting and reply target, synchronized between devices and restored
      per chat/topic. Coalesce edits before saving; recover correctly after reconnect or conflict.
- [ ] Multi-select messages and albums for copy, forward, delete and save, with keyboard/range
      selection and permission-aware actions; copy selected text across messages.
- [ ] Message links, jump to replies/originals, reply navigation history, read receipts and unread
      mentions/reactions; typing and recording status without permanent UI timers.
- [ ] Collaborative checklists, message translation and transcription where available; preserve
      entitlement checks, errors and the original content.
- [ ] Auto-delete timers, protected content and paid-message confirmation where required; joining,
      muting or restricted-chat bars replace an unusable composer.

**Exit:** actions and their server updates stay consistent across devices, including denied rights,
deleted messages and reconnects; message selection does not retain evicted history pages.

### 4.3. Channels, Groups & Moderation

- [ ] User, group and channel profiles: avatars, description, online/member/subscriber counts,
      members, shared media and links; sender avatars, admin titles, member tags and verified marks.
- [ ] Create and edit groups/channels; join/leave, invite links and QR codes, join requests,
      permissions, ownership transfer, public usernames and linked discussion groups.
- [ ] Topics/forums: create, edit, close, reopen, delete, pin and search topics; per-topic history,
      drafts, unread state and notifications, including General and bot topics where supported.
- [ ] Channel comments and discussion threads: reply counts, open the linked thread, reply,
      moderate and return to the originating post without losing the scroll anchor.
- [ ] Admin tools: promote/demote, granular rights, ban/restrict/unban, remove messages, report spam,
      slow mode, anti-spam settings, recent actions/admin log and member search.
- [ ] Channel publishing: signatures and send-as identity, scheduled/suggested posts, direct
      messages to channels, statistics, boosts, giveaways and monetization where available.
- [ ] Communities, welcome messages and ephemeral group messages as supported by the reference
      Desktop release. Check permissions and lifecycle semantics before calling them complete.

**Exit:** member and admin flows work with ordinary and administrator accounts; large forums,
member lists, logs and statistics load by page and do not grow resident history without limit.

### 4.4. Bots & Mini Apps

- [ ] Inline keyboards and callbacks, reply keyboards, force reply, keyboard removal, request
      contact/location/chat actions and button URL confirmation.
- [ ] Bot command suggestions, command/menu button, start parameters and deep links; inline
      queries with paged results, cancellation, switch-to-chat and sending the selected result.
- [ ] Bot payments/invoices, Telegram login/URL authorization, attachment-menu bots and relevant
      permission prompts; show failures and account/paid-feature restrictions clearly.
- [ ] Open Mini Apps in WebView2 on demand. Support Telegram's bridge, theme/viewport events,
      navigation, downloads and camera/microphone/clipboard permission prompts. Keep storage and
      authorization data isolated per account.
- [ ] Release WebView2 views, processes and workers after the last view closes or the client unloads
      to the tray. Measure RAM and CPU across the whole process tree and test recovery from a missing
      or crashed runtime.
- [ ] Support current Desktop bot content, including rich messages. HTML widgets and other bot
      content that require web rendering use the same WebView2 runtime and follow the same
      isolation, permission and lifetime rules as Mini Apps.
- [ ] Business/automated-account tools: quick replies, greeting/away messages, business links,
      hours/location and connected bots, where supported by Desktop and the account.

WebView2 was approved on 2026-11-19 as a narrowly scoped exception for Mini Apps and web-rendered
bot content. Review security and measure resource use during implementation.
See the [architecture decision](ARCHITECTURE.md#decision-webview2-for-web-content-2026-11-19).

**Exit:** bot interactions and Mini App lifecycle and permission tests pass. Startup adds no
WebView2 process. Closing the last web view or unloading to the tray releases the runtime and
returns the client to its measured idle budget.

### 4.5. Media Viewer & Player

Media Foundation playback and bounded partial-file video streaming are already present; details
and the remaining round-video memory cost are in the dated progress notes below.

- [ ] Fullscreen lightbox: previous/next, thumbnails, keyboard navigation, pan/zoom, rotation,
      save/copy/open externally and jump to the source message. Decode the visible image at the
      needed resolution with a small bounded neighbour prefetch; do not retain the whole album.
- [ ] Video controls: play/pause, seeking and buffering ranges, volume/mute, playback speed,
      fullscreen, picture-in-picture (PiP), quality selection and timestamp links.
- [ ] Inline GIF/animation and muted short-video playback, round-video seeking/streaming and
      floating playback; autoplay controls and no invisible decoding when inactive.
- [ ] Audio/music player: metadata, playlist/queue, previous/next, repeat/shuffle, seek, volume,
      speed and resume position; voice waveform, unread/listened state and continuous playback.
- [ ] Windows system media controls and media keys; coordinate playback so opening another
      player or a call does not leave competing audio or orphaned decoding workers.
- [ ] Shared media tabs for photos/videos, files, links, music, voice, GIFs and applicable profile
      content, with paged search, selection, download and source-message navigation.
- [ ] Instant View and supported document previews, including text recognition where available;
      perform parsing/recognition on demand and keep decoded pages and results bounded.

**Exit:** playback, seek, PiP and close work during partial downloads and failures; repeated open/
close cycles release player resources, and scrolling shared media reaches a memory plateau.

### 4.6. Stickers, Custom Emoji & Rich Text

Static WebP stickers work (2026-11-17); animated TGS/WebM currently have still-frame fallbacks.

- [ ] TGS (Lottie) and WebM sticker animation, animated emoji and message effects; decode frames
      on demand, with bounded frame buffers and a shared scheduler for visible animations.
- [ ] Emoji/sticker/GIF panel: installed packs, favourites, recent items, search, saved GIFs,
      pack previews, install/remove/reorder and supported sticker/custom-emoji creation tools.
- [ ] Inline custom emoji in text, captions, names and status, preserving shaping, RTL, selection
      and accessibility. Finish the optional local emoji set and freely licensed public fallback.
- [ ] Formatting toolbar, context menu and hotkeys: bold, italic, underline, strike, monospace,
      code blocks, quotes/expandable quotes, spoilers, links and mentions; preserve entities on
      copy/paste, edit and forward, with undo/redo and IME support.
- [ ] Rich-message editor and rendering for tables, lists, tasks, formulas, media/file blocks and
      buttons where Desktop supports them; optional server-assisted editing must be explicit.
- [ ] Clickable URLs, mentions, hashtags, `t.me`/`tg://` links and configurable link previews:
      choose/remove previews, size and position; native in-app navigation with safe external links.

**Exit:** message entities round-trip correctly; text stays selectable and accessible, and packs
and animated content have measured byte budgets with zero inactive animation work.

### 4.7. Navigation & Organization

- [ ] Main menu: account header, My Profile, Saved Messages, contacts, calls, settings, night mode,
      create group/channel and other applicable Desktop entries, each opening a working screen.
- [ ] Chat folders: create/edit/delete/reorder, include/exclude rules, counters, shared folder
      links and limits; archive/unarchive, archive visibility, pin/unpin and mark read/unread.
- [ ] Global search for chats, contacts, bots/apps and public posts; in-chat/topic search by text,
      sender, date and media type, result navigation and recent searches. Cancel stale queries
      and page results rather than building a complete local search index in RAM.
- [ ] Contacts: add/edit/delete, share cards, blocked users, birthdays and notes where supported;
      Saved Messages organization, tags and saved music.
- [ ] Chat list details: own-message prefix/read checks, pins, mute/type/verified marks and drafts;
      keyboard chat switching, back stack, unread navigation and restored chat/scroll state.
- [ ] Stories: view, publish/edit where Desktop supports it, reply/react, privacy, archive,
      profile albums and live stories; reuse bounded media pipelines and stop hidden playback.
- [ ] Premium, Stars and gifts: entitlement-aware screens, balances/history, gifting, collectible
      gifts/collections/status and supported purchases, auctions and paid content. Track newer
      Desktop financial surfaces separately with a security review and explicit confirmations.
- [ ] Account and chat data export (HTML/JSON and selected media), with cancellation and streamed
      output; do not materialize the full history in memory or bypass protected-content rules.

**Exit:** navigation reaches every supported screen and returns to the same context; searching,
exporting and browsing large accounts keep bounded state and do not block the UI.

### 4.8. Multi-Account

- [ ] Add, sign in, remove and quickly switch between 3–4 accounts, following the server's
      ordinary/Premium limits; QR/phone/2FA flows, logout and expired-session recovery per account.
- [ ] Separate TDLib sessions, encrypted databases, keys, drafts, download paths, settings and
      Mini App storage; account identity is explicit on notifications, send/payment actions and calls.
- [ ] Per-account unread badges, notifications and background updates; notification activation
      switches to the correct account/chat, and pending sends stay with their originating account.
- [ ] Reuse one visible UI, evict inactive view models and divide a global byte budget across
      accounts rather than multiplying media caches. Keep background engines event-driven and
      measure their unavoidable per-account cost; no dropped updates to make switching cheaper.

**Exit:** 3–4 account switching is fast in measured cold/warm scenarios, no state crosses accounts,
and total foreground/tray costs are compared with the same accounts in Telegram Desktop.

### 4.9. Calls

Calls remain a later stage of feature work, but are required for parity. TDLib alone does not
provide the media stack; validate a compatible tgcalls/WebRTC integration before choosing it.

- [ ] One-to-one voice/video: incoming/outgoing call UI, ringing notifications, answer/reject/end,
      mute, camera toggle, device selection, volume, reconnect and encryption verification.
- [ ] Screen/window sharing, with system audio where supported, selection preview and clear stop
      controls; handle device loss and camera/microphone permissions.
- [ ] Group calls, group voice/video chats and channel live streams: join/create/schedule, invite
      links, participants, speaker/listener roles, raise hand, moderation, recording and reactions/
      comments where supported, with paged participants and bounded visible video streams.
- [ ] Load the stack only on an incoming or outgoing call event, bound jitter/frame buffers and
      encoding workers, adapt to bandwidth, and release devices, workers and library resources
      when the call ends. A call or deliberate screen share may continue in the tray; its cost
      must be measured separately from an idle tray client.

**Exit:** interoperability with official clients, reconnect and device/lifecycle tests pass; idle
startup has no call workers, and call CPU, memory and post-call cleanup are measured.

### 4.10. Settings & Customization

- [ ] Profile editor: name, bio, username, phone, photo/video avatar, birthday and emoji status;
      account deletion/logout flows and profile visibility consistent with server permissions.
- [ ] Privacy rules and exceptions: phone, last seen/online, photos, forwards, calls, invitations,
      messages, stories and blocked users; auto-delete and account inactivity settings.
- [ ] Two-step verification (2FA): enable/change/remove password, recovery email and recovery;
      active sessions and web authorizations, device details and individual/all-other revocation.
      Passkey sign-in and management where supported; fix QR-to-phone sign-in switching.
- [ ] Master password, Windows Hello research and automatic UI/full locking, following
      [SECURITY.md](SECURITY.md) and the security work below; do not weaken TPM + DPAPI protection.
- [ ] Notifications: global/per-chat/per-account mute, previews, sounds, exceptions, quiet periods
      and locked-screen privacy; respect Windows notification and Focus settings.
- [ ] Data/storage: auto-download and autoplay rules, download destination, network usage, byte
      limits and retention for disk caches, per-chat/media cleanup, progress and cancellation.
      Keep disk and decoded-RAM budgets distinct; cleanup must preserve sessions and unsent work.
- [ ] Connection: SOCKS5 (including authentication) and MTProto proxies, system proxy and other
      Desktop-supported proxy modes, import/share, connectivity status and switching/rotation;
      respect call routing support and never put proxy credentials in logs.
- [ ] Light/night/OLED and custom themes, theme import/export/editor, wallpaper/chat appearance,
      accent, font size, existing interface scaling, language, spellcheck, send-key choice and
      shortcuts; reduced motion and high contrast without permanent animation timers.
- [ ] Settings search, persistence and migration, with safe reset flows; document downloaded-file
      cache protection and cleanup-on-lock behaviour before implementing it.

Secret chats were requested earlier, but are beyond Telegram Desktop parity. Keep them as an
explicit later extension, gated on TDLib interoperability and the security model, without blocking
the Desktop comparison or weakening protection of existing sessions.

**Exit:** settings survive restart and account switching, server rules match another client, and
cache/lock/session operations have recovery tests without data loss.

### 4.11. Windows Platform & Performance

Notifications and unread counters were added 2026-11-16; deep tray unloading is still open.

- [ ] Tray lifecycle: close/minimize preferences, menu, badges and restore; destroy UI components,
      textures, fonts, decoded media and any inactive Mini Apps. Keep notifications and server updates
      in the engine, and rebuild only the current viewport on restore. Active audio/calls have
      explicit lifetime rules; an idle tray must not retain their workers.
- [ ] Taskbar unread badge, download/upload progress and call attention; notification quick reply
      and deep-link activation, single-instance routing and launch-to-tray.
- [ ] Opt-in auto-start, sleep/resume, network changes and session shutdown; restore drafts and
      context, handle multiple monitors, mixed DPI, window placement and Windows 10/11 behaviour.
- [ ] Installer/uninstaller and portable updates, code signing and automatic updates with verified
      manifests/packages, atomic replacement and rollback; configurable update checks and no
      permanently running updater. Verify packaged TDLib hashes and runtime dependencies.
- [ ] Accessibility (a11y): keyboard-only operation, focus order, screen-reader/UI Automation text
      for custom-rendered messages, names/roles/actions for controls, high contrast, reduced motion,
      IME, text selection and copy at all supported scales.
- [ ] Automated benchmarks against Telegram Desktop: all [BENCHMARKS.md](BENCHMARKS.md) scenarios,
      plus media upload/playback, animated stickers, search, Mini Apps, web-rendered bot content,
      calls, 3–4 accounts and repeated tray/restore cycles. Use identical data/settings/hardware
      and record client versions, build commits, cold/warm state and raw samples.
- [ ] Track private bytes/working set, CPU per core, threads/handles, GPU and helper-process memory,
      startup/restore latency and scrolling frame times (including 4K); test four-hour memory
      growth and cancellation/close loops. Set measured regression budgets and automate their
      enforcement; do not invent targets or claim savings from an incomplete feature set.

**Exit:** everyday Windows integration and accessibility are verified, tray memory returns to its
measured engine-only budget, and equivalent feature scenarios retain the memory and CPU advantage
over Telegram Desktop. Regressions block completion until measured and resolved.

**Stage exit:** the parity checklist is verified against the recorded stable Desktop version on
live accounts and all subsection exits are met. Document platform, server and account-tier
restrictions. The stage 3 performance suite must still pass with the full feature set enabled,
using equivalent workloads.

## Stage 5. Systematic optimization

Profiling (WPR/WPA, ETW, allocation counters), finding bottlenecks, fixing them with
before and after measurements. Build TDLib from source at a pinned commit. Optimization continues
through stage 4. This final pass verifies the release build with the full feature set enabled.

- [ ] Resolve measured bottlenecks, including the round-video frame-copy cost, 4K scrolling and
      subsystem unload behaviour; repeat equivalent Desktop benchmarks after each material change.
- [ ] Complete dependency audits, packaging/hash checks, clean-machine installation/update tests,
      session-key recovery/copy tests and the release security work recorded below.
- [ ] Recheck the stable Desktop changelog and close newly found parity gaps. Publish supported
      features, known restrictions and reproducible benchmarks with the release.

**Exit:** the release build meets the functional and resource gates, release/security checks
pass, and the published claims match the tested build and recorded measurements.

## User requirement (2026-11-16): seamless history

No "Earlier messages", "Latest messages" or "Load more chats" buttons. History and the chat list
load when the user scrolls near the edge and old pages are dropped from memory when they are far
from the viewport, without visible jumps, flicker or loading pauses. The scroll position must
stay on the same message while pages are added above or removed. Check memory with a scripted
scroll through a long history: it must plateau, not grow.

## Known gap (2026-11-16): albums

A post with several photos (a media album, messages sharing one `media_album_id`) is shown as one
message with the first photo and the caption, followed by the other photos as separate messages
without text. Telegram shows them as one bubble with a photo grid and one caption. To do: group
consecutive messages with the same non-zero `media_album_id` into one feed row, lay the photos
out as a grid like Telegram, keep the caption once, and keep paging and the 300-message window
correct when an album is split across a page boundary.

Done on 2026-11-16: one row
per album of up to 10 items, a Telegram-style grid with centre-cropped cells, one caption, and
an anchor that survives an album split across pages. Not yet checked on a live account.

## User requirement (2026-11-16): full feature set later

The user confirmed the albums and the new look in the demo and asked to record these as future
work, so the client eventually covers what Telegram Desktop offers:

- Video with a built-in player: inline preview in the feed, playback with controls, and a
  full-screen viewer, plus GIF/animations and round video messages. Decoding off the UI thread,
  memory bounded like the image cache; the decoder and its cost must be measured before it is
  chosen.
- Settings: account and profile, privacy and security, notifications, data and storage
  (cache limits), appearance (themes, font size), language, active sessions, folders.
- Gifts (Telegram gifts and Stars), Premium features, reactions, stickers and emoji packs.
- "And all that kind of thing": everything else Telegram Desktop has (polls, voice and video
  calls, channels and comments, bots and inline buttons, forwarding, search, pinned messages,
  scheduled messages), each with the same RAM and CPU discipline.

## Progress (2026-11-16): messages and video

- Reply, edit, delete and forward from a context menu, with reply quotes, "edited" marks and
  forwarded headers.
- Video playback through the Media Foundation Media Engine (GPU decoding, audio, a native child
  window), opened from a preview with a play button and the duration. Next for video: play while
  downloading, GIFs and round video messages, inline autoplay of short muted clips.

## Progress (2026-11-17): media messages

- Voice messages: waveform, seeking and playback through the Media Engine in audio-only mode
  (the system Opus decoder handles OGG/Opus voice notes; no extra library). One voice or round
  message plays at a time, and playback marks it listened.
- Round video messages play inside their circle in the feed and keep playing in a floating
  circle when their row scrolls away, as in Telegram Desktop. A native child window with an
  elliptic region dropped out of the composed screen (the window behind Veygram showed through
  the circle), so rounds use the Media Engine in frame-server mode instead: each frame is
  transferred into a texture of the round's size, copied to the CPU and drawn by Slint. The
  preview stays until the first frame, and the playing round grows from 240 to 320 px. Measured
  on the release demo round (10 samples, one synthetic clip): Private Bytes 123 MiB inline and
  floating, about 20 MiB more than with the child window; reducing that is open.
- Interface scale, as Telegram Desktop's: the default (system) scale or 50-300 % in 5 % steps,
  stored in the data directory and applied by restarting Veygram.
- Files: download with progress and cancel, then open with the default application. Opened files
  get the internet-zone mark (SmartScreen, Protected View), and programs and scripts need a
  confirmation first. Downloaded files can be shown in their folder.
- Static WebP stickers and single animated emoji are drawn without a bubble. Animated (TGS/WebM)
  stickers show a still frame or the emoji until animation lands.
- Service messages (joins, added and removed members, pins, title and photo changes, timers,
  topics, video chats, gifts) read as in Telegram Desktop and are drawn as centred pills, also
  in the chat list preview.
- Release builds no longer open a console window; startup errors show a message box.
- Sign-in asks for the phone number first and requests a QR code only on demand. TDLib rejects
  a phone number once a QR code was requested, so switching back from the QR screen needs a
  logOut and an engine restart; that is the next sign-in task.
- The interface tokens (colours, type sizes, radii) live in the `Theme` global and are recorded
  in `.interface-design/system.md`.
- Video streaming, as in Telegram Desktop's `Media::Streaming`: a video Telegram marks as
  streamable starts while it downloads. Media Foundation reads TDLib's partial file at real
  offsets through a read-only `IMFByteStream`; a read of a missing range waits on a condition
  variable and asks TDLib to download from that offset. Each request is a window of about
  1 MiB (`downloadFile` with a `limit`) that slides ahead until 32 s of playback are loaded,
  so at most about 1 MiB is ever queued on the server and a seek or a read of the index at the
  end of the file is served at once (Telegram Desktop keeps four 128 KiB requests in flight
  for the same reason). Seeking pauses until the target range arrives, the viewer shows a
  buffering spinner and the loaded range on the seek bar (remembered across windows), and
  closing the viewer cancels the download. Round videos still download completely first.
  Checked on a live account: the first frame of a 11.5 MB clip with its index at the end
  arrives after 0.84 s instead of 3.1 s.

Next: reactions, pinning, message links and selection; switching from QR back to phone sign-in.

## Progress (2026-11-19): clickable message text

- URLs, text URLs and mentions use hit rectangles from the rendered Parley layout, including
  wrapped and RTL text and album captions. Only the text itself shows the pointer cursor;
  dragging, hidden spoilers and blank space do not activate links.
- Explicit HTTP, HTTPS, Telegram and mailto protocols are preserved; bare web addresses use
  HTTPS and mentions open `https://t.me/<username>`. The existing bounded shell worker opens
  targets with `ShellExecuteW`, off the UI thread, and validates targets again before opening.
  Unsupported schemes and local paths are rejected. In-client Telegram navigation is still planned.
- Text links whose labels hide their destination show the normalized URL and require confirmation.
- Link maps belong only to visible rows, are bounded to 256 KiB per row and count toward the
  existing 96 MiB viewport budget. Hover is event-driven; no polling or periodic timers were added.

## Progress (2026-11-18): name and first public release

- The client is now called Veygram (it was Ferrogram, one letter away from ferogram, an existing
  Rust MTProto framework). The repository moved to github.com/tikD1337/veygram; old URLs redirect.
  Storage identifiers keep tg-rust, so sessions and keys survive the rename.
- A portable release zip for GitHub Releases (`scripts/package-release.ps1`): the executable,
  TDLib with OpenSSL and zlib, the Visual C++ runtime and the license texts. The executable loads
  `tdjson.dll` from its own folder and carries Veygram's own API credentials, built in at release
  time only. Checked from a clean folder with no `.env`: TDLib loads and the server issues a QR
  sign-in code. Not checked yet on a Windows installation without the Visual C++ runtime.
- `main` requires the lint, unit test and release build checks before a pull request merges.


## Security work (2026-11-17)

A review of [SECURITY.md](SECURITY.md) against the code. Implemented today: the database key is
wrapped by a non-exportable TPM key and DPAPI (threat T1, copied files), and our copy of the key is
zeroed after it is handed to TDLib. The TPM key is not bound to boot state (PCRs), a PIN or a user
gesture, so any process running as the same Windows user can ask the TPM to unwrap it. Planned,
in order of value for cost:

1. **Process hardening (T3, client running).** Done 2026-11-17: process DACL, System32-only DLL
   search (as Telegram Desktop) and the image load policy. Signed-only images would refuse TDLib;
   the no-dynamic-code and extension-point policies are untried (known driver and input-method
   conflicts) and remain a later, measured step. Details in SECURITY.md.
2. **Lock the key page in memory** with `VirtualLock`. Done 2026-11-17.
3. **Recovery when the key cannot be unlocked.** Done 2026-11-17: a screen with Try again and
   Start fresh; Start fresh renames the data directory to a dated backup and starts with a new
   key. Tested on the real key file that stopped unlocking on 2026-11-17. The cause of that
   failure (TPM decrypt 0x80280084 on an unchanged key file) is still unknown.
4. **Master password (T2, client closed).** Optional, off by default (decided 2026-11-16). Argon2id
   from an audited RustCrypto implementation, wrapping added as the third layer; asked at start.
   It is the only measure that stops malware running as the same user from unwrapping the key.
5. **Windows Hello as an alternative to the password (T2).** Research first: the Windows Hello key
   API signs and does not decrypt. Evaluate deriving a key from a signature of a fixed challenge, or
   a TPM key that requires user presence, against published attacks before choosing.
6. **Automatic locking.** An idle timer that removes keys from memory: interface-only lock by
   default (notifications keep working), full lock (TDLib shut down) as a setting.
7. **Copy test.** A repeatable test that a copied data folder does not open in another Windows
   profile or on another computer (T1), and that a wrong key never opens the database.

Out of scope, as decided in SECURITY.md: a Chrome-style SYSTEM service for application binding,
and anti-debugging or obfuscation. Malware with administrator or kernel rights (T4) cannot be
stopped by any user-mode client.

## Cross-document follow-ups (recorded 2026-11-17)

Promises and open items found in the other documents, now tracked by stage 4 and stage 5 above.
The details below retain the original constraints and verification work; they are not additional
claims of completed functionality.

**Text and emoji** (ARCHITECTURE.md, sections 3 and 7)

- Text selection inside messages and copying the selection. The prototype had it; the client
  only copies a whole message from the menu.
- The Apple emoji set, loaded from a local directory and never committed, plus a freely licensed
  fallback set for public builds. Today only an optional emoji font (`TG_EMOJI_FONT`) is supported.
- Custom emoji inside text (stickers in the line).
- Screen reader accessibility: message text is drawn as an image, so its text must be exposed
  separately.

**Themes** (ARCHITECTURE.md, section 7)

- Finish the OLED theme: chat list, panels, hover and selection states, and no large static bright
  areas that could burn in.

**Security** (SECURITY.md)

- Run `cargo audit` and `cargo deny` in CI.
- Recommend two-step verification in the interface, next to the list of active sessions.
- Check what happens to the key after a TPM reset or a firmware update, and on a machine without
  a TPM (the DPAPI-only fallback with its warning).
- TDLib does not encrypt downloaded files. Decide whether to accept and document this, or to keep
  the cache in a protected location and clear it on lock.

**Release** (ARCHITECTURE.md, section 6)

- Done 2026-11-18: a portable zip with `tdjson.dll` built from official TDLib source at a pinned
  commit, release notes on GitHub Releases, Veygram's own API credentials (SECURITY.md).
- Done 2026-11-19: release v0.1.0 published as an installer, `Veygram-Setup-0.1.0-x64.exe`, with
  its SHA-256 in the release notes. The installer is built outside this repository.
- Done 2026-11-22: release v0.2.0 published with both an installer (`Veygram-Setup-0.2.0-x64.exe`)
  and portable archive (`Veygram-0.2.0-windows-x64.zip`), with a reproducible Inno Setup installer
  build in the repository (`scripts/Veygram.iss`, `scripts/package-release.ps1`).
- Code signing (SmartScreen warns today) and automatic updates.
- Verify the published TDLib hash in the packaging script instead of trusting the local build.


## User review (2026-11-17): what the client lacks next to Telegram Desktop

The user compared Veygram with AyuGram (the same interface as Telegram Desktop) side by side on a
live account; the screenshot stays outside the repository because it shows private chats. The
client "feels very empty". Each item is designed from Telegram Desktop's own sources first (the
same behaviour or measurably better), with its RAM and CPU cost measured.

**Messages**

- Animated stickers. TGS (Lottie) and WebM stickers play; today only their first frame or
  thumbnail is drawn. Animation runs only for visible stickers in the active window.
- Working links. URLs, `t.me` links (open the chat or post inside the client), mentions and
  hashtags are clickable; link previews under messages.
- Group feeds: sender avatars beside messages, admin and custom title badges, reactions under
  messages, the pinned message bar at the top of the chat.

**Composer**

- A panel with emoji, stickers (installed sets and recent) and GIFs (saved and search), opened
  from the composer, as in Telegram.
- No input field where the user cannot write. In a channel without posting rights, show a Mute or
  Unmute bar, or Join when not subscribed. Groups with restricted rights show the restriction.

**Navigation**

- The main menu behind the three lines, as in Telegram: account header, My profile, Settings,
  Saved Messages, night mode and the other entries, leading to real settings screens.
- The folder bar (All chats and the user's folders) and a search field above the chat list.
- Chat list rows as in Telegram: "You:" before own last messages, read checks, pinned and muted
  marks, chat type and verified icons, drafts.

**Profiles**

- A click on the avatar or title in the chat header opens the profile: for a user, a group or a
  channel, with the description, members or subscribers, shared media and links. The header shows
  the member count or the online status under the title, plus search in chat and the other actions.
