//! COM-only probe of the Word edit path (no keystrokes, no mouse). Creates its own
//! invisible document, edits it, closes it WITHOUT saving; quits Word only if it
//! started it. Run: cargo run --example word_probe
#![cfg(windows)]

use grammarlama_lib::word_com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::core::BSTR;

fn find(text: &str, needle: &str) -> (usize, usize) {
    let b = text.find(needle).expect("needle");
    (text[..b].encode_utf16().count(), needle.encode_utf16().count())
}

fn main() {
    init_com();
    let (app, started) = match attach_word() {
        Some(a) => (a, false),
        None => (create_word().expect("could not start Word"), true),
    };
    if started {
        let _ = app.put("Visible", VARIANT::from(false));
    }
    let docs = app.get_disp("Documents").unwrap();
    // Documents.Add(Template, NewTemplate, DocumentType, Visible=false): no window, nothing the user sees.
    let doc = docs.call_disp("Add", &[missing(), missing(), missing(), VARIANT::from(false)]).unwrap();
    let text = "Its a nice day.\rCheck your work.\rMe and him went home.\rI recieved teh file.\r";
    doc.get_disp("Content").unwrap().put("Text", VARIANT::from(BSTR::from(text))).unwrap();
    let p3 = doc.get_disp("Paragraphs").unwrap().call_disp("Item", &[VARIANT::from(3i32)]).unwrap().get_disp("Range").unwrap();
    p3.get_disp("ListFormat").unwrap().call("ApplyBulletDefault", &[]).unwrap();
    println!("autoformat quotes: {:?}", app.get_disp("Options").and_then(|o| o.get_bool("AutoFormatAsYouTypeReplaceQuotes")));

    let content = |d: &Disp| d.get_disp("Content").unwrap().get_str("Text").unwrap();
    // Text as GSpot would see it with bullets (UIA exposes the glyph + tab).
    let with_bullets = |t: &str| t.replacen("Me and him", "\u{f0b7}\tMe and him", 1);

    let cases: [(&str, &str, &str, bool); 4] = [
        ("Its", "It's", "plain", false),
        ("your", "you're", "plain", false),
        ("Me and him", "He and I", "bullet (UIA-style text with glyph)", true),
        ("teh", "the", "after bullet para, glyph text", true),
    ];
    for (orig, rep, label, bullets) in cases {
        let live = content(&doc);
        let before = if bullets { with_bullets(&live) } else { live };
        let (loc, len) = find(&before, orig);
        let out = replace_in_doc(&doc, loc, len, rep, &before);
        println!("[{label}] {orig:?} -> {rep:?} @{loc}: {out:?}");
    }
    println!("final: {:?}", content(&doc));
    let _ = doc.call("Close", &[VARIANT::from(0i32)]); // wdDoNotSaveChanges
    if started {
        let _ = app.call("Quit", &[VARIANT::from(0i32)]);
    }
}
