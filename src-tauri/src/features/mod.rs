//! LLM-backed sidebar features. Each returns a JSON value the UI renders.

pub mod ceo;
pub mod detector;
pub mod plagiarism;
pub mod prompts;
pub mod spotcheck;

use crate::llm::{self, Message};
use crate::settings::LlmConfig;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max_chars).collect();
        s.push_str("\n[…]");
        s
    }
}

/// Rewrites run paragraph-chunk by paragraph-chunk (at most this many chars per
/// call) so a small model cannot take a long text and "summarize" it.
pub const REWRITE_CHUNK_CHARS: usize = 900;

/// Whole paragraphs grouped into chunks of at most `max` chars, each with the
/// separator that followed it. A single paragraph longer than `max` is cut at
/// sentence boundaries.
pub fn chunk_paragraphs(text: &str, max: usize) -> Vec<(String, String)> {
    let mut paras: Vec<(String, String)> = Vec::new();
    let mut cur = String::new();
    let mut sep = String::new();
    let mut in_sep = false;
    for line in text.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        if body.trim().is_empty() {
            in_sep = true;
            sep.push_str(body);
            sep.push_str(nl);
            continue;
        }
        if in_sep {
            paras.push((std::mem::take(&mut cur), std::mem::take(&mut sep)));
            in_sep = false;
        } else if !sep.is_empty() {
            cur.push_str(&sep); // a single newline inside a paragraph
            sep.clear();
        }
        cur.push_str(body);
        sep.push_str(nl);
    }
    if !cur.is_empty() || !sep.is_empty() {
        paras.push((cur, sep));
    }
    // Cut oversized paragraphs at sentence boundaries.
    let mut pieces: Vec<(String, String)> = Vec::new();
    for (p, s) in paras {
        if p.chars().count() <= max {
            pieces.push((p, s));
            continue;
        }
        let chars: Vec<char> = p.chars().collect();
        let sents = spotcheck::split_sentences(&p);
        let mut from = 0usize;
        let mut acc_start = 0usize;
        for (i, sent) in sents.iter().enumerate() {
            let end = if i + 1 < sents.len() { sents[i + 1].start } else { chars.len() };
            if end - acc_start > max && sent.start > acc_start {
                pieces.push((chars[acc_start..sent.start].iter().collect(), String::new()));
                acc_start = sent.start;
            }
            from = end;
        }
        let _ = from;
        pieces.push((chars[acc_start..].iter().collect(), s));
    }
    // Group small paragraphs together up to `max`.
    let mut out: Vec<(String, String)> = Vec::new();
    for (p, s) in pieces {
        if let Some(last) = out.last_mut() {
            if last.0.chars().count() + last.1.chars().count() + p.chars().count() <= max {
                last.0.push_str(&last.1);
                last.0.push_str(&p);
                last.1 = s;
                continue;
            }
        }
        out.push((p, s));
    }
    out
}

pub fn count_sentences(s: &str) -> usize {
    let n = spotcheck::split_sentences(s).len();
    if n == 0 && !s.trim().is_empty() { 1 } else { n }
}

/// A rewrite that keeps the shape of the input: within ±25 % of the sentence
/// count (lenient styles like Shorten may halve it) and not empty.
pub fn preserves_sentences(orig: &str, out: &str, lenient: bool) -> bool {
    if out.trim().is_empty() {
        return false;
    }
    let (a, b) = (count_sentences(orig) as f64, count_sentences(out) as f64);
    if a < 2.0 {
        return b <= 2.0;
    }
    if lenient {
        return b >= a * 0.5;
    }
    b >= a * 0.75 && b <= a * 1.5
}

fn closer(orig: &str, a: &str, b: &str) -> bool {
    let n = count_sentences(orig) as i64;
    (count_sentences(a) as i64 - n).abs() < (count_sentences(b) as i64 - n).abs()
}

/// One chunk through `system`, retried once with the keep-every-sentence rule
/// when the model dropped sentences. Returns (rewritten, whole JSON, retried).
async fn rewrite_chunk(cfg: &LlmConfig, system: &str, user: &str, temperature: f32, chunk: &str, lenient: bool) -> Result<(String, Value, bool)> {
    let v = llm::complete_json(cfg, system, user, temperature).await?;
    let mut out = v["rewritten"].as_str().unwrap_or("").trim().to_string();
    let mut retried = false;
    if !preserves_sentences(chunk, &out, lenient) {
        retried = true;
        let n = count_sentences(chunk);
        let strict = format!("{system}\n{}", prompts::KEEP_EVERY_SENTENCE.replace("{n}", &n.to_string()));
        if let Ok(v2) = llm::complete_json(cfg, &strict, user, (temperature - 0.2).max(0.1)).await {
            let out2 = v2["rewritten"].as_str().unwrap_or("").trim().to_string();
            if !out2.is_empty() && (out.is_empty() || closer(chunk, &out2, &out)) {
                out = out2;
            }
        }
    }
    Ok((out, v, retried))
}

