//! Windows twin of `ax.rs`: the same `AXElement` surface over UI Automation.
//!
//! The host loop and the replacement ladder ask for macOS attribute names
//! (`AXValue`, `AXSelectedTextRange`, `AXBoundsForRange`…). This module answers
//! them from UIA's TextPattern / ValuePattern so the rest of the app stays
//! platform-neutral. Geometry comes back from UIA in physical pixels and is
//! converted to logical points (what Tauri positions windows in).

#![cfg(windows)]

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use windows::core::{BOOL, BSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, SAFEARRAY};
use windows::Win32::System::Ole::SafeArrayDestroy;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, IsWindowVisible, SetCursorPos,
    SetForegroundWindow, GA_ROOT, SM_CYSCREEN,
};

/// A screen rectangle in logical points, origin at the top-left of the primary display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x1 = self.x.max(o.x);
        let y1 = self.y.max(o.y);
        let x2 = (self.x + self.w).min(o.x + o.w);
        let y2 = (self.y + self.h).min(o.y + o.h);
        if x2 > x1 && y2 > y1 {
            Some(Rect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 })
        } else {
            None
        }
    }
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

thread_local! {
    /// One UIA client per thread. COM is joined as MTA when the thread has no
    /// apartment yet; a thread that is already STA (Tauri's main thread) keeps it.
    static UIA: RefCell<Option<IUIAutomation>> = const { RefCell::new(None) };
}

fn uia() -> Option<IUIAutomation> {
    UIA.with(|cell| {
        if cell.borrow().is_none() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                *cell.borrow_mut() = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
            }
        }
        cell.borrow().clone()
    })
}

/// UI Automation needs no permission grant on Windows.
pub fn is_trusted() -> bool {
    true
}

pub fn prompt_trust() -> bool {
    true
}

/// DPI scale (1.0 = 96 dpi) of the monitor under a physical point.
fn scale_at(x: i32, y: i32) -> f64 {
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() && dx > 0 {
            dx as f64 / 96.0
        } else {
            1.0
        }
    }
}

fn logical(x: f64, y: f64, w: f64, h: f64) -> Rect {
    let s = scale_at(x as i32, y as i32);
    Rect { x: x / s, y: y / s, w: w / s, h: h / s }
}

#[derive(Clone, Debug)]
enum Inner {
    System,
    El(IUIAutomationElement),
}

/// A UIA element (or the system-wide root), shaped like the macOS `AXElement`.
#[derive(Clone, Debug)]
pub struct AXElement(Inner);

// UIA client objects are free-threaded; the element is an immutable token.
unsafe impl Send for AXElement {}
unsafe impl Sync for AXElement {}

impl AXElement {
    pub fn system_wide() -> AXElement {
        AXElement(Inner::System)
    }

    pub fn application(_pid: i32) -> AXElement {
        AXElement(Inner::System)
    }

    pub fn set_timeout(&self, _secs: f32) {}

    fn el(&self) -> Option<&IUIAutomationElement> {
        match &self.0 {
            Inner::El(e) => Some(e),
            Inner::System => None,
        }
    }

    pub fn pid(&self) -> i32 {
        self.el().and_then(|e| unsafe { e.CurrentProcessId() }.ok()).unwrap_or(0)
    }

    pub fn same_as(&self, other: &AXElement) -> bool {
        match (self.el(), other.el(), uia()) {
            (Some(a), Some(b), Some(u)) => unsafe { u.CompareElements(a, b) }.map(|b| b.as_bool()).unwrap_or(false),
            _ => false,
        }
    }

