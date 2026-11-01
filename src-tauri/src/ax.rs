//! Thin, safe-ish wrapper over the macOS Accessibility (AX) C API.
//!
//! This is the same surface Grammarly's `GRAccessibilityKit` uses: the focused
//! element, its string value, selected range, on-screen frame, and
//! `AXBoundsForRange` for drawing underlines. Everything is plain C FFI over
//! ApplicationServices; no third-party AX crate.

#![cfg(target_os = "macos")]

use core_foundation::base::{CFEqual, CFGetTypeID, CFHash, CFRange, CFRelease, CFRetain, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use serde::{Deserialize, Serialize};
use std::os::raw::c_void;
use std::ptr::null;

type AXUIElementRef = CFTypeRef;
type AXValueRef = CFTypeRef;
type AXError = i32;

const K_AX_VALUE_CGPOINT: u32 = 1;
const K_AX_VALUE_CGSIZE: u32 = 2;
const K_AX_VALUE_CGRECT: u32 = 3;
const K_AX_VALUE_CFRANGE: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGSize {
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, out: *mut CFTypeRef) -> AXError;
    fn AXUIElementCopyParameterizedAttributeValue(
        el: AXUIElementRef,
        attr: CFStringRef,
        param: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: CFTypeRef) -> AXError;
    fn AXUIElementIsAttributeSettable(el: AXUIElementRef, attr: CFStringRef, out: *mut u8) -> AXError;
    fn AXUIElementGetPid(el: AXUIElementRef, out: *mut i32) -> AXError;
    fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, timeout: f32) -> AXError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, kind: u32, out: *mut c_void) -> u8;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// A screen rectangle in logical points, origin at the top-left of the primary display
/// (the coordinate system the AX API reports in).
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

/// Whether this process is trusted for Accessibility.
pub fn is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Ask for Accessibility trust and show the system prompt if it is missing.
pub fn prompt_trust() -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let dict = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
        AXIsProcessTrustedWithOptions(dict.as_CFTypeRef())
    }
}

/// An owned reference to an `AXUIElement`.
#[derive(Debug)]
pub struct AXElement(AXUIElementRef);

// AX API calls are safe from any thread; the element is an immutable token.
unsafe impl Send for AXElement {}
unsafe impl Sync for AXElement {}

impl Drop for AXElement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

impl Clone for AXElement {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0) };
        AXElement(self.0)
    }
}

impl AXElement {
    pub fn system_wide() -> AXElement {
        let el = AXElement(unsafe { AXUIElementCreateSystemWide() });
        el.set_timeout(0.5);
        el
    }

    pub fn application(pid: i32) -> AXElement {
        let el = AXElement(unsafe { AXUIElementCreateApplication(pid) });
        el.set_timeout(0.5);
        el
    }

    pub fn set_timeout(&self, secs: f32) {
        unsafe {
            AXUIElementSetMessagingTimeout(self.0, secs);
        }
    }

    pub fn pid(&self) -> i32 {
        let mut pid = 0i32;
        unsafe { AXUIElementGetPid(self.0, &mut pid) };
        pid
    }

    pub fn same_as(&self, other: &AXElement) -> bool {
        unsafe { CFEqual(self.0, other.0) != 0 }
    }

    pub fn identity_hash(&self) -> u64 {
        unsafe { CFHash(self.0) as u64 }
    }

