//! Spot check: sentence-level Clarity suggestions ("Rephrase for clarity",
//! "Remove the repeated sentence") the way Grammarly's full-sentence rewrites
//! work, built so a small local model cannot "take the whole thing and cut it
//! for clarity":
//!
//! * the document is split into sentences with exact offsets; the model sees
//!   numbered sentences in small batches and must answer one item per number;
//! * a deterministic phrase scanner adds HINTS the model can trust;
//! * every verdict passes guards before it becomes a suggestion: a rephrase
//!   must stay one sentence, keep every number, name and quote, and stay
//!   within a length band; a remove is allowed only for a repeat or a filler
//!   sentence that carries no unique fact; at most half of a batch may change;
//! * verdicts are cached per (previous sentence, sentence), so re-running
//!   after an edit only asks about the sentences that changed.
//!
//! The prompt and the guards were tuned on `tests/e2e/spotcheck.sh`'s corpus
//! (see docs/SPOTCHECK.md for the tuning log).

use crate::engine::{stable_hash, Alert, Category};
use crate::features::prompts;
use crate::llm;
use crate::settings::LlmConfig;
use crate::textmap::TextMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub const MAX_BATCH: usize = 8;
pub const MAX_BATCH_CHARS: usize = 1400;
pub const MIN_WORDS: usize = 4;
pub const MAX_REMOVE_WORDS: usize = 30;
pub const MIN_CONFIDENCE: u64 = 3;
/// At most this share of a batch may get an action (keeps a chatty model honest).
pub const MAX_ACTION_RATIO: f64 = 0.5;
pub const CACHE_MAX: usize = 600;
/// Over this many words a sentence is hinted "too long" and the model may split it.
pub const LONG_WORDS: usize = 30;
/// A rewrite may come back as two or three sentences only when the original had at least this many words.
pub const SPLIT_MIN_WORDS: usize = 24;
pub const MAX_SPLIT: usize = 3;
/// Without an accepted model split, a sentence over this many words gets the deterministic split.
pub const SPLIT_FALLBACK_WORDS: usize = 34;
/// A rewrite must keep at least this share of the original's content words (wordy phrases and hedges excluded).
pub const MIN_CONTENT_KEPT: f64 = 0.6;
/// A rewrite the scanner did not ask for needs this much model confidence.
pub const MIN_CONFIDENCE_UNHINTED: u64 = 4;
/// A sentence the scanner did not flag is left alone under this many words.
pub const MIN_WORDS_UNHINTED: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sentence {
    /// char offsets into the document; `end` is exclusive and excludes trailing whitespace
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub line: usize,
    /// false for fragments the check skips (too short, headings, URLs, code)
    pub judge: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Rephrase { text: String, why: String, flags: Vec<String> },
    Remove { why: String, flags: Vec<String> },
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    pub alerts: Vec<Alert>,
    pub sentences: usize,
    pub checked: usize,
    pub from_cache: usize,
    pub batches: usize,
    pub skipped: usize,
    pub rejected: usize,
    /// Why verdicts were dropped ("LengthBand: <sentence…>"), for the tuning log.
    pub rejections: Vec<String>,
}

pub type Cache = HashMap<u64, Option<Verdict>>;

// ---------------------------------------------------------------- splitting

const TITLE_ABBR: &[&str] = &["mr", "mrs", "ms", "dr", "prof", "st", "mt", "sr", "jr", "vs", "e.g", "i.e", "etc", "no", "u.s", "u.k", "a.m", "p.m", "inc", "ltd", "co", "fig", "approx", "dept", "est", "gen", "gov", "sen", "rep", "capt", "col", "lt", "sgt"];

fn is_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}
fn is_closing(c: char) -> bool {
    matches!(c, '"' | '”' | '\'' | '’' | ')' | ']' | '»')
}

/// Sentences with char offsets. Breaks at `.`/`!`/`?` runs followed by
/// whitespace (plus closing quotes), and at every newline. Known title
/// abbreviations ("Dr.", "e.g.") and a period followed by a lowercase letter
/// ("i.e. the") never break.
pub fn split_sentences(text: &str) -> Vec<Sentence> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut line = 0usize;
    let mut i = 0usize;
    let mut start = 0usize;
    let push = |out: &mut Vec<Sentence>, s: usize, e: usize, line: usize| {
        let mut e = e;
        while e > s && chars[e - 1].is_whitespace() {
            e -= 1;
        }
        let mut s2 = s;
        while s2 < e && chars[s2].is_whitespace() {
            s2 += 1;
        }
        if e > s2 {
            let t: String = chars[s2..e].iter().collect();
            let judge = judgeable(&t, e < n && chars[e] != '\n' && e < n);
            out.push(Sentence { start: s2, end: e, text: t, line, judge });
        }
    };
    while i < n {
        let c = chars[i];
        if c == '\n' {
            push(&mut out, start, i, line);
            line += 1;
            i += 1;
            start = i;
            continue;
        }
        if is_terminal(c) {
            let mut j = i;
            while j < n && is_terminal(chars[j]) {
                j += 1;
            }
            while j < n && is_closing(chars[j]) {
                j += 1;
            }
            let at_end = j >= n;
            let followed_by_ws = !at_end && chars[j].is_whitespace() && chars[j] != '\n';
            let followed_by_nl = !at_end && chars[j] == '\n';
            if at_end || followed_by_ws || followed_by_nl {
                let mut brk = true;
                if c == '.' && i + 1 == j.min(i + 1) {
                    // the word before the period
                    let mut k = i;
                    while k > start && !chars[k - 1].is_whitespace() {
                        k -= 1;
                    }
                    let word: String = chars[k..i].iter().collect::<String>().to_lowercase();
                    let word = word.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '.');
                    let next = chars[j..].iter().find(|ch| !ch.is_whitespace()).copied();
                    let single_letter = word.chars().count() == 1 && word.chars().all(|ch| ch.is_alphabetic());
                    if TITLE_ABBR.contains(&word) && !followed_by_nl {
                        brk = false;
                    } else if (single_letter || word.contains('.')) && next.map(|ch| ch.is_uppercase()).unwrap_or(false) && !followed_by_nl {
                        brk = false; // "J. K. Rowling", "U.S. Navy"
                    } else if next.map(|ch| ch.is_lowercase()).unwrap_or(false) && !followed_by_nl && word.chars().count() <= 3 {
                        brk = false; // "etc. and", "approx. the"
                    }
                }
                if brk {
                    push(&mut out, start, j, line);
                    i = j;
                    start = j;
                    continue;
                }
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    push(&mut out, start, n, line);
    out
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphanumeric())).count()
}

fn judgeable(t: &str, _mid_line: bool) -> bool {
    let words = word_count(t);
    if words < MIN_WORDS {
        return false;
    }
    let lower = t.to_lowercase();
    if lower.contains("http://") || lower.contains("https://") || lower.contains("www.") {
        return false;
    }
    let non_space: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    let letters = non_space.iter().filter(|c| c.is_alphabetic()).count();
    if non_space.is_empty() || letters * 2 < non_space.len() {
        return false; // code, tables, numbers
    }
    let trimmed = t.trim_start_matches(|c: char| c == '-' || c == '*' || c == '•' || c == '#' || c.is_whitespace());
    if trimmed.len() + 2 < t.len() {
        return false; // list bullet / heading marker
    }
    let ends_terminal = t.trim_end().chars().last().map(|c| is_terminal(c) || is_closing(c)).unwrap_or(false);
    if !ends_terminal && words <= 8 {
        return false; // a heading or label
    }
    true
}

// ---------------------------------------------------------------- hints

struct Wordy {
    re: Regex,
    label: &'static str,
}

fn wordy_set() -> &'static Vec<Wordy> {
    static W: OnceLock<Vec<Wordy>> = OnceLock::new();
    W.get_or_init(|| {
        let mk = |p: &str, label: &'static str| Wordy { re: Regex::new(&format!(r"(?i)\b{p}\b")).unwrap(), label };
        vec![
            mk("in order to", "in order to"),
            mk("at this point in time", "at this point in time"),
            mk("due to the fact that", "due to the fact that"),
            mk("in the event that", "in the event that"),
            mk("it is important to note that", "it is important to note that"),
            mk("it should be noted that", "it should be noted that"),
            mk("please be advised that", "please be advised that"),
            mk("in due course", "in due course"),
            mk("circle back", "circle back"),
            mk("utili[sz]ation of", "utilization of"),
            mk("utili[sz]e", "utilize"),
            mk("a (?:very )?large number of", "a large number of"),
            mk("the reason for this is", "the reason for this is"),
            mk("for the purpose of", "for the purpose of"),
            mk("in the process of", "in the process of"),
            mk("with regard to", "with regard to"),
            mk("in spite of the fact that", "in spite of the fact that"),
            mk("on a daily basis", "on a daily basis"),
            mk("at the end of the day", "at the end of the day"),
            mk("needless to say", "needless to say"),
            mk("in a position to", "in a position to"),
            mk("as a matter of fact", "as a matter of fact"),
            mk("what i am trying to say", "what I am trying to say"),
            mk("in the near future", "in the near future"),
            mk("has the ability to", "has the ability to"),
            mk("in light of the fact that", "in light of the fact that"),
            mk("take into consideration", "take into consideration"),
            mk("a sufficient number of", "a sufficient number of"),
            mk("the majority of", "the majority of"),
            mk("prior to the", "prior to"),
            mk("subsequent to", "subsequent to"),
            mk("in the absence of", "in the absence of"),
            mk("it was decided by", "it was decided by"),
            mk("was made by the", "was made by"),
        ]
    })
}

