//! AI grammar pass: sentence-level errors that fixed rules cannot find safely
//! (comma splices, run-ons, missing commas, who/whom, pronoun case,
//! subject-verb agreement, verb tense).
//!
//! The model is asked for the minimally corrected sentence; the answer is
//! never trusted as-is. It is diffed against the original at the
//! word/punctuation level, every change is checked by strict guards (a
//! rejected answer drops the whole sentence), and each surviving change
//! becomes its own small `AiGrammar` alert so the user sees "Add a comma",
//! not a rewritten sentence.

use crate::engine::{stable_hash, Alert, Category};
use crate::features::spotcheck::{split_sentences, Sentence};
use crate::llm;
use crate::settings::LlmConfig;
use crate::textmap::TextMap;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const KIND: &str = "AiGrammar";
/// Sentences per request.
pub const MAX_BATCH: usize = 4;
pub const MAX_BATCH_CHARS: usize = 700;
/// Longer sentences are left to the other checks.
pub const MAX_WORDS: usize = 80;
/// At most this many uncached sentences per run (the rest follow on the next run).
pub const MAX_PER_RUN: usize = 80;
pub const CACHE_MAX: usize = 1500;
/// Total changed characters allowed: max(MIN_CHANGE_CHARS, MAX_CHANGE_RATIO of the sentence).
pub const MIN_CHANGE_CHARS: usize = 12;
pub const MAX_CHANGE_RATIO: f64 = 0.15;
/// A hunk may change at most this many content words.
pub const MAX_HUNK_CONTENT_WORDS: usize = 2;
/// The corrected sentence may be at most this many words longer.
pub const MAX_EXTRA_WORDS: usize = 3;
/// Bump when the prompt or guards change so old cached answers are not reused.
const VERSION: u32 = 3;

/// sentence+model hash -> validated corrected sentence (`None` = leave alone).
pub type Cache = HashMap<u64, Option<String>>;

pub const SYSTEM: &str = r#"You are a careful proofreader. You receive numbered sentences. For each one, return the minimally corrected sentence.
Fix only grammar, punctuation and capitalization errors. Do not change word choice, style, tone or meaning. Do not add or remove information. If the sentence is already correct, return it unchanged.
Rules:
1. Comma splice (two complete sentences joined by only a comma, no and/but/so): replace the comma with a semicolon.
2. Run-on (two complete sentences with nothing between them): end the first with a period and capitalize the next word.
3. Add a comma after an introductory clause or word ("However,", "After he left,", "Because it rained,").
4. Add a comma before and/but/so/or when it joins two complete sentences ("It was late, so we left.").
5. Use "whom" (not "who") when it is the object of a verb or preposition ("the man whom I met", "to whom").
6. Pronoun case: "Her and Maria" -> "She and Maria"; "between him and I" -> "between him and me"; "Us students" -> "We students"; "Me and Tom" -> "Tom and I".
7. Fix subject-verb agreement and wrong verb tense.
Never remove "and", "but" or "so". Never add a semicolon to a sentence that already has a comma plus and/but/so. Keep every name, number and quotation exactly as written, including numbers written as words.
If a HINTS section follows the sentences, each hint points at a likely error in that numbered sentence; fix it. Answer with exactly one line per sentence, in the same order, in the form "N. corrected sentence". Output nothing else (no hints, no comments).

Example input:
1. After the meeting ended we walked home together.
2. The weather was cold, the roads were icy.
3. My brother and me bought a new car.
4. The report is finished, and it is on your desk.
5. The garden was quiet we sat outside for hours.
6. We cleaned the kitchen and the guests arrived soon after.
7. The woman who I called was out of town.
8. She bought an umbrella before the storm.
9. Between you and I, the plan is risky.
10. The boxes was delivered on Monday.
Example output:
1. After the meeting ended, we walked home together.
2. The weather was cold; the roads were icy.
3. My brother and I bought a new car.
4. The report is finished, and it is on your desk.
5. The garden was quiet. We sat outside for hours.
6. We cleaned the kitchen, and the guests arrived soon after.
7. The woman whom I called was out of town.
8. She bought an umbrella before the storm.
9. Between you and me, the plan is risky.
10. The boxes were delivered on Monday."#;

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub alerts: Vec<Alert>,
    pub sentences: usize,
    /// sentences sent to the model this run
    pub checked: usize,
    pub from_cache: usize,
    pub batches: usize,
    pub rejected: usize,
    pub rejections: Vec<String>,
}

// ---------------------------------------------------------------- words

const FUNCTION_WORDS: &[&str] = &["a", "an", "the", "and", "but", "so", "or", "that", "who", "whom", "have", "has", "is", "are", "was", "were", "me", "i", "he", "she", "they", "them", "him", "her", "we", "us"];
const PRONOUNS: &[&str] = &["i", "me", "he", "him", "she", "her", "they", "them", "we", "us", "who", "whom"];
const PRONOUN_FORMS: &[&[&str]] = &[&["i", "me"], &["he", "him"], &["she", "her"], &["they", "them"], &["we", "us"], &["who", "whom"]];
const CONJUNCTIONS: &[&str] = &["and", "but", "so", "or"];

/// Verb and noun families whose members may replace each other (inflection, not a synonym swap).
const FAMILIES: &[&[&str]] = &[
    &["be", "am", "is", "are", "was", "were", "been", "being"],
    &["have", "has", "had", "having"],
    &["do", "does", "did", "done", "doing"],
    &["go", "goes", "went", "gone", "going"],
    &["see", "sees", "saw", "seen", "seeing"],
    &["write", "writes", "wrote", "written", "writing"],
    &["take", "takes", "took", "taken", "taking"],
    &["give", "gives", "gave", "given", "giving"],
    &["come", "comes", "came", "coming"],
    &["run", "runs", "ran", "running"],
    &["eat", "eats", "ate", "eaten", "eating"],
    &["begin", "begins", "began", "begun", "beginning"],
    &["buy", "buys", "bought", "buying"],
    &["bring", "brings", "brought", "bringing"],
    &["think", "thinks", "thought", "thinking"],
    &["make", "makes", "made", "making"],
    &["say", "says", "said", "saying"],
    &["know", "knows", "knew", "known", "knowing"],
    &["get", "gets", "got", "gotten", "getting"],
    &["tell", "tells", "told", "telling"],
    &["find", "finds", "found", "finding"],
    &["leave", "leaves", "left", "leaving"],
    &["feel", "feels", "felt", "feeling"],
    &["keep", "keeps", "kept", "keeping"],
    &["sell", "sells", "sold", "selling"],
    &["send", "sends", "sent", "sending"],
    &["build", "builds", "built", "building"],
    &["speak", "speaks", "spoke", "spoken", "speaking"],
    &["choose", "chooses", "chose", "chosen", "choosing"],
    &["break", "breaks", "broke", "broken", "breaking"],
    &["drive", "drives", "drove", "driven", "driving"],
    &["fall", "falls", "fell", "fallen", "falling"],
    &["grow", "grows", "grew", "grown", "growing"],
    &["forget", "forgets", "forgot", "forgotten", "forgetting"],
    &["sing", "sings", "sang", "sung", "singing"],
    &["swim", "swims", "swam", "swum", "swimming"],
    &["win", "wins", "won", "winning"],
    &["lose", "loses", "lost", "losing"],
    &["meet", "meets", "met", "meeting"],
    &["stand", "stands", "stood", "standing"],
    &["teach", "teaches", "taught", "teaching"],
    &["catch", "catches", "caught", "catching"],
    &["hold", "holds", "held", "holding"],
    &["pay", "pays", "paid", "paying"],
    &["sit", "sits", "sat", "sitting"],
    &["lead", "leads", "led", "leading"],
    &["read", "reads", "reading"],
    &["fly", "flies", "flew", "flown", "flying"],
    &["draw", "draws", "drew", "drawn", "drawing"],
    &["wear", "wears", "wore", "worn", "wearing"],
    &["don't", "doesn't", "didn't"],
    &["isn't", "aren't", "wasn't", "weren't"],
    &["haven't", "hasn't", "hadn't"],
    &["child", "children"],
    &["man", "men"],
    &["woman", "women"],
    &["person", "people"],
];