    fn attr(&self, name: &str) -> Option<CFType> {
        let key = CFString::new(name);
        let mut out: CFTypeRef = null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.0, key.as_concrete_TypeRef(), &mut out) };
        if err != 0 || out.is_null() {
            return None;
        }
        Some(unsafe { CFType::wrap_under_create_rule(out) })
    }

    fn param_attr(&self, name: &str, param: CFTypeRef) -> Option<CFType> {
        let key = CFString::new(name);
        let mut out: CFTypeRef = null();
        let err = unsafe { AXUIElementCopyParameterizedAttributeValue(self.0, key.as_concrete_TypeRef(), param, &mut out) };
        if err != 0 || out.is_null() {
            return None;
        }
        Some(unsafe { CFType::wrap_under_create_rule(out) })
    }

    pub fn string(&self, name: &str) -> Option<String> {
        self.attr(name)?.downcast::<CFString>().map(|s| s.to_string())
    }

    pub fn number(&self, name: &str) -> Option<i64> {
        self.attr(name)?.downcast::<CFNumber>().and_then(|n| n.to_i64())
    }

    pub fn element(&self, name: &str) -> Option<AXElement> {
        let t = self.attr(name)?;
        let r = t.as_CFTypeRef();
        if unsafe { CFGetTypeID(r) } == unsafe { AXUIElementGetTypeID() } {
            unsafe { CFRetain(r) };
            Some(AXElement(r))
        } else {
            None
        }
    }

    pub fn role(&self) -> Option<String> {
        self.string("AXRole")
    }

    pub fn subrole(&self) -> Option<String> {
        self.string("AXSubrole")
    }

    pub fn focused_element(&self) -> Option<AXElement> {
        self.element("AXFocusedUIElement")
    }

    pub fn parent(&self) -> Option<AXElement> {
        self.element("AXParent")
    }

    pub fn window(&self) -> Option<AXElement> {
        self.element("AXWindow")
    }

    pub fn is_settable(&self, name: &str) -> bool {
        let key = CFString::new(name);
        let mut out = 0u8;
        let err = unsafe { AXUIElementIsAttributeSettable(self.0, key.as_concrete_TypeRef(), &mut out) };
        err == 0 && out != 0
    }

    pub fn has_attr(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }

    /// `(location, length)` in UTF-16 units.
    pub fn range(&self, name: &str) -> Option<(usize, usize)> {
        axvalue_range(self.attr(name)?.as_CFTypeRef())
    }

    pub fn frame(&self) -> Option<Rect> {
        let p = axvalue_point(self.attr("AXPosition")?.as_CFTypeRef())?;
        let s = axvalue_size(self.attr("AXSize")?.as_CFTypeRef())?;
        Some(Rect { x: p.x, y: p.y, w: s.w, h: s.h })
    }

    pub fn bounds_for_range(&self, loc: usize, len: usize) -> Option<Rect> {
        let r = CFRange { location: loc as isize, length: len as isize };
        let v = unsafe { AXValueCreate(K_AX_VALUE_CFRANGE, &r as *const CFRange as *const c_void) };
        if v.is_null() {
            return None;
        }
        let out = self.param_attr("AXBoundsForRange", v);
        unsafe { CFRelease(v) };
        axvalue_rect(out?.as_CFTypeRef())
    }

    pub fn line_for_index(&self, idx: usize) -> Option<i64> {
        let n = CFNumber::from(idx as i64);
        self.param_attr("AXLineForIndex", n.as_CFTypeRef())?.downcast::<CFNumber>()?.to_i64()
    }

    pub fn range_for_line(&self, line: i64) -> Option<(usize, usize)> {
        let n = CFNumber::from(line);
        axvalue_range(self.param_attr("AXRangeForLine", n.as_CFTypeRef())?.as_CFTypeRef())
    }

    pub fn set_range(&self, name: &str, loc: usize, len: usize) -> Result<(), i32> {
        let r = CFRange { location: loc as isize, length: len as isize };
        let v = unsafe { AXValueCreate(K_AX_VALUE_CFRANGE, &r as *const CFRange as *const c_void) };
        if v.is_null() {
            return Err(-1);
        }
        let key = CFString::new(name);
        let err = unsafe { AXUIElementSetAttributeValue(self.0, key.as_concrete_TypeRef(), v) };
        unsafe { CFRelease(v) };
        if err == 0 {
            Ok(())
        } else {
            Err(err)
        }
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<(), i32> {
        let key = CFString::new(name);
        let v = CFString::new(value);
        let err = unsafe { AXUIElementSetAttributeValue(self.0, key.as_concrete_TypeRef(), v.as_CFTypeRef()) };
        if err == 0 {
            Ok(())
        } else {
            Err(err)
        }
    }

    /// UTF-16 range of the text in view; not available through AX (Windows only).
    pub fn visible_range(&self) -> Option<(usize, usize)> {
        None
    }

    /// The nearest ancestor scroll area, if any (bounded walk).
    pub fn enclosing_scroll_area(&self) -> Option<AXElement> {
        let mut cur = self.parent()?;
        for _ in 0..8 {
            if cur.role().as_deref() == Some("AXScrollArea") {
                return Some(cur);
            }
            cur = cur.parent()?;
        }
        None
    }
}

