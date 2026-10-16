//! Tray icon and menu: open the main, settings or history window, or quit.
//!
//! Ported from Filee's `src/Filee.App/Services/TrayService.cs`, minus the
//! entries for features MorphRing does not have yet (pausing gestures and
//! self-updates).

use tauri::menu::{MenuBuilder, MenuEvent, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Runtime, WebviewWindowBuilder};

use crate::dropzone::MAIN_WINDOW;

/// Id of the one tray icon the app owns.
pub const TRAY_ID: &str = "main";

/// Tooltip shown when hovering the tray icon.
pub const TOOLTIP: &str = "MorphRing";

/// What a tray menu entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Show the main window, creating it again if it was closed.
    Open,
    /// Show the settings window.
    Settings,
    /// Show the history window.
    History,
    /// Exit the app.
    Quit,
}

impl TrayAction {
    /// Every entry, in menu order.
    pub const ALL: [TrayAction; 4] = [
        TrayAction::Open,
        TrayAction::Settings,
        TrayAction::History,
        TrayAction::Quit,
    ];

    /// Menu item id of this entry.
    pub fn id(self) -> &'static str {
        match self {
            TrayAction::Open => "open",
            TrayAction::Settings => "settings",
            TrayAction::History => "history",
            TrayAction::Quit => "quit",
        }
    }

    /// Menu label of this entry.
    pub fn label(self) -> &'static str {
        match self {
            TrayAction::Open => "Open MorphRing",
            TrayAction::Settings => "Settings",
            TrayAction::History => "History",
            TrayAction::Quit => "Quit",
        }
    }

    /// The entry with menu item id `id`, if it is one of ours.
    pub fn from_id(id: &str) -> Option<TrayAction> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Whether a separator goes right before this entry in the menu.
    fn separated(self) -> bool {
        // Filee puts a line above Quit so nobody quits while aiming for Open.
        matches!(self, TrayAction::Quit)
    }
}

/// Creates the tray icon. Called once from the app's setup hook.
///
/// A desktop without a tray is not an error: the app keeps running and a
/// second launch still brings the main window back.
pub fn setup<R: Runtime>(app: &mut App<R>) -> tauri::Result<()> {
    // GNOME without the AppIndicator extension has nowhere to put a tray icon. That
    // must not cost anyone their converter, so a failed tray is a warning, not a crash.
    if let Err(e) = build(app.handle()) {
        tracing::warn!("could not create the tray icon: {e}");
    }
    Ok(())
}

fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let mut menu = MenuBuilder::new(app);
    for action in TrayAction::ALL {
        if action.separated() {
            menu = menu.separator();
        }
        menu = menu.item(&MenuItemBuilder::with_id(action.id(), action.label()).build(app)?);
    }
    let menu = menu.build()?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(TOOLTIP)
        .menu(&menu)
        // Left click opens the window like Filee does, so the menu is right-click only.
        // On Linux the AppIndicator protocol never reports clicks at all and every
        // click opens the menu, which is why "Open MorphRing" is the first entry.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match TrayAction::from_id(event.id().as_ref()) {
        Some(TrayAction::Open) => show_main(app),
        Some(TrayAction::Settings) => crate::settings::open(app),
        Some(TrayAction::History) => crate::history::open(app),
        Some(TrayAction::Quit) => app.exit(0),
        None => tracing::debug!(id = ?event.id(), "unknown tray menu item"),
    }
}

fn on_tray_icon_event<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    // React on release, not press: on Windows the press is followed by a focus
    // change, and a window shown on press gets buried by it a moment later.
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_main(tray.app_handle());
    }
}

/// Brings the main window to the front, creating it again from the app config
/// when it was closed.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    show_window(app, MAIN_WINDOW);
}

/// Brings the window labelled `label` to the front, creating it again from
/// the app config when it was closed.
pub fn show_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let window = match app.get_webview_window(label) {
        Some(window) => window,
        None => match recreate(app, label) {
            Ok(window) => window,
            Err(e) => {
                tracing::warn!("could not open the {label} window: {e}");
                return;
            }
        },
    };
    // Each of these can fail on a compositor with opinions (hello, Wayland and
    // set_focus). None of the failures is worth more than a debug line.
    if let Err(e) = window.unminimize() {
        tracing::debug!("could not unminimize the {label} window: {e}");
    }
    if let Err(e) = window.show() {
        tracing::debug!("could not show the {label} window: {e}");
    }
    if let Err(e) = window.set_focus() {
        tracing::debug!("could not focus the {label} window: {e}");
    }
}

fn recreate<R: Runtime>(app: &AppHandle<R>, label: &str) -> tauri::Result<tauri::WebviewWindow<R>> {
    // Closing a window destroys it, but the hidden ring window keeps the app
    // alive, so the tray may be asked to open a window that no longer exists. The
    // config still knows what it looked like; build it again from there.
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == label)
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    WebviewWindowBuilder::from_config(app, &config)?.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn menu_ids_round_trip() {
        for action in TrayAction::ALL {
            assert_eq!(TrayAction::from_id(action.id()), Some(action));
        }
    }

    #[test]
    fn unknown_menu_id_is_none() {
        assert_eq!(TrayAction::from_id("pause"), None);
        assert_eq!(TrayAction::from_id(""), None);
        assert_eq!(TrayAction::from_id("OPEN"), None);
    }

    #[test]
    fn menu_ids_and_labels_are_unique() {
        let ids: HashSet<_> = TrayAction::ALL.iter().map(|a| a.id()).collect();
        let labels: HashSet<_> = TrayAction::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(ids.len(), TrayAction::ALL.len());
        assert_eq!(labels.len(), TrayAction::ALL.len());
    }

    #[test]
    fn open_comes_first_and_quit_last_behind_a_separator() {
        assert_eq!(TrayAction::ALL.first(), Some(&TrayAction::Open));
        assert_eq!(TrayAction::ALL.last(), Some(&TrayAction::Quit));
        assert!(TrayAction::Quit.separated());
        for action in [TrayAction::Open, TrayAction::Settings, TrayAction::History] {
            assert!(!action.separated(), "{action:?}");
        }
    }

    #[test]
    fn settings_and_history_sit_between_open_and_quit() {
        assert_eq!(
            TrayAction::ALL,
            [
                TrayAction::Open,
                TrayAction::Settings,
                TrayAction::History,
                TrayAction::Quit
            ]
        );
        assert_eq!(TrayAction::from_id("settings"), Some(TrayAction::Settings));
        assert_eq!(TrayAction::from_id("history"), Some(TrayAction::History));
    }
}
