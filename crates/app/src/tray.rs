//! Windows tray icon (`tray-icon`). Linux uses StatusNotifierItem from
//! `perch-platform-linux` instead, so GTK never enters the Linux build.
//!
//! Menu clicks are delivered as [`PlatformEvent::Command`] verbs, the same
//! vocabulary as the CLI, so the tray, the CLI and hotkeys share one path.

use perch_core::error::{Error, Result};
use perch_core::platform::{EventSink, PlatformEvent};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// The live tray icon.
pub struct Tray {
    _icon: TrayIcon,
    autostart: CheckMenuItem,
}

impl Tray {
    /// Create the icon and its menu.
    pub fn start(sink: EventSink, autostart: bool) -> Result<Self> {
        let menu = Menu::new();
        let toggle = MenuItem::with_id("toggle-notch", "Show / hide notch", true, None);
        let pin = MenuItem::with_id("pin", "Pin open", true, None);
        let settings = MenuItem::with_id("settings", "Settings\u{2026}", true, None);
        let auto =
            CheckMenuItem::with_id("autostart-toggle", "Start at login", true, autostart, None);
        let quit = MenuItem::with_id("quit", "Quit Perch", true, None);
        menu.append_items(&[
            &toggle,
            &pin,
            &settings,
            &PredefinedMenuItem::separator(),
            &auto,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| Error::backend(format!("tray menu: {e}")))?;

        let (rgba, side) = crate::icon::icon_rgba(32);
        let icon = tray_icon::Icon::from_rgba(rgba, side, side)
            .map_err(|e| Error::backend(format!("tray icon image: {e}")))?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Perch")
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| Error::backend(format!("tray icon: {e}")))?;

        let menu_sink = sink.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            menu_sink(PlatformEvent::Command(e.id().0.clone()));
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                sink(PlatformEvent::Command("toggle-notch".into()));
            }
        }));
        Ok(Self {
            _icon: tray,
            autostart: auto,
        })
    }

    /// Reflect the autostart state in the menu.
    pub fn set_autostart(&self, on: bool) {
        self.autostart.set_checked(on);
    }
}