const HEDGES: &[&str] = &["i think", "probably", "maybe", "kind of", "sort of", "consider", "i feel like", "perhaps", "i guess", "might"];

/// Deterministic hints for one sentence: wordy phrases from the scanner, a
/// hedge stack, a long sentence, and "may repeat sentence N" when most of its
/// content words already appeared in an earlier sentence of the batch.
struct Plain {
    re: Regex,
    to: &'static str,
}

/// Wordy phrase → plain words, for the deterministic rewrite used when the
/// model leaves a hinted sentence alone (or when no model is configured).
/// Only unambiguous swaps live here.
fn plain_set() -> &'static Vec<Plain> {
    static P: OnceLock<Vec<Plain>> = OnceLock::new();
    P.get_or_init(|| {
        let mk = |p: &str, to: &'static str| Plain { re: Regex::new(&format!(r"(?i)\b{p}\b")).unwrap(), to };
        vec![
            mk("it is important to note that ", ""),
            mk("it should be noted that ", ""),
            mk("please be advised that ", ""),
            mk("needless to say,? ", ""),
            mk("in order to", "to"),
            mk("at this point in time", "now"),
            mk("the reason for this is (?:due to the fact )?that", "this is because"),
            mk("due to the fact that", "because"),
            mk("in light of the fact that", "because"),
            mk("in spite of the fact that", "although"),
            mk("in the event that", "if"),
            mk("in due course", "soon"),
            mk("in the near future", "soon"),
            mk("a (?:very )?large number of", "many"),
            mk("a sufficient number of", "enough"),
            mk("the majority of", "most"),
            mk("has the ability to", "can"),
            mk("have the ability to", "can"),
            mk("in a position to", "able to"),
            mk("on a daily basis", "daily"),
            mk("take into consideration", "consider"),
            mk("in the absence of", "without"),
            mk("as a matter of fact", "in fact"),
            mk("utili[sz]ation of", "use of"),
            mk("utili[sz]e", "use"),
            mk("utili[sz]ed", "used"),
            mk("utili[sz]ing", "using"),
            mk("circle back", "follow up"),
            mk("subsequent to", "after"),
            mk("prior to", "before"),
        ]
    })
}

/// The sentence with its wordy phrases swapped for plain words, or None when
/// nothing applies. Keeps the first letter's case as the author wrote it.
pub fn plain_rewrite(orig: &str) -> Option<(String, Vec<String>)> {
    let mut t = orig.to_string();
    let mut flags = Vec::new();
    for p in plain_set() {
        if p.re.is_match(&t) {
            let m = p.re.find(&t).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
            t = p.re.replace_all(&t, p.to).to_string();
            flags.push(format!("wordy phrase \"{}\"", m.trim()));
        }
    }
    if flags.is_empty() {
        return None;
    }
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",").replace(" .", ".");
    let mut t = t.trim().to_string();
    if orig.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        let mut cs = t.chars();
        if let Some(f) = cs.next() {
            t = f.to_uppercase().collect::<String>() + cs.as_str();
        }
    }
    if normalize(&t) == normalize(orig) || word_count(&t) < 3 {
        return None;
    }
    Some((t, flags))
}

pub fn hints_for(s: &str, earlier: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for w in wordy_set() {
        if w.re.is_match(s) {
            out.push(format!("wordy phrase \"{}\"", w.label));
            if out.len() >= 2 {
                break;
            }
        }
    }
    let low = s.to_lowercase();
    if HEDGES.iter().filter(|h| low.contains(*h)).count() >= 2 {
        out.push("hedge stack".into());
    }
    let n = word_count(s);
    if n > LONG_WORDS {
        out.push(format!("too long ({n} words): split it into two or three sentences at its natural joints"));
    }
    if word_count(s) <= 12 {
        for (j, e) in earlier.iter().enumerate() {
            if overlap_with(s, &[e]) >= 0.7 {
                out.push(format!("may repeat sentence {}", j + 1));
                break;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- batching

pub fn batches(sents: &[Sentence]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut chars = 0usize;
    for (k, s) in sents.iter().enumerate() {
        if !s.judge {
            continue;
        }
        let len = s.text.chars().count();
        if !cur.is_empty() && (cur.len() >= MAX_BATCH || chars + len > MAX_BATCH_CHARS) {
            out.push(std::mem::take(&mut cur));
            chars = 0;
        }
        cur.push(k);
        chars += len;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The request line for a writing goal ("" when there is none or it is unknown).
pub fn goal_line(goal: &str) -> String {
    let what = match goal {
        "formal" => "too casual, slang, contractions",
        "friendly" => "warm and approachable, not stiff",
        "confident" => "direct, no hedging or apologizing",
        "concise" => "fewer words, no filler",
        _ => return String::new(),
    };
    format!("GOAL: {goal} ({what}) - also rephrase sentences that clearly miss this goal. Keep every number, name and quote.\n\n")
}

pub fn user_message(sents: &[Sentence], batch: &[usize], goal: &str) -> String {
    let mut m = goal_line(goal);
    if let Some(&first) = batch.first() {
        if let Some(prev) = sents[..first].iter().rev().find(|s| s.judge || word_count(&s.text) >= 2) {
            m.push_str("CONTEXT (already judged, do not include): ");
            m.push_str(&prev.text);
            m.push_str("\n\n");
        }
    }
    m.push_str("SENTENCES:\n");
    let mut hints = Vec::new();
    for (n, &k) in batch.iter().enumerate() {
        m.push_str(&format!("{}. {}\n", n + 1, sents[k].text));
        let earlier: Vec<&str> = batch[..n].iter().map(|&j| sents[j].text.as_str()).collect();
        for h in hints_for(&sents[k].text, &earlier) {
            hints.push(format!("{}: {}", n + 1, h));
        }
    }
    if !hints.is_empty() {
        m.push_str("\nHINTS:\n");
        for h in hints {
            m.push_str(&h);
            m.push('\n');
        }
    }
    m
}

pub fn cache_key(prev: &str, text: &str, goal: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    prompts::SPOT_SYSTEM.len().hash(&mut h);
    goal.hash(&mut h);
    prev.trim().hash(&mut h);
    text.trim().hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------- guards

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn numbers(s: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\d[\d,.:/%]*").unwrap());
    re.find_iter(s).map(|m| m.as_str().trim_end_matches(|c| c == '.' || c == ',' || c == ':').to_string()).collect()
}

fn proper_nouns(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let words: Vec<&str> = s.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let core: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
        if core.len() < 2 || core == "I" {
            continue;
        }
        let first_upper = core.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
        let all_upper = core.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
        if (first_upper && i > 0) || (all_upper && core.chars().any(|c| c.is_alphabetic())) {
            out.push(core.to_lowercase());
        }
    }
    out
}

const NUMBER_WORDS: &[&str] = &["two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion", "twice", "half", "dozen"];

/// Spelled-out numbers ("four songs", "six months") are facts too.
fn number_words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| NUMBER_WORDS.contains(w)).map(|w| w.to_string()).collect()
}

fn quotes(s: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#""([^"]{3,})"|“([^”]{3,})”"#).unwrap());
    re.captures_iter(s).filter_map(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().trim().to_lowercase())).collect()
}

const STOP: &[&str] = &["the", "and", "that", "this", "with", "for", "are", "was", "were", "have", "has", "had", "not", "but", "you", "your", "they", "them", "their", "from", "into", "than", "then", "there", "here", "what", "when", "which", "who", "will", "would", "could", "should", "can", "all", "any", "its", "our", "out", "very", "just", "also", "been", "being", "did", "does", "doing", "much", "many", "more", "most", "some", "such", "too", "about", "over", "under", "after", "before", "again", "still", "ever", "even", "only", "own", "same", "those", "these", "each", "other", "so", "as", "at", "by", "in", "of", "on", "or", "to", "up", "is", "it", "be", "we", "he", "she", "him", "her", "his", "me", "my", "an", "a", "i"];

fn content_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\'').to_string())
        .filter(|w| w.len() >= 3 && !STOP.contains(&w.as_str()))
        .collect()
}