fn is_function(w: &str) -> bool {
    FUNCTION_WORDS.contains(&w)
}

/// True when `a` and `b` look like two forms of one word (go/goes, bark/barks,
/// walk/walked, went/gone). A synonym ("big"/"large") is not.
fn related(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if FAMILIES.iter().any(|f| f.contains(&a) && f.contains(&b)) {
        return true;
    }
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let cp = ac.iter().zip(bc.iter()).take_while(|(x, y)| x == y).count();
    let (min, max) = (ac.len().min(bc.len()), ac.len().max(bc.len()));
    max - min <= 4 && cp >= 2.max(min.saturating_sub(2))
}

// ---------------------------------------------------------------- tokens

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tok {
    pub text: String,
    /// char offsets in the sentence
    pub start: usize,
    pub end: usize,
    pub word: bool,
}

fn is_apos(c: char) -> bool {
    c == '\'' || c == '’'
}

pub fn tokenize(s: &str) -> Vec<Tok> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_alphanumeric() {
            let st = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || (is_apos(cs[i]) && i + 1 < cs.len() && cs[i + 1].is_alphanumeric() && i > st)) {
                i += 1;
            }
            out.push(Tok { text: cs[st..i].iter().collect(), start: st, end: i, word: true });
        } else {
            out.push(Tok { text: c.to_string(), start: i, end: i + 1, word: false });
            i += 1;
        }
    }
    out
}

fn sub(s: &[char], a: usize, b: usize) -> String {
    s[a..b].iter().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hunk {
    i0: usize,
    i1: usize,
    j0: usize,
    j1: usize,
}

/// Token-level diff (longest common subsequence); the gaps between matches are hunks.
fn diff_hunks(a: &[Tok], b: &[Tok]) -> Vec<Hunk> {
    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    let mut dp = vec![0u16; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * w + j] = if a[i].text == b[j].text { dp[(i + 1) * w + j + 1] + 1 } else { dp[(i + 1) * w + j].max(dp[i * w + j + 1]) };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut hi, mut hj) = (0, 0);
    while i < n && j < m {
        if a[i].text == b[j].text {
            if hi != i || hj != j {
                hunks.push(Hunk { i0: hi, i1: i, j0: hj, j1: j });
            }
            i += 1;
            j += 1;
            hi = i;
            hj = j;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if hi != n || hj != m {
        hunks.push(Hunk { i0: hi, i1: n, j0: hj, j1: m });
    }
    hunks
}

fn lower_words(toks: &[Tok]) -> Vec<String> {
    toks.iter().filter(|t| t.word).map(|t| t.text.to_lowercase()).collect()
}

/// Words only in `r` / only in `a` (multiset difference, case-insensitive).
fn word_delta(r: &[String], a: &[String]) -> (Vec<String>, Vec<String>) {
    let mut a_left = a.to_vec();
    let mut r_left = Vec::new();
    for w in r {
        if let Some(p) = a_left.iter().position(|x| x == w) {
            a_left.remove(p);
        } else {
            r_left.push(w.clone());
        }
    }
    (r_left, a_left)
}

/// Hunks that move a word from one place to another are merged into one, so
/// each alert is a change that can be applied on its own.
fn merge_moves(a: &[Tok], b: &[Tok], mut hunks: Vec<Hunk>) -> Vec<Hunk> {
    loop {
        let sets: Vec<(Vec<String>, Vec<String>)> = hunks.iter().map(|h| (lower_words(&a[h.i0..h.i1]), lower_words(&b[h.j0..h.j1]))).collect();
        let mut found = None;
        'outer: for x in 0..hunks.len() {
            for y in x + 1..hunks.len() {
                let moved = sets[x].0.iter().any(|w| sets[y].1.contains(w)) || sets[x].1.iter().any(|w| sets[y].0.contains(w));
                if moved {
                    found = Some((x, y));
                    break 'outer;
                }
            }
        }
        match found {
            Some((x, y)) => {
                let merged = Hunk { i0: hunks[x].i0, i1: hunks[y].i1, j0: hunks[x].j0, j1: hunks[y].j1 };
                hunks.splice(x..=y, [merged]);
            }
            None => return hunks,
        }
    }
}

// ---------------------------------------------------------------- guards

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    Empty,
    TooLong,
    TooManyChars(usize),
    HunkTooBig(String),
    NumberChanged(String),
    NameChanged(String),
    QuoteChanged(String),
    WordAdded(String),
    WordRemoved(String),
    Synonym(String),
    Punctuation(String),
    Unparseable,
    Structure(String),
    Contraction(String),
}

fn numbers_of(s: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\d[\d,.:/%]*").unwrap());
    re.find_iter(s).map(|m| m.as_str().trim_end_matches(|c| c == '.' || c == ',' || c == ':').to_string()).collect()
}

const NUMBER_WORDS: &[&str] = &["two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "twenty", "thirty", "forty", "fifty", "hundred", "thousand", "million", "billion", "twice", "half", "dozen"];

fn quotes_of(s: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#""([^"]{3,})"|“([^”]{3,})”"#).unwrap());
    re.captures_iter(s).filter_map(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().to_string())).collect()
}

/// Capitalized words that are not the first word of a sentence.
fn names_of(toks: &[Tok]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at_start = true;
    for t in toks {
        if !t.word {
            if matches!(t.text.as_str(), "." | "!" | "?") {
                at_start = true;
            }
            continue;
        }
        let up = t.text.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
        if up && !at_start && t.text != "I" && t.text.chars().count() >= 2 {
            out.push(t.text.clone());
        }
        at_start = false;
    }
    out
}

