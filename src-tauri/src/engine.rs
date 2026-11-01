//! The correctness engine: Harper lints mapped onto Grammarly's four outcome
//! categories, plus small rule-based Delivery/Engagement passes.

use crate::textmap::TextMap;
use harper_core::linting::{LintGroup, LintKind, Linter, Suggestion};
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Correctness,
    Clarity,
    Engagement,
    Delivery,
}

impl Category {
    pub fn color(&self) -> &'static str {
        match self {
            Category::Correctness => "#ff7a74",
            Category::Clarity => "#7d99f0",
            Category::Engagement => "#2cc9b6",
            Category::Delivery => "#a598f0",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub category: Category,
    pub kind: String,
    pub title: String,
    pub message: String,
    /// char offsets
    pub start: usize,
    pub end: usize,
    /// UTF-16 offsets (what AX wants)
    pub u16_start: usize,
    pub u16_end: usize,
    pub original: String,
    pub suggestions: Vec<String>,
    pub priority: u8,
    /// Stable across edits elsewhere in the document: hash(kind, original, first suggestion).
    pub stable_hash: u64,
}

pub struct Engine {
    group: LintGroup,
}

impl Engine {
    pub fn new() -> Self {
        let group = LintGroup::new_curated(FstDictionary::curated(), Dialect::American);
        Engine { group }
    }

