//! The host loop: watches the focused text field of whatever app is in front,
//! keeps the document model, runs the engine, and computes underline geometry.

#![cfg(any(target_os = "macos", windows))]

use crate::ax::{self, AXElement, Rect};
use crate::engine::{Alert, Engine};
use crate::score;
use crate::settings::Settings;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const OUR_BUNDLE: &str = "com.publik.grammarlama";

/// Apps whose text view is not a writing surface. A terminal shows its whole
/// transcript as one AXTextArea, so every line of it would be linted and
/// underlined (and the G button would sit on the shell).
pub const BUILT_IN_SKIP: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
    "dev.warp.Warp",
    "com.github.wez.wezterm",
    "org.alacritty",
    "io.alacritty",
    "net.kovidgoyal.kitty",
    "com.mitchellh.ghostty",
    "co.zeit.hyper",
    "com.apple.Console",
    // Windows: the app id is the executable name.
    "windowsterminal.exe",
    "openconsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "warp.exe",
    "mintty.exe",
];

pub fn built_in_skip(bundle_id: &str) -> bool {
    BUILT_IN_SKIP.contains(&bundle_id)
}

/// Stay out of this app: a terminal, or one the user added to the skip list.
pub fn skipped_app(bundle_id: &str, settings: &crate::settings::Settings) -> bool {
    !bundle_id.is_empty() && (built_in_skip(bundle_id) || settings.skip_apps.iter().any(|a| a.bundle_id == bundle_id))
}
const POLL: Duration = Duration::from_millis(100);
const LINT_DEBOUNCE: Duration = Duration::from_millis(150);
/// Typing pause before the model judges the changed sentences.
const SPOT_DEBOUNCE: Duration = Duration::from_millis(3500);
/// Longer documents are spot-checked on demand only (the Spot check button).
const SPOT_MAX_WORDS: usize = 6000;
const MAX_CHARS: usize = 80_000;
const GBUTTON: f64 = 48.0;
const GBUTTON_INSET: f64 = 14.0;

