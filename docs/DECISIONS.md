# Architecture decisions

Recorded as they were made, with the evidence that drove them.

## ADR-001: UI toolkit = winit + softbuffer + tiny-skia (not Slint)

**Date:** 2026-11-13 - **Status:** accepted

The brief defaulted to Slint and allowed a different choice if a one-day spike
showed a better fit. A spike (`projects/spike-notch`, measured on this Windows
machine) decided it.

What the spike proved, measured:

| Requirement | Result |
|---|---|
| Borderless / transparent / topmost / non-activating | `exstyle 0x40118 -> 0x8040198` (`WS_EX_TOOLWINDOW` + `WS_EX_NOACTIVATE` + `WS_EX_TOPMOST` all set) |
| 2 px invisible hot-zone window delivering hover events | `HOTZONE CursorEntered` / `HOTZONE CursorLeft` received, no cursor polling |
| Cold start to first presented frame | 49.8 ms |
| Idle working set | 16.4 MB (private bytes 2.72 MB) |
| Idle CPU over 30 s with a hover in the middle | 0.047 s total CPU |
| Release binary (spike, with deps) | 2.7 MB |
| Antialiased rounded pill | `tiny-skia` fill_path with `anti_alias = true` |

Why not Slint:

- Slint owns its window creation and its per-window style hooks are thinner than
  raw `winit` + `raw-window-handle`, which we need because the product *is* a
  weird window: tool window, no activation, no taskbar, plus a second 2 px
  hot-zone window. Driving two such windows is `winit`'s normal job.
- The pill is a handful of rounded rects, text lines and a cover thumbnail. A
  retained-mode widget toolkit buys little and costs binary size plus a styling
  layer we would mostly fight.
- `softbuffer` keeps us on a software path, so there is no GPU context to lose,
  no driver-dependent transparency behaviour, and nothing to initialise at
  startup - which is how we get a sub-50 ms first frame.
- `tiny-skia` is pure Rust, `forbid(unsafe_code)`-compatible in our crates, and
  gives us the antialiasing quality the pill needs.

Cost accepted: we write our own layout/text/hit-testing (small, and `fontdue`
covers glyph rasterisation). This is the right trade for a single bespoke
surface; it would be the wrong trade for a settings-heavy app, so the settings
window stays deliberately simple.

## ADR-002: Software rendering, event-driven only

No polling loops anywhere. The notch state machine
(`perch_core::notch::Notch`) exposes `next_deadline() -> Option<Duration>`,
which returns `None` whenever the notch is idle (collapsed or fully expanded)
or hidden. The event loop uses that to pick `ControlFlow::Wait` versus
`WaitUntil`, so an idle Perch receives zero wakeups. Animation frames are
produced only while `wants_animation()` is true.

## ADR-003: Toolchain = `x86_64-pc-windows-gnu`

This machine had no MSVC Build Tools and no admin rights in this session, so the
MSVC target could not be installed. The pre-existing rustup default was
`stable-x86_64-pc-windows-gnu`, which only needed a MinGW-w64 toolchain with a
working `dlltool.exe` (the system `windows-sys` build failed without it).
Installed winlibs GCC 16.2.0 (UCRT, POSIX threads) to `%USERPROFILE%\tools\mingw64`
and added it to the user PATH.

Consequences: the Windows binary links against UCRT via MinGW. CI additionally
builds the MSVC target on `windows-latest` so both toolchains stay honest. If
you prefer MSVC locally, install the Build Tools and
`rustup target add x86_64-pc-windows-msvc`; no source change is needed.

## ADR-004: Name and branding

"Perch" - a small thing that sits on a ledge at the top of your screen. Mascot
is an original minimal bird glyph drawn as vector paths in
`crates/ui/src/art.rs`; no assets, names, or artwork from layzee or anything
else are used.

## ADR-005: Linux verification in a local Ubuntu 22.04 container

**Date:** 2026-11-14 - **Status:** accepted