    pub fn check(&mut self, text: &str) -> Vec<Alert> {
        let map = TextMap::new(text);
        let doc = Document::new_plain_english_curated(text);
        let source = doc.get_source();
        let lints = self.group.lint(&doc);
        let mut out: Vec<Alert> = Vec::with_capacity(lints.len());
        for lint in lints {
            let start = lint.span.start.min(source.len());
            let end = lint.span.end.min(source.len()).max(start);
            if end == start {
                continue;
            }
            let original: String = source[start..end].iter().collect();
            let mut suggestions: Vec<String> = Vec::new();
            for s in &lint.suggestions {
                match s {
                    Suggestion::ReplaceWith(chars) => suggestions.push(chars.iter().collect()),
                    Suggestion::Remove => suggestions.push(String::new()),
                    Suggestion::InsertAfter(chars) => {
                        let mut v = original.clone();
                        v.extend(chars.iter());
                        suggestions.push(v)
                    }
                    #[allow(unreachable_patterns)]
                    _ => {}
                }
            }
            suggestions.dedup();
            let kind = lint.lint_kind;
            // Harper ranks by raw edit distance ("comming" -> "cumming"); put the likely word first.
            // Typo covers split-word guesses ("adress" -> "a dress"); multi-word originals pass through.
            if matches!(kind, LintKind::Spelling | LintKind::Typo) {
                suggestions = crate::spellrank::rerank(&original, &suggestions);
            }
            // Harper only measures the long sentence. Give the note a lossless
            // split at its middle joint so Accept is there at once; the spot
            // check's model split (when it arrives) hides this note.
            if matches!(kind, LintKind::Readability) && suggestions.is_empty() {
                if let Some(s) = crate::features::spotcheck::plain_split(&original) {
                    suggestions.push(s);
                }
            }
            let category = category_for(kind);
            let title = title_for(kind, &original, suggestions.first().map(|s| s.as_str()), &lint.message);
            let message = if matches!(kind, LintKind::Readability) {
                if suggestions.is_empty() {
                    format!("{} Split it into two or three sentences, one point each.", lint.message.trim())
                } else {
                    format!("{} The split keeps every word. Accept it, or break the sentence where you prefer.", lint.message.trim())
                }
            } else {
                lint.message.clone()
            };
            let stable_hash = stable_hash(&kind.to_string(), &original, suggestions.first().map(|s| s.as_str()));
            let id = format!("{:x}-{}", stable_hash, start);
            out.push(Alert {
                id,
                category,
                kind: kind.to_string(),
                title,
                message,
                start,
                end,
                u16_start: map.char_to_utf16(start),
                u16_end: map.char_to_utf16(end),
                original,
                suggestions,
                priority: lint.priority,
                stable_hash,
            });
        }
        out.extend(delivery_rules(text, &map));
        out.extend(very_rules(text, &map));
        out.extend(engagement_rules(text, &map));
        // A misspelled word gets one card: drop other lints on exactly its span ("untill" -> "distill").
        let spelled: std::collections::HashSet<(usize, usize)> = out.iter().filter(|a| a.kind == "Spelling" || a.kind == "Typo").map(|a| (a.start, a.end)).collect();
        out.retain(|a| a.kind == "Spelling" || a.kind == "Typo" || !spelled.contains(&(a.start, a.end)));
        // Remove overlapping duplicates (keep the first, higher-priority entry).
        out.sort_by(|a, b| a.start.cmp(&b.start).then(a.priority.cmp(&b.priority)));
        let mut kept: Vec<Alert> = Vec::with_capacity(out.len());
        for a in out {
            if let Some(last) = kept.last() {
                if a.start < last.end && a.category == last.category {
                    continue;
                }
            }
            kept.push(a);
        }
        kept
    }
}

pub fn stable_hash(kind: &str, original: &str, suggestion: Option<&str>) -> u64 {
    let mut h = DefaultHasher::new();
    kind.hash(&mut h);
    original.to_lowercase().hash(&mut h);
    suggestion.unwrap_or("").hash(&mut h);
    h.finish()
}

pub fn category_for(kind: LintKind) -> Category {
    match kind {
        LintKind::Spelling
        | LintKind::Typo
        | LintKind::Capitalization
        | LintKind::Punctuation
        | LintKind::Grammar
        | LintKind::Agreement
        | LintKind::BoundaryError
        | LintKind::Malapropism
        | LintKind::Eggcorn
        | LintKind::Usage
        | LintKind::Nonstandard
        | LintKind::WordOrder
        | LintKind::Regionalism
        | LintKind::Formatting => Category::Correctness,
        LintKind::Readability | LintKind::Redundancy | LintKind::Repetition | LintKind::WordChoice | LintKind::Style | LintKind::Miscellaneous => {
            Category::Clarity
        }
        LintKind::Enhancement => Category::Engagement,
        #[allow(unreachable_patterns)]
        _ => Category::Clarity,
    }
}

pub fn title_for(kind: LintKind, original: &str, suggestion: Option<&str>, message: &str) -> String {
    let sugg = suggestion.unwrap_or("");
    match kind {
        LintKind::Spelling | LintKind::Typo => "Correct your spelling".into(),
        LintKind::Capitalization => {
            if original.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) && sugg.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                "Capitalize the word".into()
            } else {
                "Fix capitalization".into()
            }
        }
        LintKind::Punctuation => {
            let o_commas = original.matches(',').count();
            let s_commas = sugg.matches(',').count();
            if s_commas > o_commas {
                "Add a comma".into()
            } else if s_commas < o_commas {
                "Remove the comma".into()
            } else if sugg.ends_with('.') && !original.ends_with('.') {
                "Add a period".into()
            } else if sugg.contains('\'') && !original.contains('\'') {
                "Add an apostrophe".into()
            } else {
                "Punctuation problem".into()
            }
        }
        LintKind::Grammar => "Correct the grammar".into(),
        LintKind::Agreement => "Fix subject-verb agreement".into(),
        LintKind::BoundaryError => "Fix the word boundary".into(),
        LintKind::Malapropism | LintKind::Eggcorn => "Use the right word".into(),
        LintKind::Usage | LintKind::Nonstandard => "Fix nonstandard usage".into(),
        LintKind::WordOrder => "Fix the word order".into(),
        LintKind::Regionalism => "Use the American spelling".into(),
        LintKind::Formatting => "Fix formatting".into(),
        LintKind::Readability => "Split the long sentence".into(),
        LintKind::Redundancy => "Remove redundancy".into(),
        LintKind::Repetition => "Avoid repetition".into(),
        LintKind::WordChoice => "Change the wording".into(),
        LintKind::Style => "Rephrase for clarity".into(),
        LintKind::Enhancement => "Make it more engaging".into(),
        LintKind::Miscellaneous => {
            let m = message.trim_end_matches('.');
            if m.len() < 40 {
                m.to_string()
            } else {
                "Review this".into()
            }
        }
        #[allow(unreachable_patterns)]
        _ => "Review this".into(),
    }
}

struct Rule {
    re: Regex,
    title: &'static str,
    message: &'static str,
    replacement: Option<&'static str>,
}

