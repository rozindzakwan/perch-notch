# Testing Perch

This file lists what was run, where, and what came out. Raw evidence (JSON,
logs, cropped screenshots, CSVs) goes to `artifacts/` (gitignored): every
script below writes there.

Environments:

| Name | What it is |
|---|---|
| **Windows dev machine** | Windows 11 Home 25H2 (build 26200), Ryzen 7 3800X (16 threads), 32 GB, RX 6600, one 1920×1080 monitor, no admin rights. Rust stable, `x86_64-pc-windows-gnu` (MinGW-w64 GCC). |
| **Linux container** | `docker/linux-ci.Dockerfile`: Ubuntu 22.04.5 (glibc 2.35, the Mint 21 base) on Docker Desktop/WSL2 on the same PC, 8 vCPUs. Xvfb 1920×1080 + openbox, optionally picom; private D-Bus session bus; PulseAudio null sink. Runs `scripts/linux-ci.sh`, the same script GitHub Actions runs on `ubuntu-22.04`. |
| **Real Linux Mint** | Not available to the author. `scripts/mint-test.sh` produces the results; paste them in [below](#real-linux-mint). |

## 1. Static checks and unit/integration tests

| Command | Where | Result |
|---|---|---|
| `cargo fmt --all --check` | Windows, container | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | Windows (default features; GNU can't build `speech`) | clean (0 warnings) |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | container | clean (0 warnings) |
| `cargo test --workspace` | Windows | 316 passed, 0 failed, 5 ignored (opt-in hardware tests, run separately below) |
| `cargo test --workspace --all-features --locked` | container | all passed, 0 failed |
| `cargo audit` | Windows | 0 vulnerabilities; 7 warnings: 6 unmaintained crates (paste, proc-macro-error, ttf-parser, zbus_names, zvariant, zvariant_derive) and `glib` 0.18 "unsound", which is only in the lockfile through `tray-icon`'s Linux GTK deps and is never compiled (tray-icon is Windows-only here; Linux uses ksni) |

What the tests cover (`cargo test` lists each one):

- **core**: notch state machine (hover enter/leave ordering, collapse delay,
  pin, fullscreen), geometry for all 8 anchor positions with mixed-DPI fake
  monitors and work areas (taskbar on top), config parsing and defaults
  (proptest round trip), LRC parsing (proptest), media/now-playing logic,
  Pomodoro, to-do due times, reminders (idle-aware stretch, postpone),
  clipboard history and privacy rules, screen-time aggregation, hotkey parsing.
- **app**: the model driven through fake backends and a fake clock
  (`tests/model.rs`: every feature from action to effect), settings forms,
  lyrics with a fake HTTP layer and on-disk cache, screen-time SQLite round
  trip, OCR round trip (renders text, recognises it), dictation model
  picker, checksums and resampling. Opt-in tests that touch hardware are
  `#[ignore]`d: `audio` (makes sound), `camera` (switches the webcam on),
  `speech` end to end (downloads a model).
- **ui**: screenshot overlay state (selection, crop, pen/arrow/text, copy),
  mirror math, rendering of each panel tab without a window.
- **platform-linux** (container, private bus): MPRIS against a mock player,
  notifications and screen-lock signals (`--test mpris --test session_bus`),
  PulseAudio per-app volume against a null sink (`scripts/pulse-test.sh`).

## 2. Windows end-to-end

Real windows, real synthetic input (`SendInput`), real SMTC. The scripts check
that the mouse is idle before they start and never screenshot more than the
notch area.

### `scripts/smoke-windows.ps1`: 26/26 passed

Checks, in order: windows created; the pill and the 2 px hover strip exist with
`TOOLWINDOW|NOACTIVATE|TOPMOST` and no `APPWINDOW` (no taskbar/Alt-Tab);
hovering expands 200×32 → 440×132 and leaving collapses back; CLI
`toggle-notch` pins open and closes; CLI `todo`; first frame < 300 ms
(**87 ms**); clipboard records a normal copy, **skips a copy marked
`ExcludeClipboardContentFromMonitorProcessing`**, then records the next one; a
fullscreen window hides the pill and leaving fullscreen shows it again; screen
time records the foreground app; the screenshot overlay opens, copies an image
and closes; Perch **never takes the foreground**; idle private bytes
**29.0 MB** (< 40 MB) after all of that.

Evidence: `artifacts/windows/smoke-results.json`, `perch-collapsed.png`,
`perch-expanded.png`.

### `scripts/smoke-media-windows.ps1`: 5/5 passed

Re-run after the SMTC fix, alternating the pre-fix and fixed binaries (4 runs):
identical results. The "player quit" check now asserts that the quit
player's track disappears (Perch moves on to Spotify if it is running)
rather than that nothing is playing.

A test SMTC session (`mock_smtc` example) publishes a track. Perch shows
"Perch Test Band - Perch Smoke Song"; play/pause goes out through SMTC and the
new state comes back; quitting the player clears the track.

### Opt-in hardware tests

| Test | Result |
|---|---|
| `cargo test -p perch --test ocr` (Windows.Media.Ocr) | read "PERCH READS 42 WORDS" exactly |
| `cargo test -p perch --test audio -- --ignored` | passed in Phase 4 (device opens/closes, WASAPI mixer finds and sets its own stream); the mixer also set and restored Spotify's volume (0.97 → 0.60 → 0.97) during the bug hunt |
| `cargo test -p perch-platform-windows --test smtc_sta -- --ignored` | passes (start returns in ~40 µs); with `PERCH_SMTC_NO_MOCK=1` and Spotify current it **blocked > 5 s before the fix** |
| `cargo test -p perch --test camera -- --ignored` | **not run**: it switches the webcam (Logitech C310) on; run it yourself |
| dictation (`--features speech`) | not built on this machine: needs MSVC (see README); built by CI on `windows-latest` |

### `scripts/soak-windows.ps1`: 30 minutes

PASS, run on the dev machine **while it was in use** (Spotify and Chrome
playing, app switches, copies; the mixer and lyrics were also turned on from
the settings window 22 minutes in). Features on: media, to-do, Pomodoro,
clipboard, shelf, reminders, screen time. Samples every 10 s, the notch
opened and closed through the CLI every 5 minutes.

| Metric (median of first 5 min after warm-up → last 5 min) | Start | End | Growth |
|---|---|---|---|
| Private bytes | 30.95 MB | 30.83 MB | −0.12 MB |
| Handles | 421 | 435 | +14 |
| GDI objects | 18 | 18 | 0 |
| USER objects | 32 | 30 | −2 |

CPU at rest (median 10 s interval): 0.000 % of a core. Average including
reacting to the user's activity: 0.55 % of one core (0.035 % of the machine).
Peak private bytes 49 MB while the settings window was open; back to ~31 MB
after it closed. The run's first scoring, done with naive first-sample →
last-sample deltas, "failed" on warm-up (24 → 31 MB as clipboard history and
screen time filled); that is why the scoring now skips warm-up (section 4).
Evidence: `artifacts/windows/soak-windows.{csv,md}`.


### Packaging

| Check | Result |
|---|---|
| `perch.exe` carries the icon + version resource | `ProductName=Perch`, `FileVersion=0.1.0`, `OriginalFilename=perch.exe` |
| `scripts/package-windows.ps1` | `perch-0.1.0-windows-x64-setup.exe` (3.2 MB) and `-portable.zip` (4.0 MB), SHA-256 printed |
| Silent per-user install (`setup.exe /S /D=...`) | files, Add/Remove Programs entry (HKCU), Start-menu shortcut created; installed `perch.exe --version` runs |
| Silent uninstall | folder, uninstall key and shortcut removed; `%APPDATA%perch` kept by design |
| Portable zip: unzip, `perch.exe --version` | runs; contains perch.exe, README, README-portable.txt, licenses |

All of it: `scripts/test-installer-windows.ps1`, 11/11 passed.

## 3. Linux (container)

`scripts/linux-ci.sh all` = `check test build smoke mpris pulse mail deb debinstall`.

### X11 smoke (`scripts/smoke-x11.sh`): 32/32 passed, without and with picom

Checks: the pill and the hover strip are `_NET_WM_WINDOW_TYPE_DOCK` with
`ABOVE + STICKY + SKIP_TASKBAR + SKIP_PAGER` and no strut; the collapsed pill is at
the top centre; `xdotool` hover expands to 480×200 and
leaving collapses; CLI over the Unix socket (toggle, todo, tab); **MPRIS
end to end** (mock player → D-Bus → Perch, paused with `playerctl`, resumed
with `perch play-pause`); clipboard records `xclip` copies and **skips a
KeePassXC-style `x-kde-passwordManagerHint` owner**; the screenshot overlay
opens, a dragged region is copied as `image/png`; EWMH fullscreen hides and
un-fullscreen shows; screen time records apps; **`openbox --replace` (a WM
restart) keeps `_NET_WM_STATE_ABOVE`**; idle RSS < 40 MB and 0 CPU ticks in
10 s; `perch quit` exits cleanly.

Measured: first frame 60–66 ms; idle RSS 36 MB after the whole run.

Evidence: `artifacts/linux/x11-smoke-{none,picom}.txt` and cropped
`x11-*-{none,picom}.png`.

### Other Linux stages

| Stage | Result |
|---|---|
| `mpris` (D-Bus tests on a private bus) | passed |
| `pulse` (per-app volume vs. PulseAudio null sink) | passed |
| dictation end to end (`--features speech`, espeak-ng voice → whisper tiny, checksum-verified) | passed (needs ≥ 2 of 4 key words; tiny + synthetic voice is imperfect by nature) |
| OCR (tesseract) | read "PERCH READS 42 WORDS" exactly |
| `build`: binary size / glibc floor | 11.8 MB / GLIBC_2.34 |
| `deb`: `cargo deb`, tarball, SHA256SUMS | `perch_0.1.0-1_amd64.deb` (4.2 MB; Depends libasound2, libc6 >= 2.34, libpulse0; Recommends tesseract-ocr), tarball (5.5 MB), SHA256SUMS |
| `debinstall`: `apt install ./perch_*.deb` in a clean container, `perch --version`, menu entry and icon present, `apt remove` | passed; the tarball's `install.sh` installs to ~/.local and `--uninstall` leaves nothing behind |

### `scripts/soak-linux.sh`: 30 minutes

PASS (ubuntu-22.04 container, Xvfb + openbox, same feature set, 30 min).

| Metric (median of first 5 min after warm-up → last 5 min) | Start | End | Growth |
|---|---|---|---|
| VmRSS | 34.0 MB | 35.1 MB | +1.1 MB |
| Open fds | 23 | 23 | 0 |
| Threads | 7 | 7 | 0 |

CPU at rest: 0.000 % of a core; average 0.070 % of one core. (The script
itself crashed at the very end because it was edited while running; the CSV
was complete and re-scored with `--summarize`.) Evidence:
`artifacts/linux/soak-linux.{csv,md}`.

## 3b. Mail (read-only IMAP)

No test signs into a real account. They run against a scripted in-process
IMAP server and a throwaway **Dovecot** (the server Spacemail runs) in the
Linux container.

| Test | Where | Result |
|---|---|---|
| core `mail` unit tests: new-mail detection (first snapshot quiet, UIDVALIDITY reset), RFC 2047 subjects (UTF-8 B/Q, Latin-1, adjacent words), base64, sender display, INTERNALDATE, presets | both | 8 passed |
| `tests/mail_client.rs` (client vs. scripted server): sign-in, EXAMINE, decoded envelopes, **IDLE push**, never `SELECT`/`STORE`/body fetches, wrong password **reported and not retried**, missing password, polling fallback without IDLE, plaintext to a remote host refused | both | 5 passed |
| `tests/model.rs` mail: tab, "N unread" pill, notifications only for new mail, subject-free notifications, problems listed, turning the feature off clears it | both | 3 passed |
| forms: add-account dialog round trip, validation, presets fill server/port, masked password field | both | 2 passed (+1 mask test in `perch-ui`) |
| keychain round trip, Windows Credential Manager (`platform-windows` unit test, throwaway `Perch/test:*` item) | Windows | passed |
| keychain round trip, Secret Service (`platform-linux --test secrets`, throwaway GNOME Keyring on a private bus) | container | passed |
| `scripts/mail-test.sh` (Perch release build vs. Dovecot, under Xvfb, password via `perch mail-password` into GNOME Keyring) | container | **17/17**: password in keyring and not in config; online over **TLS** and **STARTTLS** (private CA via `ca_file`); 4 unread listed; `Jürgen - Halo 😀` decoded; new message **pushed in ~620 ms**; **0 of 3 messages `\Seen` on the server**; wrong password reported, exactly one attempt per account; feature off closes all connections; add-account dialog opened with `perch mail-add`, **typed into with xdotool**, account saved, typed password in keyring only, dialog closed, account online; RSS 36 MB |
| `scripts/mail-test-windows.ps1` (`perch.exe` vs. the same Dovecot on `localhost:1993`) | Windows | **10/10**: password in Credential Manager and not in config; online over TLS; 2 unread; subjects decoded; new message pushed in ~650 ms; 0 of 3 `\Seen`; 26.9 MB private; quits cleanly; test credential removed |

Bugs these runs found (fixed, with tests):

- `perch mail-password` stored a **UTF-8 byte-order mark** as part of the
  password when it was piped from Windows PowerShell 5.1 (bytes `ef bb bf`
  read back from Credential Manager), so sign-in failed. The line is now
  cleaned (`cli::password_from_line`).
- The masked password field drew `{2022}` instead of `•` (a mangled escape),
  caught on the dialog screenshot. Now `form::mask`, with a test.

## 4. How the soak is scored

Both soak scripts sample every 10 s and open/close the notch through the CLI
every 5 minutes. The first 2 minutes are a warm-up and don't count, because
features fill their caches then (clipboard history, screen-time rows, glyph
cache). Leak checks compare the **median of the first 5 minutes after warm-up**
with the **median of the last 5 minutes**:

- Windows: private bytes +5 MB, handles +50, GDI/USER objects +20.
- Linux: RSS +5 MB, open fds +20.

CPU is gated on the **median 10-second interval** (≤ 0.5 % of one core),
because a busy loop raises every interval. The **average** (≤ 2 %) also counts
the real work of reacting to whoever is using the machine. The Windows soak ran
on the dev machine while it was in use (app switches, copies, hotkey presses
show up in its log), not on an idle desktop. To re-score a run:
`soak-windows.ps1 -SummarizeFrom file.csv` or
`soak-linux.sh --summarize file.csv`.

## 5. CI

`.github/workflows/ci.yml`:

- **windows** (matrix): `x86_64-pc-windows-msvc` with `--all-features`
  (also builds `perch-speech.exe`) and `x86_64-pc-windows-gnu` with default
  features: fmt, clippy `-D warnings`, tests, release build, the SMTC smoke test.
- **linux** (`ubuntu-22.04`): installs the same packages as the Dockerfile,
  then `scripts/linux-ci.sh all`. Uploads the `.deb`, tarball and evidence.
- **audit**: `cargo audit`.

The repository has no remote yet, so the workflow hasn't run on GitHub. Its
Linux half is exactly the container run above. The Windows MSVC leg
(dictation) is the only part not exercised anywhere yet.

## Real Linux Mint

Run on a Mint Cinnamon (X11) machine:

```bash
./scripts/mint-setup.sh --with-optional
```

```bash
./scripts/mint-test.sh
```

That covers environment detection, fmt/clippy/tests, release build, D-Bus
tests, the X11 smoke test against the real desktop (`--real-display`), a real
media player if one is playing, the tray watcher, OCR and a 30-minute soak. It
writes `artifacts/mint/mint-results.md`. Paste that file here, then mark the
README rows that passed as "verified on real Mint".

*No results yet.*
