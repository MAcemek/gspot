//! Window creation and placement: the panel (docked left by default), the
//! floating G button, and the click-through underline layer.

use crate::ax::Rect;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};

pub const PANEL_W: f64 = 440.0;
pub const PANEL_H: f64 = 740.0;
/// Hover card window: initial size, until the card reports its own.
pub const CARD_W: f64 = 340.0;
pub const CARD_H: f64 = 220.0;
const MARGIN: f64 = 12.0;
/// Edge tab window: the tab shape is 40 pt wide and nudges out on hover, so the
/// window leaves 8 pt of transparent room for that.
pub const TAB_W: f64 = 48.0;
pub const TAB_H: f64 = 96.0;
const SLIDE_STEPS: u32 = 16;
const SLIDE_STEP_MS: u64 = 16;

/// True while the panel is slid away behind the edge tab.
static TUCKED: AtomicBool = AtomicBool::new(true);
/// Generation counter so a newer tuck/untuck cancels a slide still in flight.
static ANIM_GEN: AtomicU64 = AtomicU64::new(0);
/// Where the panel was when it tucked, so untuck brings it back there.
static LAST_POS: Mutex<Option<(f64, f64)>> = Mutex::new(None);

pub fn is_tucked() -> bool {
    TUCKED.load(Ordering::SeqCst)
}

pub fn primary_logical(app: &AppHandle) -> (f64, f64, f64, f64) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let s = m.scale_factor();
        let size = m.size();
        let pos = m.position();
        return (pos.x as f64 / s, pos.y as f64 / s, size.width as f64 / s, size.height as f64 / s);
    }
    (0.0, 0.0, 1440.0, 900.0)
}

pub fn panel_rect(app: &AppHandle, side: &str) -> (f64, f64, f64, f64) {
    let (mx, my, mw, mh) = primary_logical(app);
    let h = PANEL_H.min(mh - 80.0).max(520.0);
    let y = my + 40.0;
    let x = if side == "right" { mx + mw - PANEL_W - MARGIN } else { mx + MARGIN };
    (x, y, PANEL_W, h)
}

/// The edge tab sits flush against the screen edge on the dock side, a bit above centre.
pub fn tab_rect(app: &AppHandle, side: &str) -> (f64, f64, f64, f64) {
    let (mx, my, mw, mh) = primary_logical(app);
    let x = if side == "right" { mx + mw - TAB_W } else { mx };
    let y = my + ((mh - TAB_H) * 0.42).max(60.0);
    (x, y, TAB_W, TAB_H)
}

fn offscreen_x(app: &AppHandle, side: &str) -> f64 {
    let (mx, _, mw, _) = primary_logical(app);
    if side == "right" { mx + mw + 24.0 } else { mx - PANEL_W - 24.0 }
}

fn logical_pos(w: &tauri::WebviewWindow) -> Option<(f64, f64)> {
    let s = w.scale_factor().unwrap_or(1.0);
    w.outer_position().ok().map(|p| (p.x as f64 / s, p.y as f64 / s))
}

fn slide(w: &tauri::WebviewWindow, from: f64, to: f64, y: f64, gen: u64) -> bool {
    for i in 1..=SLIDE_STEPS {
        if ANIM_GEN.load(Ordering::SeqCst) != gen {
            return false;
        }
        let t = i as f64 / SLIDE_STEPS as f64;
        let eased = 1.0 - (1.0 - t).powi(3);
        let x = from + (to - from) * eased;
        let _ = w.set_position(tauri::Position::Logical(LogicalPosition::new(x, y)));
        std::thread::sleep(Duration::from_millis(SLIDE_STEP_MS));
    }
    true
}

/// Slide the panel away behind the edge tab (`tucked = true`) or out from the
/// edge (`tucked = false`). Runs on its own thread so the caller never blocks;
/// a newer call cancels an older slide.
pub fn set_tucked(app: &AppHandle, side: &str, tucked: bool, animate: bool, focus: bool) {
    let gen = ANIM_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    TUCKED.store(tucked, Ordering::SeqCst);
    let app = app.clone();
    let side = side.to_string();
    std::thread::spawn(move || {
        let panel = app.get_webview_window("panel");
        let tab = app.get_webview_window("tab");
        if tucked {
            if let Some(p) = &panel {
                let visible = p.is_visible().unwrap_or(false);
                if visible {
                    if let Some((x, y)) = logical_pos(p) {
                        *LAST_POS.lock().unwrap() = Some((x, y));
                        if animate && !slide(p, x, offscreen_x(&app, &side), y, gen) {
                            return;
                        }
                    }
                }
                let _ = p.hide();
            }
            if let Some(t) = &tab {
                let (x, y, _, _) = tab_rect(&app, &side);
                let _ = t.set_position(tauri::Position::Logical(LogicalPosition::new(x, y)));
                let _ = t.show();
            }
        } else {
            if let Some(t) = &tab {
                let _ = t.hide();
            }
            if let Some(p) = &panel {
                let (dx, dy, _, _) = panel_rect(&app, &side);
                let (tx, ty) = LAST_POS.lock().unwrap().unwrap_or((dx, dy));
                if animate && !p.is_visible().unwrap_or(false) {
                    let from = offscreen_x(&app, &side);
                    let _ = p.set_position(tauri::Position::Logical(LogicalPosition::new(from, ty)));
                    let _ = p.show();
                    if !slide(p, from, tx, ty, gen) {
                        return;
                    }
                }
                let _ = p.set_position(tauri::Position::Logical(LogicalPosition::new(tx, ty)));
                if !p.is_visible().unwrap_or(false) {
                    let _ = p.show();
                }
                if focus {
                    let _ = p.set_focus();
                }
            }
        }
        let _ = app.emit_to("panel", "panel://tucked", json!({"tucked": tucked}));
    });
}

