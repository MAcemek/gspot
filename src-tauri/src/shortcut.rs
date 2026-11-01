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

/// Ctrl+Shift+G (Cmd+Shift+G on macOS): open the panel, or tuck it away when it is open.
pub fn plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, pressed, event| {
            if event.state() == ShortcutState::Pressed && *pressed == shortcut() {
                toggle(app);
            }
        })
        .build()
}

/// Register the shortcut; logs and carries on if another app already owns it.
pub fn register(app: &AppHandle) {
    if let Err(e) = app.global_shortcut().register(shortcut()) {
        log::warn!("Ctrl+Shift+G is taken by another app: {e}");
    }
}

fn toggle(app: &AppHandle) {
    if crate::overlay::is_tucked() {
        crate::commands::open_panel_inner(app, "review", true);
    } else {
        crate::commands::tuck_panel_inner(app);
    }
}