    /// Hash of the UIA runtime id, stable for the element's lifetime.
    pub fn identity_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(e) = self.el() {
            unsafe {
                if let Ok(psa) = e.GetRuntimeId() {
                    for v in safearray_vec::<i32>(psa) {
                        v.hash(&mut h);
                    }
                }
            }
        }
        h.finish()
    }

    fn text_pattern(&self) -> Option<IUIAutomationTextPattern> {
        unsafe { self.el()?.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.ok()
    }

    fn value_pattern(&self) -> Option<IUIAutomationValuePattern> {
        unsafe { self.el()?.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }.ok()
    }

    fn value_read_only(&self) -> Option<bool> {
        let vp = self.value_pattern()?;
        unsafe { vp.CurrentIsReadOnly() }.ok().map(|b| b.as_bool())
    }

    /// Map the UIA control type onto the macOS role names the host loop knows.
    fn role_name(&self) -> Option<String> {
        let e = self.el()?;
        unsafe {
            if e.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) {
                return Some("AXSecureTextField".into());
            }
            let ct = e.CurrentControlType().ok()?;
            let ro = self.value_read_only();
            let role = if ct == UIA_EditControlTypeId {
                if ro == Some(true) { "AXStaticText" } else { "AXTextArea" }
            } else if ct == UIA_DocumentControlTypeId {
                // A web page is a read-only document; an editor body is not.
                let framework = e.CurrentFrameworkId().map(|s| s.to_string()).unwrap_or_default();
                let web = matches!(framework.as_str(), "Chrome" | "Gecko");
                match ro {
                    Some(true) => "AXWebArea",
                    None if web => "AXWebArea",
                    _ if self.text_pattern().is_some() => "AXTextArea",
                    _ => "AXWebArea",
                }
            } else if ct == UIA_ComboBoxControlTypeId {
                if ro == Some(false) { "AXComboBox" } else { "AXGroup" }
            } else if ct == UIA_WindowControlTypeId {
                "AXWindow"
            } else {
                "AXGroup"
            };
            Some(role.into())
        }
    }

    /// Whole text of the field: TextPattern when present, else the Value.
    fn value_text(&self) -> Option<String> {
        if let Some(tp) = self.text_pattern() {
            if let Ok(t) = unsafe { tp.DocumentRange().and_then(|r| r.GetText(-1)) } {
                return Some(t.to_string());
            }
        }
        unsafe { self.value_pattern()?.CurrentValue() }.ok().map(|b| b.to_string())
    }

    /// UTF-16 offset of `r`'s start inside the document.
    fn offset_of(doc: &IUIAutomationTextRange, r: &IUIAutomationTextRange) -> Option<usize> {
        unsafe {
            let prefix = doc.Clone().ok()?;
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, r, TextPatternRangeEndpoint_Start).ok()?;
            Some(prefix.GetText(-1).ok()?.to_string().encode_utf16().count())
        }
    }

    /// A text range covering `len` UTF-16 units at `loc`.
    fn range_at(&self, loc: usize, len: usize) -> Option<IUIAutomationTextRange> {
        let tp = self.text_pattern()?;
        unsafe {
            let doc = tp.DocumentRange().ok()?;
            let r = doc.Clone().ok()?;
            r.MoveEndpointByRange(TextPatternRangeEndpoint_End, &doc, TextPatternRangeEndpoint_Start).ok()?;
            if loc + len > 0 {
                r.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, (loc + len) as i32).ok()?;
            }
            if loc > 0 {
                r.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, loc as i32).ok()?;
            }
            Some(r)
        }
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let tp = self.text_pattern()?;
        unsafe {
            let sel = tp.GetSelection().ok()?;
            if sel.Length().ok()? < 1 {
                return None;
            }
            let r = sel.GetElement(0).ok()?;
            let doc = tp.DocumentRange().ok()?;
            let start = Self::offset_of(&doc, &r)?;
            let len = r.GetText(-1).ok()?.to_string().encode_utf16().count();
            Some((start, len))
        }
    }

    /// UTF-16 `(start, end)` of the text currently scrolled into view
    /// (first visible range's start to the last one's end); `None` when unsupported.
    pub fn visible_range(&self) -> Option<(usize, usize)> {
        let tp = self.text_pattern()?;
        unsafe {
            let vis = tp.GetVisibleRanges().ok()?;
            let n = vis.Length().ok()?;
            if n < 1 {
                return None;
            }
            let doc = tp.DocumentRange().ok()?;
            let start = Self::offset_of(&doc, &vis.GetElement(0).ok()?)?;
            let last = vis.GetElement(n - 1).ok()?;
            let end = Self::offset_of(&doc, &last)? + last.GetText(-1).ok()?.to_string().encode_utf16().count();
            Some((start, end.max(start)))
        }
    }

    pub fn string(&self, name: &str) -> Option<String> {
        match name {
            "AXValue" => self.value_text(),
            "AXRole" => self.role_name(),
            "AXSelectedText" => {
                let tp = self.text_pattern()?;
                unsafe {
                    let sel = tp.GetSelection().ok()?;
                    (sel.Length().ok()? > 0).then_some(())?;
                    Some(sel.GetElement(0).ok()?.GetText(-1).ok()?.to_string())
                }
            }
            _ => None,
        }
    }

    pub fn number(&self, _name: &str) -> Option<i64> {
        None
    }

    pub fn element(&self, name: &str) -> Option<AXElement> {
        match name {
            "AXFocusedUIElement" => self.focused_element(),
            "AXParent" => self.parent(),
            "AXWindow" => self.window(),
            _ => None,
        }
    }

    pub fn role(&self) -> Option<String> {
        self.role_name()
    }

    pub fn subrole(&self) -> Option<String> {
        None
    }

    pub fn focused_element(&self) -> Option<AXElement> {
        let e = unsafe { uia()?.GetFocusedElement() }.ok()?;
        Some(AXElement(Inner::El(e)))
    }

    pub fn parent(&self) -> Option<AXElement> {
        let u = uia()?;
        unsafe {
            let walker = u.ControlViewWalker().ok()?;
            let p = walker.GetParentElement(self.el()?).ok()?;
            Some(AXElement(Inner::El(p)))
        }
    }

    /// The top-level window holding this element (falls back to the foreground window:
    /// web content has no window handle of its own).
    pub fn window(&self) -> Option<AXElement> {
        let u = uia()?;
        unsafe {
            let own = self.el().and_then(|e| e.CurrentNativeWindowHandle().ok()).filter(|h| !h.is_invalid());
            let hwnd = own.map(|h| GetAncestor(h, GA_ROOT)).unwrap_or_else(|| GetForegroundWindow());
            if hwnd.is_invalid() {
                return None;
            }
            Some(AXElement(Inner::El(u.ElementFromHandle(hwnd).ok()?)))
        }
    }

    pub fn is_settable(&self, name: &str) -> bool {
        match name {
            "AXValue" => self.value_read_only() == Some(false),
            "AXSelectedTextRange" => self.text_pattern().is_some(),
            _ => false,
        }
    }

    pub fn has_attr(&self, name: &str) -> bool {
        match name {
            "AXSelectedTextRange" | "AXSelectedText" => self.text_pattern().is_some(),
            "AXValue" => self.text_pattern().is_some() || self.value_pattern().is_some(),
            _ => self.string(name).is_some(),
        }
    }

    /// `(location, length)` in UTF-16 units.
    pub fn range(&self, name: &str) -> Option<(usize, usize)> {
        match name {
            "AXSelectedTextRange" => self.selection(),
            _ => None,
        }
    }

    pub fn frame(&self) -> Option<Rect> {
        let r: RECT = unsafe { self.el()?.CurrentBoundingRectangle() }.ok()?;
        Some(logical(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64))
    }

    /// Box of a single-line range. A range that wraps lines answers `None`, so the
    /// host loop walks it in short pieces (one box per visual line).
    pub fn bounds_for_range(&self, loc: usize, len: usize) -> Option<Rect> {
        let r = self.range_at(loc, len)?;
        let vals = unsafe { safearray_vec::<f64>(r.GetBoundingRectangles().ok()?) };
        let mut boxes: Vec<[f64; 4]> = vals.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).filter(|b| b[2] > 0.0 && b[3] > 0.0).collect();
        // Chromium adds a 1 px sliver at the end of the previous line for a word right after a soft wrap.
        if boxes.len() > 1 {
            boxes.retain(|b| b[2] > 2.0);
        }
        let first = boxes.first()?;
        if boxes.iter().any(|b| (b[1] - first[1]).abs() > first[3] / 2.0) {
            return None;
        }
        let left = boxes.iter().map(|b| b[0]).fold(f64::MAX, f64::min);
        let top = boxes.iter().map(|b| b[1]).fold(f64::MAX, f64::min);
        let right = boxes.iter().map(|b| b[0] + b[2]).fold(f64::MIN, f64::max);
        let bottom = boxes.iter().map(|b| b[1] + b[3]).fold(f64::MIN, f64::max);
        Some(logical(left, top, right - left, bottom - top))
    }

    pub fn line_for_index(&self, _idx: usize) -> Option<i64> {
        None
    }

    pub fn range_for_line(&self, _line: i64) -> Option<(usize, usize)> {
        None
    }

    pub fn set_range(&self, name: &str, loc: usize, len: usize) -> Result<(), i32> {
        if name != "AXSelectedTextRange" {
            return Err(-1);
        }
        let r = self.range_at(loc, len).ok_or(-1)?;
        unsafe { r.Select() }.map_err(|e| e.code().0)
    }

    /// `AXValue` replaces the whole field through ValuePattern. UIA has no
    /// "replace the selection" call, so `AXSelectedText` is refused and the
    /// replacement ladder pastes instead (which keeps formatting and undo).
    pub fn set_string(&self, name: &str, value: &str) -> Result<(), i32> {
        match name {
            "AXValue" => {
                let vp = self.value_pattern().ok_or(-1)?;
                unsafe { vp.SetValue(&BSTR::from(value)) }.map_err(|e| e.code().0)
            }
            _ => Err(-1),
        }
    }

    /// Text of `len` UTF-16 units at `loc` (one small range, cheap even in Word).
    pub fn text_range(&self, loc: usize, len: usize) -> Option<String> {
        let r = self.range_at(loc, len)?;
        unsafe { r.GetText(-1) }.ok().map(|s| s.to_string())
    }

    /// Whether the element currently has keyboard focus.
    pub fn has_focus(&self) -> bool {
        self.el().map(|e| unsafe { e.CurrentHasKeyboardFocus() }.map(|b| b.as_bool()).unwrap_or(false)).unwrap_or(false)
    }

    /// Give the element keyboard focus (brings its window to the front).
    pub fn focus(&self) -> bool {
        self.el().map(|e| unsafe { e.SetFocus() }.is_ok()).unwrap_or(false)
    }

    pub fn enclosing_scroll_area(&self) -> Option<AXElement> {
        None
    }
}

