//! Talk like a CEO: the model cuts the message down to the ask, then a
//! deterministic pass roughens it the way a busy executive types (dropped
//! periods, "&", "w/", lower-case starts, apostrophes skipped). The model never
//! invents the imperfections, so it cannot slip a typo into a name or a number;
//! the same text and level always come back with the same rough edges.

use super::{clip, prompts};
use crate::llm;
use crate::settings::LlmConfig;
use anyhow::{anyhow, Result};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// How rough the result reads: `polished` (short, clean), `casual`, `phone` (typed on a phone between meetings).
pub const LEVELS: &[&str] = &["polished", "casual", "phone"];

/// First words that read naturally in lower case. A sentence that starts with
/// anything else (a name, "I", an acronym) keeps its capital.
const LOWER_OK: &[&str] = &[
    "the", "this", "that", "these", "we", "let", "let's", "lets", "can", "can't", "cant", "will", "please", "need", "looks", "sounds", "thanks",
    "ok", "okay", "yes", "no", "just", "quick", "going", "sure", "great", "good", "send", "ship", "call", "loop", "what", "when", "where",
    "who", "why", "how", "it", "it's", "its", "is", "are", "do", "don't", "dont", "did", "should", "would", "could", "my", "our", "your", "if", "also",
    "agree", "approved", "done", "nice", "love", "happy", "move", "push", "pls", "thx", "all", "any", "one", "two", "next", "fine", "got", "make",
];

/// Contractions a CEO types without the apostrophe.
const BARE_CONTRACTIONS: &[(&str, &str)] = &[
    ("don't", "dont"),
    ("can't", "cant"),
    ("won't", "wont"),
    ("didn't", "didnt"),
    ("isn't", "isnt"),
    ("doesn't", "doesnt"),
    ("that's", "thats"),
    ("let's", "lets"),
    ("we're", "were"),
    ("you're", "youre"),
];

fn numbers_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d+(?:[.,:/]\d+)*").unwrap())
}

/// Numbers (prices, dates, times) in `original` that `out` lost.
pub fn missing_numbers(original: &str, out: &str) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for m in numbers_re().find_iter(original) {
        let n = m.as_str().to_string();
        if !out.contains(&n) && !missing.contains(&n) {
            missing.push(n);
        }
    }
    missing
}

fn seed_for(text: &str, level: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    level.hash(&mut h);
    h.finish()
}

/// Replace up to `max` whole-word, case-sensitive occurrences of `from`, each with probability `p`.
fn swap_words(text: &str, from: &str, to: &str, max: usize, p: f64, rng: &mut StdRng) -> String {
    let re = Regex::new(&format!(r"\b{}\b", regex::escape(from))).unwrap();
    let mut done = 0usize;
    re.replace_all(text, |c: &regex::Captures| {
        if done < max && rng.gen_bool(p) {
            done += 1;
            to.to_string()
        } else {
            c[0].to_string()
        }
    })
    .into_owned()
}

/// Byte offsets where a sentence starts: the text start, after a newline, or after ". ", "! ", "? ".
fn sentence_starts(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at_start = true;
    let mut after_stop = false;
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            at_start = true;
            after_stop = false;
            continue;
        }
        if ch.is_whitespace() {
            if after_stop {
                at_start = true;
                after_stop = false;
            }
            continue;
        }
        if at_start && ch.is_alphabetic() {
            out.push(i);
        }
        at_start = false;
        after_stop = matches!(ch, '.' | '!' | '?');
    }
    out
}

fn lower_sentence_starts(text: &str) -> String {
    let mut s = text.to_string();
    for &i in sentence_starts(text).iter().rev() {
        let word: String = s[i..].chars().take_while(|c| c.is_alphanumeric() || *c == '\'').collect();
        if LOWER_OK.contains(&word.to_lowercase().as_str()) {
            let first = s[i..].chars().next().unwrap();
            s.replace_range(i..i + first.len_utf8(), &first.to_lowercase().to_string());
        }
    }
    s
}