fn is_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// Make the end of the model's answer match the original's (it may add or
/// drop the final period, wrap the line in quotes, or add a label).
fn clean_answer(orig: &str, ans: &str) -> String {
    let mut t = ans.trim().to_string();
    for p in ["Corrected:", "corrected:", "Answer:"] {
        if let Some(r) = t.strip_prefix(p) {
            t = r.trim().to_string();
        }
    }
    let quoted = |s: &str| s.starts_with('"') || s.starts_with('“');
    if !quoted(orig) && t.len() > 2 && quoted(&t) && (t.ends_with('"') || t.ends_with('”')) {
        t = t.chars().skip(1).take(t.chars().count() - 2).collect::<String>().trim().to_string();
    }
    let o_term = orig.trim_end().chars().last().map(is_terminal).unwrap_or(false);
    let t_term = t.chars().last().map(is_terminal).unwrap_or(false);
    if o_term && !t_term {
        t.push(orig.trim_end().chars().last().unwrap());
    } else if !o_term && t_term {
        while t.chars().last().map(is_terminal).unwrap_or(false) {
            t.pop();
        }
    }
    t
}

fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn token_chars(toks: &[Tok]) -> usize {
    toks.iter().map(|t| t.end - t.start).sum()
}

/// Apply every guard. `Ok(None)` = the answer equals the original. `Ok(Some)` =
/// the cleaned corrected sentence, safe to diff.
pub fn validate(orig: &str, answer: &str) -> std::result::Result<Option<String>, Reject> {
    let corrected = clean_answer(orig, answer);
    if corrected.trim().is_empty() || corrected.contains('\n') {
        return Err(Reject::Empty);
    }
    if norm_ws(&corrected) == norm_ws(orig) {
        return Ok(None);
    }
    let corrected = canonicalize(orig, &corrected);
    if norm_ws(&corrected) == norm_ws(orig) {
        return Ok(None);
    }
    let (a, b) = (tokenize(orig), tokenize(&corrected));
    if a.len() > 300 || b.len() > 300 {
        return Err(Reject::TooLong);
    }
    let (aw, bw) = (lower_words(&a), lower_words(&b));
    // the corrected sentence may not grow by more than a few words
    if bw.len() > aw.len() + MAX_EXTRA_WORDS {
        return Err(Reject::TooLong);
    }
    // numbers, spelled-out numbers, quotes, names
    for n in numbers_of(orig) {
        if !corrected.contains(&n) {
            return Err(Reject::NumberChanged(n));
        }
    }
    for w in aw.iter().filter(|w| NUMBER_WORDS.contains(&w.as_str())) {
        if !bw.contains(w) {
            return Err(Reject::NumberChanged(w.clone()));
        }
    }
    for q in quotes_of(orig) {
        if !corrected.contains(&q) {
            return Err(Reject::QuoteChanged(q));
        }
    }
    let b_texts: Vec<&str> = b.iter().map(|t| t.text.as_str()).collect();
    for nm in names_of(&a) {
        if !b_texts.contains(&nm.as_str()) {
            return Err(Reject::NameChanged(nm));
        }
    }
    // only , ; . punctuation may change
    let hunks = merge_moves(&a, &b, diff_hunks(&a, &b));
    for h in &hunks {
        for t in a[h.i0..h.i1].iter().chain(b[h.j0..h.j1].iter()) {
            if !t.word && !matches!(t.text.as_str(), "," | ";" | ".") {
                return Err(Reject::Punctuation(t.text.clone()));
            }
        }
    }
    check_structure(&a, &b, &hunks)?;
    // total size of the edit
    let changed: usize = hunks.iter().map(|h| token_chars(&a[h.i0..h.i1]).max(token_chars(&b[h.j0..h.j1]))).sum();
    let limit = MIN_CHANGE_CHARS.max((orig.chars().count() as f64 * MAX_CHANGE_RATIO) as usize);
    if changed > limit {
        return Err(Reject::TooManyChars(changed));
    }
    // per hunk: at most two content words change
    for h in &hunks {
        let (r, ad) = word_delta(&lower_words(&a[h.i0..h.i1]), &lower_words(&b[h.j0..h.j1]));
        let content = |v: &[String]| v.iter().filter(|w| !is_function(w)).count();
        if content(&r).max(content(&ad)) > MAX_HUNK_CONTENT_WORDS {
            return Err(Reject::HunkTooBig(format!("{} -> {}", r.join(" "), ad.join(" "))));
        }
    }
    // words added or removed: function words only, or a new form of a word that was removed
    let (mut removed, mut added) = word_delta(&aw, &bw);
    let mut unpaired_removed = Vec::new();
    for r in removed.drain(..) {
        if let Some(p) = added.iter().position(|x| related(&r, x)) {
            added.remove(p);
        } else {
            unpaired_removed.push(r);
        }
    }
    if let Some(w) = unpaired_removed.iter().find(|w| CONJUNCTIONS.contains(&w.as_str()) || matches!(w.as_str(), "who" | "whom" | "that")) {
        return Err(Reject::WordRemoved(w.clone()));
    }
    // "it's" -> "it is" and back is a style change, not a fix
    for (x, y) in [(&aw, &bw), (&bw, &aw)] {
        for w in x.iter().filter(|w| w.contains('\'') || w.contains('’')) {
            let base = w.split(|c| c == '\'' || c == '’').next().unwrap_or("");
            if !y.contains(w) && y.iter().any(|v| v == base) && !x.iter().any(|v| v == base) {
                return Err(Reject::Contraction(w.clone()));
            }
        }
    }
    // a pronoun may only change case ("me" -> "I"), never person ("me" -> "she")
    let group = |w: &str| PRONOUN_FORMS.iter().position(|g| g.contains(&w));
    for r in unpaired_removed.iter().filter(|w| group(w).is_some()) {
        let g = group(r);
        match added.iter().position(|x| group(x) == g) {
            Some(i) => {
                added.remove(i);
            }
            None => return Err(Reject::Synonym(r.clone())),
        }
    }
    if let Some(w) = added.iter().find(|w| group(w).is_some()) {
        return Err(Reject::WordAdded(w.clone()));
    }
    if let Some(w) = unpaired_removed.iter().find(|w| !is_function(w)) {
        // a replaced word that is not a form of any added word is a synonym swap
        return Err(if added.iter().any(|x| !is_function(x)) { Reject::Synonym(w.clone()) } else { Reject::WordRemoved(w.clone()) });
    }
    if let Some(w) = added.iter().find(|w| !is_function(w)) {
        return Err(Reject::WordAdded(w.clone()));
    }
    Ok(Some(corrected))
}

