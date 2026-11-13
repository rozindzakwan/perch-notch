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