pub async fn paraphrase(cfg: &LlmConfig, text: &str, style: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Err(anyhow!("Select or write some text first."));
    }
    let lenient = matches!(style, "Shorten" | "Streamlined" | "Expand");
    let text = clip(text, 12000);
    let chunks = chunk_paragraphs(&text, REWRITE_CHUNK_CHARS);
    let mut rewritten = String::new();
    let mut summary = String::new();
    let mut retries = 0usize;
    for (i, (chunk, sep)) in chunks.iter().enumerate() {
        if chunk.trim().is_empty() {
            rewritten.push_str(chunk);
            rewritten.push_str(sep);
            continue;
        }
        let user = format!("STYLE: {style}\n\nTEXT:\n{chunk}");
        let (out, v, retried) = rewrite_chunk(cfg, prompts::PARAPHRASE_SYSTEM, &user, 0.5, chunk, lenient).await?;
        if retried {
            retries += 1;
        }
        if i == 0 {
            summary = v["summary"].as_str().unwrap_or("").to_string();
        }
        rewritten.push_str(&out);
        rewritten.push_str(sep);
    }
    let rewritten = rewritten.trim_end().to_string();
    Ok(json!({
        "rewritten": rewritten,
        "summary": summary,
        "style": style,
        "chunks": chunks.len(),
        "retries": retries,
        "sentences_in": count_sentences(&text),
        "sentences_out": count_sentences(&rewritten),
    }))
}

pub async fn humanize(cfg: &LlmConfig, text: &str, voice_name: &str, voice_description: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Err(anyhow!("Select or write some text first."));
    }
    let text = clip(text, 12000);
    let chunks = chunk_paragraphs(&text, REWRITE_CHUNK_CHARS);
    let mut rewritten = String::new();
    let mut changes: Vec<Value> = Vec::new();
    let mut retries = 0usize;
    for (chunk, sep) in &chunks {
        if chunk.trim().is_empty() {
            rewritten.push_str(chunk);
            rewritten.push_str(sep);
            continue;
        }
        let user = format!("VOICE: {voice_name}\nVOICE GUIDE: {voice_description}\n\nTEXT:\n{chunk}");
        let (out, v, retried) = rewrite_chunk(cfg, prompts::HUMANIZE_SYSTEM, &user, 0.6, chunk, false).await?;
        if retried {
            retries += 1;
        }
        if let Some(c) = v["changes"].as_array() {
            changes.extend(c.iter().cloned());
        }
        rewritten.push_str(&out);
        rewritten.push_str(sep);
    }
    changes.truncate(4);
    let rewritten = rewritten.trim_end().to_string();
    Ok(json!({
        "rewritten": rewritten,
        "changes": changes,
        "voice": voice_name,
        "chunks": chunks.len(),
        "retries": retries,
        "sentences_in": count_sentences(&text),
        "sentences_out": count_sentences(&rewritten),
    }))
}

pub async fn create_voice(cfg: &LlmConfig, sample: &str, name_hint: &str) -> Result<Value> {
    let v = llm::complete_json(cfg, prompts::CREATE_VOICE_SYSTEM, &format!("NAME HINT: {name_hint}\n\nSAMPLE:\n{}", clip(sample, 8000)), 0.4).await?;
    Ok(json!({"name": v["name"].as_str().unwrap_or("My voice"), "description": v["description"].as_str().unwrap_or("")}))
}

pub async fn reader_reactions(cfg: &LlmConfig, text: &str, audience: &str, audience_desc: &str) -> Result<Value> {
    let words = crate::score::word_count(text);
    if words < 25 {
        return Ok(json!({"too_short": true, "min_words": 25}));
    }
    let v = llm::complete_json(
        cfg,
        prompts::REACTIONS_SYSTEM,
        &format!("AUDIENCE: {audience}\nAUDIENCE DETAILS: {audience_desc}\n\nTEXT:\n{}", clip(text, 12000)),
        0.5,
    )
    .await?;
    Ok(json!({
        "summary": v["summary"].as_str().unwrap_or(""),
        "remember": v["remember"].as_array().cloned().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>(),
        "ask": v["ask"].as_array().cloned().unwrap_or_default().into_iter().take(5).collect::<Vec<_>>(),
        "audience": audience,
    }))
}

pub async fn translate(cfg: &LlmConfig, text: &str, target: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Err(anyhow!("Select some text first."));
    }
    let v = llm::complete_json(cfg, prompts::TRANSLATE_SYSTEM, &format!("TARGET LANGUAGE: {target}\n\nTEXT:\n{}", clip(text, 12000)), 0.2).await?;
    Ok(json!({
        "translation": v["translation"].as_str().unwrap_or("").trim(),
        "detected_language": v["detected_language"].as_str().unwrap_or(""),
        "idioms": v["idioms"].as_array().cloned().unwrap_or_default(),
        "target": target,
    }))
}