#[derive(Clone, Debug, Serialize)]
pub struct HostDoc {
    pub id: String,
    pub app_name: String,
    pub bundle_id: String,
    pub pid: i32,
    pub role: String,
    pub text: String,
    pub sel_start: usize,
    pub sel_end: usize,
    pub selected_text: String,
    pub frame: Rect,
    pub visible: Rect,
    pub version: u64,
    pub active: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Status {
    pub trusted: bool,
    pub frontmost: String,
    pub frontmost_bid: String,
    /// The app in front is one GSpot stays out of.
    pub skipped: bool,
    /// ... because it is a terminal (built in), not a user choice.
    pub skip_built_in: bool,
    pub has_doc: bool,
    pub linting: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct UnderlineRect {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub color: String,
    pub category: String,
    /// A whole-sentence (spot check) underline, drawn beneath word-level ones.
    #[serde(default)]
    pub sentence: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Geometry {
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub rects: Vec<UnderlineRect>,
    pub hovered: Option<String>,
    /// The pointer is over the open hover card (hit-tested here, not in the webview).
    #[serde(default)]
    pub over_card: bool,
}

#[derive(Default)]
pub struct HostState {
    pub doc: Option<HostDoc>,
    pub element: Option<AXElement>,
    pub alerts: Vec<Alert>,
    pub all_alerts: Vec<Alert>,
    /// Sentence-level suggestions from the spot check (LLM), kept across Harper re-lints.
    pub spot_alerts: Vec<Alert>,
    pub spot_cache: crate::features::spotcheck::Cache,
    pub spot_running: bool,
    pub spot_doc_id: String,
    pub spot_dirty: bool,
    pub spot_pending_since: Option<Instant>,
    /// Text hash the last automatic spot check was started for.
    pub spot_last_hash: u64,
    pub status: Status,
    pub geometry: Geometry,
    pub settings: Settings,
    pub history: Vec<crate::history::Session>,
    /// Extra hit rectangles (the hover card) reported by the overlay UI, in overlay-local coords.
    pub card_rects: Vec<Rect>,
    pub last_text_hash: u64,
    pub lint_pending_since: Option<Instant>,
    pub lint_dirty: bool,
    pub lost_focus_at: Option<Instant>,
    pub paused: bool,
    /// Window labels whose webview reported it mounted (test introspection).
    pub ui_ready: Vec<String>,
    pub ui_log: Vec<String>,
}

pub type Shared = Arc<Mutex<HostState>>;

impl HostState {
    /// The list the panel shows: Harper + rule alerts, then spot-check
    /// sentences, minus everything dismissed. Word-level alerts come first so
    /// a hover over a word inside a flagged sentence opens the word's card.
    pub fn visible_alerts(&self) -> Vec<Alert> {
        crate::features::spotcheck::merge_visible(&self.all_alerts, &self.spot_alerts, |h| self.settings.dismissed.contains(&h))
    }
}

fn hash_text(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn is_editable(el: &AXElement, role: &str) -> bool {
    if role == "AXSecureTextField" {
        return false;
    }
    if matches!(role, "AXTextArea" | "AXTextField" | "AXComboBox" | "AXSearchField") {
        return true;
    }
    if role == "AXWebArea" || role == "AXWindow" || role == "AXApplication" {
        return false;
    }
    el.has_attr("AXSelectedTextRange") && el.is_settable("AXValue")
}

fn visible_frame(el: &AXElement, frame: Rect) -> Rect {
    let mut vis = frame;
    if let Some(sa) = el.enclosing_scroll_area() {
        if let Some(f) = sa.frame() {
            if let Some(i) = vis.intersect(&f) {
                vis = i;
            }
        }
    }
    if let Some(w) = el.window() {
        if let Some(f) = w.frame() {
            if let Some(i) = vis.intersect(&f) {
                vis = i;
            }
        }
    }
    vis
}

/// Underline rects kept between ticks; recomputed when the text, alerts, frame or scroll change.
#[derive(Default)]
struct RectCache {
    key: u64,
    /// Bounds of the first alert's range, to notice scrolling.
    probe: Option<Rect>,
    rects: Vec<UnderlineRect>,
}

fn rect_close(a: &Option<Rect>, b: &Option<Rect>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a.x - b.x).abs() <= 0.5 && (a.y - b.y).abs() <= 0.5 && (a.w - b.w).abs() <= 0.5 && (a.h - b.h).abs() <= 0.5,
        _ => false,
    }
}

/// Which underline (and whether the card) the pointer is over; local coords relative to the geometry origin.
fn hit_test(g: &Geometry, card_rects: &[Rect], mx: f64, my: f64) -> (Option<String>, bool) {
    let lx = mx - g.origin_x;
    let ly = my - g.origin_y;
    let mut hovered = None;
    for r in &g.rects {
        if lx >= r.x - 2.0 && lx <= r.x + r.w + 2.0 && ly >= r.y - 2.0 && ly <= r.y + r.h + 4.0 {
            hovered = Some(r.id.clone());
            break;
        }
    }
    let over_card = card_rects.iter().any(|c| c.contains(lx, ly));
    (hovered, over_card)
}

/// Fast pointer loop: hover state is hit-tested every 30 ms against the last geometry,
/// independent of how long a slow accessibility tick takes.
fn spawn_hover(app: AppHandle, shared: Shared) {
    std::thread::Builder::new()
        .name("hover-loop".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(30));
            let Some((mx, my)) = ax::mouse_location_top_left() else { continue };
            let changed = {
                let Ok(mut st) = shared.lock() else { continue };
                if st.geometry.rects.is_empty() && !st.geometry.over_card && st.geometry.hovered.is_none() {
                    continue;
                }
                let hit = hit_test(&st.geometry, &st.card_rects, mx, my);
                if hit != (st.geometry.hovered.clone(), st.geometry.over_card) {
                    st.geometry.hovered = hit.0.clone();
                    st.geometry.over_card = hit.1;
                    Some((st.geometry.clone(), hit.0.is_some() || hit.1))
                } else {
                    None
                }
            };
            if let Some((geometry, interactive)) = changed {
                let _ = app.emit_to("underlines", "host://geometry", &geometry);
                if let Some(w) = app.get_webview_window("underlines") {
                    let _ = w.set_ignore_cursor_events(!interactive);
                }
            }
        })
        .expect("hover loop thread");
}

pub fn spawn(app: AppHandle, shared: Shared) {
    spawn_hover(app.clone(), shared.clone());
    std::thread::Builder::new()
        .name("host-loop".into())
        .spawn(move || {
            let mut engine = Engine::new();
            let system = AXElement::system_wide();
            let mut last_geom_sig = String::new();
            let mut last_status_sig = String::new();
            let mut cache = RectCache::default();
            loop {
                let t0 = Instant::now();
                tick(&app, &shared, &system, &mut engine, &mut last_geom_sig, &mut last_status_sig, &mut cache);
                let spent = t0.elapsed();
                if spent < POLL {
                    std::thread::sleep(POLL - spent);
                }
            }
        })
        .expect("host loop thread");
}