fn delivery_rule_set() -> &'static Vec<Rule> {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mk = |pat: &str, title: &'static str, message: &'static str, replacement: Option<&'static str>| Rule {
            re: Regex::new(pat).expect("delivery regex"),
            title,
            message,
            replacement,
        };
        vec![
            mk(r"(?i)\bI (?:just )?think (?:that )?", "Sound more confident", "Hedging words like “I think” can make your statement sound uncertain. Consider stating it directly.", Some("")),
            mk(r"(?i)\bI feel like ", "Sound more confident", "“I feel like” softens your point. Readers trust a direct claim more.", Some("")),
            mk(r"(?i)\b(?:kind of|sort of) ", "Remove the hedge", "“Kind of” and “sort of” weaken your delivery.", Some("")),
            mk(r"(?i)\bjust ", "Remove the filler", "“Just” often adds nothing. Removing it makes the sentence more assertive.", Some("")),
            mk(r"(?i)\bactually ", "Remove the filler", "“Actually” can sound dismissive or unsure.", Some("")),
            mk(r"(?i)\bbasically ", "Remove the filler", "“Basically” is a filler word that weakens delivery.", Some("")),
            mk(r"(?i)\breally ", "Choose a stronger word", "“Really” is a weak intensifier.", Some("")),
            mk(r"!{2,}", "Tone down the punctuation", "Multiple exclamation marks can read as unprofessional.", Some("!")),
            mk(r"(?i)\bsorry to bother you\b", "Sound more confident", "Apologizing before asking undermines your request.", None),
        ]
    })
}

fn delivery_rules(text: &str, map: &TextMap) -> Vec<Alert> {
    let mut out = Vec::new();
    let chars_prefix = char_prefix(text);
    for rule in delivery_rule_set() {
        for m in rule.re.find_iter(text) {
            let (bs, be) = (m.start(), m.end());
            let start = chars_prefix[bs];
            let end = chars_prefix[be];
            let original = m.as_str().to_string();
            let suggestions: Vec<String> = rule.replacement.map(|r| vec![r.to_string()]).unwrap_or_default();
            let stable = stable_hash("Delivery", original.trim(), suggestions.first().map(|s| s.as_str()));
            out.push(Alert {
                id: format!("{:x}-{}", stable, start),
                category: Category::Delivery,
                kind: "Delivery".into(),
                title: rule.title.into(),
                message: rule.message.into(),
                start,
                end,
                u16_start: map.char_to_utf16(start),
                u16_end: map.char_to_utf16(end),
                original,
                suggestions,
                priority: 200,
                stable_hash: stable,
            });
        }
    }
    out
}

/// "very <word>": the whole phrase is flagged, with stronger single words when the table has
/// them; otherwise the card offers AI options ("Suggest options") for the same phrase.
fn very_rules(text: &str, map: &TextMap) -> Vec<Alert> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\bvery ([a-z][a-z'-]*)").unwrap());
    let chars_prefix = char_prefix(text);
    let mut out = Vec::new();
    for c in re.captures_iter(text) {
        let m = c.get(0).unwrap();
        let word = &c[1];
        // Keep the capital of a sentence-initial "Very".
        let capital = m.as_str().starts_with('V');
        let suggestions: Vec<String> = crate::wordopts::stronger_than_very(word)
            .map(|opts| {
                opts.iter()
                    .map(|o| {
                        let mut s = o.to_string();
                        if capital {
                            s = s[..1].to_uppercase() + &s[1..];
                        }
                        s
                    })
                    .collect()
            })
            .unwrap_or_default();
        let message = if suggestions.is_empty() {
            format!("“Very {word}” leans on an intensifier. A single precise word has more impact.")
        } else {
            format!("“Very {word}” leans on an intensifier. One stronger word says it better.")
        };
        let stable = stable_hash("Delivery", &m.as_str().to_lowercase(), suggestions.first().map(|s| s.as_str()));
        let start = chars_prefix[m.start()];
        let end = chars_prefix[m.end()];
        out.push(Alert {
            id: format!("{:x}-{}", stable, start),
            category: Category::Delivery,
            kind: "Delivery".into(),
            title: "Choose a stronger word".into(),
            message,
            start,
            end,
            u16_start: map.char_to_utf16(start),
            u16_end: map.char_to_utf16(end),
            original: m.as_str().to_string(),
            suggestions,
            priority: 200,
            stable_hash: stable,
        });
    }
    out
}

fn engagement_rules(text: &str, map: &TextMap) -> Vec<Alert> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\b(good|bad|nice|big|small|thing|things|stuff|a lot|got|get)\b").unwrap());
    let chars_prefix = char_prefix(text);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in re.find_iter(text) {
        let word = m.as_str().to_lowercase();
        if !seen.insert(word.clone()) {
            continue; // one engagement note per overused word
        }
        let count = re.find_iter(text).filter(|x| x.as_str().eq_ignore_ascii_case(&word)).count();
        if count < 2 {
            continue;
        }
        let start = chars_prefix[m.start()];
        let end = chars_prefix[m.end()];
        let stable = stable_hash("Engagement", &word, None);
        out.push(Alert {
            id: format!("{:x}-{}", stable, start),
            category: Category::Engagement,
            kind: "Engagement".into(),
            title: "Choose a more vivid word".into(),
            message: format!("“{}” appears {} times. A more specific word keeps readers engaged.", word, count),
            start,
            end,
            u16_start: map.char_to_utf16(start),
            u16_end: map.char_to_utf16(end),
            original: m.as_str().to_string(),
            suggestions: vec![],
            priority: 220,
            stable_hash: stable,
        });
    }
    out
}