/// Light stemming so "delayed" matches "delay" and "emails" matches "email".
fn stem(w: &str) -> String {
    let mut t = w.to_string();
    for suf in ["ing", "ed", "es", "ly", "s"] {
        if t.ends_with(suf) && t.len() - suf.len() >= 3 {
            t.truncate(t.len() - suf.len());
            break;
        }
    }
    if t.ends_with('e') && t.len() > 3 {
        t.pop();
    }
    t
}

/// Share of the original's content words that the rewrite keeps, after the
/// scanner's wordy phrases and hedges are taken out of the original (those are
/// the words a good rewrite is supposed to drop).
pub fn content_kept(orig: &str, rewrite: &str) -> f64 {
    let (lost, total) = content_loss(orig, rewrite);
    if total == 0 {
        return 1.0;
    }
    (total - lost) as f64 / total as f64
}

/// (content words lost, content words in the original) for a rewrite.
/// Share of content words a goal rewrite must keep (normal rephrases use MIN_CONTENT_KEPT).
const GOAL_CONTENT_KEPT: f64 = 0.5;

/// Slang and filler a writing goal ("formal") is expected to drop; not counted as lost content.
const CASUAL_WORDS: &[&str] = &["hey", "hi", "guys", "gotta", "gonna", "wanna", "kinda", "sorta", "yeah", "yep", "nope", "lol", "ngl", "tbh", "idk", "stuff", "awesome", "totally", "super", "asap", "bunch", "ya", "yall", "pls", "thx"];

fn strip_casual(s: &str) -> String {
    s.split_whitespace()
        .filter(|w| !CASUAL_WORDS.contains(&w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase().as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn content_loss(orig: &str, rewrite: &str) -> (usize, usize) {
    let mut stripped = orig.to_lowercase();
    for w in wordy_set() {
        stripped = w.re.replace_all(&stripped, " ").to_string();
    }
    for p in plain_set() {
        stripped = p.re.replace_all(&stripped, " ").to_string();
    }
    for h in HEDGES {
        stripped = stripped.replace(h, " ");
    }
    let want: Vec<String> = content_words(&stripped).iter().map(|w| stem(w)).collect();
    let have: HashSet<String> = content_words(rewrite).iter().map(|w| stem(w)).collect();
    let lost = want.iter().filter(|w| !have.contains(*w)).count();
    (lost, want.len())
}

/// A lossless split of a long sentence at the joint nearest its middle
/// (", and" / ", but" / ", so" / ";"), when both halves stand on their own.
pub fn plain_split(orig: &str) -> Option<String> {
    if word_count(orig) <= LONG_WORDS {
        return None;
    }
    let joints: [(&str, &str); 4] = [(", and ", ""), (", but ", "But "), (", so ", "So "), ("; ", "")];
    let mid = orig.len() / 2;
    let mut best: Option<(usize, usize, &str, &str)> = None;
    for (j, lead) in joints {
        for (idx, _) in orig.match_indices(j) {
            if word_count(&orig[..idx]) < 8 || word_count(&orig[idx + j.len()..]) < 8 {
                continue;
            }
            // ", and butter" after "milk, eggs, bread" closes a list, not a clause:
            // the run since the previous comma must be a clause of its own.
            let before = &orig[..idx];
            let clause = before.rsplit([',', ';']).next().unwrap_or(before);
            if word_count(clause) < 3 {
                continue;
            }
            let d = (idx as isize - mid as isize).unsigned_abs();
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, idx, j, lead));
            }
        }
    }
    let (_, idx, j, lead) = best?;
    let left = orig[..idx].trim_end();
    let right = orig[idx + j.len()..].trim_start();
    let right = if lead.is_empty() {
        let mut cs = right.chars();
        cs.next().map(|f| f.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default()
    } else {
        format!("{lead}{right}")
    };
    let term = if left.chars().last().map(is_terminal).unwrap_or(false) { "" } else { "." };
    Some(format!("{left}{term} {right}"))
}

/// Deliberately casual writing (lowercase sentence start, lowercase "i", chat
/// markers): the author is not writing formal prose, so only an objectively
/// wordy phrase (a scanner hint) justifies a rewrite.
pub fn casual_voice(s: &str) -> bool {
    let t = s.trim();
    if t.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) {
        return true;
    }
    let padded = format!(" {} ", t.to_lowercase());
    if t.split_whitespace().any(|w| w == "i") {
        return true;
    }
    [" lol ", " ngl ", " tbh ", " idk ", " im ", " dont ", " cant ", " wasnt ", " didnt ", " isnt ", " gonna ", " wanna ", " kinda ", " lmao ", " yall "].iter().any(|m| padded.contains(m))
}

fn filler_like(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:so|anyway|anyways|basically|well|ok|okay|right|honestly)?[,\s]*(?:as (?:i|we) (?:said|mentioned|noted|stated)|as (?:mentioned|noted|stated|discussed) (?:above|before|earlier|previously)|that is what happened|that'?s what happened|here is the thing|here'?s the thing|what i am trying to say|what i'?m trying to say|it is what it is|needless to say|in conclusion|to sum up|to summarize|long story short|at the end of the day|that is all|that'?s all|moving on|let me explain|as you can see|as we all know|it goes without saying|the thing is|the point is)").unwrap()
    });
    re.is_match(s.trim())
}

/// Share of this sentence's content words that already appear in `earlier`.
fn overlap_with(s: &str, earlier: &[&str]) -> f64 {
    let cw = content_words(s);
    if cw.is_empty() {
        return 0.0;
    }
    let pool: HashSet<String> = earlier.iter().flat_map(|e| content_words(e)).collect();
    cw.iter().filter(|w| pool.contains(*w)).count() as f64 / cw.len() as f64
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct RawItem {
    #[serde(default)]
    pub i: u64,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub confidence: Option<u64>,
    #[serde(default)]
    pub flags: Vec<String>,
}

/// Why a verdict was rejected (for the tuning log / tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    LowConfidence,
    Identical,
    TooShortToRephrase,
    /// Casual lowercase voice is only rephrased when the scanner itself found something.
    CasualVoice,
    NotOneSentence,
    LengthBand,
    NumberLost(String),
    NameLost(String),
    QuoteLost(String),
    AiPhrase,
    /// The rewrite dropped too many of the original's content words (a summary, not a rephrase).
    ContentLost(String),
    RemoveTooLong,
    RemoveUniqueFact(String),
    RemoveNotRepeatOrFiller,
    RemoveOnlySentence,
}