The development machine is Windows and has no GitHub remote configured, so
GitHub Actions cannot run yet. `docker/linux-ci.Dockerfile` builds an
`ubuntu:22.04` image (glibc 2.35, the Linux Mint 21 base) and
`scripts/linux-ci.sh` holds the exact Linux CI steps; `.github/workflows/ci.yml`
runs the same script on `ubuntu-22.04`. Results from the container are labelled
"CI-verified (ubuntu-22.04 container)". Only `scripts/mint-test.sh` output from
a real Mint machine is labelled "verified on real Mint".

## ADR-006: Event plumbing = winit `EventLoopProxy`, no async runtime

Every backend owns at most one blocking thread (Win32 message loop with
WinEvent hooks, X11 `wait_for_event`, zbus blocking, IPC accept) and delivers
`PlatformEvent`s through an `EventSink` that wraps winit's `EventLoopProxy`, so
any backend event wakes an otherwise fully idle loop. Timers (animation,
Pomodoro seconds, reminders, to-do due times, the seek bar while visible) are
deadlines folded into `ControlFlow::WaitUntil`. There is no tokio; `zbus` runs
on its own async-io executor behind the blocking API. Idle time is read once at
a reminder's deadline (plus lock/unlock events), never on a loop.

## ADR-007: Tray = `tray-icon` on Windows, StatusNotifierItem on Linux

`tray-icon` pulls GTK into Linux builds, so it is a Windows-only dependency.
Linux uses `ksni` (pure Rust SNI over D-Bus; Mint ships `xapp-sn-watcher`) and
falls back to a minimal XEmbed icon (`_NET_SYSTEM_TRAY_S0`) when no SNI host
exists. The tray, CLI and hotkeys all emit the same command verbs.

## ADR-008: Packaging = NSIS (per-user) + portable zip; `.deb` from the container

NSIS runs from a portable zip (no admin) and produces a per-user installer
(`RequestExecutionLevel user`, `%LOCALAPPDATA%\Programs\Perch`, HKCU uninstall
key). WiX was skipped: per-user MSIs are awkward and WiX needs .NET. The `.deb`
is built with `cargo-deb` inside the 22.04 container for glibc compatibility.

## ADR-009: Notifications always have an in-notch fallback

Windows toasts need a registered AppUserModelID; Perch registers one under
`HKCU\Software\Classes\AppUserModelId\Perch.Notch` (no admin, no shortcut).
Linux uses `org.freedesktop.Notifications`. Every notification is also shown as
a transient message in the collapsed pill, so a missing notification daemon
loses nothing.

## ADR-010: Heavy features are compile-time *and* run-time optional

The default release build includes everything except dictation (`speech`),
which ships as a separate `perch-speech` build. All Phase 5 features are off in
the default config and initialise on first use.

## ADR-011: The model/shell split

`crates/app/src/model.rs` holds all feature state and turns actions, platform
events and deadlines into `Effect`s; it never touches the OS. The winit shell
carries effects out. The renderer paints a plain-data `PanelView` and returns
hit regions tagged with `Action`s, so clicks round-trip through pure code. This
is what lets `tests/model.rs` drive every feature with fake backends and a fake
clock.

## Bugs found by the smoke tests (and fixed)

- **CLI commands never woke an idle loop.** IPC pushed into an mpsc channel
  drained only in `about_to_wait`; with `ControlFlow::Wait` nothing ran it.
  Fixed by delivering through `EventLoopProxy` (ADR-006).
- **Maximised windows hid the notch.** On a monitor whose taskbar auto-hides, a
  maximised window covers the whole screen; the geometry check called it
  fullscreen. Zoomed windows are now never fullscreen, and Windows 11 shell
  overlays (`XamlExplorerHostIslandWindow`, Task View, Alt-Tab) are excluded.
- **The pill stayed hidden after a fullscreen app closed.** The shell's "quiet"
  state lags the window that caused it. A single one-shot re-check is armed
  1.5 s after each foreground change (one extra wakeup, never a loop).