fn is_axvalue(r: CFTypeRef) -> bool {
    unsafe { CFGetTypeID(r) == AXValueGetTypeID() }
}

fn axvalue_range(r: CFTypeRef) -> Option<(usize, usize)> {
    if !is_axvalue(r) {
        return None;
    }
    let mut out = CFRange { location: 0, length: 0 };
    let ok = unsafe { AXValueGetValue(r, K_AX_VALUE_CFRANGE, &mut out as *mut CFRange as *mut c_void) };
    if ok == 0 || out.location < 0 || out.length < 0 {
        return None;
    }
    Some((out.location as usize, out.length as usize))
}

fn axvalue_point(r: CFTypeRef) -> Option<CGPoint> {
    if !is_axvalue(r) {
        return None;
    }
    let mut out = CGPoint::default();
    let ok = unsafe { AXValueGetValue(r, K_AX_VALUE_CGPOINT, &mut out as *mut CGPoint as *mut c_void) };
    (ok != 0).then_some(out)
}

fn axvalue_size(r: CFTypeRef) -> Option<CGSize> {
    if !is_axvalue(r) {
        return None;
    }
    let mut out = CGSize::default();
    let ok = unsafe { AXValueGetValue(r, K_AX_VALUE_CGSIZE, &mut out as *mut CGSize as *mut c_void) };
    (ok != 0).then_some(out)
}

fn axvalue_rect(r: CFTypeRef) -> Option<Rect> {
    if !is_axvalue(r) {
        return None;
    }
    let mut out = CGRect::default();
    let ok = unsafe { AXValueGetValue(r, K_AX_VALUE_CGRECT, &mut out as *mut CGRect as *mut c_void) };
    if ok == 0 {
        return None;
    }
    Some(Rect { x: out.origin.x, y: out.origin.y, w: out.size.w, h: out.size.h })
}

/// Frontmost application as `(pid, bundle id, localized name)`.
pub fn frontmost_app() -> Option<(i32, String, String)> {
    use objc2_app_kit::NSWorkspace;
    #[allow(unused_unsafe)]
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let app = ws.frontmostApplication()?;
        let pid = app.processIdentifier();
        let bid = app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default();
        let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
        Some((pid, bid, name))
    }
}

/// Bring another app to the front (needed before a pasteboard-based replacement).
pub fn activate_app(pid: i32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    #[allow(unused_unsafe)]
    unsafe {
        match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            Some(app) => {
                #[allow(deprecated)]
                app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
            }
            None => false,
        }
    }
}

/// Current mouse location in the AX coordinate system (top-left origin, logical points).
/// Uses CoreGraphics, which is safe from any thread.
pub fn mouse_location_top_left() -> Option<(f64, f64)> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CFRelease(cf: *const c_void);
    }
    unsafe {
        let ev = CGEventCreate(null());
        if ev.is_null() {
            return None;
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev as *const c_void);
        Some((p.x, p.y))
    }
}

/// Height of the main display in logical points.
pub fn primary_screen_height() -> Option<f64> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }
    unsafe {
        let r = CGDisplayBounds(CGMainDisplayID());
        Some(r.size.h)
    }
}

/// Post a synthetic pointer move to the HID event tap (top-left-origin logical
/// points). Debug-probe only: lets the e2e harness hover an underline.
pub fn post_mouse_move(x: f64, y: f64) -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateMouseEvent(source: *const c_void, kind: u32, point: CGPoint, button: u32) -> *mut c_void;
        fn CGEventPost(tap: u32, event: *mut c_void);
    }
    const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
    const K_CG_HID_EVENT_TAP: u32 = 0;
    unsafe {
        let ev = CGEventCreateMouseEvent(null(), K_CG_EVENT_MOUSE_MOVED, CGPoint { x, y }, 0);
        if ev.is_null() {
            return false;
        }
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
    }
    true
}