fn tick(app: &AppHandle, shared: &Shared, system: &AXElement, engine: &mut Engine, last_geom_sig: &mut String, last_status_sig: &mut String, cache: &mut RectCache) {
    let t_start = Instant::now();
    let mut read_ms = 0u128;
    let mut lint_ms = 0u128;
    let mut rects_ms = 0u128;
    let mut cached_rects = false;
    let trusted = ax::is_trusted();
    let front = ax::frontmost_app();
    let (front_pid, front_bid, front_name) = front.clone().unwrap_or((0, String::new(), String::new()));
    let ours = front_bid == OUR_BUNDLE;

    let (paused, skipped, built_in) = shared
        .lock()
        .map(|s| (s.paused, skipped_app(&front_bid, &s.settings), built_in_skip(&front_bid)))
        .unwrap_or((false, false, false));

    // 1. Resolve the focused editable element (unless we are in front: keep the last one).
    let mut focused: Option<(AXElement, String)> = None;
    if trusted && !ours && !paused && !skipped {
        if let Some(el) = system.focused_element() {
            let role = el.role().unwrap_or_default();
            if el.pid() != std::process::id() as i32 && is_editable(&el, &role) {
                focused = Some((el, role));
            }
        }
    }

    let mut doc_event: Option<HostDoc> = None;
    let mut alerts_event: Option<Vec<Alert>> = None;
    let mut sel_event: Option<(usize, usize, String)> = None;
    let mut clear_overlays = false;
    let mut doc_cleared = false;
    let mut spot_job: Option<crate::settings::LlmConfig> = None;

    {
        let mut st = shared.lock().unwrap();
        st.status.trusted = trusted;
        st.status.frontmost = front_name.clone();
        st.status.frontmost_bid = front_bid.clone();
        st.status.skipped = skipped;
        st.status.skip_built_in = built_in;
        // The app the document lives in was just added to the skip list: let it go.
        if st.doc.as_ref().map(|d| skipped_app(&d.bundle_id, &st.settings)).unwrap_or(false) {
            st.doc = None;
            st.element = None;
            st.alerts.clear();
            st.all_alerts.clear();
            st.spot_alerts.clear();
            st.spot_doc_id.clear();
            st.status.has_doc = false;
            clear_overlays = true;
            doc_cleared = true;
            alerts_event = Some(vec![]);
        }

        if let Some((el, role)) = focused {
            // Switched element?
            let same = st.element.as_ref().map(|e| e.same_as(&el)).unwrap_or(false);
            if !same {
                st.element = Some(el.clone());
                st.doc = None;
                st.alerts.clear();
                st.all_alerts.clear();
                st.spot_alerts.clear();
                st.spot_doc_id.clear();
                st.spot_dirty = false;
                st.spot_last_hash = 0;
                st.last_text_hash = 0;
                st.lint_dirty = false;
                alerts_event = Some(vec![]);
            }
            st.lost_focus_at = None;
            let text = el.string("AXValue").unwrap_or_default();
            let truncated = text.chars().count() > MAX_CHARS;
            let text: String = if truncated { text.chars().take(MAX_CHARS).collect() } else { text };
            let (sel_start, sel_len) = el.range("AXSelectedTextRange").unwrap_or((0, 0));
            let frame = el.frame().unwrap_or_default();
            let visible = visible_frame(&el, frame);
            read_ms = t_start.elapsed().as_millis();
            let h = hash_text(&text);
            let changed = h != st.last_text_hash;
            let selected_text = crate::textmap::slice_utf16(&text, sel_start, sel_start + sel_len);
            let doc_id = format!("{}-{}", el.pid(), el.identity_hash());
            let version = st.doc.as_ref().map(|d| d.version).unwrap_or(0) + if changed { 1 } else { 0 };
            let prev_sel = st.doc.as_ref().map(|d| (d.sel_start, d.sel_end));
            let doc = HostDoc {
                id: doc_id,
                app_name: front_name.clone(),
                bundle_id: front_bid.clone(),
                pid: front_pid,
                role,
                text: text.clone(),
                sel_start,
                sel_end: sel_start + sel_len,
                selected_text: selected_text.clone(),
                frame,
                visible,
                version,
                active: true,
                truncated,
            };
            if changed {
                st.last_text_hash = h;
                st.lint_pending_since = Some(Instant::now());
                st.lint_dirty = true;
                st.spot_dirty = true;
                st.spot_pending_since = Some(Instant::now());
                // Keep existing alerts only if they still match their original text (cheap
                // re-validation); spot-check sentences follow the text if they moved.
                let text_ref = &text;
                st.all_alerts.retain(|a| crate::textmap::slice_utf16(text_ref, a.u16_start, a.u16_end) == a.original);
                crate::features::spotcheck::re_anchor(&mut st.spot_alerts, text_ref);
                st.alerts = st.visible_alerts();
                doc_event = Some(doc.clone());
            } else if st.doc.is_none() {
                doc_event = Some(doc.clone());
            }
            if prev_sel != Some((sel_start, sel_start + sel_len)) {
                sel_event = Some((sel_start, sel_start + sel_len, selected_text));
            }
            st.doc = Some(doc);
            st.status.has_doc = true;
        } else if st.doc.is_some() {
            // No editable element in front. Keep the last document for the panel.
            if !ours {
                if st.lost_focus_at.is_none() {
                    st.lost_focus_at = Some(Instant::now());
                }
                let expired = st.lost_focus_at.map(|t| t.elapsed() > Duration::from_millis(900)).unwrap_or(false);
                if let Some(d) = st.doc.as_mut() {
                    if expired && d.active {
                        d.active = false;
                        clear_overlays = true;
                        doc_event = Some(d.clone());
                    }
                }
            }
        }

        // 2. Debounced lint.
        if st.lint_dirty && st.lint_pending_since.map(|t| t.elapsed() >= LINT_DEBOUNCE).unwrap_or(false) {
            if let Some(d) = st.doc.clone() {
                st.status.linting = true;
                let t_lint = Instant::now();
                let all = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.check(&d.text))) {
                    Ok(v) => v,
                    Err(_) => {
                        log::error!("engine panicked on a {}-char document; keeping previous alerts", d.text.len());
                        st.all_alerts.clone()
                    }
                };
                lint_ms = t_lint.elapsed().as_millis();
                st.all_alerts = all;
                st.alerts = st.visible_alerts();
                st.lint_dirty = false;
                st.status.linting = false;
                alerts_event = Some(st.alerts.clone());
                // history
                let sc = score::compute(&d.text, &st.alerts);
                let session = crate::history::Session {
                    doc_id: d.id.clone(),
                    app_name: d.app_name.clone(),
                    bundle_id: d.bundle_id.clone(),
                    title: crate::history::title_from_text(&d.text),
                    words: sc.words,
                    score: sc.overall,
                    last_seen: chrono::Utc::now().timestamp(),
                };
                if sc.words >= 3 {
                    crate::history::upsert(&mut st.history, session);
                    crate::history::save(&st.history);
                }
            }
        }
    }

    // 2b. Debounced spot check: once typing pauses, the model judges the
    // sentences that changed (the cache keeps the rest). Runs for any panel
    // view and for a tucked panel; the result arrives as host://alerts.
    {
        let mut st = shared.lock().unwrap();
        if st.settings.auto_spot_check && st.spot_dirty && !st.lint_dirty && !st.spot_running && st.spot_pending_since.map(|t| t.elapsed() >= SPOT_DEBOUNCE).unwrap_or(false) {
            if let Some(d) = st.doc.as_ref().filter(|d| d.active) {
                let words = score::word_count(&d.text);
                let hash = st.last_text_hash;
                st.spot_dirty = false;
                if (8..=SPOT_MAX_WORDS).contains(&words) && hash != st.spot_last_hash {
                    if let Ok(cfg) = crate::commands::effective_llm(&st.settings) {
                        st.spot_last_hash = hash;
                        spot_job = Some(cfg);
                    }
                }
            }
        }
    }
    if let Some(cfg) = spot_job {
        let (a, s) = (app.clone(), shared.clone());
        tauri::async_runtime::spawn(async move { crate::commands::spot_check_auto(a, s, cfg).await });
    }

    // 3. Emit document/alert/selection events.
    if let Some(d) = &doc_event {
        let _ = app.emit("host://document", d);
    }
    if doc_cleared {
        let _ = app.emit("host://document", Option::<HostDoc>::None);
    }
    if let Some(a) = &alerts_event {
        let _ = app.emit("host://alerts", a);
    }
    if let Some((s, e, t)) = &sel_event {
        let _ = app.emit("host://selection", serde_json::json!({"start": s, "end": e, "text": t}));
    }

    // 4. Geometry + overlays.
    let (doc, element, alerts, show_gbutton, show_underlines, prev_geom, text_hash) = {
        let st = shared.lock().unwrap();
        (st.doc.clone(), st.element.clone(), st.alerts.clone(), st.settings.show_gbutton, st.settings.show_underlines, st.geometry.clone(), st.last_text_hash)
    };
    let active_doc = doc.as_ref().filter(|d| d.active && !clear_overlays);
    match (active_doc, element) {
        (Some(d), Some(el)) => {
            let vis = d.visible;
            let big_enough = vis.w >= 60.0 && vis.h >= 28.0;
            let mut geometry = Geometry { origin_x: vis.x, origin_y: vis.y, width: vis.w, height: vis.h, rects: vec![], hovered: None, over_card: false };
            if big_enough && show_underlines {
                let t_rects = Instant::now();
                let shown: Vec<&Alert> = alerts.iter().take(80).collect();
                let key = {
                    use std::hash::{Hash, Hasher};
                    let mut hs = std::collections::hash_map::DefaultHasher::new();
                    text_hash.hash(&mut hs);
                    for a in &shown {
                        a.id.hash(&mut hs);
                    }
                    for v in [vis.x, vis.y, vis.w, vis.h] {
                        (v.round() as i64).hash(&mut hs);
                    }
                    show_underlines.hash(&mut hs);
                    hs.finish()
                };
                // One accessibility call to notice scrolling.
                let probe = shown.first().and_then(|a| el.bounds_for_range(a.u16_start, a.u16_end.saturating_sub(a.u16_start).max(1)));
                if key == cache.key && rect_close(&probe, &cache.probe) {
                    cached_rects = true;
                } else {
                    let mut rects = Vec::new();
                    for a in &shown {
                        for r in rects_for_range(&el, a.u16_start, a.u16_end.saturating_sub(a.u16_start).max(1)) {
                            if let Some(c) = r.intersect(&vis) {
                                rects.push(UnderlineRect {
                                    id: a.id.clone(),
                                    x: c.x - vis.x,
                                    y: c.y - vis.y,
                                    w: c.w,
                                    h: c.h,
                                    color: a.category.color().into(),
                                    category: format!("{:?}", a.category).to_lowercase(),
                                    sentence: a.kind.starts_with("Spot"),
                                });
                            }
                        }
                    }
                    *cache = RectCache { key, probe, rects };
                }
                geometry.rects = cache.rects.clone();
                rects_ms = t_rects.elapsed().as_millis();
            }
            // Hover state comes from the hover thread; carry it over while the id still exists.
            geometry.hovered = prev_geom.hovered.filter(|id| geometry.rects.iter().any(|r| &r.id == id));
            geometry.over_card = prev_geom.over_card;
            let over_card = geometry.over_card;
            let sig = serde_json::to_string(&geometry).unwrap_or_default();
            if sig != *last_geom_sig {
                *last_geom_sig = sig;
                shared.lock().unwrap().geometry = geometry.clone();
                let _ = app.emit_to("underlines", "host://geometry", &geometry);
            }
            if let Some(w) = app.get_webview_window("underlines") {
                if big_enough && show_underlines {
                    let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(vis.x, vis.y)));
                    let _ = w.set_size(tauri::Size::Logical(tauri::LogicalSize::new(vis.w.max(1.0), vis.h.max(1.0))));
                    let _ = w.set_ignore_cursor_events(!(geometry.hovered.is_some() || over_card));
                    if !w.is_visible().unwrap_or(false) {
                        let _ = w.show();
                    }
                } else if w.is_visible().unwrap_or(false) {
                    let _ = w.hide();
                }
            }
            if let Some(w) = app.get_webview_window("gbutton") {
                if big_enough && show_gbutton && vis.h >= GBUTTON + 2.0 * GBUTTON_INSET {
                    let x = vis.x + vis.w - GBUTTON - GBUTTON_INSET;
                    let y = vis.y + vis.h - GBUTTON - GBUTTON_INSET;
                    let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
                    if !w.is_visible().unwrap_or(false) {
                        let _ = w.show();
                    }
                } else if w.is_visible().unwrap_or(false) {
                    let _ = w.hide();
                }
            }
        }
        _ => {
            for label in ["underlines", "gbutton"] {
                if let Some(w) = app.get_webview_window(label) {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    }
                }
            }
            if !last_geom_sig.is_empty() {
                last_geom_sig.clear();
                shared.lock().unwrap().geometry = Geometry::default();
                let _ = app.emit_to("underlines", "host://geometry", Geometry::default());
            }
        }
    }

    let total_ms = t_start.elapsed().as_millis();
    if total_ms > 150 {
        log::info!("slow tick: total={}ms read={}ms lint={}ms rects={}ms alerts={} cached_rects={}", total_ms, read_ms, lint_ms, rects_ms, shared.lock().map(|s| s.alerts.len()).unwrap_or(0), cached_rects);
    }

    // 5. Status (only when changed).
    let status = shared.lock().unwrap().status.clone();
    let ssig = format!("{}|{}|{}|{}|{}", status.trusted, status.frontmost, status.has_doc, status.skipped, status.frontmost_bid);
    if ssig != *last_status_sig {
        *last_status_sig = ssig;
        let _ = app.emit("host://status", &status);
    }
}

