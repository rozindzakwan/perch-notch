# Glass UI: a modern look and feel for Perch

Status: approved direction (Glass, all screens); design awaiting approval.
Mockups: [docs/design/glass-directions.html](../../design/glass-directions.html)
(the three directions; A was picked) and
[docs/design/glass-screens.html](../../design/glass-screens.html) (Glass on
every screen, approved as shown). They are HTML fragments made for the
brainstorm viewer; opened directly in a browser they still show the look.

## Intent

The user finds Perch's UI "basic" and wants **all of it** modern: the pill,
notifications, lists, mail, music, tasks, settings. They picked the **Glass**
direction: translucent, frosted surfaces with soft light edges, colours that
follow the album art, round avatars, icon tabs in a segmented capsule,
springy motion, and notifications shown **in the notch itself**. Perch is
something they look at all day; it should feel as polished as a phone's
dynamic island, not like a debug panel.

## Goals

1. Every surface restyled to Glass: collapsed pill, expanded panel and all
   tabs (Music, Tasks, Mail, Clipboard, Shelf, Mixer, Sounds, Time, Tools),
   the settings window, the to-do and mail dialogs, the screenshot toolbar,
   the camera mirror frame.
2. Modern type: Inter as the UI font, consistent sizes and weights, tabular
   digits for times and counts.
3. Motion: springy open, quick collapse, content fade/slide, sliding tab
   highlight, hover highlights that fade.
4. Notifications as **in-notch banners**: the pill stretches into a banner
   (avatar or icon, title, one line of detail) for about 4 s, then shrinks
   back and keeps a count badge.
5. A coherent design system in code (tokens and components) so new screens
   come out consistent without per-screen styling.
6. Light theme and dark theme both done properly, following the system
   setting as today.

## Non-goals

- New features or new tabs. Behaviour stays the same except notifications
  and the visual and motion changes listed here.
- A UI toolkit switch, a web view, or any GPU dependency.
- Custom themes or user-editable colours beyond light/dark/system.
- Redesigning what each tab shows (the content is the same as today, presented
  as in the mockups).

## Anti-goals (a failure even if it "works")

- Any continuous animation while idle, including while music plays. Idle CPU
  must stay at 0 and the loop must sleep when nothing moves. (The pill's
  equaliser bars are a static glyph, not animated.)
- Text that becomes hard to read over a busy wallpaper. Legibility beats
  transparency.
- Blowing the budgets: idle RAM < 40 MB, cold start < 300 ms, binary
  < 15 MB, no idle wake-ups.
- A blurred **rectangle** behind a rounded pill. If the OS blur can't follow
  the shape, don't use it.
- Clicks or hover "stolen" by invisible areas around the pill (for example a
  big transparent shadow margin) while it is collapsed.
- Breaking the existing behaviour: focus is never stolen, fullscreen
  auto-hide still works, the read-only mail guarantees hold, and all
  existing tests (or their updated equivalents) still pass.

## Constraints

- Rendering stays CPU-side: winit + softbuffer + tiny-skia + fontdue
  (ADR-001/002). Redraws happen only while something changes.
- Windows 10/11 and Linux X11 (Cinnamon first). Linux has no OS blur, and
  Windows 10 has no backdrop API for this.
- Per-monitor DPI: everything is drawn in logical units times the monitor
  scale (1x, 1.25x, 1.5x, 2x must look right).
- The model/shell split (ADR-011) stays: the model produces plain-data views
  and actions; the renderer paints them and returns hit regions. The new
  look is a renderer and view change, plus model state for banners.
- Reduced motion: respect the OS setting (Windows "Animation effects",
  `SPI_GETCLIENTAREAANIMATION`; Cinnamon/GNOME `enable-animations`). With it
  off, there are no springs or slides, only instant changes or short fades.

## Approaches considered

1. **Upgrade the existing renderer (picked).** Keep drawing everything
   ourselves, but add a design-token layer, reusable components, a vector
   icon set, an animation module (springs and fades), and model support for
   banners. Keeps the measured footprint, startup time and all tests; the
   look is limited only by what we draw.
2. **Switch to a UI toolkit (Slint, egui, iced).** Ready-made widgets, but a
   rewrite of every screen. It reverses ADR-001's measured choice (binary
   and RAM), and none of them handles notch windows or OS backdrops better.
   Rejected.
3. **Pre-rendered image skins.** Fast to fake, but blurry at fractional DPI
   and can't follow album colours. Rejected.

The user left the "how" to us; approach 1 is our call.

## The design

### Tokens (one place: `crates/ui/src/theme.rs`)

Dark glass (light glass mirrors it with dark text on a light fill):