// ---------------------------------------------------------------- structure guards

const SUBORDINATORS: &[&str] = &["after", "although", "as", "because", "before", "if", "once", "since", "though", "unless", "until", "when", "whenever", "while", "whereas"];
const PREPOSITIONS: &[&str] = &["in", "on", "at", "by", "during", "for", "with", "without", "within", "under", "over", "through", "throughout", "despite", "among", "of"];
const TRANSITIONS: &[&str] = &["however", "therefore", "moreover", "furthermore", "meanwhile", "otherwise", "finally", "thus", "nevertheless", "consequently", "additionally", "besides", "instead", "still", "unfortunately", "fortunately", "luckily", "sadly", "ultimately", "eventually", "overall"];
const GREETINGS: &[&str] = &["hi", "hello", "hey", "thanks", "dear", "yes", "no", "okay", "ok", "well", "please", "thank"];
const SUBJECT_STARTERS: &[&str] = &[
    "the", "a", "an", "my", "our", "your", "his", "her", "their", "its", "this", "that", "these", "those", "i", "he", "she", "it", "we", "they", "you", "there", "here", "someone", "somebody", "nobody", "everyone", "everybody", "anyone", "something", "nothing", "everything", "no", "some", "many", "most", "all", "each", "every", "both", "then", "now", "today", "tomorrow", "tonight", "yesterday", "later", "inside", "outside", "soon",
];

const NON_STARTERS: &[&str] = &["and", "but", "or", "so", "yet", "nor", "because", "than", "as", "while", "which", "who", "whom", "whose", "to", "of", "in", "on", "at", "by", "for", "with", "from", "into", "if", "though", "although", "when", "whenever", "before", "after", "until", "unless", "since", "whereas", "however"];

fn is_stop(t: &Tok) -> bool {
    !t.word && matches!(t.text.as_str(), "." | "!" | "?" | ";")
}

/// Word tokens of `b` before index `p`, back to the previous sentence stop.
fn words_before(b: &[Tok], p: usize) -> usize {
    b[..p].iter().rev().take_while(|t| !is_stop(t)).filter(|t| t.word).count()
}

/// Word tokens of `b` after index `p`, up to the next sentence stop.
fn words_after(b: &[Tok], p: usize) -> usize {
    b[p + 1..].iter().take_while(|t| !is_stop(t)).filter(|t| t.word).count()
}

fn next_word(b: &[Tok], p: usize) -> Option<&Tok> {
    b[p + 1..].iter().find(|t| t.word)
}

fn punct_count(toks: &[Tok], set: &[&str]) -> usize {
    toks.iter().filter(|t| !t.word && set.contains(&t.text.as_str())).count()
}

/// A model that writes ";" where two sentences simply run together gets a
/// period and a capital instead (the usual run-on fix).
fn canonicalize(orig: &str, corrected: &str) -> String {
    let (a, b) = (tokenize(orig), tokenize(corrected));
    if a.len() > 300 || b.len() > 300 {
        return corrected.to_string();
    }
    let hunks = merge_moves(&a, &b, diff_hunks(&a, &b));
    let mut fixes: Vec<(usize, usize)> = Vec::new(); // (semicolon char index, next word start)
    let mut conj: Vec<(usize, usize, String)> = Vec::new(); // (start, end, replacement) in corrected
    for h in &hunks {
        let (ra, rb) = (&a[h.i0..h.i1], &b[h.j0..h.j1]);
        // "X and Y" -> "X; Y" and "X; but Y" are the comma-before-conjunction fix in disguise
        if rb.len() == 1 && !rb[0].word && rb[0].text == ";" && ra.len() == 1 && ra[0].word && CONJUNCTIONS.contains(&ra[0].text.to_lowercase().as_str()) {
            conj.push((rb[0].start, rb[0].end, format!(", {}", ra[0].text)));
            continue;
        }
        if ra.is_empty() && rb.len() == 1 && !rb[0].word && rb[0].text == ";" {
            if let Some(nw) = next_word(&b, h.j0) {
                if CONJUNCTIONS.contains(&nw.text.to_lowercase().as_str()) {
                    conj.push((rb[0].start, rb[0].end, ",".to_string()));
                    continue;
                }
            }
        }
        if punct_count(ra, &[",", ";"]) == 0 {
            if let Some(off) = rb.iter().position(|t| !t.word && t.text == ";") {
                let p = h.j0 + off;
                if let Some(nw) = next_word(&b, p) {
                    fixes.push((b[p].start, nw.start));
                }
            }
        }
    }
    if fixes.is_empty() && conj.is_empty() {
        return corrected.to_string();
    }
    if !conj.is_empty() {
        let mut out: Vec<char> = corrected.chars().collect();
        for (st, en, r) in conj.into_iter().rev() {
            out.splice(st..en, r.chars());
        }
        return out.into_iter().collect();
    }
    let mut out: Vec<char> = corrected.chars().collect();
    for (semi, nw) in fixes.into_iter().rev() {
        out[semi] = '.';
        if let Some(c) = out.get(nw).copied() {
            let up: Vec<char> = c.to_uppercase().collect();
            if up.len() == 1 {
                out[nw] = up[0];
            }
        }
    }
    out.into_iter().collect()
}