/// Drop the period that ends a line (and the text), keeping "?", "!" and "...".
fn drop_line_periods(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let t = line.trim_end();
            if t.ends_with('.') && !t.ends_with("..") {
                t[..t.len() - 1].to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The deterministic CEO pass. `polished` leaves the text alone.
pub fn roughen(text: &str, level: &str) -> String {
    let mut rng = StdRng::seed_from_u64(seed_for(text, level));
    let mut s = text.trim().to_string();
    match level {
        "casual" => {
            s = swap_words(&s, "and", "&", 2, 0.6, &mut rng);
            s = swap_words(&s, "with", "w/", 1, 0.7, &mut rng);
            if s.ends_with('.') && !s.ends_with("..") {
                s.pop();
            }
        }
        "phone" => {
            s = swap_words(&s, "and", "&", 3, 0.8, &mut rng);
            s = swap_words(&s, "with", "w/", 2, 0.9, &mut rng);
            s = swap_words(&s, "thanks", "thx", 1, 1.0, &mut rng);
            s = swap_words(&s, "Thanks", "thx", 1, 1.0, &mut rng);
            s = swap_words(&s, "please", "pls", 1, 0.8, &mut rng);
            s = swap_words(&s, "Please", "pls", 1, 0.8, &mut rng);
            for (from, to) in BARE_CONTRACTIONS {
                s = swap_words(&s, from, to, 9, 0.6, &mut rng);
            }
            s = drop_line_periods(&s);
            s = lower_sentence_starts(&s);
        }
        _ => {}
    }
    s
}

/// Rewrite `text` the way a CEO would send it.
pub async fn rewrite(cfg: &LlmConfig, text: &str, level: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Err(anyhow!("Select or write some text first."));
    }
    let level = if LEVELS.contains(&level) { level } else { "casual" };
    let text = clip(text, 6000);
    let user = format!("MESSAGE:\n{text}");
    let mut v = llm::complete_json(cfg, prompts::CEO_SYSTEM, &user, 0.4).await?;
    let mut short = v["rewritten"].as_str().unwrap_or("").trim().to_string();
    let mut missing = missing_numbers(&text, &short);
    let mut retried = false;
    if short.is_empty() || !missing.is_empty() {
        retried = true;
        let fix = format!("{user}\n\nYour previous version dropped these numbers or was empty: {}. Keep every number, date and amount.", missing.join(", "));
        v = llm::complete_json(cfg, prompts::CEO_SYSTEM, &fix, 0.2).await?;
        short = v["rewritten"].as_str().unwrap_or("").trim().to_string();
        missing = missing_numbers(&text, &short);
    }
    if short.is_empty() {
        return Err(anyhow!("The model returned nothing. Try again."));
    }
    let rewritten = roughen(&short, level);
    let words_in = crate::score::word_count(&text);
    let words_out = crate::score::word_count(&rewritten);
    Ok(json!({
        "rewritten": rewritten,
        "clean": short,
        "cut": v["cut"].as_array().cloned().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>(),
        "level": level,
        "words_in": words_in,
        "words_out": words_out,
        "missing_numbers": missing,
        "retried": retried,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polished_is_untouched() {
        let t = "Ship it Friday. Budget is $40,000.";
        assert_eq!(roughen(t, "polished"), t);
    }

    #[test]
    fn roughening_is_deterministic() {
        let t = "Thanks for the deck. Let's move the launch to 3/14 and loop in Dana with the numbers.";
        assert_eq!(roughen(t, "phone"), roughen(t, "phone"));
    }

    #[test]
    fn phone_keeps_names_numbers_and_capital_i() {
        let t = "Dana, I need the numbers by 5:30.\nThe Q3 budget is $40,000. Please send it with the deck.";
        let r = roughen(t, "phone");
        assert!(r.contains("Dana"), "{r}");
        assert!(r.contains("I need"), "{r}");
        assert!(r.contains("5:30"), "{r}");
        assert!(r.contains("$40,000"), "{r}");
        assert!(r.contains("Q3"), "{r}");
        assert!(r.starts_with("Dana"), "names keep their capital: {r}");
        assert!(r.contains("\nthe Q3"), "common starters go lower case: {r}");
        assert!(!r.ends_with('.'), "{r}");
    }

    #[test]
    fn casual_only_drops_the_final_period() {
        let r = roughen("Looks good. Ship it.", "casual");
        assert_eq!(r, "Looks good. Ship it");
    }

    #[test]
    fn question_marks_survive() {
        assert!(roughen("Can we ship Friday?", "phone").ends_with('?'));
    }

    #[test]
    fn finds_dropped_numbers() {
        assert_eq!(missing_numbers("Meet at 3:30 on 10/31, budget 1,200.", "meet 3:30, budget 1,200"), vec!["10/31".to_string()]);
        assert!(missing_numbers("No numbers here.", "none").is_empty());
    }
}