/// Apply the guards. `earlier` = every judged sentence before this one in the
/// document (for repeat detection); `doc_rest` = the document without this sentence.
pub fn validate(item: &RawItem, orig: &str, earlier: &[&str], doc_rest: &str) -> Result<Option<Verdict>, Reject> {
    let action = item.action.trim().to_lowercase();
    if action != "rephrase" && action != "remove" {
        return Ok(None);
    }
    if item.confidence.unwrap_or(5) < MIN_CONFIDENCE {
        return Err(Reject::LowConfidence);
    }
    let flags: Vec<String> = item.flags.iter().map(|f| f.to_lowercase()).collect();
    let why = polish_reason(&item.why);
    // A filler sentence "rephrased" into shorter filler is still filler: treat it as a remove.
    let action = if action == "rephrase" && filler_like(orig) && word_count(orig) <= 14 { "remove".to_string() } else { action };
    if action == "rephrase" {
        // The user's writing goal counts as a hint: "formal" exists to rewrite casual or short lines.
        // The fact guards below still apply in full.
        let goal_miss = flags.iter().any(|f| f == "misses goal");
        let hinted = goal_miss || !hints_for(orig, earlier).is_empty();
        if word_count(orig) < MIN_WORDS_UNHINTED && !hinted {
            return Err(Reject::TooShortToRephrase);
        }
        if casual_voice(orig) && !hinted {
            return Err(Reject::CasualVoice);
        }
        if !hinted && item.confidence.unwrap_or(5) < MIN_CONFIDENCE_UNHINTED {
            return Err(Reject::LowConfidence);
        }
        let mut t = item.text.trim().to_string();
        if t.is_empty() || normalize(&t) == normalize(orig) {
            return Err(Reject::Identical);
        }
        // One sentence in, one sentence out; except that a long original (over
        // SPLIT_MIN_WORDS words) may come back as two or three shorter sentences.
        let pieces: Vec<String> = split_sentences(&t).into_iter().filter(|p| word_count(&p.text) >= 1).map(|p| p.text).collect();
        let wo_n = word_count(orig);
        let is_split = pieces.len() > 1;
        if pieces.is_empty() || pieces.len() > MAX_SPLIT || (is_split && wo_n < SPLIT_MIN_WORDS) {
            return Err(Reject::NotOneSentence);
        }
        if is_split && pieces.iter().any(|p| word_count(p) >= wo_n) {
            return Err(Reject::NotOneSentence);
        }
        // A rephrase mostly shortens; the wordier the original, the more it may shrink.
        // The band only blocks degenerate answers ("We sent emails." for a 24-word sentence).
        // A split keeps nearly every word, so its band is tight.
        let (wo, wt) = (wo_n.max(1) as f64, word_count(&t) as f64);
        let ratio = wt / wo;
        let floor = if is_split { 0.55 } else if !hinted { 0.5 } else if wo >= 20.0 { 0.28 } else { 0.4 };
        let ceiling = if is_split { 1.35 } else if !hinted { 1.1 } else { 1.4 };
        if ratio < floor || ratio > ceiling {
            return Err(Reject::LengthBand);
        }
        // A rephrase keeps what the sentence says; a rewrite that loses its
        // content words is a summary, however fluent.
        // One lost word is always allowed (a synonym), more only in proportion. A goal rewrite
        // drops slang by design and rewords more freely; names and numbers are still checked below.
        let (lost, total) = if goal_miss { content_loss(&strip_casual(orig), &t) } else { content_loss(orig, &t) };
        let kept = if goal_miss { GOAL_CONTENT_KEPT } else { MIN_CONTENT_KEPT };
        let allowed = ((total as f64) * (1.0 - kept)).floor().max(1.0) as usize;
        if lost > allowed {
            return Err(Reject::ContentLost(format!("{lost} of {total} content words lost")));
        }
        for num in numbers(orig) {
            if !t.contains(&num) {
                return Err(Reject::NumberLost(num));
            }
        }
        let tl = t.to_lowercase();
        for w in number_words(orig) {
            if !tl.split(|c: char| !c.is_alphanumeric()).any(|x| x == w) {
                return Err(Reject::NumberLost(w));
            }
        }
        for name in proper_nouns(orig) {
            if !tl.contains(&name) {
                return Err(Reject::NameLost(name));
            }
        }
        for q in quotes(orig) {
            if !tl.contains(&q) {
                return Err(Reject::QuoteLost(q));
            }
        }
        let before: HashSet<String> = crate::aiphrases::find(orig).into_iter().map(|p| p.phrase.to_lowercase()).collect();
        if crate::aiphrases::find(&t).into_iter().any(|p| matches!(p.bucket, crate::aiphrases::Bucket::Frequent | crate::aiphrases::Bucket::Often) && !before.contains(&p.phrase.to_lowercase())) {
            return Err(Reject::AiPhrase);
        }
        // keep the author's casing at the start (a lowercase pronoun "i" may become "I") and the terminal punctuation
        let starts_with_i = orig.split_whitespace().next().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()) == "i").unwrap_or(false);
        if orig.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) && !starts_with_i {
            let mut cs = t.chars();
            if let Some(f) = cs.next() {
                t = f.to_lowercase().collect::<String>() + cs.as_str();
            }
        }
        let orig_term = orig.trim_end().chars().last().filter(|c| is_terminal(*c));
        if let Some(term) = orig_term {
            let last = t.chars().last().unwrap_or(' ');
            if !is_terminal(last) && !is_closing(last) {
                t.push(term);
            }
        }
        let mut flags = flags;
        if is_split && wo_n > LONG_WORDS {
            flags.push(format!("split into {} sentences", pieces.len()));
        } else if is_split {
            // a medium sentence rewritten as two is a rephrase, not a "long sentence" split
            flags.push(format!("rephrased as {} sentences", pieces.len()));
        }
        Ok(Some(Verdict::Rephrase { text: t, why, flags }))
    } else {
        if word_count(orig) > MAX_REMOVE_WORDS {
            return Err(Reject::RemoveTooLong);
        }
        if earlier.is_empty() && doc_rest.trim().is_empty() {
            return Err(Reject::RemoveOnlySentence);
        }
        let rest_lower = doc_rest.to_lowercase();
        for num in numbers(orig) {
            if !doc_rest.contains(&num) {
                return Err(Reject::RemoveUniqueFact(num));
            }
        }
        for name in proper_nouns(orig) {
            if !rest_lower.contains(&name) {
                return Err(Reject::RemoveUniqueFact(name));
            }
        }
        let repeat = overlap_with(orig, earlier) >= 0.6;
        let filler = filler_like(orig) || flags.iter().any(|f| f.contains("filler")) && word_count(orig) <= 12;
        if !repeat && !filler {
            return Err(Reject::RemoveNotRepeatOrFiller);
        }
        let mut flags = flags;
        if repeat && !flags.iter().any(|f| f.contains("repeat")) {
            flags.push("repeats earlier sentence".into());
        }
        if !repeat && !flags.iter().any(|f| f.contains("filler")) {
            flags.push("empty filler".into());
        }
        Ok(Some(Verdict::Remove { why, flags }))
    }
}

fn polish_reason(s: &str) -> String {
    let t = s.trim().trim_end_matches('.').trim();
    if t.is_empty() {
        return String::new();
    }
    let mut c = t.chars();
    let first = c.next().unwrap().to_uppercase().collect::<String>();
    format!("{first}{}.", c.as_str())
}

// ---------------------------------------------------------------- alerts

fn title_and_message(v: &Verdict) -> (&'static str, &'static str, String) {
    match v {
        Verdict::Rephrase { why, flags, .. } => {
            let f = flags.join(" ");
            let (kind, title, default) = if f.contains("split into") {
                ("SpotRephrase", "Split the long sentence", "This sentence runs long. Two shorter sentences make the same points and read faster.")
            } else if f.contains("passive") {
                ("SpotRephrase", "Use the active voice", "Say who does what. The active voice is shorter and easier to follow.")
            } else if f.contains("wordy") {
                ("SpotRephrase", "Make it more concise", "This sentence uses more words than it needs. The rewrite says the same thing in plain words.")
            } else if f.contains("goal") {
                ("SpotRephrase", "Match your writing goal", "The rewrite fits the tone you asked for and keeps the meaning.")
            } else if f.contains("tangled") || f.contains("long") {
                ("SpotRephrase", "Rephrase for clarity", "This sentence is hard to follow in one read. The rewrite keeps the meaning and untangles it.")
            } else {
                ("SpotRephrase", "Rephrase for clarity", "The rewrite says the same thing more clearly.")
            };
            (kind, title, if why.is_empty() { default.to_string() } else { why.clone() })
        }
        Verdict::Remove { why, flags } => {
            let f = flags.join(" ");
            let (title, default) = if f.contains("repeat") {
                ("Remove the repeated sentence", "This sentence restates a point an earlier sentence already made.")
            } else {
                ("Remove the filler sentence", "This sentence adds nothing for the reader. Removing it tightens the paragraph.")
            };
            ("SpotRemove", title, if why.is_empty() { default.to_string() } else { why.clone() })
        }
    }
}

/// The char range a Remove replaces: the sentence plus the gap to the next
/// sentence on the same line, so no double space is left behind.
pub fn remove_range(sents: &[Sentence], k: usize) -> (usize, usize) {
    let s = &sents[k];
    if let Some(next) = sents.get(k + 1) {
        if next.line == s.line && next.start >= s.end {
            return (s.start, next.start);
        }
    }
    (s.start, s.end)
}

pub fn alert_for(text: &str, map: &TextMap, sents: &[Sentence], k: usize, v: &Verdict) -> Alert {
    let s = &sents[k];
    let (kind, title, message) = title_and_message(v);
    let (start, end, original, suggestion) = match v {
        Verdict::Rephrase { text: t, .. } => (s.start, s.end, s.text.clone(), t.clone()),
        Verdict::Remove { .. } => {
            let (a, b) = remove_range(sents, k);
            let orig: String = text.chars().skip(a).take(b - a).collect();
            (a, b, orig, String::new())
        }
    };
    let hash = stable_hash(kind, s.text.trim(), None);
    Alert {
        id: format!("spot-{:x}-{}", hash, start),
        category: Category::Clarity,
        kind: kind.into(),
        title: title.into(),
        message,
        start,
        end,
        u16_start: map.char_to_utf16(start),
        u16_end: map.char_to_utf16(end),
        original,
        suggestions: vec![suggestion],
        priority: 150,
        stable_hash: hash,
    }
}