/// Punctuation changes must look like a real fix: a sentence break needs a
/// clause on each side, a new comma needs an introductory clause, a
/// conjunction joining two clauses, or direct address.
fn check_structure(a: &[Tok], b: &[Tok], hunks: &[Hunk]) -> std::result::Result<(), Reject> {
    let bad = |m: &str| Err(Reject::Structure(m.to_string()));
    for h in hunks {
        let (ra, rb) = (&a[h.i0..h.i1], &b[h.j0..h.j1]);
        let (rs, bs) = (punct_count(ra, &[".", ";"]), punct_count(rb, &[".", ";"]));
        let (rc, bc) = (punct_count(ra, &[","]), punct_count(rb, &[","]));
        if bs < rs {
            return bad("sentence break removed");
        }
        if bs > rs {
            let off = rb.iter().position(|t| !t.word && (t.text == "." || t.text == ";")).unwrap_or(0);
            let p = h.j0 + off;
            if words_before(b, p) < 3 || words_after(b, p) < 3 {
                return bad("clause too short");
            }
            let Some(nw) = next_word(b, p) else { return bad("no next word") };
            let low = nw.text.to_lowercase();
            let upper = nw.text.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            let is_name = upper && nw.text != "I" && a.iter().skip(1).any(|t| t.text == nw.text);
            let replaced_comma = punct_count(ra, &[","]) > 0;
            if replaced_comma {
                // a comma splice: any clause may follow, but not a fragment starter
                if NON_STARTERS.contains(&low.as_str()) || low.ends_with("ing") {
                    return bad("next clause is a fragment");
                }
            } else if !SUBJECT_STARTERS.contains(&low.as_str()) && !is_name {
                return bad("next clause does not start with a subject");
            }
            if b[p].text == "." && !upper {
                return bad("period without capital");
            }
            continue;
        }
        if bc > rc {
            let off = rb.iter().position(|t| !t.word && t.text == ",").unwrap_or(0);
            let p = h.j0 + off;
            let before = words_before(b, p);
            let after = words_after(b, p);
            let Some(nw) = next_word(b, p) else { return bad("comma at the end") };
            let nl = nw.text.to_lowercase();
            let first = b.iter().find(|t| t.word).map(|t| t.text.to_lowercase()).unwrap_or_default();
            let cap_name = |t: &Tok| t.text != "I" && t.text.chars().count() >= 2 && t.text.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) && a.iter().skip(1).any(|x| x.text == t.text);
            let prev_tok = b[..p].iter().rev().find(|t| t.word);
            let clause_after = next_word(b, p + 1).map(|t| SUBJECT_STARTERS.contains(&t.text.to_lowercase().as_str()) || cap_name(t)).unwrap_or(false);
            let ok = (["and", "but", "so", "or", "yet"].contains(&nl.as_str()) && before >= 3 && after >= 3 && clause_after)
                || (SUBORDINATORS.contains(&first.as_str()) && (2..=14).contains(&before) && after >= 3)
                || (PREPOSITIONS.contains(&first.as_str()) && (4..=14).contains(&before) && after >= 3)
                || (TRANSITIONS.contains(&first.as_str()) && before == 1 && after >= 3)
                || (GREETINGS.contains(&first.as_str()) && before <= 2 && after >= 1)
                || cap_name(nw)
                || prev_tok.map(|t| cap_name(t)).unwrap_or(false);
            if !ok {
                return bad("comma not in a known position");
            }
        } else if rc > bc {
            let off = ra.iter().position(|t| !t.word && t.text == ",").unwrap_or(0);
            let p = h.i0 + off;
            let nl = next_word(a, p).map(|t| t.text.to_lowercase()).unwrap_or_default();
            if !["that", "than", "of", "to"].contains(&nl.as_str()) {
                return bad("comma removal not recognised");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- hunks -> edits

/// One change to a sentence, in sentence-local char offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub original: String,
    pub replacement: String,
    pub title: String,
    pub message: String,
}

fn count(s: &str, c: char) -> usize {
    s.chars().filter(|x| *x == c).count()
}

fn classify(a: &[Tok], b: &[Tok], anchor: &str) -> (String, String) {
    let (rw, aw) = (lower_words(a), lower_words(b));
    let (r_left, a_left) = word_delta(&rw, &aw);
    let punct = |t: &[Tok]| t.iter().filter(|x| !x.word).map(|x| x.text.as_str()).collect::<String>();
    let (rp, ap) = (punct(a), punct(b));
    let t = |title: &str, msg: &str| (title.to_string(), msg.to_string());
    if r_left.is_empty() && a_left.is_empty() {
        if rp == ap {
            return t("Fix the capitalization", "This word should be capitalized differently.");
        }
        let (rc, ac) = (count(&rp, ','), count(&ap, ','));
        let a_stop = ap.contains(';') || ap.contains('.');
        let r_stop = rp.contains(';') || rp.contains('.');
        if rc > ac && a_stop && !r_stop {
            return t("Fix the comma splice", "Two complete sentences are joined by only a comma; use a semicolon or a period.");
        }
        if a_stop && !r_stop && rc == ac {
            return t("Fix the run-on sentence", "Two complete sentences run together; separate them.");
        }
        if ac > rc {
            return t("Add a comma", "A comma is needed here.");
        }
        if rc > ac {
            return t("Remove the comma", "This comma is not needed.");
        }
        return t("Fix the punctuation", "The punctuation here is off.");
    }
    if let Some(w) = a_left.iter().find(|w| *w == "whom" || *w == "who") {
        let shown = b.iter().find(|x| x.word && x.text.to_lowercase() == *w).map(|x| x.text.clone()).unwrap_or(w.clone());
        return (format!("Use “{shown}”"), if w == "whom" { "“Whom” is the object here.".into() } else { "“Who” is the subject here.".into() });
    }
    if r_left.len() == 1 && a_left.len() == 1 {
        let (r, ad) = (&r_left[0], &a_left[0]);
        if PRONOUNS.contains(&r.as_str()) && PRONOUNS.contains(&ad.as_str()) {
            let shown = b.iter().find(|x| x.word && x.text.to_lowercase() == *ad).map(|x| x.text.clone()).unwrap_or(ad.clone());
            return (format!("Use “{shown}”"), "This pronoun is in the wrong case.".into());
        }
        if related(r, ad) {
            return t("Fix the verb", "This word form does not fit the sentence.");
        }
    }
    if r_left.is_empty() {
        if a_left.iter().any(|w| CONJUNCTIONS.contains(&w.as_str())) && (anchor.contains(',') || ap.contains(',')) {
            return t("Fix the comma splice", "Two complete sentences need a connecting word after the comma.");
        }
        if a_left.len() == 1 {
            let shown = b.iter().find(|x| x.word && x.text.to_lowercase() == a_left[0]).map(|x| x.text.clone()).unwrap_or(a_left[0].clone());
            return (format!("Add “{shown}”"), "A word is missing here.".into());
        }
        return t("Add the missing word", "A word is missing here.");
    }
    if a_left.is_empty() {
        if r_left.len() == 1 {
            return (format!("Remove “{}”", r_left[0]), "This word is not needed.".into());
        }
        return t("Remove the extra word", "These words are not needed.");
    }
    t("Fix the grammar", "This part has a grammar error.")
}

/// Diff `orig` against `corrected` and turn each changed hunk into an `Edit`.
pub fn hunks_to_edits(orig: &str, corrected: &str) -> Vec<Edit> {
    let (a, b) = (tokenize(orig), tokenize(corrected));
    let (oc, cc): (Vec<char>, Vec<char>) = (orig.chars().collect(), corrected.chars().collect());
    let hunks = merge_moves(&a, &b, diff_hunks(&a, &b));
    let mut edits = Vec::new();
    for h in hunks {
        let (ra, rb) = (&a[h.i0..h.i1], &b[h.j0..h.j1]);
        let (os, oe, cs, ce);
        if !ra.is_empty() && !rb.is_empty() {
            // punctuation attached to the word before ("storm. The"): include that word
            let ext = h.i0 > 0 && h.j0 > 0 && !rb[0].word && ra[0].word;
            let (xs, ys) = if ext { (a[h.i0 - 1].start, b[h.j0 - 1].start) } else { (ra[0].start, rb[0].start) };
            (os, oe, cs, ce) = (xs, ra[ra.len() - 1].end, ys, rb[rb.len() - 1].end);
        } else if h.i0 > 0 {
            // insertion or deletion: include the word to its left (and any punctuation in between)
            let mut x = h.i0 - 1;
            let mut y = h.j0 - 1;
            while !a[x].word && x > 0 && y > 0 && a[x - 1].text == b[y - 1].text {
                x -= 1;
                y -= 1;
            }
            (os, oe) = (a[x].start, if ra.is_empty() { a[h.i0 - 1].end } else { ra[ra.len() - 1].end });
            (cs, ce) = (b[y].start, if rb.is_empty() { b[h.j0 - 1].end } else { rb[rb.len() - 1].end });
        } else if h.i1 < a.len() && h.j1 < b.len() {
            // at the start of the sentence: include the word to the right
            (os, oe) = (if ra.is_empty() { a[h.i1].start } else { ra[0].start }, a[h.i1].end);
            (cs, ce) = (if rb.is_empty() { b[h.j1].start } else { rb[0].start }, b[h.j1].end);
        } else {
            (os, oe, cs, ce) = (0, oc.len(), 0, cc.len());
        }
        let anchor = sub(&oc, os, oe);
        let (title, message) = classify(ra, rb, &anchor);
        edits.push(Edit { start: os, end: oe, original: anchor, replacement: sub(&cc, cs, ce), title, message });
    }
    edits
}

/// Alerts for one sentence whose corrected form passed `validate`.
pub fn alerts_for(map: &TextMap, sentence: &Sentence, corrected: &str) -> Vec<Alert> {
    hunks_to_edits(&sentence.text, corrected)
        .into_iter()
        .map(|e| {
            let (start, end) = (sentence.start + e.start, sentence.start + e.end);
            let hash = stable_hash(KIND, &format!("{}|{}", sentence.text.trim(), e.original), Some(&e.replacement));
            Alert {
                id: format!("gram-{hash:x}-{start}"),
                category: Category::Correctness,
                kind: KIND.into(),
                title: e.title,
                message: e.message,
                start,
                end,
                u16_start: map.char_to_utf16(start),
                u16_end: map.char_to_utf16(end),
                original: e.original,
                suggestions: vec![e.replacement],
                priority: 150,
                stable_hash: hash,
            }
        })
        .collect()
}

/// The deterministic alert wins: drop an AiGrammar alert when a rule/Harper
/// correctness alert overlaps its span.
pub fn dedupe_against(ai: Vec<Alert>, deterministic: &[Alert]) -> Vec<Alert> {
    ai.into_iter()
        .filter(|a| !deterministic.iter().any(|d| d.kind != KIND && d.category == Category::Correctness && !d.kind.to_lowercase().contains("readab") && d.u16_start < a.u16_end && a.u16_start < d.u16_end))
        .collect()
}

// ---------------------------------------------------------------- prompt + parsing

pub fn user_message(sents: &[&Sentence]) -> String {
    let mut m: String = sents.iter().enumerate().map(|(n, s)| format!("{}. {}\n", n + 1, s.text)).collect();
    let hints: Vec<String> = sents.iter().enumerate().flat_map(|(n, s)| hints_for(&s.text).into_iter().map(move |h| format!("{}: {}\n", n + 1, h))).collect();
    if !hints.is_empty() {
        m.push_str("\nHINTS (patterns that are often wrong):\n");
        m.extend(hints);
    }
    m
}

/// Cheap pattern hints for the cases a small model overlooks. They only point
/// the model at a spot; the answer still goes through every guard.
pub fn hints_for(s: &str) -> Vec<&'static str> {
    static RES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        let mk = |p: &str, h: &'static str| (Regex::new(p).unwrap(), h);
        vec![
            mk(r"(?i)\bwho\s+(i|we|you|they|he|she)\b", "\"who\" is the object here; it should be \"whom\"."),
            mk(r"(?i)\b(to|for|with|from|by|about|of)\s+who\b", "\"who\" after a preposition should be \"whom\"."),
            mk(r"^(?i:me|him|her|us|them)\s+and\b", "Wrong pronoun case at the start (use the subject form, e.g. \"He and I\")."),
            mk(r"^Us\s+\w+", "\"Us\" before a noun used as the subject should be \"We\"."),
            mk(r"(?i)\b(between|for|to|with|from|by|of|about)\s+(?:\w+\s+){0,2}and\s+I\b", "After a preposition use \"me\", not \"I\"."),
            mk(r"(?i)\b(by|to|with|for|between)\s+\w+\s+and\s+(she|he|they|we)\b", "After a preposition use the object form (her, him, them, us)."),
            mk(r"(?i)\b(each|everyone|everybody|neither|either|someone|somebody|nobody|anyone|no one)\b[^.,;]{0,30}\b(have|are|were|do)\b", "These words take a singular verb."),
            mk(r"\b[A-Z]\w+\s+and\s+[A-Z]\w+\s+(was|is|has|does)\b", "Two subjects joined by \"and\" take a plural verb."),
        ]
    });
    res.iter().filter(|(re, _)| re.is_match(s)).map(|(_, h)| *h).collect()
}

