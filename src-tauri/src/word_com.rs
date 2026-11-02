//! Microsoft Word through its own COM object model (the way Grammarly's Word
//! add-in edits): find the paragraph by content, write `Range.Text`, read it back.
//!
//! UI Automation offsets disagree with the document text once list bullets,
//! tables or fields are involved, so nothing here trusts them blindly: the
//! target is located by paragraph index + a content search, and verified
//! against the live Word range before anything is written.

#![cfg(windows)]

use crate::textmap::splice_utf16;
use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::core::{Interface, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::{DISP_E_PARAMNOTFOUND, TYPE_E_TYPEMISMATCH, RPC_E_CALL_REJECTED, RPC_E_SERVERCALL_RETRYLATER};
use windows::Win32::System::Com::{
    CLSIDFromProgID, CoCreateInstance, CoInitializeEx, IDispatch, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO,
};
use windows::Win32::System::Ole::{GetActiveObject, DISPID_PROPERTYPUT};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BSTR, VT_ERROR};

/// Longest we keep retrying a call Word rejects because it is busy.
const BUSY_BUDGET: Duration = Duration::from_millis(500);
/// Hard ceiling for the whole COM edit; a hung Word must never hang GSpot.
const OVERALL_TIMEOUT: Duration = Duration::from_millis(3000);

// ---------------------------------------------------------------------------
// Pure text logic (unit tested, no Word needed)
// ---------------------------------------------------------------------------

/// Map typographic variants onto their plain form, one UTF-16 unit to one unit,
/// so positions in normalized text equal positions in the original.
pub fn norm_unit(u: u16) -> u16 {
    match u {
        0x2018 | 0x2019 | 0x201A | 0x201B | 0x2032 => 0x27,
        0x201C | 0x201D | 0x201E | 0x201F | 0x2033 => 0x22,
        0x00A0 | 0x2007 | 0x2009 | 0x202F => 0x20,
        0x2010..=0x2015 | 0x2212 => 0x2D,
        _ => u,
    }
}

pub fn normalize(s: &str) -> String {
    let units: Vec<u16> = s.encode_utf16().map(norm_unit).collect();
    String::from_utf16_lossy(&units)
}

/// Equal after quote / dash / non-breaking-space normalization.
pub fn norm_eq(a: &str, b: &str) -> bool {
    let (a, b): (Vec<u16>, Vec<u16>) = (a.encode_utf16().collect(), b.encode_utf16().collect());
    units_norm_eq(&a, &b)
}

fn units_norm_eq(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| norm_unit(*x) == norm_unit(*y))
}

