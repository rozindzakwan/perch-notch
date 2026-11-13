//! Tray icon: StatusNotifierItem (KDE/freedesktop spec, over D-Bus via
//! `ksni`), which Linux Mint supports through `xapp-sn-watcher`. When no SNI
//! watcher/host exists, falls back to a minimal XEmbed system-tray icon
//! (`_NET_SYSTEM_TRAY_S0`): left click toggles the notch, right click opens
//! settings.
//!
//! Menu choices are delivered as [`PlatformEvent::Command`] verbs, the same
//! vocabulary as the CLI.

use perch_core::error::{Error, Result};
use perch_core::platform::{EventSink, PlatformEvent};

struct PerchTray {
    sink: EventSink,
    autostart: bool,
    icon: (Vec<u8>, u32),
}

impl PerchTray {
    fn send(&self, verb: &str) {
        (self.sink)(PlatformEvent::Command(verb.to_string()));
    }
}

impl ksni::Tray for PerchTray {
    fn id(&self) -> String {
        "perch".into()
    }

    fn title(&self) -> String {
        "Perch".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let (data, side) = &self.icon;
        if *side == 0 {
            return Vec::new();
        }
        vec![ksni::Icon {
            width: *side as i32,
            height: *side as i32,
            data: data.clone(),
        }]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send("toggle-notch");
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, StandardItem};
        vec![
            StandardItem {
                label: "Show / hide notch".into(),
                activate: Box::new(|t: &mut Self| t.send("toggle-notch")),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Pin open".into(),
                activate: Box::new(|t: &mut Self| t.send("pin")),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Settings\u{2026}".into(),
                activate: Box::new(|t: &mut Self| t.send("settings")),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            CheckmarkItem {
                label: "Start at login".into(),
                checked: self.autostart,
                activate: Box::new(|t: &mut Self| t.send("autostart-toggle")),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit Perch".into(),
                activate: Box::new(|t: &mut Self| t.send("quit")),
                ..Default::default()
            }
            .into(),
        ]
    }
}

enum Backend {
    Sni(ksni::blocking::Handle<PerchTray>),
    // Held only to keep the tray thread alive.
    XEmbed { _tray: XEmbedTray },
}

/// The live tray icon, whichever protocol it uses.
pub struct TrayHandle {
    backend: Backend,
}

impl TrayHandle {
    /// Start the tray. `icon` is ARGB32 (network byte order) with its side.
    pub fn start(sink: EventSink, autostart: bool, icon: (Vec<u8>, u32)) -> Result<Self> {
        use ksni::blocking::TrayMethods;
        let tray = PerchTray {
            sink: sink.clone(),
            autostart,
            icon: icon.clone(),
        };
        match tray.spawn() {
            Ok(h) => {
                log::info!("tray: StatusNotifierItem");
                Ok(Self {
                    backend: Backend::Sni(h),
                })
            }
            Err(e) => {
                log::info!("no StatusNotifierItem host ({e}); trying the XEmbed system tray");
                let x = XEmbedTray::start(sink, icon)?;
                Ok(Self {
                    backend: Backend::XEmbed { _tray: x },
                })
            }
        }
    }

    /// Reflect the autostart state in the menu.
    pub fn set_autostart(&self, on: bool) {
        if let Backend::Sni(h) = &self.backend {
            h.update(|t| t.autostart = on);
        }
    }

    /// Which protocol is in use (for logs and tests).
    pub fn protocol(&self) -> &'static str {
        match self.backend {
            Backend::Sni(_) => "sni",
            Backend::XEmbed { .. } => "xembed",
        }
    }
}

/// Minimal XEmbed tray icon (freedesktop System Tray spec).
struct XEmbedTray {
    _join: std::thread::JoinHandle<()>,
}

impl XEmbedTray {
    fn start(sink: EventSink, icon: (Vec<u8>, u32)) -> Result<Self> {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{
            ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode, WindowClass,
        };
        use x11rb::wrapper::ConnectionExt as _;

        let (conn, screen_num) = crate::x11::connect()?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| Error::backend("no such X screen"))?
            .clone();
        let sel = crate::x11::intern(&conn, &format!("_NET_SYSTEM_TRAY_S{screen_num}"))?;
        let owner = conn
            .get_selection_owner(sel)
            .map_err(|e| Error::backend(e.to_string()))?
            .reply()
            .map_err(|e| Error::backend(e.to_string()))?
            .owner;
        if owner == x11rb::NONE {
            return Err(Error::unavailable(
                "tray",
                "no StatusNotifierItem host and no XEmbed system tray",
            ));
        }
        let opcode = crate::x11::intern(&conn, "_NET_SYSTEM_TRAY_OPCODE")?;
        let xembed_info = crate::x11::intern(&conn, "_XEMBED_INFO")?;