/// Snapshot for the debug probe and tests.
pub fn panel_state(app: &AppHandle) -> serde_json::Value {
    let (visible, x, y) = app
        .get_webview_window("panel")
        .map(|w| {
            let (x, y) = logical_pos(&w).unwrap_or((0.0, 0.0));
            (w.is_visible().unwrap_or(false), x, y)
        })
        .unwrap_or((false, 0.0, 0.0));
    let tab_visible = app.get_webview_window("tab").map(|w| w.is_visible().unwrap_or(false)).unwrap_or(false);
    json!({"tucked": is_tucked(), "visible": visible, "x": x, "y": y, "tab_visible": tab_visible})
}

pub fn dock_panel(app: &AppHandle, side: &str) {
    *LAST_POS.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window("panel") {
        let (x, y, pw, ph) = panel_rect(app, side);
        let _ = w.set_size(tauri::Size::Logical(LogicalSize::new(pw, ph)));
        let _ = w.set_position(tauri::Position::Logical(LogicalPosition::new(x, y)));
    }
    if let Some(t) = app.get_webview_window("tab") {
        let (x, y, _, _) = tab_rect(app, side);
        let _ = t.set_position(tauri::Position::Logical(LogicalPosition::new(x, y)));
    }
}

pub fn create_windows(app: &AppHandle, side: &str) -> tauri::Result<()> {
    let (x, y, pw, ph) = panel_rect(app, side);
    WebviewWindowBuilder::new(app, "panel", WebviewUrl::App("index.html".into()))
        .title("GSpot")
        .inner_size(pw, ph)
        .min_inner_size(380.0, 520.0)
        .position(x, y)
        .always_on_top(true)
        .resizable(true)
        .theme(Some(tauri::Theme::Dark))
        .visible(false)
        .build()?;

    let (tx, ty, tw, th) = tab_rect(app, side);
    let t = WebviewWindowBuilder::new(app, "tab", WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(tw, th)
        .position(tx, ty)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        .visible_on_all_workspaces(true)
        .visible(false)
        .build()?;
    let _ = t;

    let g = WebviewWindowBuilder::new(app, "gbutton", WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(60.0, 60.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        .visible_on_all_workspaces(true)
        .visible(false)
        .build()?;
    let _ = g;

    let u = WebviewWindowBuilder::new(app, "underlines", WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(10.0, 10.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        .visible_on_all_workspaces(true)
        .visible(false)
        .build()?;
    // Permanently click-through: the hover card lives in its own window.
    let _ = u.set_ignore_cursor_events(true);

    let c = WebviewWindowBuilder::new(app, "card", WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(CARD_W, CARD_H)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        .visible_on_all_workspaces(true)
        .visible(false)
        .build()?;
    let _ = c;
    Ok(())
}

/// The screen (logical coords) containing the point, else the primary one.
pub fn screen_at(app: &AppHandle, px: f64, py: f64) -> Rect {
    if let Ok(ms) = app.available_monitors() {
        for m in ms {
            let s = m.scale_factor();
            let (p, z) = (m.position(), m.size());
            let r = Rect { x: p.x as f64 / s, y: p.y as f64 / s, w: z.width as f64 / s, h: z.height as f64 / s };
            if r.contains(px, py) {
                return r;
            }
        }
    }
    let (x, y, w, h) = primary_logical(app);
    Rect { x, y, w, h }
}

/// Top-left of the hover card for a word at `anchor`: below it, or above when
/// there is no room, always kept on `screen` (all logical screen coordinates).
pub fn place_card(anchor: Rect, size: (f64, f64), screen: Rect) -> (f64, f64) {
    let x = anchor.x.min(screen.x + screen.w - size.0 - 4.0).max(screen.x + 4.0);
    let mut y = anchor.y + anchor.h + 6.0;
    if y + size.1 > screen.y + screen.h - 4.0 {
        y = anchor.y - 6.0 - size.1;
    }
    let y = y.min(screen.y + screen.h - size.1 - 4.0).max(screen.y + 4.0);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };
    const SIZE: (f64, f64) = (340.0, 220.0);

    fn word(x: f64, y: f64) -> Rect {
        Rect { x, y, w: 40.0, h: 16.0 }
    }

    #[test]
    fn card_goes_below_the_word() {
        assert_eq!(place_card(word(100.0, 200.0), SIZE, SCREEN), (100.0, 222.0));
    }

    #[test]
    fn card_flips_above_near_the_bottom() {
        assert_eq!(place_card(word(100.0, 700.0), SIZE, SCREEN), (100.0, 700.0 - 6.0 - 220.0));
    }

    #[test]
    fn card_is_clamped_at_the_right_edge() {
        assert_eq!(place_card(word(900.0, 200.0), SIZE, SCREEN).0, 1000.0 - 340.0 - 4.0);
    }

    #[test]
    fn card_is_clamped_at_the_left_edge() {
        let screen = Rect { x: 50.0, ..SCREEN };
        assert_eq!(place_card(word(10.0, 200.0), SIZE, screen).0, 54.0);
    }

    #[test]
    fn a_tall_card_on_a_short_screen_stays_on_screen() {
        let screen = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 200.0 };
        let (_, y) = place_card(word(100.0, 100.0), (340.0, 300.0), screen);
        assert_eq!(y, 4.0);
        let (_, y) = place_card(word(100.0, 100.0), SIZE, screen);
        assert_eq!(y, 4.0, "the top edge wins when it cannot fit");
    }
}