| Token | Value |
|---|---|
| Surface fill, OS blur available | rgba(18, 20, 28, 0.58) |
| Surface fill, no blur (Win10, Linux, blur off) | rgba(18, 20, 28, 0.88) |
| Hairline border | 1 px, rgba(255, 255, 255, 0.16) |
| Inner top highlight | 1 px line, rgba(255, 255, 255, 0.16) |
| Raised element (active tab, buttons, rows on hover) | rgba(255, 255, 255, 0.08 / 0.12 / 0.20) |
| Text primary / secondary | #FFFFFF / 62% opacity |
| Accents | music: from album art (fallback coral #FF6F91 → #FFB199); mail: #5B8CFF → #8A5BFF; tasks done: #2BB673 → #7EE0A1; focus ring: coral gradient; status ok/error: #5BD38A / #FF6B6B |
| Radii | panel 26, pill fully round, banner 22, rows 14, cover art 16, tabs/buttons/avatars fully round |
| Spacing | 4-pt grid; panel padding 12; one-line rows 34, two-line rows 42 |
| Type (Inter) | title 15/700; body 12.5/500; secondary 12/400 at 62%; caption 10.5/500; numbers tabular |
| Shadow | expanded: soft, ~12 px below and to the sides, 40% black at the edge; collapsed: ≤ 4 px |

Exact values may be tuned against screenshots; the mockups are the reference
for the overall feel.

### Components (`crates/ui/src/`, new modules as needed)

Glass surface (fill + border + highlight + optional tint glow), segmented
icon tabs with a sliding highlight, two-line list row (leading
avatar/icon/thumbnail, title, secondary line, trailing time or chip, hover
background), avatar (initials, gradient picked by hashing the address/name
into 8 gradient pairs), round button and primary button (white disc), thick
slider (22 px track, grab anywhere), toggle switch, ring progress (Pomodoro),
progress bar with gradient fill, chip/badge, section header and grouped list
(settings), text field, banner. Icons: a small hand-drawn vector set (music,
check, mail, clipboard, tray/shelf, speaker, waves, clock, tools, gear, pin,
play, pause, next, previous, plus, close, link, image, mute, warning),
drawn with tiny-skia paths so they stay sharp at any scale.

### Screens (as in `glass-screens.html`)

- **Pill:** cover thumbnail (or feature icon) + one line of status (track,
  countdown, "Connecting…", messages) + badges on the right (mail count). Its
  width follows the content between the configured collapsed width and
  about 320 logical px, animated.
- **Music:** cover-colour glow behind the panel, 72 px cover, title/artist,
  current lyric line bold and the next one dimmed (when lyrics are on),
  gradient progress bar with times, previous / big white play-pause / next.
  Needs "next lyric line" in `MediaView` (the model already has the lyrics).
- **Tasks:** Pomodoro ring with time inside and session text, pause/reset
  buttons; to-dos with round checks that fill green, due time on the right,
  "＋ Add a task" row (opens the dialog).
- **Mail:** avatar, sender, subject, age (plus account when there are
  several); account problems as a warning row at the top; "Accounts" chip.
- **Clipboard / Shelf:** type icon or image thumbnail per row, "Copy" chip on
  hover, header with count and "Clear".
- **Mixer / Sounds:** app initial in a coloured disc (known apps get their
  brand colour; others hashed), thick sliders, click the disc to mute.
- **Time, Tools:** same components (bars become rounded gradient bars;
  tools become round icon buttons with labels).
- **Settings and dialogs:** dark (or light) sheet with grouped sections,
  real toggles, choice controls as segmented pills or steppers, text fields
  with focus ring, status dot per mail account. These are normal windows; on
  Windows 11 they use the system Mica backdrop where available.
- **Screenshot overlay:** the toolbar becomes a glass pill; selection handles
  and annotation colours follow the tokens.
- **Camera mirror:** rounded corners, hairline border, shadow.

### Motion (`perch-core` notch state machine + a small UI animation module)

- **Open:** spring (critically under-damped, about 320 ms to settle, ≤ 4 %
  overshoot). **Close:** ease-out, about 180 ms, no overshoot.
- **Content:** fades in (140 ms) and slides up 6 px once the panel is ~40 %
  open; fades out at the start of closing.
- **Tabs:** the highlight capsule slides (spring, about 220 ms); the content
  cross-fades (120 ms).
- **Hover:** row and button highlights fade in and out (100 ms).
- **Pill width changes** (new track, badge appears) animate with the same
  spring.
- Everything runs on deadlines: frames only while an animation is active,
  then the loop sleeps (no deadline once settled), as today.
- Reduced motion: as in Constraints.

### Notifications in the notch

- The model gets a **banner queue**. Sources: new mail (sender + subject,
  or "N new messages" when several arrive together), Pomodoro phase done,
  water/stretch reminders, to-do due, and the existing short messages
  (`flash`, e.g. "Added Work").
- While collapsed and visible, a banner morphs the pill to banner size
  (about 300 × 56 logical), shows for 4 s (paused while the cursor is over
  it), then morphs back. Clicking it opens the panel on the relevant tab.
  Queued banners show one after another; at most 3 queued, the rest merge
  into a count.
- While the panel is open, no banner: the content and badges update in
  place.
- While hidden by a fullscreen app: no banner and no sound. Badges update,
  and the most recent banner (one) shows when the pill comes back.
- **System notifications** (Windows toasts / freedesktop) become optional:
  new config `[notifications] system = false` (default off) and `in_notch =
  true`, both in settings. With `in_notch = false`, behaviour is today's
  (system toast plus a short pill message).

### Blur behind the glass (Windows 11 only)

Do a **spike first**: on the pill window, `DwmExtendFrameIntoClientArea`
with the whole client area plus `DWMWA_SYSTEMBACKDROP_TYPE =
DWMSBT_TRANSIENTWINDOW` (Acrylic). Check that (a) the blur shows behind our
translucent pixels, (b) it can be confined to the rounded pill/panel shape
(window region or another method) during the resize animation, and (c) it
adds no idle cost. If all three hold, use it with the "OS blur" fill alpha.
Otherwise drop it and use the "no blur" alpha everywhere. A config switch
`[appearance] blur = "auto" | "off"` lets users turn it off. Linux and
Windows 10 always use the no-blur fill.

### Album colours

On track change (only then), extract two dominant colours from the cover
(downscale to about 32×32, bucket, pick the two strongest that aren't
near-grey; fall back to the coral pair). Used for the Music glow, the
progress bar and the play button ring, and for the pill's cover thumbnail
border. Cached per track.

### Font

Bundle **Inter** Regular, Medium and Bold (SIL Open Font License; licence
file next to the fonts), and keep **DejaVu Sans** as the fallback for glyphs
Inter lacks (symbols, emoji such as 😀 in subjects). Text layout picks the
first font that has a glyph. Binary growth stays within budget (expected
under +1 MB). If the binary budget is at risk, subset the fonts.

### Config compatibility

Existing config files contain the old defaults (`expanded_width = 480`,
`expanded_height = 200`) because settings saves the whole file. The panel
now sizes itself to its content per tab, up to a maximum: `expanded_height`
becomes that maximum, and old files holding exactly the previous default
pair are treated as "use the new defaults" (about 380 × 300). New
`[notifications]` and `[appearance]` sections default as above; unknown-
field strictness stays.

## Calls made while writing this up (not discussed; easy to change)

- **Banners follow the existing mail settings:** `mail.notify = false` means
  no mail banner; `mail.notify_subject = false` shows the sender only in the
  banner too.
- **Hovering a banner pauses it; reaching the top edge still opens the
  panel** as it does today; clicking the banner opens the matching tab.
- **Avatars are initials only.** Perch never fetches profile pictures (that
  would be network traffic and a privacy leak).
- **Shadows and glows are drawn as cached alpha masks** (rebuilt only when
  the size or scale changes), since tiny-skia has no blur filter and the
  frame budget is tight.
- **Light theme:** same structure with a light fill (about rgba(246, 247,
  250, 0.72) with OS blur, 0.92 without), dark text, and borders at 10 %
  black; tuned against screenshots.

## Verification

- Unit tests: spring settles, overshoot ≤ 4 %, collapse has none, no
  deadline once settled; reduced-motion paths; banner queue (ordering,
  merging, pause on hover, fullscreen deferral, click opens the right tab);
  album colour extraction (deterministic on fixed images); avatar colour
  hashing stable; font fallback picks DejaVu for missing glyphs.
- Render tests (`crates/ui/tests/render.rs`): every body variant and the
  banner render at 1x and 2x without panics; text stays inside its box
  (ellipsized, never overflowing); hit regions lie inside the panel shape;
  pixel samples: outside the rounded shape fully transparent, surface pixel
  alpha matches the token.
- Smoke tests (Windows and X11) updated for the new geometry, plus a banner
  check (new mail → pill becomes banner → back to pill with a badge), with
  cropped screenshots of every tab for the README.
- Budgets, measured and recorded in TESTING.md: frame render time for the
  expanded panel ≤ 4 ms at 1x and ≤ 8 ms at 2x on the dev machine; idle CPU
  0 after settling (soak); idle RAM < 40 MB with album colours and fonts;
  first frame < 300 ms; binary < 15 MB on both OSes.
- The mail Dovecot tests, media smoke and everything in linux-ci still pass.

## Left to the builder

- Exact pixel values and curves within the ranges above, tuned against
  screenshots so they match the mockups' feel.
- Module layout inside `perch-ui` and how the animation clock is wired
  (keeping it deadline-driven).
- The colour-extraction algorithm details and the brand-colour table for
  common apps in the mixer.
- Whether choice fields in settings are segmented pills or steppers,
  depending on the number of options.
- The order of work, as long as each step keeps the app working and the
  tests green (suggested: tokens + font + components → pill and panel shell
  + tabs → each screen → motion → banners → settings/dialogs → blur spike →
  polish and screenshots).