pub async fn explain(cfg: &LlmConfig, text: &str, whole_page: bool) -> Result<Value> {
    if text.trim().is_empty() {
        return Err(anyhow!("Highlight some text first."));
    }
    let mode = if whole_page { "WHOLE PAGE" } else { "SELECTION" };
    let v = llm::complete_json(cfg, prompts::EXPLAIN_SYSTEM, &format!("MODE: {mode}\n\nTEXT:\n{}", clip(text, 12000)), 0.3).await?;
    Ok(json!({
        "summary": v["summary"].as_str().unwrap_or(""),
        "explanation": v["explanation"].as_str().unwrap_or(""),
        "terms": v["terms"].as_array().cloned().unwrap_or_default(),
    }))
}

pub async fn tone(cfg: &LlmConfig, text: &str) -> Result<Value> {
    let words = crate::score::word_count(text);
    if words < 12 {
        return Ok(json!({"too_short": true}));
    }
    let v = llm::complete_json(cfg, prompts::TONE_SYSTEM, &format!("TEXT:\n{}", clip(text, 8000)), 0.2).await?;
    let mut tones: Vec<Value> = v["tones"].as_array().cloned().unwrap_or_default();
    tones.truncate(3);
    for t in tones.iter_mut() {
        if let Some(o) = t.as_object_mut() {
            o.remove("emoji");
        }
    }
    Ok(json!({"tones": tones, "summary": v["summary"].as_str().unwrap_or("")}))
}

pub async fn rewrite_phrase(cfg: &LlmConfig, sentence: &str, phrase: &str) -> Result<Value> {
    let v = llm::complete_json(cfg, prompts::REWRITE_PHRASE_SYSTEM, &format!("PHRASE: {phrase}\n\nSENTENCE:\n{sentence}"), 0.6).await?;
    let mut s: Vec<Value> = v["suggestions"].as_array().cloned().unwrap_or_default();
    s.truncate(3);
    Ok(json!({"suggestions": s}))
}

pub async fn summarize(cfg: &LlmConfig, text: &str) -> Result<String> {
    let r = llm::complete(cfg, prompts::SUMMARIZE_SYSTEM, &[llm::msg("user", clip(text, 16000))], false, 0.3).await?;
    Ok(r.trim().to_string())
}

/// Chat system prompt with the current document as context.
pub fn chat_system(user_name: &str, app_name: &str, document: &str, selection: &str) -> String {
    let mut s = prompts::CHAT_SYSTEM.replace("{name}", user_name).replace("{app}", if app_name.is_empty() { "an app" } else { app_name });
    if !document.trim().is_empty() {
        s.push_str("\n\nThe user's current document (from ");
        s.push_str(app_name);
        s.push_str("):\n\"\"\"\n");
        s.push_str(&clip(document, 12000));
        s.push_str("\n\"\"\"");
    }
    if !selection.trim().is_empty() {
        s.push_str("\n\nThe user's current selection:\n\"\"\"\n");
        s.push_str(&clip(selection, 4000));
        s.push_str("\n\"\"\"");
    }
    s
}

pub fn as_messages(v: &Value) -> Vec<Message> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| Some(Message { role: m["role"].as_str()?.to_string(), content: m["content"].as_str()?.to_string() }))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_paragraphs_whole_and_separators_intact() {
        let text = "Para one sentence A. Para one sentence B.\n\nPara two.\n\n\nPara three is here.";
        let c = chunk_paragraphs(text, 900);
        assert_eq!(c.len(), 1, "small paragraphs group into one chunk: {c:?}");
        let joined: String = c.iter().map(|(p, s)| format!("{p}{s}")).collect();
        assert_eq!(joined, text);
        let c2 = chunk_paragraphs(text, 30);
        assert_eq!(c2.len(), 3);
        assert_eq!(c2[0].0, "Para one sentence A. Para one sentence B.");
        assert_eq!(c2[0].1, "\n\n");
        assert_eq!(c2[1].1, "\n\n\n");
        let joined2: String = c2.iter().map(|(p, s)| format!("{p}{s}")).collect();
        assert_eq!(joined2, text);
    }

    #[test]
    fn an_oversized_paragraph_is_cut_at_sentence_boundaries() {
        let text: String = (0..12).map(|i| format!("Sentence number {i} is right here. ")).collect();
        let c = chunk_paragraphs(text.trim_end(), 120);
        assert!(c.len() >= 3, "{}", c.len());
        for (p, _) in &c {
            assert!(p.trim_end().ends_with('.'), "cut mid-sentence: {p:?}");
            assert!(p.chars().count() <= 160);
        }
        let joined: String = c.iter().map(|(p, s)| format!("{p}{s}")).collect();
        assert_eq!(joined, text.trim_end());
    }

    #[test]
    fn sentence_preservation_rule() {
        let orig = "One here. Two here. Three here. Four here.";
        assert!(preserves_sentences(orig, "Uno here. Dos here. Tres here. Cuatro here.", false));
        assert!(!preserves_sentences(orig, "All four, shorter.", false), "a 4→1 cut is a summary, not a paraphrase");
        assert!(preserves_sentences(orig, "One and two. Three and four.", true), "Shorten may merge");
        assert!(!preserves_sentences(orig, "", true));
        assert!(preserves_sentences("Just one sentence.", "Only one sentence.", false));
    }
}