/// Numbered answer lines -> {number: text}. Junk lines, unnumbered lines,
/// numbers out of range and repeated numbers are ignored. With a single
/// sentence an unnumbered one-line answer is accepted.
pub fn parse_answer(answer: &str, n: usize) -> HashMap<usize, String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\W{0,3}(\d{1,3})\s*[.):\]-]\s*(.*)$").unwrap());
    let mut out = HashMap::new();
    for line in answer.lines() {
        let line = line.trim();
        if let Some(c) = re.captures(line) {
            let k: usize = c[1].parse().unwrap_or(0);
            let text = c[2].trim().to_string();
            if k >= 1 && k <= n && !text.is_empty() {
                out.entry(k).or_insert(text);
            }
        }
    }
    if out.is_empty() && n == 1 {
        let lines: Vec<&str> = answer.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if lines.len() == 1 {
            out.insert(1, lines[0].to_string());
        }
    }
    out
}

pub fn cache_key(model: &str, text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    VERSION.hash(&mut h);
    SYSTEM.len().hash(&mut h);
    model.hash(&mut h);
    text.trim().hash(&mut h);
    h.finish()
}

fn eligible(s: &Sentence) -> bool {
    s.judge && s.text.split_whitespace().count() <= MAX_WORDS && !s.text.contains('`') && !s.text.contains('{') && !s.text.contains('=')
}

// ---------------------------------------------------------------- run

