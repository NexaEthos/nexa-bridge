//! Tray menu — the primary surface. See docs/wiki/desktop.md

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::supervisor::{Phase, Status};

pub const STATUS_ITEM: &str = "status";
pub const TOGGLE_ITEM: &str = "toggle";
pub const OPEN_ITEM: &str = "open";
pub const QUIT_ITEM: &str = "quit";

fn status_label(status: &Status) -> String {
    match status.phase {
        Phase::Stopped => "Not connected".to_string(),
        Phase::Starting => "Starting…".to_string(),
        Phase::Running => "Connected".to_string(),
        Phase::Failed => "Error — restart required".to_string(),
        Phase::Retrying => match &status.detail {
            Some(d) if d.len() <= 60 => format!("Reconnecting — {d}"),
            _ => "Reconnecting…".to_string(),
        },
    }
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let status = MenuItem::with_id(app, STATUS_ITEM, "Not connected", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, TOGGLE_ITEM, "Connect", true, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN_ITEM, "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ITEM, "Quit", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )
}

/// Rebuild the menu labels from the current status. Called on every status change.
pub fn refresh(app: &AppHandle, status: &Status) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let menu = match build(app) {
        Ok(menu) => menu,
        Err(error) => {
            tracing::error!(error = %error, "could not build tray menu");
            return;
        }
    };
    if let Some(item) = menu.get(STATUS_ITEM).and_then(|i| i.as_menuitem().cloned())
        && let Err(error) = item.set_text(status_label(status))
    {
        tracing::error!(error = %error, "could not update tray status");
    }
    if let Some(item) = menu.get(TOGGLE_ITEM).and_then(|i| i.as_menuitem().cloned()) {
        if let Err(error) = item.set_text(if status.phase == Phase::Stopped {
            "Connect"
        } else {
            "Disconnect"
        }) {
            tracing::error!(error = %error, "could not update tray connection action");
        }
        if let Err(error) = item.set_enabled(status.phase != Phase::Failed) {
            tracing::error!(error = %error, "could not update tray action availability");
        }
    }
    if let Err(error) = tray.set_menu(Some(menu)) {
        tracing::error!(error = %error, "could not replace tray menu");
    }
}

pub fn on_menu_event(app: &AppHandle, id: &str) {
    match id {
        TOGGLE_ITEM => crate::toggle(app),
        OPEN_ITEM => show_panel(app),
        QUIT_ITEM => {
            crate::supervisor::shutdown(app);
            app.exit(0);
        }
        _ => {}
    }
}

/// Left click opens the panel; right click is left to the menu.
pub fn on_tray_event(app: &AppHandle, event: &TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_panel(app);
    }
}

pub fn show_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("panel") {
        if let Err(error) = window.show() {
            tracing::error!(error = %error, "could not show settings panel");
        }
        if let Err(error) = window.set_focus() {
            tracing::error!(error = %error, "could not focus settings panel");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_reflects_each_phase() {
        let base = Status::default();
        assert_eq!(status_label(&base), "Not connected");
        assert_eq!(
            status_label(&Status {
                phase: Phase::Running,
                ..base.clone()
            }),
            "Connected"
        );
        // A short detail is shown inline; a long one is dropped rather than truncated mid-word.
        assert_eq!(
            status_label(&Status {
                phase: Phase::Retrying,
                detail: Some("connection refused".into()),
                ..base
            }),
            "Reconnecting — connection refused"
        );
        assert_eq!(
            status_label(&Status {
                phase: Phase::Retrying,
                detail: Some("x".repeat(200)),
                ..base
            }),
            "Reconnecting…"
        );
    }
}
