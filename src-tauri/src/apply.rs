//! The replacement ladder: AX selected-text set, then pasteboard patch, each
//! verified by reading the field back (what Grammarly's TextCorruptionDetector does).

#![cfg(any(target_os = "macos", windows))]

use crate::ax::{self, AXElement};
use crate::textmap::{slice_utf16, splice_utf16};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How long to wait for a pasted fix to show up; slow apps such as Word read the clipboard late.
const PASTE_WAIT: Duration = Duration::from_millis(2000);
/// How often to re-check the field while waiting for a pasted fix.
const PASTE_POLL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Ax,
    Paste,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApplyResult {
    pub method: Method,
    pub new_text: String,
}

#[cfg(target_os = "macos")]
fn post_paste_keys() {
    use std::os::raw::c_void;
    use std::ptr::null;
    type CGEventRef = *mut c_void;
    type CGEventSourceRef = *const c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, keycode: u16, keydown: bool) -> CGEventRef;
        fn CGEventSetFlags(event: CGEventRef, flags: u64);
        fn CGEventPost(tap: u32, event: CGEventRef);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
    const KEYCODE_V: u16 = 9;

    unsafe {
        let down = CGEventCreateKeyboardEvent(null(), KEYCODE_V, true);
        CGEventSetFlags(down, KCG_FLAG_MASK_COMMAND);
        CGEventPost(KCG_HID_EVENT_TAP, down);
        CFRelease(down as *const c_void);
        let up = CGEventCreateKeyboardEvent(null(), KEYCODE_V, false);
        CGEventSetFlags(up, KCG_FLAG_MASK_COMMAND);
        CGEventPost(KCG_HID_EVENT_TAP, up);
        CFRelease(up as *const c_void);
    }
}

/// Ctrl+V through SendInput.
#[cfg(windows)]
fn post_paste_keys() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_V};
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn verified(el: &AXElement, loc: usize, replacement: &str) -> Option<String> {
    let now = el.string("AXValue")?;
    let got = slice_utf16(&now, loc, loc + replacement.encode_utf16().count());
    if got == replacement {
        Some(now)
    } else {
        None
    }
}

/// Replace `len` UTF-16 units at `loc` with `replacement` in the host field.
pub fn replace_range(el: &AXElement, pid: i32, loc: usize, len: usize, replacement: &str, before: &str) -> Result<ApplyResult> {
    // Guard: the field must still contain what we think it does.
    let live = el.string("AXValue").ok_or_else(|| anyhow!("The text field is no longer available."))?;
    if live != before {
        // Tolerate if the target range still holds the original text.
        let want = slice_utf16(before, loc, loc + len);
        if slice_utf16(&live, loc, loc + len) != want {
            return Err(anyhow!("The text changed since this suggestion was made. Try again."));
        }
    }
    let expected = splice_utf16(&live, loc, len, replacement);

    // Rung 1: select the range, then set the selected text through AX.
    if el.set_range("AXSelectedTextRange", loc, len).is_ok() {
        let sel_ok = el.range("AXSelectedTextRange").map(|(l, n)| l == loc && n == len).unwrap_or(false);
        if sel_ok && el.set_string("AXSelectedText", replacement).is_ok() {
            sleep(Duration::from_millis(70));
            if let Some(new_text) = verified(el, loc, replacement) {
                let _ = el.set_range("AXSelectedTextRange", loc + replacement.encode_utf16().count(), 0);
                return Ok(ApplyResult { method: Method::Ax, new_text });
            }
        }
        // Rung 2: pasteboard patch (what Grammarly calls "hybrid replacements").
        if sel_ok || el.set_range("AXSelectedTextRange", loc, len).is_ok() {
            #[cfg(windows)]
            if !el.focus() {
                ax::activate_app(pid);
            }
            #[cfg(windows)]
            let _ = el.set_range("AXSelectedTextRange", loc, len);
            #[cfg(target_os = "macos")]
            ax::activate_app(pid);
            sleep(Duration::from_millis(120));
            if let Ok(saved) = paste_text(replacement) {
                let start = Instant::now();
                while start.elapsed() < PASTE_WAIT {
                    sleep(PASTE_POLL);
                    if let Some(new_text) = verified(el, loc, replacement) {
                        restore_clipboard(saved, replacement);
                        return Ok(ApplyResult { method: Method::Paste, new_text });
                    }
                    // Some editors normalise text (smart quotes, trailing space). Accept if the whole value matches the expectation.
                    if let Some(now) = el.string("AXValue") {
                        if now == expected {
                            restore_clipboard(saved, replacement);
                            return Ok(ApplyResult { method: Method::Paste, new_text: now });
                        }
                    }
                }
                restore_clipboard(saved, replacement);
            }
        }
    }
    // Rung 3 (Windows): a plain field with a writable Value takes the whole new text.
    #[cfg(windows)]
    if el.is_settable("AXValue") && el.set_string("AXValue", &expected).is_ok() {
        sleep(Duration::from_millis(70));
        if let Some(new_text) = verified(el, loc, replacement) {
            return Ok(ApplyResult { method: Method::Ax, new_text });
        }
    }
    Err(anyhow!("This app did not accept the replacement. Try editing it directly."))
}

/// Put `text` on the clipboard and send the paste keys. Returns the user's previous clipboard text.
fn paste_text(text: &str) -> Result<Option<String>> {
    use arboard::Clipboard;
    let mut cb = Clipboard::new()?;
    let saved = cb.get_text().ok();
    cb.set_text(text.to_string())?;
    sleep(Duration::from_millis(60));
    post_paste_keys();
    Ok(saved)
}

/// Put the user's clipboard text back, but only if the clipboard still holds our replacement.
fn restore_clipboard(saved: Option<String>, replacement: &str) {
    use arboard::Clipboard;
    let Some(prev) = saved else { return };
    let Ok(mut cb) = Clipboard::new() else { return };
    if cb.get_text().ok().as_deref() == Some(replacement) {
        let _ = cb.set_text(prev);
    }
}
