//! Global keyboard shortcut that opens or tucks the panel from any app.

use tauri::{AppHandle, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg(target_os = "macos")]
const MODS: Modifiers = Modifiers::SUPER.union(Modifiers::SHIFT);
#[cfg(not(target_os = "macos"))]
const MODS: Modifiers = Modifiers::CONTROL.union(Modifiers::SHIFT);

fn shortcut() -> Shortcut {
    Shortcut::new(Some(MODS), Code::KeyG)
}

fn accept_shortcut() -> Shortcut {
    Shortcut::new(Some(MODS), Code::KeyY)
}

/// Ctrl+Shift+G (Cmd+Shift+G on macOS): open the panel, or tuck it away when it is open.
/// Ctrl+Shift+Y (Cmd+Shift+Y): accept the suggestion under the cursor.
pub fn plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, pressed, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            if *pressed == shortcut() {
                toggle(app);
            } else if *pressed == accept_shortcut() {
                crate::commands::accept_at_cursor(app);
            }
        })
        .build()
}

/// Register the shortcuts; logs and carries on if another app already owns one.
pub fn register(app: &AppHandle) {
    if let Err(e) = app.global_shortcut().register(shortcut()) {
        log::warn!("Ctrl+Shift+G is taken by another app: {e}");
    }
    if let Err(e) = app.global_shortcut().register(accept_shortcut()) {
        log::warn!("Ctrl+Shift+Y is taken by another app: {e}");
    }
}

fn toggle(app: &AppHandle) {
    if crate::overlay::is_tucked() {
        crate::commands::open_panel_inner(app, "review", true);
    } else {
        crate::commands::tuck_panel_inner(app);
    }
}