/// Start of the occurrence of `needle` in `hay` (normalized compare) nearest to
/// `expected`; ties go to the earlier one. An empty needle answers `expected`
/// clamped to the text.
pub fn find_nearest(hay: &[u16], needle: &[u16], expected: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(expected.min(hay.len()));
    }
    if needle.len() > hay.len() {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    for s in 0..=hay.len() - needle.len() {
        if units_norm_eq(&hay[s..s + needle.len()], needle) {
            let d = s.abs_diff(expected);
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, s));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// Find `want` in `live` within `window` UTF-16 units of `loc`, nearest first.
pub fn relocate(live: &str, want: &str, loc: usize, window: usize) -> Option<usize> {
    let hay: Vec<u16> = live.encode_utf16().collect();
    let needle: Vec<u16> = want.encode_utf16().collect();
    if needle.is_empty() {
        return None;
    }
    let lo = loc.saturating_sub(window);
    let hi = (loc + window + needle.len()).min(hay.len());
    if lo >= hi {
        return None;
    }
    find_nearest(&hay[lo..hi], &needle, loc - lo).map(|p| p + lo).filter(|p| p.abs_diff(loc) <= window)
}

/// Which paragraph (0-based) holds UTF-16 offset `loc` of `text`, and the offset inside it.
/// Breaks: `\r\n`, `\r\x07` (Word's cell end), `\r`, `\n`, `\x07`. A vertical tab
/// (`\x0b`, Word's soft line break) stays inside its paragraph unless `vt_break`.
pub fn paragraph_of(text: &[u16], loc: usize, vt_break: bool) -> (usize, usize) {
    let mut para = 0;
    let mut start = 0;
    let mut i = 0;
    let end = loc.min(text.len());
    while i < end {
        let brk = match text[i] {
            0x0D => {
                if matches!(text.get(i + 1), Some(0x0A) | Some(0x07)) {
                    2
                } else {
                    1
                }
            }
            0x0A | 0x07 => 1,
            0x0B if vt_break => 1,
            _ => 0,
        };
        if brk == 0 {
            i += 1;
            continue;
        }
        if i + brk > loc {
            break; // `loc` sits inside the break itself
        }
        i += brk;
        para += 1;
        start = i;
    }
    (para, loc - start)
}

/// Convert straight quotes to typographic ones, as Word's AutoFormat-as-you-type
/// does. `prev` is the character just before the text (None at a paragraph start).
pub fn smart_quotes(s: &str, prev: Option<char>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut p = prev;
    for c in s.chars() {
        let opening_ctx = match p {
            None => true,
            Some(q) => q.is_whitespace() || matches!(q, '(' | '[' | '{' | '<' | '\u{2014}' | '\u{2013}' | '\u{2018}' | '\u{201C}' | '\u{b}' | '\u{a0}'),
        };
        let r = match c {
            '\'' => {
                if opening_ctx && !p.map_or(false, |q| q.is_alphanumeric()) {
                    '\u{2018}'
                } else {
                    '\u{2019}'
                }
            }
            '"' => {
                if opening_ctx {
                    '\u{201C}'
                } else {
                    '\u{201D}'
                }
            }
            o => o,
        };
        out.push(r);
        p = Some(r);
    }
    out
}

/// Paragraph indices (1-based, within `1..=count`) to try, most likely first:
/// the computed one, its neighbours, then the same under the alternative reading
/// of vertical tabs. Each comes with the offset to expect inside it
/// (`usize::MAX` = the end of a preceding paragraph, 0 = the start of a following one).
fn candidates(before: &[u16], loc: usize, count: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for vt in [false, true] {
        let (p0, off) = paragraph_of(before, loc, vt);
        let p = (p0 + 1) as isize;
        let mut push = |idx: isize, exp: usize| {
            if idx >= 1 && (idx as usize) <= count && !out.iter().any(|(i, _)| *i == idx as usize) {
                out.push((idx as usize, exp));
            }
        };
        push(p, off);
        push(p - 1, usize::MAX);
        push(p + 1, 0);
    }
    out
}

// ---------------------------------------------------------------------------
// IDispatch helpers
// ---------------------------------------------------------------------------

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// A late-bound COM object.
#[derive(Clone)]
pub struct Disp(IDispatch);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// VARIANT for an omitted optional argument.
pub fn missing() -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 { vt: VT_ERROR, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: VARIANT_0_0_0 { scode: DISP_E_PARAMNOTFOUND.0 } }),
        },
    }
}

impl Disp {
    pub fn new(d: IDispatch) -> Disp {
        Disp(d)
    }