- **Hover collapsed while the cursor was on the pill.** Windows delivers the
  growing pill's `CursorEntered` *before* the strip's `CursorLeft`. The notch
  state machine now tracks the strip and the pill separately and collapses only
  when the pointer has left both.
- **A GUI instance could freeze on a blocked console write.** Release builds
  attached to the parent console; with nobody draining it the first log line
  blocked the UI thread. Only short-lived CLI invocations attach now; the
  running instance logs to a size-capped `perch.log` in its data directory.
- **X11 keep-above was lost.** Once mapped, the WM owns `_NET_WM_STATE`, and
  requests sent before the WM has managed the window are ignored (also after
  "Restart Cinnamon"). The X11 watcher now re-requests the states whenever the
  WM sets `WM_STATE` on a notch window or `_NET_SUPPORTING_WM_CHECK` changes.
- **Idle RSS grew 26 MB after opening settings (Linux).** glibc kept the freed
  frame buffers. Forms reuse their pixmap and Perch calls `malloc_trim` after
  closing a form.
- **The notch hid on startup with no fullscreen app.** Windows reported the
  shell state `QUNS_BUSY` with nothing fullscreen in front (overlay software
  was running) and kept reporting it after fullscreen windows closed. Perch now
  trusts only `QUNS_RUNNING_D3D_FULL_SCREEN` and `QUNS_PRESENTATION_MODE` from
  the shell; borderless fullscreen is caught by the geometry rule.

## ADR-012: Clipboard privacy is decided before content is read

Windows: the clipboard listener's change triggers a check of
`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory`
and `CanUploadToCloudClipboard` before `arboard` reads anything. X11: the
watcher answers each CLIPBOARD owner change (XFixes) by requesting the owner's
`TARGETS` itself and emits `ClipboardChanged` only when
`x-kde-passwordManagerHint` is absent, so a password-manager copy is never
even signalled. History lives in memory only and is cleared when the feature
is turned off. Both paths are covered by end-to-end smoke checks.

## ADR-013: Shelf drag-out

Windows uses an OLE drag source over the shell's own data object for the files
(`IShellItemArray` → `BHID_DataObject`), so every drop target that understands
Explorer drags works. Linux has no drag-out (an XDND source is a lot of
protocol for little gain); clicking a shelf item copies it as a file to the
clipboard (`text/uri-list` via `arboard`), which file managers paste.

## ADR-014: Audio