/// Keep spot alerts valid after the document changed: same text at the same
/// offsets, else the (unique) new position of the same sentence, else drop.
pub fn re_anchor(alerts: &mut Vec<Alert>, text: &str) {
    let map = TextMap::new(text);
    alerts.retain_mut(|a| {
        if crate::textmap::slice_utf16(text, a.u16_start, a.u16_end) == a.original {
            return true;
        }
        let needle = a.original.as_str();
        if needle.trim().is_empty() {
            return false;
        }
        let mut it = text.match_indices(needle);
        let (Some((b, _)), None) = (it.next(), it.next()) else { return false };
        let start = text[..b].chars().count();
        let end = start + needle.chars().count();
        a.start = start;
        a.end = end;
        a.u16_start = map.char_to_utf16(start);
        a.u16_end = map.char_to_utf16(end);
        a.id = format!("spot-{:x}-{}", a.stable_hash, start);
        true
    });
}

// ---------------------------------------------------------------- run

/// A deterministic verdict for a sentence the model left alone: the plain
/// rewrite of its wordy phrases, run through the same guards.
fn plain_verdict(orig: &str, earlier: &[&str], doc_rest: &str) -> Option<Verdict> {
    if let Some((text, flags)) = plain_rewrite(orig) {
        let item = RawItem { i: 0, action: "rephrase".into(), text, why: "Plain words say the same thing in less space.".into(), confidence: Some(5), flags };
        if let Ok(Some(v)) = validate(&item, orig, earlier, doc_rest) {
            return Some(v);
        }
    }
    // A long sentence the model left alone (or summarised) still gets a
    // lossless split at its middle joint.
    let n = word_count(orig);
    if n > SPLIT_FALLBACK_WORDS {
        let text = plain_split(orig)?;
        let item = RawItem { i: 0, action: "rephrase".into(), text, why: format!("This sentence runs {n} words. Two shorter sentences make the same points and read faster."), confidence: Some(5), flags: vec!["too long".into()] };
        return validate(&item, orig, earlier, doc_rest).ok().flatten();
    }
    None
}

/// Spot-check `text`. Sentences already in `cache` are not sent to the model.
/// With no model (`None`) only the deterministic wordy-phrase rewrites run.
/// `goal` is the writing goal ("" for none); it is part of every cache key.
pub async fn run(cfg: Option<&LlmConfig>, text: &str, cache: &mut Cache, goal: &str) -> anyhow::Result<Outcome> {
    let sents = split_sentences(text);
    let map = TextMap::new(text);
    let mut out = Outcome { sentences: sents.iter().filter(|s| s.judge).count(), skipped: sents.iter().filter(|s| !s.judge).count(), ..Default::default() };
    let mut verdicts: HashMap<usize, Verdict> = HashMap::new();
    let mut pending: Vec<usize> = Vec::new();
    let prev_of = |k: usize| -> String { sents[..k].iter().rev().find(|s| s.judge).map(|s| s.text.clone()).unwrap_or_default() };
    for (k, s) in sents.iter().enumerate() {
        if !s.judge {
            continue;
        }
        match cache.get(&cache_key(&prev_of(k), &s.text, goal)) {
            Some(Some(v)) => {
                verdicts.insert(k, v.clone());
                out.from_cache += 1;
            }
            Some(None) => out.from_cache += 1,
            None => pending.push(k),
        }
    }
    let pending_sents: Vec<Sentence> = sents.iter().enumerate().map(|(k, s)| Sentence { judge: s.judge && pending.contains(&k), ..s.clone() }).collect();
    for batch in batches(&pending_sents) {
        out.batches += 1;
        let by_i: HashMap<u64, RawItem> = match cfg {
            Some(cfg) => {
                let user = user_message(&sents, &batch, goal);
                let v = llm::complete_json(cfg, prompts::SPOT_SYSTEM, &user, 0.2).await?;
                let items: Vec<RawItem> = v["items"].as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()).unwrap_or_default();
                items.into_iter().map(|it| (it.i, it)).collect()
            }
            None => HashMap::new(),
        };
        let mut candidates: Vec<(usize, u64, Verdict)> = Vec::new();
        for (n, &k) in batch.iter().enumerate() {
            out.checked += 1;
            let key = cache_key(&prev_of(k), &sents[k].text, goal);
            let earlier: Vec<&str> = sents[..k].iter().filter(|s| s.judge).map(|s| s.text.as_str()).collect();
            let doc_rest: String = sents.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, s)| s.text.as_str()).collect::<Vec<_>>().join(" ");
            let model_verdict = match by_i.get(&(n as u64 + 1)) {
                Some(item) => match validate(item, &sents[k].text, &earlier, &doc_rest) {
                    Ok(v) => Some((item.confidence.unwrap_or(5), v)),
                    Err(r) => {
                        out.rejected += 1;
                        out.rejections.push(format!("{r:?}: {} => {}", sents[k].text.chars().take(50).collect::<String>(), item.text.chars().take(80).collect::<String>()));
                        None
                    }
                },
                None => None,
            };
            match model_verdict {
                Some((c, Some(v))) => candidates.push((k, c, v)),
                _ => {
                    // The model passed (or was absent): a hinted wordy phrase still gets its plain rewrite.
                    match plain_verdict(&sents[k].text, &earlier, &doc_rest) {
                        Some(v) => candidates.push((k, 5, v)),
                        None => {
                            cache.insert(key, None);
                        }
                    }
                }
            }
        }
        // Churn cap: a model that wants to change most of a batch is overreaching.
        // Sentences the scanner itself flagged (a wordy phrase, a repeat) are exempt:
        // those are objectively wordy, the cap exists for the model's own whims.
        let cap = ((batch.len() as f64) * MAX_ACTION_RATIO).ceil() as usize;
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let hinted: Vec<bool> = candidates
            .iter()
            .map(|(k, _, _)| {
                let earlier: Vec<&str> = sents[..*k].iter().filter(|s| s.judge).map(|s| s.text.as_str()).collect();
                !hints_for(&sents[*k].text, &earlier).is_empty()
            })
            .collect();
        if cfg.is_none() {
            out.batches -= 1;
        }
        let mut unhinted_kept = 0usize;
        for (idx, (k, _, v)) in candidates.into_iter().enumerate() {
            let key = cache_key(&prev_of(k), &sents[k].text, goal);
            let keep = if hinted[idx] { true } else { unhinted_kept += 1; unhinted_kept <= cap };
            if keep {
                cache.insert(key, Some(v.clone()));
                verdicts.insert(k, v);
            } else {
                out.rejected += 1;
                out.rejections.push(format!("ChurnCap: {}", sents[k].text.chars().take(50).collect::<String>()));
                cache.insert(key, None);
            }
        }
    }
    if cache.len() > CACHE_MAX {
        cache.clear();
    }
    let mut keys: Vec<usize> = verdicts.keys().copied().collect();
    keys.sort_unstable();
    for k in keys {
        out.alerts.push(alert_for(text, &map, &sents, k, &verdicts[&k]));
    }
    Ok(out)
}