/// byte offset → char index table (len = bytes + 1)
fn char_prefix(text: &str) -> Vec<usize> {
    let mut v = vec![0usize; text.len() + 1];
    let mut ci = 0;
    let mut last = 0;
    for (bi, _) in text.char_indices() {
        for b in last..=bi {
            v[b] = ci;
        }
        last = bi + 1;
        ci += 1;
    }
    for b in last..=text.len() {
        v[b] = ci;
    }
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_long_sentence_note_carries_an_instant_lossless_split() {
        let text = "The committee reviewed the three proposals on Tuesday and asked each team for a revised budget by Friday, and it plans to announce the winner at the all-hands meeting on the first of March, which gives everyone time to prepare their slides and book travel.";
        let alerts = super::Engine::new().check(text);
        let a = alerts.iter().find(|a| a.kind == "Readability").expect("Harper flags a 45-word sentence");
        assert_eq!(a.title, "Split the long sentence");
        assert!(a.message.contains("The split keeps every word."), "{}", a.message);
        assert_eq!(a.category, super::Category::Clarity);
        let s = a.suggestions.first().expect("Accept is available before any model runs");
        assert!(s.contains("by Friday. It plans"), "{s}");
        assert_eq!(crate::features::spotcheck::content_loss(&a.original, s).0, 0, "nothing removed");
        // A long sentence with no joint keeps the plain note.
        let flat = "The committee reviewed the three proposals on Tuesday then asked each team for a revised budget by Friday then planned to announce the winner at the all-hands meeting on the first of March which gives everyone time to prepare slides and book travel.";
        let b = super::Engine::new().check(flat);
        let b = b.iter().find(|a| a.kind == "Readability").expect("still flagged");
        assert!(b.suggestions.is_empty());
        assert!(b.message.ends_with("Split it into two or three sentences, one point each."), "{}", b.message);
    }

    use super::*;

    #[test]
    fn harper_finds_a_misspelling_with_a_suggestion() {
        let mut e = Engine::new();
        let alerts = e.check("This is an test of teh engine.");
        let teh = alerts.iter().find(|a| a.original == "teh").expect("teh flagged");
        assert_eq!(teh.category, Category::Correctness);
        assert!(teh.suggestions.iter().any(|s| s == "the"), "{:?}", teh.suggestions);
        assert_eq!(teh.title, "Correct your spelling");
        // char offsets and utf16 offsets agree for ASCII
        assert_eq!((teh.start, teh.end), (teh.u16_start, teh.u16_end));
        assert_eq!(&"This is an test of teh engine."[teh.start..teh.end], "teh");
    }

    #[test]
    fn spelling_puts_the_likely_word_first() {
        let alerts = Engine::new().check("I am comming home with the adress.");
        let first = |w: &str| alerts.iter().find(|a| a.original == w).and_then(|a| a.suggestions.first().cloned());
        assert_eq!(first("comming").as_deref(), Some("coming"));
        assert_eq!(first("adress").as_deref(), Some("address"));
        let all = Engine::new().check("Wait untill noon.");
        let untill: Vec<&Alert> = all.iter().filter(|a| a.original == "untill").collect();
        assert_eq!(untill.len(), 1, "one card per misspelled word");
        assert_eq!(untill[0].suggestions.first().map(String::as_str), Some("until"));
    }

    #[test]
    fn very_offers_stronger_words_for_the_whole_phrase() {
        let alerts = Engine::new().check("Very good work, but I am very tired and very xylophone.");
        let very: Vec<&Alert> = alerts.iter().filter(|a| a.title == "Choose a stronger word").collect();
        let tired = very.iter().find(|a| a.original == "very tired").expect("very tired flagged");
        assert!(tired.suggestions.iter().any(|s| s == "exhausted"), "{:?}", tired.suggestions);
        assert_eq!(&"Very good work, but I am very tired and very xylophone."[tired.start..tired.end], "very tired");
        let good = very.iter().find(|a| a.original == "Very good").expect("Very good flagged");
        assert!(good.suggestions[0].starts_with(char::is_uppercase), "{:?}", good.suggestions);
        let odd = very.iter().find(|a| a.original == "very xylophone").expect("unknown word still flagged");
        assert!(odd.suggestions.is_empty(), "no table entry: the card offers AI options");
    }

    #[test]
    fn utf16_offsets_follow_emoji() {
        let mut e = Engine::new();
        let text = "😀😀 I has teh cat.";
        let alerts = e.check(text);
        let teh = alerts.iter().find(|a| a.original == "teh").expect("teh flagged");
        // two emoji = 2 chars but 4 utf16 units
        assert_eq!(teh.u16_start, teh.start + 2);
        assert_eq!(crate::textmap::slice_utf16(text, teh.u16_start, teh.u16_end), "teh");
    }

    #[test]
    fn delivery_rules_flag_hedging() {
        let mut e = Engine::new();
        let alerts = e.check("I think that we should just ship it.");
        assert!(alerts.iter().any(|a| a.category == Category::Delivery && a.title == "Sound more confident"));
        assert!(alerts.iter().any(|a| a.category == Category::Delivery && a.original.trim() == "just"));
    }

    #[test]
    fn categories_cover_every_lint_kind() {
        for k in [LintKind::Spelling, LintKind::Readability, LintKind::Enhancement, LintKind::Punctuation] {
            let _ = category_for(k);
        }
        assert_eq!(category_for(LintKind::Spelling), Category::Correctness);
        assert_eq!(category_for(LintKind::Readability), Category::Clarity);
        assert_eq!(category_for(LintKind::Enhancement), Category::Engagement);
    }

    #[test]
    fn clean_text_has_no_correctness_alerts() {
        let mut e = Engine::new();
        let alerts = e.check("The quick brown fox jumps over the lazy dog.");
        assert!(alerts.iter().all(|a| a.category != Category::Correctness), "{:?}", alerts);
    }

    #[test]
    fn real_sample_text_produces_alerts_without_panicking() {
        let text = "I took the biggest risk of my ENTIRE LIFE.\n\nbut to make you understand, i have to take you back, to exactly 42 days ago.\n\nthe last day to withdraw from classes. Up til this point I had been working on publik after going viral over the summer for cloning apps, i wasnt getting me any closer ot my goals.\n\nI pulled out of school, and shut the fuck up. I didnt tell my family, not my friends, and not my peers. I had very little money.\n\nI lived in an apartment with a blanket over the window to block out the light cycle, watching people around me go to class while I sent thousands of emils. It didnt feel great.\n\nThen after thousands of similar emails had gone into the void, a founder replied. We got on a call and by the next day the deal was done. We raised over half a million dollars to kill slop companies.\n\nI think that I will be able to lead yall into a future where software is free and im keeping it accessible. SUBSCRIPTIONS, im coming after each and every one of you.";
        let mut e = Engine::new();
        let alerts = e.check(text);
        let originals: Vec<&str> = alerts.iter().map(|a| a.original.as_str()).collect();
        assert!(alerts.len() >= 8, "expected many alerts, got {:?}", originals);
        assert!(originals.iter().any(|o| *o == "emils" || *o == "didnt" || *o == "wasnt"), "{:?}", originals);
        for a in &alerts {
            assert_eq!(crate::textmap::slice_utf16(text, a.u16_start, a.u16_end), a.original, "offsets must round-trip for {}", a.title);
        }
    }
}