/// Copy a one-dimensional SAFEARRAY into a Vec and free it.
unsafe fn safearray_vec<T: Copy>(psa: *mut SAFEARRAY) -> Vec<T> {
    if psa.is_null() {
        return vec![];
    }
    let sa = &*psa;
    let n = sa.rgsabound[0].cElements as usize;
    let out = if sa.pvData.is_null() || n == 0 { vec![] } else { std::slice::from_raw_parts(sa.pvData as *const T, n).to_vec() };
    let _ = SafeArrayDestroy(psa);
    out
}

fn process_image(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// Foreground application as `(pid, app id, display name)`. The app id is the
/// lower-case executable name (`notepad.exe`), Windows' stand-in for a bundle id.
pub fn frontmost_app() -> Option<(i32, String, String)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        if pid == std::process::id() {
            return Some((pid as i32, crate::host::OUR_BUNDLE.into(), "GSpot".into()));
        }
        let path = process_image(pid).unwrap_or_default();
        let file = path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
        let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(&file);
        let mut c = stem.chars();
        let name = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
        Some((pid as i32, file.to_lowercase(), name))
    }
}

/// Bring a process's main window to the front (needed before a clipboard paste).
pub fn activate_app(pid: i32) -> bool {
    struct Find {
        pid: u32,
        hwnd: HWND,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp.0 as *mut Find);
        let mut p = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut p));
        if p == f.pid && IsWindowVisible(hwnd).as_bool() && GetAncestor(hwnd, GA_ROOT) == hwnd {
            f.hwnd = hwnd;
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut f = Find { pid: pid as u32, hwnd: HWND(std::ptr::null_mut()) };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut f as *mut Find as isize));
        !f.hwnd.is_invalid() && SetForegroundWindow(f.hwnd).as_bool()
    }
}

/// Mouse location in logical points, top-left origin.
pub fn mouse_location_top_left() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok()?;
    let s = scale_at(p.x, p.y);
    Some((p.x as f64 / s, p.y as f64 / s))
}

/// Height of the primary display in logical points.
pub fn primary_screen_height() -> Option<f64> {
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    (h > 0).then(|| h as f64 / scale_at(0, 0))
}

/// Move the pointer (logical points). Debug-probe only: lets the e2e harness hover an underline.
pub fn post_mouse_move(x: f64, y: f64) -> bool {
    let s = scale_at((x) as i32, (y) as i32);
    unsafe { SetCursorPos((x * s) as i32, (y * s) as i32) }.is_ok()
}