- **Mixer.** Windows: WASAPI sessions on the default render endpoint, grouped
  per application, with `IAudioSessionNotification` + per-session
  `IAudioSessionEvents` callbacks (own changes are tagged with a context GUID so
  their echoes are ignored). Linux: PulseAudio sink inputs over
  `libpulse-binding`'s threaded mainloop with a sink-input subscription; works
  on PipeWire through `pipewire-pulse` (Mint 22's default). Limitation: the
  Windows mixer follows the default device it started on; switching the
  default output device needs the feature toggled (documented).
- **White noise.** Procedural (core `NoiseGenerator`), played through `rodio`
  only while the mix is audible: the output stream is dropped the moment the
  last layer goes silent, so an idle Perch has no audio callback running.
- **Screen time.** Per-day, per-app totals in `usage.sqlite` (WAL), upserted
  as absolute values on every app switch and on exit, last 30 days restored at
  start. On X11 the desktop gaining focus is reported as "desktop", so the last
  app stops accruing time.

## ADR-015: Phase 5 (heavy features)

- **Hotkeys.** Windows: `RegisterHotKey` on the event thread (press only, so
  dictation toggles). X11: `XGrabKey` on the root window for every CapsLock /
  NumLock combination, XKB detectable auto-repeat for clean push-to-talk; a
  `BadAccess` grab means the desktop already owns the shortcut and is logged.
  Only shortcuts for live features are grabbed. The CLI verbs remain the
  fallback for keybindings in Cinnamon's Keyboard settings.
- **Screenshot.** Capture: GDI `BitBlt` (Windows), X11 `GetImage` (32-bit
  visuals). The overlay (`perch_ui::shot`) is pure state + painting, tested
  without a window. Copy puts a PNG on the clipboard, Save writes
  `Pictures/Perch/perch-shot-<unix>.png`, "Copy text" runs OCR:
  `Windows.Media.Ocr` (built in, offline) or the `tesseract` CLI (deb
  Recommends). The notch collapses before capture (250 ms deadline).
  Wayland: not implemented (would need the xdg-desktop-portal Screenshot API).
- **Camera mirror.** `nokhwa` (MSMF / V4L2) on a capture thread that exists
  only while the mirror is open; frames downscaled to <= 640 px wide.
- **Dictation.** `whisper-rs` (whisper.cpp) behind the `speech` feature,
  shipped as a separate build. Model picked from RAM/cores (tiny/base/small,
  multilingual, auto language), downloaded once from a pinned Hugging Face
  revision (`5359861c…`) and verified by size + SHA-256. Microphone open only
  while recording; result pasted via clipboard + synthetic Ctrl+V (SendInput /
  XTEST). **Windows limitation:** whisper.cpp's CMake passes the MSVC-only
  `/utf-8` flag to MinGW's g++, so the dictation build needs the MSVC
  toolchain (CI's windows-latest); this machine has no admin rights to
  install MSVC, so the Windows dictation build is CI-only.

## ADR-016: Packaging details and soak scoring

- **Windows.** `build.rs` embeds the icon and version resource with
  `winresource` (MinGW `windres` or MSVC `rc`). The NSIS installer is
  per-user, kills a running `perch.exe` before it overwrites files, and on
  uninstall removes the HKCU Run value and the toast AppUserModelID but keeps
  `%APPDATA%\perch` (settings and data belong to the user). The portable zip
  has the same files plus a note about SmartScreen.
- **Linux.** Besides the `.deb` there is a tarball holding `install.sh`, which
  installs into `~/.local` (binary, menu entry, hicolor icons) and removes
  everything again with `--uninstall`. Both artifacts get a `SHA256SUMS`.
  `linux-ci.sh debinstall` installs the `.deb` into the clean container,
  runs it and removes it.
- **Soak scoring.** The soak runs on a desktop that is in use, so naive
  first-sample/last-sample deltas fail on warm-up (clipboard history and
  screen-time rows filling up) and on a single busy sample. Scoring therefore
  skips a 2-minute warm-up, compares medians of the first and last 5
  minutes, and gates CPU on the median 10-second interval (≤ 0.5 % of a
  core: a busy loop raises every interval) with an average ceiling of 2 %.
  Both scripts can re-score an existing CSV.
- **Real-desktop smoke.** `smoke-x11.sh` takes its coordinates from the
  primary monitor (`xrandr`), so the same script runs on Xvfb and, through
  `mint-test.sh`, on a real Cinnamon desktop of any size.

## Bugs found during packaging and real use (and fixed)

- **Startup froze while Spotify was playing (Windows).** `SmtcMedia::start`
  ran on the UI thread, a COM single-threaded apartment, and blocked on WinRT
  async operations (`RequestAsync().get()`, then reading the current track's
  cover thumbnail). Reading Spotify's thumbnail needs a call back into the
  calling apartment, which was blocked: a deadlock. The window appeared but
  Perch never finished starting (no hover, no mixer, no lyrics, no CLI).
  Found with a symbolized backtrace (`media.rs` `read_track` → `.get()` from
  `Shell::resumed`). Fix: the blocking setup runs on its own MTA thread and
  `start` returns at once; the first track arrives through the sink like
  every later change. Regression test: `platform-windows/tests/smtc_sta.rs`
  (blocked > 5 s before the fix with Spotify current; 41 µs after).
- **Lyrics never appeared when turned on mid-song.** They were requested
  only on a track change, so enabling the feature while a song played did
  nothing until the next song. `Model::apply_config` now requests lyrics for
  the current track when the feature becomes active. Test:
  `turning_lyrics_on_mid_song_fetches_them_for_the_current_track`.
- The status file now reports `mixer_sessions` and `lyrics_loaded`, so both
  features can be checked end to end without screenshots.

## ADR-017: Read-only mail over IMAP

- **Protocol.** IMAP over TLS (or STARTTLS), because every mainstream
  provider and company server speaks it with password or app-password
  sign-in. Provider REST APIs (Gmail API, Microsoft Graph) would need OAuth
  app registrations and per-provider code.
- **Read-only by construction.** `EXAMINE` (the server enforces read-only for
  the session), `UID SEARCH UNSEEN`, and `UID FETCH (UID INTERNALDATE
  ENVELOPE)` for the newest few. No `SELECT`, `STORE`, or body fetches, which
  the scripted-server test asserts and the Dovecot test confirms on the server
  side (0 messages `\Seen`).
- **Event-driven.** One thread per account, blocked in IMAP IDLE (re-issued
  every 25 min per RFC 2177). A check every `poll_minutes` only for servers
  without IDLE. Reconnects back off 5 s → 5 min. A rejected sign-in stops the
  thread until the password or the settings change, because retrying could
  lock the account.
- **Small footprint.** Own ~700-line client on `imap-proto` (parser only),
  reusing the rustls/ring/webpki-roots that `ureq` already brings, so no
  OpenSSL and no async runtime. +0.35 MB on Windows, +0.58 MB on Linux.
- **Secrets.** Passwords only in the OS keychain: Credential Manager
  (generic credential `Perch/mail:<user>@<host>`, local machine persistence)
  and the Secret Service (`application=perch`, `perch-key=...`) over the zbus
  already used for MPRIS. Read on the account's own thread, never logged.
  Without a Secret Service, kept in memory until exit.
- **No OAuth (yet).** Microsoft 365/Outlook.com require it. It needs an app
  registration per provider and a browser sign-in flow; documented as a
  limitation.
- **Presets.** Spacemail (the user's company mail, confirmed from the
  domain's MX records and the server's IMAP greeting: Dovecot with IDLE and
  AUTH=PLAIN), Gmail, Yahoo, iCloud, Fastmail, Zoho, Other.

## ADR-018: No acrylic blur behind the notch (yet)

The Glass design has blur-aware alphas (surface 0.58 with blur, 0.88
without). Before using them, a spike (`crates/ui/examples/blur_spike.rs`)
tested the Windows 11 backdrop on the exact window Perch uses: a borderless,
per-pixel-transparent winit + softbuffer window, with
`DwmExtendFrameIntoClientArea(-1)` and
`DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`, drawing a translucent
26 px rounded rectangle over an opaque checkerboard window of its own.
Windows 11 25H2 (build 26200), 100 % scale. Captures in `docs/img/adr-018/`
contain only the spike's two windows.

| Check | Result |
|---|---|
| (a) blur behind translucent pixels | **Yes.** The checkerboard under the glass becomes a flat acrylic grey (pixel standard deviation 53.5 without the backdrop, 0 with it). |
| (b) confined to the rounded shape | **No.** The backdrop fills the whole window rectangle, clipped only by DWM's own small corner radius. That leaves lighter acrylic wedges outside the 26 px corners, and DWM also paints a ghost caption-button glyph in the top-right corner. A `SetWindowRgn` rounded region removes the glyph and most of the wedge, but the region is 1-bit, so the edge is jagged. It would also have to be rebuilt on every frame of the spring animation, and it would cut off the shadow margin the panel draws around itself (without a region, acrylic fills that margin instead). |
| (c) no idle CPU | **Yes.** 0 ms of CPU over 4 s idle in every mode. |

**Decision.** Blur stays off on every platform: the palette always uses the
no-blur alphas (`Palette::resolve(.., blur = false)`). They were raised from
the spec's 0.88 (dark) / 0.92 (light) to 0.94 / 0.96 after a Windows smoke
capture showed text from the window behind reading through the open panel.
There is no
`[appearance] blur` setting, since a switch that can't do anything would only
confuse. `Frame.blur` and the blur alphas stay in place for when it becomes
possible. Things that could change this:
- a composition-based renderer (DirectComposition / Windows.UI.Composition),
  where a `CompositionBackdropBrush` can be clipped to an anti-aliased
  rounded geometry;
- on Linux, compositor blur that honours a shape (KWin's
  `_KDE_NET_WM_BLUR_BEHIND_REGION`, picom's `blur-background`), which only
  some desktops offer, and not Cinnamon's Muffin.

The settings window uses `apply_mica` (Mica backdrop on its title bar and a
dark title bar in the dark theme). That works because it is an ordinary
decorated window, where none of the problems above apply.

## ADR-019: Glass UI

The first UI was plain: flat fills, DejaVu text, letters for icons, linear
timing. The redesign (spec `docs/superpowers/specs/2026-11-14-glass-ui-design.md`,
chosen as "A · Glass" from three browser mockups) keeps the software renderer
from ADR-001 and builds the look from a few cached primitives:

- **Tokens.** One palette per theme (`theme.rs`: surface, three raised
  levels, hairline border, top highlight, ok/error, and gradients for the
  accent, mail and tasks) and one set of radii, spacing and type styles
  (`tokens.rs`). Components only use tokens.
- **Type.** Inter (Regular/Medium/Bold, SIL OFL) for Latin, Greek and
  Cyrillic, with DejaVu kept as the fallback for everything Inter lacks
  (emoji, symbols, more scripts). fontdue builds every glyph outline when it
  loads a font, and the full files cost 49 MB of RAM, mostly OpenType
  alternates fontdue can't reach without shaping. So the fonts are subset
  (`scripts/subset-fonts.sh`: 110 KB per Inter weight), and DejaVu is split
  in two (common symbols and emoji; other scripts and rare symbols), each
  parsed only when a character needs it: 18 MB for Latin text, +6 MB once an
  emoji appears, +6 MB more for another script. Digits can be drawn tabular (fontdue has no
  OpenType `tnum`, so each digit is centred in the widest digit's advance),
  so the seek-bar times and the Pomodoro countdown don't jitter.
- **Icons.** About thirty vector icons on a 24-unit grid, stroked with round
  caps, so they stay sharp at any scale (`icons.rs`).
- **Depth.** A soft shadow as a nine-slice blurred mask, cached by radius and
  blur and stretched to any size, so the opening animation doesn't rebuild
  it. Album colours (two dominant colours from the cover, `core::palette`)
  tint the Music panel with a glow that is rendered once per size and colour
  pair and then blitted.
- **Motion.** A critically damped spring (ζ = 0.75, overshoot ≤ 4 %) for the
  notch's open/close, the pill width, the panel height and the tab
  highlight. Content fades in after 40 % of the open. Every motion is stepped
  by explicit time and returns its next deadline, so the event loop sleeps
  as soon as everything settles: idle CPU stays at zero. Animation advances
  at most once per frame interval, whatever wakes the loop. On X11 each
  resize comes back as an event, and stepping on every one of them (with
  microsecond `dt` that the f32 spring couldn't resolve) once turned a
  banner into a self-feeding busy loop; the spring now integrates in f64.
  The OS
  reduced-motion setting (Windows `SPI_GETCLIENTAREAANIMATION`, GNOME/Cinnamon
  `enable-animations`) snaps everything.
- **Notifications in the notch.** New mail, due to-dos, Pomodoro phases and
  reminders show as a banner that grows out of the pill (300×56) for 4 s,
  pausing while hovered. At most three are queued, and extra mail merges into
  "N new messages". Banners wait while a fullscreen app is active. System
  toasts became opt-in (`[notifications] system`).
- **Windows.** The settings window and dialogs use the same tokens: grouped
  sections, switches, segmented choices, status dots for mail accounts, and
  Mica on the Windows 11 title bar. The screenshot toolbar is a glass pill of
  icon buttons, and the camera mirror has rounded corners.

Budgets, checked by tests: a settled Music panel draws in ≤ 4 ms at 1× and
≤ 8 ms at 2× (measured 1.9 / 6.5 ms on the Ryzen 7 3800X, release build);
building a 380×300 shadow at 2× takes < 15 ms once, < 2 ms cached. Idle
memory is where it was before the redesign (Windows: ~24 MB private);
`perch.exe` grew from 8.6 to 9.2 MB. Acrylic blur was evaluated and not used
(ADR-018).

## ADR-020: Mirroring other apps' notifications

The user wanted other apps' notifications (WhatsApp, Teams, Discord...) in
the notch, not only Perch's own banners. A spike
(`crates/platform-windows/examples/notif_spike.rs`, which prints only app
names and line counts) found:

- `UserNotificationListener` **does** grant an unpackaged Win32 app access
  (status Allowed) and lists the action center's toasts with app name and
  text.
- Its `NotificationChanged` event fails with "Element not found" without
  package identity. Getting identity means a signed sparse MSIX package,
  which needs a trusted certificate (admin) or Developer Mode.
- The notification store (`wpndatabase.db-wal`) is written through a memory
  map, so directory-change notifications never fire for it, even though its
  size and timestamp change with every toast. A WinEvent hook saw no toast
  window either, and toasts suppressed by Do Not Disturb never show one.

**Decision.** While the feature is on (it is off by default), a worker thread
compares the store file's size and timestamp once a second (microseconds),
and reads the list only when they changed. Reading the list costs about
37 ms; reading it unconditionally every 2 s used ~2 % of a core, and the
gated check measures 16 ms of CPU per 20 s. Only notifications not seen
before are read in full, and the first snapshot is a baseline, so turning the
feature on doesn't replay the action center. A banner appears 0.5–1.1 s after
the toast. Perch's own toasts are skipped. The text is held in memory for the
banner only, never stored or logged.

This is the one feature that checks on a timer (Windows only). It's opt-in,
and the check is a file stat.

**Linux** needs no timer. Every app sends notifications as an
`org.freedesktop.Notifications.Notify` call on the session bus, so a separate
connection calls `BecomeMonitor` with a match rule for exactly that method
and receives each call as it is made, alongside the desktop's daemon, which
still shows its own popup and answers the call. Markup and entities in the
body (`<b>`, `<a href>`, `&amp;`) are stripped to plain text, and Perch's own
notifications are skipped. Closing the connection stops the reader. Tested
on a private bus against a fake daemon (`tests/session_bus.rs`).

## ADR-021: A stripe instead of hiding, over fullscreen and maximized apps

The notch used to disappear while a fullscreen app was in front. The user
wanted it to stay reachable without disturbing them, and for maximized
windows too, since the notch sits on top of their title bar and content.

**Decision.** While a fullscreen app or a maximized window is in front of
the notch's monitor (and `[notch] hide_on_fullscreen` is on, as by default),
the collapsed notch shrinks to a 120×5 stripe. Hovering the stripe starts a
`reveal_delay_ms` (2 s) timer: leaving cancels it, staying opens the notch,
and moving away shrinks it back to the stripe. The hotkey, the CLI and a
banner click still open it at once. Banners wait until nothing is in front.
The pill's content fades with its height, so the shrink animation is smooth
and the stripe is a plain bar. The timer is one deadline, so nothing runs
while the stripe rests (no wake-ups, 0 CPU).

- Windows: the foreground hook already re-checks fullscreen on focus and
  location changes; it now also reports which monitor holds a maximized
  foreground window (`IsZoomed` + window centre), so a maximized window on
  another monitor leaves this one alone.
- X11: the watcher already follows the active window's `_NET_WM_STATE`; it
  now also reports `_NET_WM_STATE_MAXIMIZED_VERT` + `_HORZ` with the monitor
  under the window's centre.
- Limitation: X11 window managers stack a focused fullscreen window above
  docks, so there the stripe is covered and can't be hovered (it works over
  maximized windows). Making the notch an override-redirect window would get
  around that, at the cost of managing its stacking ourselves.
