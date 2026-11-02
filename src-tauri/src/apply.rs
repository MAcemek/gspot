//! The replacement ladder: AX selected-text set, then pasteboard patch, each
//! verified by reading the field back (what Grammarly's TextCorruptionDetector does).

#![cfg(any(target_os = "macos", windows))]

#[cfg(target_os = "macos")]
use crate::ax;
use crate::ax::AXElement;
use crate::textmap::{slice_utf16, splice_utf16};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::thread::sleep;
use std::time::Duration;
#[cfg(target_os = "macos")]
use std::time::Instant;

/// How long to wait for a pasted fix to show up; slow apps such as Word read the clipboard late.
#[cfg(target_os = "macos")]
const PASTE_WAIT: Duration = Duration::from_millis(2000);
/// How often to re-check the field while waiting for a pasted fix.
#[cfg(target_os = "macos")]
const PASTE_POLL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Ax,
    Paste,
    Type,
    Word,
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

/// Keystrokes that type `text`: Unicode key events, Enter for newlines, Delete for empty text.
#[cfg(windows)]
fn type_inputs(text: &str) -> Vec<windows::Win32::UI::Input::KeyboardAndMouse::INPUT> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_DELETE, VK_RETURN};
    let key = |vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let mut out = Vec::new();
    if text.is_empty() {
        out.push(key(VK_DELETE, 0, KEYBD_EVENT_FLAGS(0)));
        out.push(key(VK_DELETE, 0, KEYEVENTF_KEYUP));
        return out;
    }
    for u in text.encode_utf16() {
        match u {
            0x0D => {}
            0x0A => {
                out.push(key(VK_RETURN, 0, KEYBD_EVENT_FLAGS(0)));
                out.push(key(VK_RETURN, 0, KEYEVENTF_KEYUP));
            }
            _ => {
                out.push(key(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE));
                out.push(key(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
    }
    out
}

/// Send input events in chunks so a long fix never overruns the input queue.
#[cfg(windows)]
fn send_inputs(inputs: &[windows::Win32::UI::Input::KeyboardAndMouse::INPUT]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT};
    for chunk in inputs.chunks(400) {
        unsafe { SendInput(chunk, std::mem::size_of::<INPUT>() as i32) };
    }
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

/// Where the target sits: `doc_loc` in the text GSpot read (`AXValue`), `ui_loc` in
/// the field's UI Automation range coordinates (they differ in Word once bullets,
/// tables or fields are involved).
#[cfg(windows)]
struct Resolved {
    doc_loc: usize,
    ui_loc: usize,
    base: String,
}

/// Make sure the range still holds the original text; if not, look for it nearby
/// in the live text (and, if UIA's offsets are skewed, nearby in UIA coordinates).
#[cfg(windows)]
fn resolve_target(el: &AXElement, loc: usize, len: usize, before: &str) -> Result<Resolved> {
    use crate::word_com::{norm_eq, relocate};
    let changed = || anyhow!("The text changed since this suggestion was made. Try again.");
    let want = slice_utf16(before, loc, loc + len);
    let range_ok = |l: usize| -> Option<bool> { el.text_range(l, len).map(|got| norm_eq(&got, &want)) };
    if range_ok(loc) == Some(true) {
        return Ok(Resolved { doc_loc: loc, ui_loc: loc, base: before.to_string() });
    }
    let live = el.string("AXValue").ok_or_else(|| anyhow!("The text field is no longer available."))?;
    // Where the original text is in the live document text.
    let doc_loc = if norm_eq(&slice_utf16(&live, loc, loc + len), &want) { loc } else { relocate(&live, &want, loc, 200).ok_or_else(changed)? };
    match range_ok(doc_loc) {
        Some(true) => return Ok(Resolved { doc_loc, ui_loc: doc_loc, base: live }),
        None => return Ok(Resolved { doc_loc, ui_loc: doc_loc, base: live }), // no range support: trust the text
        Some(false) => {}
    }
    // UIA's offsets disagree with the text (list bullets, tables...): find the range that holds it.
    for d in 1..=64usize {
        for cand in [Some(doc_loc + d), doc_loc.checked_sub(d)].into_iter().flatten() {
            if range_ok(cand) == Some(true) {
                return Ok(Resolved { doc_loc, ui_loc: cand, base: live });
            }
        }
    }
    Err(changed())
}

/// Windows: Word through its COM object model first; otherwise select the range
/// and type the fix as Unicode keystrokes (no clipboard, no whole-field reads).
#[cfg(windows)]
pub fn replace_range(el: &AXElement, _pid: i32, loc: usize, len: usize, replacement: &str, before: &str) -> Result<ApplyResult> {
    use crate::word_com::{norm_eq, try_replace, Outcome};
    let exe = crate::ax::exe_name(el.pid()).unwrap_or_default();
    // Word: edit the document itself. It does not need to be in front.
    if exe == "winword.exe" {
        let in_front = crate::ax::frontmost_app().map(|(pid, _, _)| pid) == Some(el.pid());
        match try_replace(el.top_level_hwnd(), in_front, loc, len, replacement, before) {
            Outcome::Done { new_text } => return Ok(ApplyResult { method: Method::Word, new_text }),
            Outcome::Failed(msg) => return Err(anyhow!(msg)),
            Outcome::NotHandled(why) => log::debug!("word com not used: {why}"),
        }
    }
    let r = resolve_target(el, loc, len, before)?;
    let (loc, before) = (r.ui_loc, r.base.as_str());
    let doc_loc = r.doc_loc;
    // Keystrokes go to the foreground window, so the field's app must be in front, not our panel.
    let in_front = crate::ax::frontmost_app().map(|(pid, _, _)| pid) == Some(el.pid());
    if !el.has_focus() || !in_front {
        el.focus();
        sleep(Duration::from_millis(40));
        if crate::ax::frontmost_app().map(|(pid, _, _)| pid) != Some(el.pid()) {
            return Err(anyhow!("Could not bring the app to the front. Click into your text and try again."));
        }
    }
    if el.set_range("AXSelectedTextRange", loc, len).is_err() {
        // Fallback: a plain field with a writable Value takes the whole new text.
        let live = el.string("AXValue").ok_or_else(|| anyhow!("The text field is no longer available."))?;
        let expected = splice_utf16(&live, doc_loc, len, replacement);
        if el.is_settable("AXValue") && el.set_string("AXValue", &expected).is_ok() {
            sleep(Duration::from_millis(70));
            if let Some(new_text) = verified(el, doc_loc, replacement) {
                return Ok(ApplyResult { method: Method::Ax, new_text });
            }
        }
        return Err(anyhow!("This app did not accept the replacement. Try editing it directly."));
    }
    send_inputs(&type_inputs(replacement));
    let new_text = splice_utf16(before, doc_loc, len, replacement);
    if replacement.is_empty() {
        return Ok(ApplyResult { method: Method::Type, new_text });
    }
    // Word, Outlook and friends autocorrect (smart quotes, dashes) and read input late.
    let wait = if matches!(exe.as_str(), "winword.exe" | "outlook.exe") { 800 } else { 400 };
    let n = replacement.encode_utf16().count();
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(wait) {
        sleep(Duration::from_millis(30));
        if el.text_range(loc, n).map_or(false, |got| norm_eq(&got, replacement)) {
            return Ok(ApplyResult { method: Method::Type, new_text });
        }
    }
    Err(anyhow!("This app did not accept the replacement. Try editing it directly."))
}

/// Replace `len` UTF-16 units at `loc` with `replacement` in the host field.
#[cfg(target_os = "macos")]
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
    Err(anyhow!("This app did not accept the replacement. Try editing it directly."))
}

/// Put `text` on the clipboard and send the paste keys. Returns the user's previous clipboard text.
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
fn restore_clipboard(saved: Option<String>, replacement: &str) {
    use arboard::Clipboard;
    let Some(prev) = saved else { return };
    let Ok(mut cb) = Clipboard::new() else { return };
    if cb.get_text().ok().as_deref() == Some(replacement) {
        let _ = cb.set_text(prev);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_DELETE, VK_RETURN};

    fn vk(i: &INPUT) -> u16 {
        unsafe { i.Anonymous.ki.wVk.0 }
    }
    fn flags(i: &INPUT) -> u32 {
        unsafe { i.Anonymous.ki.dwFlags.0 }
    }

    #[test]
    fn types_plain_text() {
        let v = type_inputs("ab");
        assert_eq!(v.len(), 4);
        assert!(v.iter().all(|i| flags(i) & KEYEVENTF_UNICODE.0 != 0));
    }

    #[test]
    fn newline_is_return() {
        let v = type_inputs("a\nb");
        assert_eq!(v.len(), 6);
        assert_eq!(vk(&v[2]), VK_RETURN.0);
        assert_eq!(vk(&v[3]), VK_RETURN.0);
        assert_eq!(flags(&v[3]) & KEYEVENTF_KEYUP.0, KEYEVENTF_KEYUP.0);
    }

    #[test]
    fn empty_is_delete() {
        let v = type_inputs("");
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|i| vk(i) == VK_DELETE.0));
    }

    #[test]
    fn long_text_two_inputs_per_unit() {
        assert_eq!(type_inputs(&"x".repeat(1000)).len(), 2000);
    }
}