/// The list the panel shows: Harper + rule alerts, then spot-check sentences,
/// minus everything dismissed. Harper's own long-sentence notice carries no
/// rewrite, so it gives way to a spot rewrite that covers the same sentence.
pub fn merge_visible(all: &[Alert], spot: &[Alert], dismissed: impl Fn(u64) -> bool) -> Vec<Alert> {
    let spot: Vec<&Alert> = spot.iter().filter(|a| !dismissed(a.stable_hash)).collect();
    let mut v: Vec<Alert> = all
        .iter()
        .filter(|a| !dismissed(a.stable_hash))
        .filter(|a| !(a.kind == "Readability" && spot.iter().any(|s| s.u16_start < a.u16_end && a.u16_start < s.u16_end)))
        .cloned()
        .collect();
    v.extend(spot.into_iter().cloned());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: &[Sentence]) -> Vec<&str> {
        v.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn splits_on_terminal_punctuation_and_newlines() {
        let t = "First one. Second one!  Third one?\nFourth on a new line\n\nFifth.";
        let s = split_sentences(t);
        assert_eq!(texts(&s), vec!["First one.", "Second one!", "Third one?", "Fourth on a new line", "Fifth."]);
        assert_eq!(s[3].line, 1);
        assert_eq!(s[4].line, 3);
        for x in &s {
            assert_eq!(t.chars().skip(x.start).take(x.end - x.start).collect::<String>(), x.text);
        }
    }

    #[test]
    fn abbreviations_decimals_and_initials_do_not_split() {
        let s = split_sentences("Dr. Smith paid $1.2M to Acme Inc. in March. J. K. Rowling wrote it, e.g. the first one. Then we left.");
        assert_eq!(texts(&s), vec!["Dr. Smith paid $1.2M to Acme Inc. in March.", "J. K. Rowling wrote it, e.g. the first one.", "Then we left."]);
    }

    #[test]
    fn closing_quotes_stay_with_their_sentence() {
        let s = split_sentences("She said \"no way.\" Then she left.");
        assert_eq!(texts(&s), vec!["She said \"no way.\"", "Then she left."]);
    }

    #[test]
    fn short_fragments_headings_and_urls_are_not_judged() {
        let s = split_sentences("Agenda for Monday\nWe meet at three in the main room.\nSee https://example.com/x for the map.\nok.");
        let judged: Vec<&str> = s.iter().filter(|x| x.judge).map(|x| x.text.as_str()).collect();
        assert_eq!(judged, vec!["We meet at three in the main room."]);
    }

    #[test]
    fn batches_respect_size_limits() {
        let text: String = (0..20).map(|i| format!("This is sentence number {i} in the batch test. ")).collect();
        let s = split_sentences(&text);
        let b = batches(&s);
        assert_eq!(b.iter().map(|x| x.len()).sum::<usize>(), 20);
        assert!(b.iter().all(|x| x.len() <= MAX_BATCH));
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn user_message_numbers_sentences_and_adds_hints() {
        let s = split_sentences("First we met at the cafe. In order to be able to make progress we talked.");
        let m = user_message(&s, &[0, 1], "");
        assert!(m.starts_with("SENTENCES:\n1. First we met at the cafe.\n2. In order to"));
        assert!(m.contains("HINTS:\n2: wordy phrase \"in order to\""));
        let m2 = user_message(&s, &[1], "");
        assert!(m2.starts_with("CONTEXT (already judged, do not include): First we met at the cafe."));
    }

    #[test]
    fn a_writing_goal_adds_a_request_line_and_changes_the_cache_key() {
        let s = split_sentences("First we met at the cafe. Then we talked for hours.");
        assert!(!user_message(&s, &[0], "").contains("GOAL"));
        assert!(!user_message(&s, &[0], "bogus").contains("GOAL"));
        let m = user_message(&s, &[0, 1], "formal");
        assert!(m.starts_with("GOAL: formal (too casual, slang, contractions) - also rephrase"), "{m}");
        assert!(m.contains("SENTENCES:\n1. First we met at the cafe."));
        assert!(user_message(&s, &[0], "concise").contains("fewer words, no filler"));
        assert_ne!(cache_key("a", "b", ""), cache_key("a", "b", "formal"));
        assert_ne!(cache_key("a", "b", "formal"), cache_key("a", "b", "friendly"));
        assert_eq!(cache_key("a", "b", "formal"), cache_key("a", "b", "formal"));
    }

    fn item(action: &str, text: &str, conf: u64, flags: &[&str]) -> RawItem {
        RawItem { i: 1, action: action.into(), text: text.into(), why: "shorter".into(), confidence: Some(conf), flags: flags.iter().map(|f| f.to_string()).collect() }
    }

    #[test]
    fn rephrase_guards_keep_numbers_names_quotes_and_one_sentence() {
        let orig = "We raised $500,000 from Verdict Capital on October 12 to kill slop companies.";
        let ok = validate(&item("rephrase", "On October 12 we raised $500,000 from Verdict Capital to kill slop companies.", 4, &["wordy phrase"]), orig, &[], "");
        assert!(matches!(ok, Ok(Some(Verdict::Rephrase { .. }))), "{ok:?}");
        assert_eq!(validate(&item("rephrase", "We raised money from Verdict Capital on October 12 to kill slop companies.", 4, &[]), orig, &[], "").unwrap_err(), Reject::NumberLost("500,000".into()));
        assert_eq!(validate(&item("rephrase", "We raised $500,000 on October 12 to kill slop companies.", 4, &[]), orig, &[], "").unwrap_err(), Reject::NameLost("verdict".into()));
        assert_eq!(validate(&item("rephrase", "We raised $500,000 from Verdict Capital on October 12. We kill slop companies.", 4, &[]), orig, &[], "").unwrap_err(), Reject::NotOneSentence);
        assert_eq!(validate(&item("rephrase", "We raised $500,000 from Verdict Capital on October 12 to kill slop companies.", 4, &[]), orig, &[], "").unwrap_err(), Reject::Identical);
        let wordy = "In order to be able to make progress at this point in time, it was decided by me that we should send many cold emails to founders.";
        assert_eq!(validate(&item("rephrase", "We sent emails.", 4, &[]), wordy, &[], "").unwrap_err(), Reject::LengthBand);
        assert!(validate(&item("rephrase", "To make progress now, I decided to send many cold emails.", 4, &[]), wordy, &[], "").unwrap().is_some(), "a 36-word sentence may shrink to 11 words");
        let padded = format!("{} {}", wordy, "And then we waited for weeks and weeks for anyone at all to answer any of them.");
        assert_eq!(validate(&item("rephrase", &padded, 4, &[]), wordy, &[], "").unwrap_err(), Reject::NotOneSentence);
        assert_eq!(validate(&item("rephrase", "We raised $500,000 from Verdict Capital on October 12 to kill slop companies, really.", 2, &[]), orig, &[], "").unwrap_err(), Reject::LowConfidence);
        let q = "She said, \"we are not raising this year,\" and walked out.";
        assert_eq!(validate(&item("rephrase", "She said they were not raising this year and walked out.", 4, &[]), q, &[], "").unwrap_err(), Reject::QuoteLost("we are not raising this year,".into()));
    }

    #[test]
    fn rephrase_keeps_casual_casing_and_terminal_punctuation() {
        let orig = "in order to win we have to ship it";
        let v = validate(&item("rephrase", "To win we have to ship it", 5, &["wordy phrase"]), orig, &[], "").unwrap().unwrap();
        assert_eq!(v, Verdict::Rephrase { text: "to win we have to ship it".into(), why: "Shorter.".into(), flags: vec!["wordy phrase".into()] });
        let orig2 = "In order to win we have to ship it.";
        let v2 = validate(&item("rephrase", "To win we have to ship it", 5, &[]), orig2, &[], "").unwrap().unwrap();
        assert!(matches!(v2, Verdict::Rephrase { text, .. } if text == "To win we have to ship it."));
    }

    #[test]
    fn remove_guards_allow_repeats_and_filler_only() {
        let earlier = ["I love this city.", "The food is amazing and the people are kind."];
        let rest = "I love this city. The food is amazing and the people are kind. Every street has a story.";
        assert!(matches!(validate(&item("remove", "", 4, &["repeats earlier sentence"]), "I love this city so much.", &earlier, rest), Ok(Some(Verdict::Remove { .. }))));
        assert!(matches!(validate(&item("remove", "", 4, &["empty filler"]), "As I said before, that is what happened.", &earlier, rest), Ok(Some(Verdict::Remove { .. }))));
        assert_eq!(validate(&item("remove", "", 4, &["repeats earlier sentence"]), "We signed 17 new customers in March.", &earlier, rest).unwrap_err(), Reject::RemoveUniqueFact("17".into()));
        assert_eq!(validate(&item("remove", "", 4, &["repeats earlier sentence"]), "Every street has its own long story to tell the visitors.", &earlier, rest).unwrap_err(), Reject::RemoveNotRepeatOrFiller);
        let long = "This is a long sentence that keeps going on and on with many words so that it passes the limit of thirty words which the remove guard enforces very strictly indeed.";
        assert_eq!(validate(&item("remove", "", 4, &["empty filler"]), long, &earlier, rest).unwrap_err(), Reject::RemoveTooLong);
        assert_eq!(validate(&item("remove", "", 4, &["empty filler"]), "Anyway that is it.", &[], "").unwrap_err(), Reject::RemoveOnlySentence);
    }

    #[test]
    fn filler_rephrases_become_removes_and_short_sentences_stay() {
        let earlier = ["The launch went well."];
        let v = validate(&item("rephrase", "Here it is.", 4, &["empty filler"]), "Basically, what I am trying to say is this.", &earlier, "The launch went well. We had 400 signups.").unwrap().unwrap();
        assert!(matches!(v, Verdict::Remove { .. }), "{v:?}");
        assert_eq!(validate(&item("rephrase", "That line has stayed with me.", 4, &[]), "That line stuck with me.", &earlier, "x").unwrap_err(), Reject::TooShortToRephrase);
    }

    #[test]
    fn casual_voice_is_only_rephrased_on_a_hint() {
        assert!(casual_voice("but to make you understand, i have to take you back, to exactly 42 days ago."));
        assert!(casual_voice("Up til this point i wasnt getting any closer to my goals."));
        assert!(!casual_voice("The meeting has been rescheduled to Friday at noon."));
        let casual = "but to make you understand, i have to take you back, to exactly 42 days ago.";
        assert_eq!(validate(&item("rephrase", "42 days ago, let me take you back.", 5, &["tangled"]), casual, &[], "x").unwrap_err(), Reject::CasualVoice);
        let casual_wordy = "ngl in order to be able to ship this week i need the designs by monday.";
        assert!(validate(&item("rephrase", "ngl to ship this week i need the designs by monday.", 5, &["wordy phrase"]), casual_wordy, &[], "x").unwrap().is_some());
    }

    #[test]
    fn a_writing_goal_may_rewrite_casual_voice_but_not_drop_facts() {
        let casual = "Hey guys, we gotta get the report done by Friday because it's kinda late already.";
        assert_eq!(validate(&item("rephrase", "We need to finish the report by Friday, as it is already late.", 5, &[]), casual, &[], "x").unwrap_err(), Reject::CasualVoice);
        let v = validate(&item("rephrase", "We need to finish the report by Friday, as it is already late.", 5, &["misses goal"]), casual, &[], "x").unwrap();
        assert!(v.is_some(), "a goal miss may rewrite casual voice");
        assert!(validate(&item("rephrase", "We need to finish the report soon, as it is already late.", 5, &["misses goal"]), casual, &[], "x").is_err(), "names and days still have to survive");
    }

    fn mk_alert(kind: &str, u16_start: usize, u16_end: usize, hash: u64) -> Alert {
        Alert { id: format!("{kind}-{u16_start}"), category: Category::Clarity, kind: kind.into(), title: String::new(), message: String::new(), start: u16_start, end: u16_end, u16_start, u16_end, original: String::new(), suggestions: vec![], priority: 100, stable_hash: hash }
    }

    #[test]
    fn a_long_sentence_may_come_back_as_two_or_three() {
        let long = "i wish i could say that i felt some sort of conviction, but honestly, it was stranger than anything I had ever experienced, and with every day that went by without results i couldnt help but question if i had made the right choice.";
        let split = "I wish I could say I felt some conviction, but honestly, it was stranger than anything I had ever experienced. With every day that went by without results, I couldn't help but question whether I had made the right choice.";
        let v = validate(&item("rephrase", split, 5, &["too long"]), long, &[], "x").unwrap().unwrap();
        match &v {
            Verdict::Rephrase { text, flags, .. } => {
                assert_eq!(text, split, "a lowercase pronoun start may become I");
                assert!(flags.iter().any(|f| f == "split into 2 sentences"), "{flags:?}");
            }
            _ => panic!("expected a rephrase"),
        }
        let (kind, title, _) = title_and_message(&v);
        assert_eq!((kind, title), ("SpotRephrase", "Split the long sentence"));
        // four pieces is a paragraph rewrite, not a split
        let four = "I wish I could say I felt conviction. Honestly, it was stranger than anything I had experienced. Every day went by without results. I questioned whether I had made the right choice.";
        assert_eq!(validate(&item("rephrase", four, 5, &["too long"]), long, &[], "x").unwrap_err(), Reject::NotOneSentence);
        // a short original never splits
        let short = "We sent the report to the client on Monday and they replied by noon with two questions.";
        assert_eq!(validate(&item("rephrase", "We sent the report on Monday. They replied by noon with two questions.", 5, &["too long"]), short, &[], "x").unwrap_err(), Reject::NotOneSentence);
        // a split that drops half the words is a summary
        assert_eq!(validate(&item("rephrase", "I had no conviction. I questioned the choice.", 5, &["too long"]), long, &[], "x").unwrap_err(), Reject::LengthBand);
    }

    #[test]
    fn rewrites_that_drop_content_are_rejected() {
        let o1 = "But to make you understand why, I have to take you back, to exactly 41 days ago.";
        assert!(validate(&item("rephrase", "Go back 41 days to understand why.", 5, &["tangled"]), o1, &[], "x").is_err(), "a summary of a plain sentence");
        let o2 = "But what held me back was the idea of \"hedging my bets\", thinking that dropping out was risky when, in reality, school would always be there.";
        let bad = "But the risk of 'hedging my bets' kept me back.";
        assert!(content_kept(o2, bad) < 0.4, "{}", content_kept(o2, bad));
        assert!(validate(&item("rephrase", bad, 5, &["tangled"]), o2, &[], "x").is_err());
        let in_band = "But what held me back was fear, thinking that dropping out was a gamble when school would still exist.";
        assert!(matches!(validate(&item("rephrase", in_band, 5, &["tangled"]), o2, &[], "x").unwrap_err(), Reject::ContentLost(_)));
        // the canonical wordy rewrite keeps its content words
        let o3 = "In order to be able to make progress at this point in time, it was decided by me that the best course of action would be to send a very large number of cold emails to founders.";
        let good = "To make progress now, I decided to send many cold emails to founders.";
        assert!(content_kept(o3, good) >= MIN_CONTENT_KEPT, "{}", content_kept(o3, good));
        assert!(validate(&item("rephrase", good, 5, &["wordy phrase"]), o3, &[], "x").unwrap().is_some());
        // a 25-word sentence rewritten as two is titled as a rephrase, not a split
        let o25 = "But what held me back was the idea of \"hedging my bets\", thinking that dropping out was risky when, in reality, school would always be there.";
        let two = "But the idea of \"hedging my bets\" held me back. I thought dropping out was risky when school would always be there.";
        let v = validate(&item("rephrase", two, 5, &["tangled"]), o25, &[], "x").unwrap().unwrap();
        assert_eq!(title_and_message(&v).1, "Rephrase for clarity");
        // a short sentence may swap one content word for a synonym
        let o5 = "We will circle back with further details in due course.";
        assert!(validate(&item("rephrase", "We will follow up with more details soon.", 5, &["wordy phrase"]), o5, &[], "x").unwrap().is_some());
        // passive to active survives the stemmer
        let o4 = "It was decided by the leadership team that the launch would be delayed after the engineers found the payment system unreliable.";
        let active = "The leadership team delayed the launch after engineers found the payment system unreliable.";
        assert!(validate(&item("rephrase", active, 5, &["needless passive"]), o4, &[], "x").unwrap().is_some());
    }

    #[test]
    fn clean_short_sentences_and_spelled_numbers_are_safe() {
        let o = "The radio played the same four songs.";
        assert_eq!(validate(&item("rephrase", "The radio played the same songs over and over.", 5, &["tangled"]), o, &[], "x").unwrap_err(), Reject::TooShortToRephrase);
        let o2 = "The venue had to be booked six months ahead because the caterer needed a final headcount.";
        assert_eq!(validate(&item("rephrase", "The venue had to be booked months ahead because the caterer needed a headcount.", 5, &["tangled"]), o2, &[], "x").unwrap_err(), Reject::NumberLost("six".into()));
        // an unhinted rewrite may not grow
        let o3 = "The committee met on Tuesday and asked every team for a revised budget by Friday.";
        assert_eq!(validate(&item("rephrase", "On Tuesday the committee met and then asked every single team for a revised budget by Friday at the latest.", 5, &["tangled"]), o3, &[], "x").unwrap_err(), Reject::LengthBand);
    }

    #[test]
    fn unhinted_rewrites_need_confidence_and_a_light_touch() {
        let o = "The meeting has been rescheduled to Friday at noon because the room was double booked.";
        let r = "The meeting moved to Friday at noon because the room was double booked.";
        assert_eq!(validate(&item("rephrase", r, 3, &["tangled"]), o, &[], "x").unwrap_err(), Reject::LowConfidence);
        assert!(validate(&item("rephrase", r, 4, &["tangled"]), o, &[], "x").unwrap().is_some());
        assert_eq!(validate(&item("rephrase", "The meeting moved.", 5, &["tangled"]), o, &[], "x").unwrap_err(), Reject::LengthBand);
    }

    #[test]
    fn plain_split_cuts_a_long_sentence_at_its_middle_joint() {
        let long = "i wish i could say that i felt some sort of conviction, but honestly, it was stranger than anything I had ever experienced, and with every day that went by without results i couldnt help but question if i had made the right choice.";
        assert_eq!(plain_split(long).unwrap(), "i wish i could say that i felt some sort of conviction, but honestly, it was stranger than anything I had ever experienced. With every day that went by without results i couldnt help but question if i had made the right choice.");
        assert!(plain_split("Short one, and short two.").is_none());
        let v = plain_verdict(long, &[], "x").expect("the fallback split");
        assert_eq!(title_and_message(&v).1, "Split the long sentence");
        let but = "Because the venue had to be booked six months ahead and the caterer needed a final headcount and the speakers could only confirm after their visa interviews, but the sponsors still wanted a date before the end of the quarter so the date moved to May 14.";
        assert!(plain_split(but).unwrap().contains(". But the sponsors"), "{:?}", plain_split(but));
        // The list comma nearest the middle is not a joint; the clause joint further out is.
        let list = "On the way home from the long meeting I bought milk, eggs, bread, and butter at the corner store because the fridge at the office had been empty since the week before the holiday, and nobody on the team had remembered to restock it before Monday.";
        let s = plain_split(list).expect("split at the clause joint");
        assert!(s.contains("holiday. Nobody on the team"), "{s}");
        assert!(!s.contains("bread. Butter"), "{s}");
        assert_eq!(content_loss(list, &s).0, 0, "lossless");
    }

    #[test]
    fn too_long_hint_names_the_word_count() {
        let h = hints_for("i wish i could say that i felt some sort of conviction, but honestly, it was stranger than anything I had ever experienced, and with every day that went by without results i couldnt help but question if i had made the right choice.", &[]);
        assert!(h.iter().any(|x| x.starts_with("too long (44 words)")), "{h:?}");
        assert!(hints_for("Short and clear, this one stays.", &[]).is_empty());
    }

    #[test]
    fn merge_hides_harpers_long_sentence_note_under_a_spot_rewrite() {
        let harper = vec![mk_alert("Readability", 10, 60, 1), mk_alert("Readability", 100, 150, 2), mk_alert("Spelling", 12, 14, 3)];
        let spot = vec![mk_alert("SpotRephrase", 10, 60, 4)];
        let v = merge_visible(&harper, &spot, |h| h == 3);
        let kinds: Vec<(&str, usize)> = v.iter().map(|a| (a.kind.as_str(), a.u16_start)).collect();
        assert_eq!(kinds, vec![("Readability", 100), ("SpotRephrase", 10)], "the covered note and the dismissed word alert are gone, word alerts stay first");
    }

    /// Live tuning run against a local model (no app windows, no desktop):
    /// `GRAMMARLAMA_LIVE_TEXT=/path/to/file.txt cargo test live_text -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_text() {
        let Ok(path) = std::env::var("GRAMMARLAMA_LIVE_TEXT") else { return };
        let text = std::fs::read_to_string(&path).expect("read GRAMMARLAMA_LIVE_TEXT");
        let cfg = crate::settings::LlmConfig {
            provider: crate::settings::Provider::Ollama,
            base_url: std::env::var("GRAMMARLAMA_LIVE_BASE").unwrap_or_else(|_| "http://127.0.0.1:11434".into()),
            api_key: String::new(),
            model: std::env::var("GRAMMARLAMA_LIVE_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into()),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cache = Cache::new();
        let t0 = std::time::Instant::now();
        let o = rt.block_on(run(Some(&cfg), &text, &mut cache, &std::env::var("GRAMMARLAMA_LIVE_GOAL").unwrap_or_default())).expect("spot check");
        println!("LIVE sentences={} checked={} batches={} rejected={} ms={}", o.sentences, o.checked, o.batches, o.rejected, t0.elapsed().as_millis());
        let mut j = serde_json::to_value(&o).unwrap();
        j["ms"] = serde_json::json!(t0.elapsed().as_millis() as u64);
        println!("LIVE_JSON {}", j);
        for a in &o.alerts {
            println!("ALERT [{}] {}\n   - {}\n   + {}\n   why: {}", a.kind, a.title, a.original, a.suggestions[0], a.message);
        }
        for r in &o.rejections {
            println!("REJECTED {r}");
        }
    }

    #[test]
    fn plain_rewrites_swap_wordy_phrases_and_keep_case() {
        assert_eq!(plain_rewrite("It is important to note that attendance is mandatory for all team members.").unwrap().0, "Attendance is mandatory for all team members.");
        assert_eq!(plain_rewrite("In order to win we must ship it at this point in time.").unwrap().0, "To win we must ship it now.");
        assert_eq!(plain_rewrite("in order to win we ship.").unwrap().0, "to win we ship.");
        assert_eq!(plain_rewrite("The reason for this is due to the fact that the designs are not ready.").unwrap().0, "This is because the designs are not ready.");
        assert!(plain_rewrite("We shipped it on Friday.").is_none());
        let v = plain_verdict("Please be advised that the meeting has been rescheduled.", &[], "x").unwrap();
        assert!(matches!(v, Verdict::Rephrase { text, .. } if text == "The meeting has been rescheduled."));
    }

    #[tokio::test]
    async fn run_without_a_model_still_rewrites_hinted_sentences() {
        let text = "We shipped it on Friday. In the event that you are unable to attend, please notify your manager. Thanks.";
        let mut cache = Cache::new();
        let o = run(None, text, &mut cache, "").await.unwrap();
        assert_eq!(o.batches, 0);
        assert_eq!(o.alerts.len(), 1, "{:?}", o.alerts);
        assert_eq!(o.alerts[0].suggestions[0], "If you are unable to attend, please notify your manager.");
        assert_eq!(o.alerts[0].title, "Make it more concise");
        // cached: a second run asks nothing and finds the same thing
        let o2 = run(None, text, &mut cache, "").await.unwrap();
        assert_eq!(o2.alerts.len(), 1);
        assert_eq!(o2.from_cache, 2);
    }

    #[test]
    fn keep_and_unknown_actions_are_no_verdict() {
        assert_eq!(validate(&item("keep", "", 5, &[]), "Fine sentence here.", &[], "x").unwrap(), None);
        assert_eq!(validate(&item("merge", "", 5, &[]), "Fine sentence here.", &[], "x").unwrap(), None);
    }

    #[test]
    fn alerts_carry_round_trip_offsets_even_after_emoji() {
        let text = "😀 Hello there friend. In order to be able to win we must ship. The end is here now.";
        let s = split_sentences(text);
        let map = TextMap::new(text);
        let v = Verdict::Rephrase { text: "To win we must ship.".into(), why: "Shorter.".into(), flags: vec!["wordy phrase".into()] };
        let a = alert_for(text, &map, &s, 1, &v);
        assert_eq!(crate::textmap::slice_utf16(text, a.u16_start, a.u16_end), "In order to be able to win we must ship.");
        assert_eq!(a.title, "Make it more concise");
        assert_eq!(a.category, Category::Clarity);
        let r = Verdict::Remove { why: String::new(), flags: vec!["repeats earlier sentence".into()] };
        let b = alert_for(text, &map, &s, 1, &r);
        assert_eq!(crate::textmap::slice_utf16(text, b.u16_start, b.u16_end), "In order to be able to win we must ship. ");
        assert_eq!(crate::textmap::splice_utf16(text, b.u16_start, b.u16_end - b.u16_start, ""), "😀 Hello there friend. The end is here now.");
        assert_eq!(b.title, "Remove the repeated sentence");
    }

    #[test]
    fn re_anchor_follows_a_moved_sentence_and_drops_a_changed_one() {
        let text = "Alpha beta gamma delta. In order to win we must ship. Omega.";
        let s = split_sentences(text);
        let map = TextMap::new(text);
        let v = Verdict::Rephrase { text: "To win we must ship.".into(), why: String::new(), flags: vec![] };
        let mut alerts = vec![alert_for(text, &map, &s, 1, &v)];
        let moved = "Prefix added here. Alpha beta gamma delta. In order to win we must ship. Omega.";
        re_anchor(&mut alerts, moved);
        assert_eq!(alerts.len(), 1);
        assert_eq!(crate::textmap::slice_utf16(moved, alerts[0].u16_start, alerts[0].u16_end), "In order to win we must ship.");
        re_anchor(&mut alerts, "Alpha beta gamma delta. We must ship. Omega.");
        assert!(alerts.is_empty());
    }

    #[test]
    fn hints_name_wordy_phrases_hedges_repeats_and_long_sentences() {
        assert_eq!(hints_for("At this point in time we wait.", &[]), vec!["wordy phrase \"at this point in time\""]);
        assert!(hints_for("Short and clear.", &[]).is_empty());
        let long: String = (0..35).map(|_| "word ").collect();
        assert_eq!(hints_for(&long, &[]), vec!["too long (35 words): split it into two or three sentences at its natural joints"]);
        assert_eq!(hints_for("I think that we should probably consider maybe moving the deadline.", &[]), vec!["hedge stack"]);
        assert_eq!(hints_for("The food here is amazing.", &["I love this city.", "The food is amazing and the people are kind."]), vec!["may repeat sentence 2"]);
    }

    #[test]
    fn filler_and_overlap_detectors() {
        assert!(filler_like("As I said before, it was bad."));
        assert!(filler_like("Anyway, that is what happened."));
        assert!(filler_like("So here is the thing."));
        assert!(!filler_like("Thanks for reading."));
        assert!(overlap_with("The food here is amazing.", &["The food is amazing and the people are kind."]) >= 0.6);
        assert!(overlap_with("I didnt tell my friends and I didnt tell my peers.", &["Six weeks ago I pulled out of school."]) < 0.6);
    }
}