        let side: u16 = 22;
        let win = conn
            .generate_id()
            .map_err(|e| Error::backend(e.to_string()))?;
        conn.create_window(
            screen.root_depth,
            win,
            screen.root,
            0,
            0,
            side,
            side,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .background_pixel(0x0016171c)
                .event_mask(
                    EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::STRUCTURE_NOTIFY,
                ),
        )
        .map_err(|e| Error::backend(format!("tray window: {e}")))?;
        // _XEMBED_INFO: version 0, flags XEMBED_MAPPED.
        conn.change_property32(PropMode::REPLACE, win, xembed_info, xembed_info, &[0, 1])
            .map_err(|e| Error::backend(e.to_string()))?;
        // SYSTEM_TRAY_REQUEST_DOCK = 0.
        let ev = ClientMessageEvent::new(32, owner, opcode, [x11rb::CURRENT_TIME, 0, win, 0, 0]);
        conn.send_event(false, owner, EventMask::NO_EVENT, ev)
            .map_err(|e| Error::backend(e.to_string()))?;
        conn.flush().map_err(|e| Error::backend(e.to_string()))?;

        // Pre-scale the icon to the tray slot as 32-bit BGRX for PutImage.
        let pixels = scale_icon_bgrx(&icon.0, icon.1, u32::from(side));
        let join = std::thread::Builder::new()
            .name("perch-xembed-tray".into())
            .spawn(move || {
                use x11rb::protocol::xproto::{ButtonPressEvent, ImageFormat};
                use x11rb::protocol::Event;
                let gc = match conn.generate_id() {
                    Ok(id) => id,
                    Err(_) => return,
                };
                let _ = conn.create_gc(gc, win, &Default::default());
                loop {
                    match conn.wait_for_event() {
                        Ok(Event::Expose(_)) => {
                            if screen.root_depth >= 24 {
                                let _ = conn.put_image(
                                    ImageFormat::Z_PIXMAP,
                                    win,
                                    gc,
                                    side,
                                    side,
                                    0,
                                    0,
                                    0,
                                    screen.root_depth,
                                    &pixels,
                                );
                                let _ = conn.flush();
                            }
                        }
                        Ok(Event::ButtonPress(ButtonPressEvent { detail, .. })) => {
                            let verb = if detail == 3 {
                                "settings"
                            } else {
                                "toggle-notch"
                            };
                            sink(PlatformEvent::Command(verb.into()));
                        }
                        Ok(Event::DestroyNotify(_)) => return,
                        Ok(_) => {}
                        Err(e) => {
                            log::warn!("XEmbed tray stopped: {e}");
                            return;
                        }
                    }
                }
            })
            .map_err(|e| Error::io("spawning the tray thread", e))?;
        log::info!("tray: XEmbed system tray");
        Ok(Self { _join: join })
    }
}

/// Nearest-neighbour scale of an ARGB32 (network order) icon into BGRX rows,
/// blending over the tray background colour. Pure.
pub fn scale_icon_bgrx(argb: &[u8], src: u32, dst: u32) -> Vec<u8> {
    let bg = [0x1c_u8, 0x17, 0x16]; // B, G, R of the background pixel
    let mut out = Vec::with_capacity((dst * dst * 4) as usize);
    for y in 0..dst {
        for x in 0..dst {
            let (sx, sy) = if src == 0 {
                (0, 0)
            } else {
                (x * src / dst.max(1), y * src / dst.max(1))
            };
            let i = ((sy * src + sx) * 4) as usize;
            let (a, r, g, b) = match argb.get(i..i + 4) {
                Some(p) => (p[0], p[1], p[2], p[3]),
                None => (0, 0, 0, 0),
            };
            let blend = |c: u8, bgc: u8| -> u8 {
                ((u16::from(c) * u16::from(a) + u16::from(bgc) * (255 - u16::from(a))) / 255) as u8
            };
            out.extend_from_slice(&[blend(b, bg[0]), blend(g, bg[1]), blend(r, bg[2]), 0]);
        }
    }
    out
}