    fn invoke(&self, name: &str, flags: DISPATCH_FLAGS, args: &[VARIANT], put: bool) -> windows::core::Result<VARIANT> {
        unsafe {
            let w = wide(name);
            let mut id = 0i32;
            self.0.GetIDsOfNames(&GUID::zeroed(), &PCWSTR(w.as_ptr()), 1, 0, &mut id)?;
            // COM wants the arguments right-to-left.
            let mut rev: Vec<VARIANT> = args.iter().rev().cloned().collect();
            let mut named = DISPID_PROPERTYPUT;
            let params = DISPPARAMS {
                rgvarg: if rev.is_empty() { std::ptr::null_mut() } else { rev.as_mut_ptr() },
                rgdispidNamedArgs: if put { &mut named } else { std::ptr::null_mut() },
                cArgs: rev.len() as u32,
                cNamedArgs: put as u32,
            };
            loop {
                let mut result = VARIANT::default();
                let mut exc = EXCEPINFO::default();
                let r = self.0.Invoke(id, &GUID::zeroed(), 0, flags, &params, Some(&mut result), Some(&mut exc), None);
                ManuallyDrop::drop(&mut exc.bstrSource);
                ManuallyDrop::drop(&mut exc.bstrDescription);
                ManuallyDrop::drop(&mut exc.bstrHelpFile);
                match r {
                    Ok(()) => return Ok(result),
                    Err(e) if e.code() == RPC_E_CALL_REJECTED || e.code() == RPC_E_SERVERCALL_RETRYLATER => {
                        let until = DEADLINE.with(|d| d.get()).unwrap_or_else(|| Instant::now() + BUSY_BUDGET);
                        if Instant::now() >= until {
                            return Err(e);
                        }
                        sleep(Duration::from_millis(40));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> windows::core::Result<VARIANT> {
        self.invoke(name, DISPATCH_PROPERTYGET, &[], false)
    }
    pub fn get_disp(&self, name: &str) -> windows::core::Result<Disp> {
        IDispatch::try_from(&self.get(name)?).map(Disp)
    }
    pub fn get_i32(&self, name: &str) -> windows::core::Result<i32> {
        i32::try_from(&self.get(name)?)
    }
    pub fn get_bool(&self, name: &str) -> windows::core::Result<bool> {
        bool::try_from(&self.get(name)?)
    }
    pub fn get_str(&self, name: &str) -> windows::core::Result<String> {
        let v = self.get(name)?;
        unsafe {
            let inner = &v.Anonymous.Anonymous;
            if inner.vt == VT_BSTR {
                Ok(inner.Anonymous.bstrVal.to_string())
            } else {
                Err(windows::core::Error::from_hresult(TYPE_E_TYPEMISMATCH))
            }
        }
    }
    pub fn call(&self, name: &str, args: &[VARIANT]) -> windows::core::Result<VARIANT> {
        self.invoke(name, DISPATCH_METHOD | DISPATCH_PROPERTYGET, args, false)
    }
    pub fn call_disp(&self, name: &str, args: &[VARIANT]) -> windows::core::Result<Disp> {
        IDispatch::try_from(&self.call(name, args)?).map(Disp)
    }
    pub fn put(&self, name: &str, value: VARIANT) -> windows::core::Result<()> {
        self.invoke(name, DISPATCH_PROPERTYPUT, &[value], true).map(|_| ())
    }
}

/// Join COM on this thread the way `ax_win` does (multithreaded; ignored if already initialised).
pub fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// The running Word (never starts one).
pub fn attach_word() -> Option<Disp> {
    unsafe {
        let clsid = CLSIDFromProgID(PCWSTR(wide("Word.Application").as_ptr())).ok()?;
        let mut unk = None;
        GetActiveObject(&clsid, None, &mut unk).ok()?;
        unk?.cast::<IDispatch>().ok().map(Disp)
    }
}

/// Start a new, invisible Word (probe/testing only).
pub fn create_word() -> Option<Disp> {
    unsafe {
        let clsid = CLSIDFromProgID(PCWSTR(wide("Word.Application").as_ptr())).ok()?;
        let d: IDispatch = CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER).ok()?;
        Some(Disp(d))
    }
}

// ---------------------------------------------------------------------------
// The edit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Written and verified; the document text after the edit.
    Done { new_text: String },
    /// Nothing was changed; let the keystroke path try.
    NotHandled(String),
    /// Word may have been changed or hung: do not retry another way.
    Failed(String),
}

fn i4(n: i32) -> VARIANT {
    VARIANT::from(n)
}

/// Locate `before[loc..loc+len]` in `doc` by content and replace it with `replacement`.
pub fn replace_in_doc(doc: &Disp, loc: usize, len: usize, replacement: &str, before: &str) -> Outcome {
    match replace_in_doc_inner(doc, loc, len, replacement, before) {
        Ok(o) => o,
        Err(e) => Outcome::NotHandled(format!("Word COM: {e}")),
    }
}

fn replace_in_doc_inner(doc: &Disp, loc: usize, len: usize, replacement: &str, before: &str) -> windows::core::Result<Outcome> {
    let before_u: Vec<u16> = before.encode_utf16().collect();
    if loc + len > before_u.len() {
        return Ok(Outcome::NotHandled("range outside the document text".into()));
    }
    let want: Vec<u16> = before_u[loc..loc + len].to_vec();
    let paras = doc.get_disp("Paragraphs")?;
    let count = paras.get_i32("Count")?.max(0) as usize;

    let mut found: Option<(i32, usize, Vec<u16>)> = None; // (abs start, in-paragraph offset, paragraph units)
    for (idx, exp) in candidates(&before_u, loc, count) {
        let range = paras.call_disp("Item", &[i4(idx as i32)])?.get_disp("Range")?;
        let pstart = range.get_i32("Start")?;
        let units: Vec<u16> = range.get_str("Text")?.encode_utf16().collect();
        let expected = if exp == usize::MAX { units.len() } else { exp };
        if let Some(off) = find_nearest(&units, &want, expected) {
            // `off` counts Text units; Start counts document positions (equal outside fields/hidden text; verified below).
            found = Some((pstart + off as i32, off, units));
            break;
        }
    }
    let Some((abs, off, units)) = found else {
        return Ok(Outcome::NotHandled("text not found in the nearby paragraphs".into()));
    };

    let range = doc.call_disp("Range", &[i4(abs), i4(abs + len as i32)])?;
    let now = range.get_str("Text")?;
    let want_s = String::from_utf16_lossy(&want);
    if !norm_eq(&now, &want_s) {
        return Ok(Outcome::NotHandled("Word range does not hold the expected text".into()));
    }

    let smart = doc
        .get_disp("Application")
        .and_then(|a| a.get_disp("Options"))
        .and_then(|o| o.get_bool("AutoFormatAsYouTypeReplaceQuotes"))
        .unwrap_or(false);
    let written = if smart {
        let prev = (off > 0).then(|| char::from_u32(units[off - 1] as u32)).flatten();
        smart_quotes(replacement, prev)
    } else {
        replacement.to_string()
    };

    range.put("Text", VARIANT::from(BSTR::from(written.as_str())))?;
    let after = range.get_str("Text").unwrap_or_default();
    if !norm_eq(&after, &written) {
        return Ok(Outcome::Failed("Word did not keep the replacement".into()));
    }
    Ok(Outcome::Done { new_text: splice_utf16(before, loc, len, &written) })
}

/// Edit the Word window `hwnd` (a top-level window handle) through COM.
fn edit_window(hwnd: Option<isize>, foreground: bool, loc: usize, len: usize, replacement: &str, before: &str) -> Outcome {
    init_com();
    DEADLINE.with(|d| d.set(Some(Instant::now() + BUSY_BUDGET)));
    let Some(app) = attach_word() else {
        return Outcome::NotHandled("Word is not available over COM".into());
    };
    let doc = (|| -> windows::core::Result<Option<Disp>> {
        if let Some(h) = hwnd {
            let wins = app.get_disp("Windows")?;
            let n = wins.get_i32("Count")?;
            for i in 1..=n {
                let Ok(w) = wins.call_disp("Item", &[i4(i)]) else { continue };
                if w.get_i32("Hwnd").map(|x| x as isize == h).unwrap_or(false) {
                    if let Ok(d) = w.get_disp("Document") {
                        return Ok(Some(d));
                    }
                }
            }
        }
        if foreground {
            return app.get_disp("ActiveDocument").map(Some);
        }
        Ok(None)
    })();
    match doc {
        Ok(Some(d)) => replace_in_doc(&d, loc, len, replacement, before),
        Ok(None) => Outcome::NotHandled("no matching Word window".into()),
        Err(e) => Outcome::NotHandled(format!("Word COM: {e}")),
    }
}

/// Try the COM edit on a worker thread with a hard timeout.
pub fn try_replace(hwnd: Option<isize>, foreground: bool, loc: usize, len: usize, replacement: &str, before: &str) -> Outcome {
    let (tx, rx) = std::sync::mpsc::channel();
    let (rep, bef) = (replacement.to_string(), before.to_string());
    std::thread::spawn(move || {
        let _ = tx.send(edit_window(hwnd, foreground, loc, len, &rep, &bef));
    });
    rx.recv_timeout(OVERALL_TIMEOUT).unwrap_or_else(|_| Outcome::Failed("Word did not respond in time.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn normalizes_quotes_dashes_nbsp() {
        assert!(norm_eq("It\u{2019}s", "It's"));
        assert!(norm_eq("\u{201C}done\u{201D}", "\"done\""));
        assert!(norm_eq("a\u{a0}b", "a b"));
        assert!(norm_eq("a\u{2014}b", "a-b"));
        assert!(!norm_eq("It's", "Its"));
        assert_eq!(normalize("\u{2018}x\u{2019}"), "'x'");
    }

    #[test]
    fn smart_quote_conversion() {
        assert_eq!(smart_quotes("It's", None), "It\u{2019}s");
        assert_eq!(smart_quotes("you're", Some(' ')), "you\u{2019}re");
        assert_eq!(smart_quotes("\"done\"", None), "\u{201C}done\u{201D}");
        assert_eq!(smart_quotes("say \"hi\" now", Some('x')), "say \u{201C}hi\u{201D} now");
        assert_eq!(smart_quotes("'tis", Some(' ')), "\u{2018}tis");
        let s90 = smart_quotes("the '90s", None);
        assert!(s90 == "the \u{2018}90s" || s90 == "the \u{2019}90s");
        assert_eq!(smart_quotes("it", Some('a')), "it");
    }

    #[test]
    fn paragraph_index_and_offset() {
        let t = u("ab\rcd\r\nef\u{b}gh\u{7}ij\rkl");
        assert_eq!(paragraph_of(&t, 0, false), (0, 0));
        assert_eq!(paragraph_of(&t, 2, false), (0, 2));
        assert_eq!(paragraph_of(&t, 3, false), (1, 0));
        assert_eq!(paragraph_of(&t, 6, false), (1, 3)); // inside "\r\n"
        let pos = |c: char| t.iter().position(|&x| x == c as u16).unwrap();
        assert_eq!(paragraph_of(&t, pos('e'), false), (2, 0)); // \r\n is one break
        assert_eq!(paragraph_of(&t, pos('g'), false), (2, 3)); // \v stays in the paragraph
        assert_eq!(paragraph_of(&t, pos('g'), true), (3, 0)); // unless asked
        assert_eq!(paragraph_of(&t, pos('i'), false), (3, 0)); // \a is a break
        assert_eq!(paragraph_of(&t, pos('k'), false), (4, 0));
        let c = u("a\r\u{7}b"); // Word's cell end "\r\a" is one break
        assert_eq!(paragraph_of(&c, 3, false), (1, 0));
    }

    #[test]
    fn nearest_occurrence() {
        let hay = u("the cat the dog the end");
        let n = u("the");
        assert_eq!(find_nearest(&hay, &n, 0), Some(0));
        assert_eq!(find_nearest(&hay, &n, 9), Some(8));
        assert_eq!(find_nearest(&hay, &n, 100), Some(16));
        assert_eq!(find_nearest(&hay, &u("zzz"), 0), None);
        assert_eq!(find_nearest(&u("It\u{2019}s ok"), &u("It's"), 0), Some(0));
    }

    #[test]
    fn candidates_cover_neighbours() {
        let t = u("one\rtwo\rthree");
        let c = candidates(&t, 5, 3);
        assert_eq!(c[0], (2, 1));
        assert!(c.iter().any(|(i, _)| *i == 1) && c.iter().any(|(i, _)| *i == 3));
        assert!(c.iter().all(|(i, _)| (1..=3).contains(i)));
    }

    #[test]
    fn relocates_within_window() {
        let live = format!("{}Me and him went{}", "x".repeat(300), "y".repeat(10));
        assert_eq!(relocate(&live, "Me and him", 310, 200), Some(300));
        assert_eq!(relocate(&live, "Me and him", 10, 200), None); // too far
        assert_eq!(relocate("a b a b", "a", 3, 200), Some(4)); // nearest
        assert_eq!(relocate("nothing here", "zzz", 3, 200), None);
        assert_eq!(relocate("It\u{2019}s fine", "It's", 1, 200), Some(0));
    }
}