/// Grammar-pass `text`. Sentences already in `cache` are not sent to the model.
pub async fn run(cfg: &LlmConfig, text: &str, cache: &mut Cache) -> Result<Output> {
    let sents = split_sentences(text);
    let map = TextMap::new(text);
    let mut out = Output { sentences: sents.iter().filter(|s| eligible(s)).count(), ..Default::default() };
    let mut corrected: HashMap<usize, String> = HashMap::new();
    let mut pending: Vec<usize> = Vec::new();
    for (k, s) in sents.iter().enumerate() {
        if !eligible(s) {
            continue;
        }
        match cache.get(&cache_key(&cfg.model, &s.text)) {
            Some(Some(c)) => {
                corrected.insert(k, c.clone());
                out.from_cache += 1;
            }
            Some(None) => out.from_cache += 1,
            None => pending.push(k),
        }
    }
    pending.truncate(MAX_PER_RUN);
    // batches
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let (mut cur, mut chars) = (Vec::<usize>::new(), 0usize);
    for &k in &pending {
        let len = sents[k].text.chars().count();
        if !cur.is_empty() && (cur.len() >= MAX_BATCH || chars + len > MAX_BATCH_CHARS) {
            batches.push(std::mem::take(&mut cur));
            chars = 0;
        }
        cur.push(k);
        chars += len;
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    let mut failed: Option<anyhow::Error> = None;
    for batch in batches {
        let refs: Vec<&Sentence> = batch.iter().map(|&k| &sents[k]).collect();
        let reply = match llm::complete(cfg, SYSTEM, &[llm::msg("user", user_message(&refs))], false, 0.0).await {
            Ok(r) => r,
            Err(e) => {
                failed = Some(e);
                break;
            }
        };
        out.batches += 1;
        let answers = parse_answer(&reply, batch.len());
        for (n, &k) in batch.iter().enumerate() {
            let key = cache_key(&cfg.model, &sents[k].text);
            let Some(ans) = answers.get(&(n + 1)) else {
                // no usable line for this sentence: ask again next time
                continue;
            };
            out.checked += 1;
            match validate(&sents[k].text, ans) {
                Ok(Some(c)) => {
                    cache.insert(key, Some(c.clone()));
                    corrected.insert(k, c);
                }
                Ok(None) => {
                    cache.insert(key, None);
                }
                Err(r) => {
                    out.rejected += 1;
                    let note = format!("{r:?}: {} => {}", sents[k].text.chars().take(60).collect::<String>(), ans.chars().take(80).collect::<String>());
                    log::debug!("grammar pass rejected {note}");
                    out.rejections.push(note);
                    cache.insert(key, None);
                }
            }
        }
    }
    if let Some(e) = failed {
        if out.batches == 0 && corrected.is_empty() {
            return Err(e);
        }
        log::warn!("grammar pass stopped early: {e}");
    }
    if cache.len() > CACHE_MAX {
        cache.clear();
    }
    let mut keys: Vec<usize> = corrected.keys().copied().collect();
    keys.sort_unstable();
    for k in keys {
        out.alerts.extend(alerts_for(&map, &sents[k], &corrected[&k]));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edits(orig: &str, fixed: &str) -> Vec<(String, String, String)> {
        hunks_to_edits(orig, fixed).into_iter().map(|e| (e.original, e.replacement, e.title)).collect()
    }
    fn ok(orig: &str, fixed: &str) -> Option<String> {
        validate(orig, fixed).unwrap_or_else(|r| panic!("{r:?}"))
    }
    fn rej(orig: &str, fixed: &str) -> Reject {
        validate(orig, fixed).unwrap_err()
    }

    #[test]
    fn an_insertion_expands_to_the_neighbouring_word() {
        let e = edits("However we left early.", "However, we left early.");
        assert_eq!(e, vec![("However".into(), "However,".into(), "Add a comma".into())]);
    }

    #[test]
    fn a_comma_splice_fixed_with_a_semicolon_is_titled_correctly() {
        let e = edits("The server crashed at noon, nobody noticed until evening.", "The server crashed at noon; nobody noticed until evening.");
        assert_eq!(e, vec![(",".into(), ";".into(), "Fix the comma splice".into())]);
    }

    #[test]
    fn a_comma_splice_fixed_with_a_period_and_capital() {
        let e = edits("Our lease ends in March, we need a new office.", "Our lease ends in March. We need a new office.");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, ", we");
        assert_eq!(e[0].1, ". We");
        assert_eq!(e[0].2, "Fix the comma splice");
    }

    #[test]
    fn a_comma_splice_fixed_with_a_conjunction() {
        let e = edits("It rained all day, we stayed inside.", "It rained all day, so we stayed inside.");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].2, "Fix the comma splice");
        assert_eq!(e[0].1, "day, so");
    }

    #[test]
    fn multiple_hunks_become_separate_edits() {
        let e = edits("After the meeting ended we walked home and she go to bed.", "After the meeting ended, we walked home and she goes to bed.");
        let titles: Vec<&str> = e.iter().map(|x| x.2.as_str()).collect();
        assert_eq!(titles.len(), 2, "{e:?}");
        assert_eq!(titles[0], "Add a comma");
        assert_eq!(e[0].0, "ended");
        assert_eq!(e[1].2, "Fix the verb");
    }

    #[test]
    fn removing_a_comma_and_pronoun_titles() {
        assert_eq!(edits("She said, that it was fine.", "She said that it was fine.")[0].2, "Remove the comma");
        assert_eq!(edits("The woman who we met was kind.", "The woman whom we met was kind.")[0].2, "Use “whom”");
        assert_eq!(edits("My brother and me bought a car.", "My brother and I bought a car.")[0].2, "Use “I”");
        assert_eq!(edits("Between you and I there is a secret.", "Between you and me there is a secret.")[0].2, "Use “me”");
        assert_eq!(edits("The dogs in the yard barks loudly.", "The dogs in the yard bark loudly.")[0].2, "Fix the verb");
        assert_eq!(edits("it was a long day.", "It was a long day.")[0].2, "Fix the capitalization");
    }

    #[test]
    fn moved_words_stay_in_one_edit() {
        let e = edits("Me and John went to the store.", "John and I went to the store.");
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].0, "Me and John");
    }

    #[test]
    fn valid_fixes_pass_the_guards() {
        assert!(ok("The server crashed at noon, nobody noticed until evening.", "The server crashed at noon; nobody noticed until evening.").is_some());
        assert!(ok("My brother and me bought a new car.", "My brother and I bought a new car.").is_some());
        assert!(ok("Neither of the options are good enough.", "Neither of the options is good enough.").is_some());
        assert!(ok("She go to school every day.", "She goes to school every day.").is_some());
        assert!(ok("Yesterday we walk to the park together.", "Yesterday we walked to the park together.").is_some());
        assert!(ok("Nothing to fix in this sentence.", "Nothing to fix in this sentence.").is_none());
    }

    #[test]
    fn a_synonym_swap_is_rejected() {
        assert!(matches!(rej("The big dog ran across the yard.", "The large dog ran across the yard."), Reject::Synonym(_)));
        assert!(matches!(rej("We bought a car, it was very cheap.", "We purchased a car; it was very cheap."), Reject::Synonym(_)));
    }

    #[test]
    fn an_added_content_word_is_rejected() {
        assert!(matches!(rej("The team finished the project on time.", "The team finished the project quickly on time."), Reject::WordAdded(_)));
        assert!(matches!(rej("The team finished the big project.", "The team finished the project."), Reject::WordRemoved(_)));
    }

    #[test]
    fn a_changed_number_name_or_quote_is_rejected() {
        assert!(matches!(rej("We shipped 12 boxes to Chicago, they arrived Monday.", "We shipped 13 boxes to Chicago; they arrived Monday."), Reject::NumberChanged(_)));
        assert!(matches!(rej("We shipped boxes to Chicago, they arrived Monday.", "We shipped boxes to Boston; they arrived Monday."), Reject::NameChanged(_)));
        assert!(matches!(rej("She said \"we are done here\" and left early.", "She said \"we were done here\" and left early."), Reject::QuoteChanged(_)));
        assert!(matches!(rej("We ordered four chairs, they came late today.", "We ordered five chairs; they came late today."), Reject::NumberChanged(_)));
    }

    #[test]
    fn a_too_large_rewrite_is_rejected() {
        let o = "The committee reviewed the proposal and found several problems with the budget.";
        assert!(matches!(rej(o, "The committee reviewed the proposal, finding problems with budget numbers."), Reject::TooManyChars(_) | Reject::WordAdded(_) | Reject::WordRemoved(_) | Reject::Synonym(_) | Reject::HunkTooBig(_) | Reject::Structure(_)));
        assert!(matches!(rej("We left at noon and arrived at dusk today.", "We left at noon and arrived at dusk today and it was great fun for all."), Reject::TooLong));
        assert!(matches!(rej("It is a good plan for us all.", "Good plan."), Reject::TooManyChars(_) | Reject::WordRemoved(_)));
    }

    #[test]
    fn other_punctuation_changes_are_rejected() {
        assert!(matches!(rej("The plan was simple we left early today.", "The plan was simple: we left early today."), Reject::Punctuation(_)));
    }

    #[test]
    fn answers_are_cleaned_before_the_diff() {
        // trailing period added by the model for a fragment without one
        assert_eq!(ok("However we left early", "However, we left early."), Some("However, we left early".into()));
        // wrapped in quotes
        assert_eq!(ok("However we left early.", "\"However, we left early.\""), Some("However, we left early.".into()));
        // missing final period restored
        assert_eq!(ok("However we left early.", "However, we left early"), Some("However, we left early.".into()));
    }

    #[test]
    fn parses_numbered_answers_with_junk_and_gaps() {
        let ans = "Here are the corrections:\n1. First one.\n\n3) Third one.\nnot a line\n3. duplicate ignored\n9. out of range\n4: Fourth.";
        let m = parse_answer(ans, 4);
        assert_eq!(m.get(&1).map(String::as_str), Some("First one."));
        assert_eq!(m.get(&2), None);
        assert_eq!(m.get(&3).map(String::as_str), Some("Third one."));
        assert_eq!(m.get(&4).map(String::as_str), Some("Fourth."));
        assert_eq!(m.len(), 3);
        // a single sentence may come back without its number
        assert_eq!(parse_answer("Only this line.", 1).get(&1).map(String::as_str), Some("Only this line."));
        assert!(parse_answer("", 3).is_empty());
    }

    #[test]
    fn alerts_are_anchored_in_the_document() {
        let text = "Intro line here is fine.\nHowever we left early today.";
        let sents = split_sentences(text);
        let map = TextMap::new(text);
        let k = sents.iter().position(|s| s.text.starts_with("However")).unwrap();
        let alerts = alerts_for(&map, &sents[k], "However, we left early today.");
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.kind, "AiGrammar");
        assert_eq!(a.category, Category::Correctness);
        assert_eq!(a.original, "However");
        assert_eq!(a.suggestions[0], "However,");
        assert_eq!(crate::textmap::slice_utf16(text, a.u16_start, a.u16_end), "However");
    }

    #[test]
    fn a_deterministic_alert_on_the_same_span_wins() {
        let text = "However we left early today.";
        let sents = split_sentences(text);
        let map = TextMap::new(text);
        let ai = alerts_for(&map, &sents[0], "However, we left early today.");
        let mut det = ai[0].clone();
        det.kind = "Punctuation".into();
        assert!(dedupe_against(ai.clone(), &[det.clone()]).is_empty());
        det.u16_start = 20;
        det.u16_end = 25;
        assert_eq!(dedupe_against(ai, &[det]).len(), 1);
    }

    #[test]
    fn a_semicolon_between_run_on_sentences_becomes_a_period() {
        assert_eq!(ok("She left early she had a doctor's appointment at three.", "She left early; she had a doctor's appointment at three."), Some("She left early. She had a doctor's appointment at three.".into()));
        // a real comma splice keeps its semicolon
        assert_eq!(ok("The server crashed at noon, nobody noticed until evening.", "The server crashed at noon; nobody noticed until evening."), Some("The server crashed at noon; nobody noticed until evening.".into()));
    }

    #[test]
    fn structure_guards_reject_invented_breaks_and_commas() {
        assert!(matches!(rej("She bought an umbrella before the storm.", "She bought an umbrella; before the storm."), Reject::Structure(_)));
        assert!(matches!(rej("The museum opens at nine, and the line forms early.", "The museum opens at nine; and the line forms early."), Reject::Structure(_)));
        assert!(matches!(rej("Yesterday I went to the dentist for a checkup.", "Yesterday, I went to the dentist for a checkup."), Reject::Structure(_)));
        assert!(matches!(rej("The author who I met signed my copy.", "The author, who I met, signed my copy."), Reject::Structure(_)));
        assert!(ok("We tested the update on Monday and it passed every check.", "We tested the update on Monday, and it passed every check.").is_some());
        assert!(ok("Because the shipment was late we had to reschedule.", "Because the shipment was late, we had to reschedule.").is_some());
    }

    #[test]
    fn removing_a_conjunction_or_expanding_a_contraction_is_rejected() {
        assert!(matches!(rej("First we mix the dry ingredients, and then we add the eggs.", "First we mix the dry ingredients; then we add the eggs."), Reject::WordRemoved(_) | Reject::Structure(_)));
        assert!(matches!(rej("I love this app because it's simple and fast.", "I love this app because it is simple and fast."), Reject::Contraction(_)));
        assert!(ok("She don't know the answer yet.", "She doesn't know the answer yet.").is_some());
    }

    #[test]
    fn a_pronoun_may_change_case_but_not_person() {
        assert!(matches!(rej("Me and Priya finished the slides last night.", "She and Priya finished the slides last night."), Reject::Synonym(_)));
        assert!(ok("Me and him went to the conference together.", "He and I went to the conference together.").is_some());
    }
}
