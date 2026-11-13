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