/// Screen rects (one per visual line) for a UTF-16 range.
fn rects_for_range(el: &AXElement, loc: usize, len: usize) -> Vec<Rect> {
    let mut out = Vec::new();
    if len == 0 {
        return out;
    }
    let first = el.line_for_index(loc);
    let last = el.line_for_index(loc + len - 1);
    if let (Some(l0), Some(l1)) = (first, last) {
        if l1 > l0 && l1 - l0 <= 40 {
            for line in l0..=l1 {
                if let Some((ls, ll)) = el.range_for_line(line) {
                    let s = loc.max(ls);
                    let e = (loc + len).min(ls + ll);
                    if e > s {
                        if let Some(r) = el.bounds_for_range(s, e - s) {
                            if !r.is_empty() {
                                out.push(r);
                            }
                        }
                    }
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
    }
    if let Some(r) = el.bounds_for_range(loc, len) {
        if !r.is_empty() {
            out.push(r);
            return out;
        }
    }
    // Some text views answer an empty rect for a range that wraps lines (a
    // whole sentence): walk the range in short pieces and keep each piece's box.
    if len > 24 {
        let mut s = loc;
        while s < loc + len {
            let e = (s + 24).min(loc + len);
            if let Some(r) = el.bounds_for_range(s, e - s) {
                if !r.is_empty() {
                    match out.last_mut() {
                        Some(last) if (last.y - r.y).abs() < 2.0 && r.x >= last.x => {
                            let right = (r.x + r.w).max(last.x + last.w);
                            last.w = right - last.x;
                            last.h = last.h.max(r.h);
                        }
                        _ => out.push(r),
                    }
                }
            }
            s = e;
        }
        if out.is_empty() {
            log::debug!("no AX bounds for range {loc}+{len}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_are_skipped_and_the_user_list_adds_apps() {
        let mut s = crate::settings::Settings::default();
        assert!(skipped_app("com.apple.Terminal", &s));
        assert!(skipped_app("com.googlecode.iterm2", &s));
        assert!(built_in_skip("com.mitchellh.ghostty"));
        assert!(!skipped_app("com.apple.Notes", &s));
        assert!(!skipped_app("", &s), "no app in front is not a skip");
        s.skip_apps.push(crate::settings::SkipApp { bundle_id: "com.apple.Notes".into(), name: "Notes".into() });
        assert!(skipped_app("com.apple.Notes", &s));
        assert!(!built_in_skip("com.apple.Notes"), "a user choice is not built in");
    }

    #[test]
    fn hit_test_finds_underlines_and_the_card() {
        let g = Geometry {
            origin_x: 100.0,
            origin_y: 200.0,
            rects: vec![UnderlineRect { id: "a".into(), x: 10.0, y: 10.0, w: 40.0, h: 12.0, color: String::new(), category: String::new(), sentence: false }],
            ..Default::default()
        };
        assert_eq!(hit_test(&g, &[], 115.0, 215.0), (Some("a".to_string()), false));
        assert_eq!(hit_test(&g, &[], 300.0, 300.0), (None, false));
        // Card rect in local coords: (200..250, 200..250); pointer at local (210, 210).
        let card = Rect { x: 200.0, y: 200.0, w: 50.0, h: 50.0 };
        assert_eq!(hit_test(&g, &[card], 310.0, 410.0), (None, true));
        assert_eq!(hit_test(&g, &[card], 115.0, 215.0).1, false);
    }
}
