//! Deterministic punctuation-and-grammar rules. No model, no network: a tokenizer, a few
//! word lists and narrow patterns. Precision comes first: every rule is written so that
//! correct text stays silent, and when a pattern is ambiguous the rule does not fire.
//!
//! `rule_alerts` is called from `engine::finish` next to the delivery rules.

use crate::engine::{char_prefix, stable_hash, Alert, Category};
use crate::textmap::TextMap;
use std::collections::HashSet;
use std::sync::OnceLock;

macro_rules! wordset {
    ($name:ident, $words:expr) => {
        fn $name(w: &str) -> bool {
            static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
            S.get_or_init(|| $words.split_whitespace().collect()).contains(w)
        }
    };
}

wordset!(is_aux, "is are was were am be been being has have had do does did will would can could cannot shall should may might must");
wordset!(is_prep, "of in on at for with by from to into onto about between among against through during over under after before since until within without toward towards upon across around near beside behind beyond above below along");
wordset!(is_det, "the a an my our your his her their its this that these those some any each every another no");
wordset!(is_pron, "i we you he she they it");
wordset!(is_subord, "when if because although though since after before while once unless until whenever whereas wherever");
wordset!(
    is_block_prev,
    "the a an of to and or but nor in on at for with by from into onto about as than that which who whom whose what where when how why if whether because while although though unless until since after before once so is are was were am be been being not no has have had do does did will would can could should may might must this these those my our your his her their its such every each some any all both either neither very more most less least i we you he she they it"
);
wordset!(
    is_compl_verb,
    "said say says think thinks thought know knew knows believe believed believes hope hoped hopes feel felt guess assume assumed realize realized realised see saw hear heard tell told wish wished mean means show showed shows found find learn learned notice noticed remember remembered forget forgot expect expected suppose supposed imagine decide decided doubt discovered discover understand understood claim claimed argue argued suggest suggested promise promised sure"
);
wordset!(
    is_present_verb,
    "need want like love hate think know see feel believe hope try work make take go come get give find use look seem mean wish miss prefer expect understand enjoy appreciate say tell ask help keep let put run read write call start stop turn play move live show hear leave meet buy pay send build sell eat drink watch wait stay sit stand lose win open close add check fix join visit order change create finish begin tend remember forget agree suggest recommend require allow offer provide support trust fear care mind matter happen appear become remain continue plan decide choose cost fit hold learn bring carry cut draw drive ride speak spend teach reach pass pick press push pull raise serve share shut sing sleep walk wear arrive collect reply attend review note finish complete submit confirm"
);
wordset!(
    is_irregular_past,
    "went came saw took made got gave found thought told left knew ran began felt kept held brought built wrote stood heard met sat spoke lost paid sent spent won taught bought caught fell grew drew drove flew rose chose broke ate drank sang swam wore threw understood became led said did was were had slept sold shook woke hid bit blew tore swore stole froze forgot rode rang sank"
);
wordset!(not_ed, "need feed seed speed hundred indeed bed red weed breed exceed proceed succeed bleed sled shed wed fled bred");
wordset!(
    is_sing_s,
    "news series species means physics mathematics politics economics analysis crisis basis status bus class business process access address success glass focus virus campus bonus canvas census chorus consensus genius lens atlas gas always perhaps sometimes besides towards thanks yes this his its was has does goes plus lots"
);
wordset!(
    is_adv_word,
    "still also just never always usually often really now soon already even only not probably quickly slowly finally currently recently simply sometimes then"
);
wordset!(is_abbrev, "mr mrs ms dr prof sr jr st vs etc inc ltd co corp fig eg ie al approx dept est cf");

fn is_ly(w: &str) -> bool {
    w.len() > 4 && w.ends_with("ly") && !matches!(w, "family" | "apply" | "reply" | "supply" | "rally" | "assembly" | "july" | "italy" | "early" | "lonely" | "friendly")
}
fn ed_verb(w: &str) -> bool {
    w.len() > 3 && w.ends_with("ed") && !not_ed(w)
}
fn verb_strict(w: &str) -> bool {
    is_aux(w) || is_irregular_past(w) || ed_verb(w) || w.ends_with("n't")
}
fn s_form(w: &str) -> bool {
    w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") && !is_sing_s(w) && !w.contains('\'')
}
fn verb_any(w: &str) -> bool {
    verb_strict(w) || is_present_verb(w) || s_form(w)
}
/// "i'm", "we're", "it's" ...: a pronoun and its verb in one token.
fn subj_contraction(w: &str) -> bool {
    match w.split_once('\'') {
        Some((p, rest)) => is_pron(p) && matches!(rest, "m" | "re" | "ve" | "ll" | "s" | "d"),
        None => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    W,
    N,
    P,
    O,
}

#[derive(Clone, Copy)]
struct Tok {
    s: usize,
    e: usize,
    k: K,
}

struct Doc<'a> {
    t: &'a str,
    tk: Vec<Tok>,
    lw: Vec<String>,
    ss: Vec<bool>,
    first: Vec<usize>,
    last: Vec<usize>,
}

fn opaque(r: &str) -> bool {
    if r.len() < 4 {
        return false;
    }
    if r.contains("://") || r.starts_with("www.") || r.contains('\\') || r.contains('_') || r.contains('/') {
        return true;
    }
    if r.contains('@') && r.contains('.') {
        return true;
    }
    let core = r.trim_end_matches(|c: char| !c.is_alphanumeric());
    if let Some(dot) = core.rfind('.') {
        let tld = &core[dot + 1..];
        if dot > 0
            && matches!(
                tld,
                "com" | "org" | "net" | "io" | "edu" | "gov" | "co" | "uk" | "us" | "ai" | "app" | "dev" | "html" | "htm" | "pdf" | "txt" | "docx" | "xlsx" | "png" | "jpg" | "js" | "rs" | "py" | "json" | "md" | "exe"
            )
        {
            return true;
        }
    }
    false
}

fn tokenize(t: &str) -> Vec<Tok> {
    let cs: Vec<(usize, char)> = t.char_indices().collect();
    let n = cs.len();
    let be = |idx: usize| if idx < n { cs[idx].0 } else { t.len() };
    let mut out = Vec::with_capacity(n / 4);
    let mut i = 0;
    while i < n {
        let (bs, c) = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_alphanumeric() {
            let mut r = i;
            while r < n && !cs[r].1.is_whitespace() {
                r += 1;
            }
            if r - i >= 4 && opaque(&t[bs..be(r)]) {
                let mut e = r;
                while e > i && matches!(cs[e - 1].1, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '"' | '\'' | '”' | '’' | '>') {
                    e -= 1;
                }
                if e > i {
                    out.push(Tok { s: bs, e: be(e), k: K::O });
                    i = e;
                    continue;
                }
            }
            let first_digit = c.is_ascii_digit();
            let mut j = i;
            while j < n {
                let ch = cs[j].1;
                if ch.is_alphanumeric() {
                    j += 1;
                } else if (ch == '\'' || ch == '’') && j > i && j + 1 < n && cs[j + 1].1.is_alphabetic() {
                    j += 1;
                } else if first_digit && (ch == '.' || ch == ',') && j > i && j + 1 < n && cs[j + 1].1.is_ascii_digit() && cs[j - 1].1.is_ascii_digit() {
                    j += 1;
                } else {
                    break;
                }
            }
            out.push(Tok { s: bs, e: be(j), k: if first_digit { K::N } else { K::W } });
            i = j;
            continue;
        }
        out.push(Tok { s: bs, e: be(i + 1), k: K::P });
        i += 1;
    }
    out
}

impl<'a> Doc<'a> {
    fn new(t: &'a str) -> Doc<'a> {
        let tk = tokenize(t);
        let lw: Vec<String> = tk.iter().map(|k| t[k.s..k.e].to_lowercase().replace('’', "'")).collect();
        let n = tk.len();
        let mut d = Doc { t, tk, lw, ss: vec![false; n], first: vec![0; n], last: vec![0; n] };
        d.sentences();
        d
    }

    fn sentences(&mut self) {
        let n = self.tk.len();
        let mut st = true;
        for i in 0..n {
            if i > 0 && self.gap(i).contains(['\n', '\r']) {
                st = true;
            }
            match self.tk[i].k {
                K::W | K::N | K::O => {
                    self.ss[i] = st;
                    st = false;
                }
                K::P => {
                    let c = self.t[self.tk[i].s..self.tk[i].e].chars().next().unwrap_or(' ');
                    match c {
                        '.' | '!' | '?' => {
                            if c == '.' && self.abbrev_before(i) {
                                // an abbreviation does not end the sentence
                            } else if c == '.' && i > 0 && self.p(i - 1, '.') {
                                st = false; // ellipsis
                            } else {
                                st = true;
                            }
                        }
                        // openers, closers and list bullets keep the current state
                        '"' | '“' | '”' | '‘' | '’' | '\'' | '(' | ')' | '[' | ']' | '*' | '•' | '#' | '>' => {}
                        '-' | '–' | '—' => {}
                        _ => st = false,
                    }
                }
            }
        }
        for i in 0..n {
            self.first[i] = if self.ss[i] {
                i
            } else if i > 0 {
                self.first[i - 1]
            } else {
                0
            };
        }
        for i in (0..n).rev() {
            self.last[i] = if i + 1 >= n {
                i
            } else if self.ss[i + 1] {
                i
            } else {
                self.last[i + 1]
            };
        }
    }

    fn abbrev_before(&self, i: usize) -> bool {
        if i == 0 || !self.tight(i) || self.tk[i - 1].k != K::W {
            return false;
        }
        let w = &self.lw[i - 1];
        if is_abbrev(w) {
            return true;
        }
        if w == "no" {
            return matches!(self.k(i + 1), Some(K::N));
        }
        let raw = self.w(i - 1);
        raw.chars().count() == 1 && raw != "I"
    }

    fn n(&self) -> usize {
        self.tk.len()
    }
    fn w(&self, i: usize) -> &str {
        self.tk.get(i).map(|k| &self.t[k.s..k.e]).unwrap_or("")
    }
    fn l(&self, i: usize) -> &str {
        self.lw.get(i).map(|s| s.as_str()).unwrap_or("")
    }
    fn k(&self, i: usize) -> Option<K> {
        self.tk.get(i).map(|k| k.k)
    }
    fn is_w(&self, i: usize) -> bool {
        matches!(self.k(i), Some(K::W))
    }
    fn is_wn(&self, i: usize) -> bool {
        matches!(self.k(i), Some(K::W) | Some(K::N))
    }
    fn p(&self, i: usize, c: char) -> bool {
        self.tk.get(i).map_or(false, |k| k.k == K::P && self.t[k.s..k.e].starts_with(c))
    }
    fn is_p(&self, i: usize) -> bool {
        matches!(self.k(i), Some(K::P))
    }
    fn tight(&self, i: usize) -> bool {
        i > 0 && i < self.tk.len() && self.tk[i - 1].e == self.tk[i].s
    }
    fn gap(&self, i: usize) -> &str {
        let s = if i == 0 { 0 } else { self.tk[i - 1].e };
        let e = self.tk.get(i).map_or(self.t.len(), |k| k.s);
        &self.t[s.min(e)..e]
    }
    fn nl(&self, i: usize) -> bool {
        self.gap(i).contains(['\n', '\r'])
    }
    fn cap(&self, i: usize) -> bool {
        self.w(i).chars().next().map_or(false, |c| c.is_uppercase())
    }
    fn sent_first(&self, i: usize) -> usize {
        self.first[i]
    }
    fn sent_last(&self, i: usize) -> usize {
        self.last[i]
    }
    /// Is there a comma or semicolon in `a..=b`?
    fn has_comma(&self, a: usize, b: usize) -> bool {
        (a..=b.min(self.n().saturating_sub(1))).any(|k| self.p(k, ',') || self.p(k, ';'))
    }
    /// Only words/numbers (and hyphen-joined words) in `a..b`.
    fn plain_words(&self, a: usize, b: usize) -> bool {
        (a..b).all(|k| match self.tk[k].k {
            K::P => self.p(k, '-') && self.tight(k) && self.tight(k + 1),
            _ => true,
        })
    }
    fn ends_question(&self, i: usize) -> bool {
        let e = self.sent_last(i);
        self.p(e, '?')
    }
}

struct Hit {
    s: usize,
    e: usize,
    sug: Vec<String>,
    kind: &'static str,
    title: String,
    msg: String,
}

fn hit(h: &mut Vec<Hit>, s: usize, e: usize, sug: Vec<String>, kind: &'static str, title: impl Into<String>, msg: impl Into<String>) {
    h.push(Hit { s, e, sug, kind, title: title.into(), msg: msg.into() });
}

fn hit1(h: &mut Vec<Hit>, s: usize, e: usize, sug: String, kind: &'static str, title: impl Into<String>, msg: impl Into<String>) {
    hit(h, s, e, vec![sug], kind, title, msg);
}

/// Put `new` in place of `orig`, keeping an initial capital.
fn like_case(orig: &str, new: &str) -> String {
    if orig.chars().next().map_or(false, |c| c.is_uppercase()) {
        let mut cs = new.chars();
        match cs.next() {
            Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
            None => String::new(),
        }
    } else {
        new.to_string()
    }
}

fn capitalize(s: &str) -> String {
    like_case("A", s)
}

pub fn rule_alerts(text: &str, map: &TextMap) -> Vec<Alert> {
    let d = Doc::new(text);
    let mut h: Vec<Hit> = Vec::new();
    // Word swaps first: on an identical span the earlier hit wins in `to_alerts`.
    r_whom(&d, &mut h);
    r_pronoun_case(&d, &mut h);
    r_confusables(&d, &mut h);
    r_agreement(&d, &mut h);
    r_tense(&d, &mut h);
    r_apostrophes(&d, &mut h);
    r_dashes(&d, &mut h);
    r_intro_words(&d, &mut h);
    r_intro_clause(&d, &mut h);
    r_compound(&d, &mut h);
    r_splice(&d, &mut h);
    r_runon(&d, &mut h);
    r_that(&d, &mut h);
    r_address(&d, &mut h);
    r_compound_adj(&d, &mut h);
    r_end_punct(&d, &mut h);
    r_quotes(&d, &mut h);
    r_space_punct(&d, &mut h);
    to_alerts(text, map, h)
}

fn to_alerts(text: &str, map: &TextMap, mut h: Vec<Hit>) -> Vec<Alert> {
    if h.is_empty() {
        return Vec::new();
    }
    h.sort_by(|a, b| a.s.cmp(&b.s).then(b.e.cmp(&a.e)));
    let mut kept: Vec<Hit> = Vec::with_capacity(h.len());
    for x in h {
        if let Some(l) = kept.last() {
            if x.s < l.e {
                continue;
            }
        }
        kept.push(x);
    }
    let cp = char_prefix(text);
    kept.into_iter()
        .map(|x| {
            let original = text[x.s..x.e].to_string();
            let (start, end) = (cp[x.s], cp[x.e]);
            let stable = stable_hash(x.kind, &original, x.sug.first().map(|s| s.as_str()));
            Alert {
                id: format!("{:x}-{}", stable, start),
                category: Category::Correctness,
                kind: x.kind.into(),
                title: x.title,
                message: x.msg,
                start,
                end,
                u16_start: map.char_to_utf16(start),
                u16_end: map.char_to_utf16(end),
                original,
                suggestions: x.sug,
                priority: 25,
                stable_hash: stable,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Dashes
// ---------------------------------------------------------------------------------------

wordset!(range_ctx, "pages page pp chapters chapter ages age years lines line rows items steps sections section grades grade slides questions paragraphs verses exercises problems figures days weeks from");
wordset!(range_unit, "minutes hours days weeks months years people items pages words times percent miles km kg lbs degrees students employees seconds");

fn dash_ok(d: &Doc, a: usize, b: usize) -> bool {
    let left = d.is_w(a) || d.p(a, ')') || d.p(a, '"') || d.p(a, '”');
    let right = d.is_w(b) || d.p(b, '(') || d.p(b, '"') || d.p(b, '“');
    if !left || !right {
        return false;
    }
    if d.is_w(a) && d.is_w(b) && d.cap(a) && d.cap(b) {
        return false; // "Name - Title", headings
    }
    true
}

fn r_dashes(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    if n < 3 {
        return;
    }
    for i in 1..n - 1 {
        if !d.p(i, '-') {
            continue;
        }
        // number ranges: 10-20
        if d.tight(i) && d.tight(i + 1) && d.k(i - 1) == Some(K::N) && d.k(i + 1) == Some(K::N) {
            let (a, b) = (d.w(i - 1), d.w(i + 1));
            let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit() || c == ',');
            if !digits(a) || !digits(b) {
                continue;
            }
            if i >= 2 && d.tight(i - 1) && (d.p(i - 2, '-') || d.p(i - 2, ':') || d.p(i - 2, '/') || d.p(i - 2, '.') || d.p(i - 2, '$') || d.p(i - 2, '+') || d.p(i - 2, '#')) {
                continue;
            }
            if d.tight(i + 2) && (d.p(i + 2, '-') || d.p(i + 2, ':') || d.p(i + 2, '/') || d.p(i + 2, '.')) {
                continue;
            }
            let (Ok(av), Ok(bv)) = (a.replace(',', "").parse::<u64>(), b.replace(',', "").parse::<u64>()) else { continue };
            if av >= bv {
                continue;
            }
            let mut c = i.wrapping_sub(2);
            if d.p(c, '.') {
                c = c.wrapping_sub(1);
            }
            let ctx = range_ctx(d.l(c));
            let unit = range_unit(d.l(i + 2));
            let years = a.len() == 4 && b.len() == 4 && (1500..=2100).contains(&av) && (1500..=2100).contains(&bv);
            if ctx || unit || years {
                hit1(
                    h,
                    d.tk[i - 1].s,
                    d.tk[i + 1].e,
                    format!("{a}–{b}"),
                    "Punctuation",
                    "Use an en dash in the range",
                    "Use an en dash (–), not a hyphen, between the two numbers in a range.",
                );
            }
            continue;
        }
        // "--"
        if d.p(i + 1, '-') && d.tight(i + 1) {
            if i + 2 >= n || (d.p(i + 2, '-') && d.tight(i + 2)) || (d.p(i - 1, '-') && d.tight(i)) {
                continue;
            }
            let (gb, ga) = (d.gap(i), d.gap(i + 2));
            if gb.contains(['\n', '\r']) || ga.contains(['\n', '\r']) || !dash_ok(d, i - 1, i + 2) {
                continue;
            }
            if !gb.is_empty() && ga.is_empty() {
                continue; // a command-line flag such as " --verbose"
            }
            let s = if gb.is_empty() { d.tk[i].s } else { d.tk[i - 1].e };
            let e = if ga.is_empty() { d.tk[i + 1].e } else { d.tk[i + 2].s };
            hit1(h, s, e, "—".into(), "Punctuation", "Use an em dash", "Two hyphens stand in for an em dash (—). Use the real dash.");
            continue;
        }
        // spaced single hyphen between words
        if d.p(i - 1, '-') || d.p(i + 1, '-') || d.gap(i) != " " || d.gap(i + 1) != " " {
            continue;
        }
        if !dash_ok(d, i - 1, i + 1) {
            continue;
        }
        hit1(h, d.tk[i - 1].e, d.tk[i + 1].s, "—".into(), "Punctuation", "Use an em dash", "A hyphen with spaces around it is meant as a dash. Use an em dash (—) to set off a break in the sentence.");
    }
}

// ---------------------------------------------------------------------------------------
// Introductory words and phrases
// ---------------------------------------------------------------------------------------

wordset!(
    intro_subj,
    "the a an this that these those we i you he she they it there our my his her their its some many most all everyone nobody people if when let's"
);
wordset!(however_next, "the a an this that these those we i you he she they it there our my his her their its some many most all in on at after for with not if when as by");
wordset!(intro_pron, "i we you he she they it there");

/// (phrase, mode): 0 = any following word, 1 = following word must start a clause,
/// 2 = "however" (clause start, no comma soon after), 3 = pronoun only, no comma soon after.
const INTRO: &[(&str, u8)] = &[
    ("however", 2),
    ("therefore", 0),
    ("unfortunately", 0),
    ("fortunately", 0),
    ("meanwhile", 0),
    ("finally", 1),
    ("additionally", 0),
    ("instead", 1),
    ("otherwise", 0),
    ("furthermore", 0),
    ("moreover", 0),
    ("consequently", 0),
    ("nevertheless", 0),
    ("nonetheless", 0),
    ("similarly", 0),
    ("likewise", 0),
    ("sadly", 0),
    ("luckily", 0),
    ("yes", 1),
    ("no", 1),
    ("well", 1),
    ("in fact", 0),
    ("of course", 0),
    ("as a result", 0),
    ("for example", 0),
    ("for instance", 0),
    ("on the other hand", 0),
    ("in addition", 0),
    ("in general", 1),
    ("after all", 3),
    ("by the way", 0),
    ("in conclusion", 0),
    ("in summary", 0),
    ("in contrast", 0),
    ("in other words", 0),
    ("as a matter of fact", 0),
    ("in the meantime", 0),
    ("on the contrary", 0),
    ("to be honest", 0),
    ("in short", 1),
    ("in particular", 1),
    ("at the same time", 0),
    ("as a consequence", 0),
    ("first of all", 0),
];

fn r_intro_words(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 0..n {
        if !d.ss[i] || !d.is_w(i) {
            continue;
        }
        let w0 = d.l(i);
        for &(phrase, mode) in INTRO {
            if !phrase.starts_with(w0) || !(phrase.len() == w0.len() || phrase.as_bytes()[w0.len()] == b' ') {
                continue;
            }
            let m = phrase.split(' ').count();
            if !phrase.split(' ').enumerate().all(|(k, p)| d.is_w(i + k) && d.l(i + k) == p && (k == 0 || (!d.nl(i + k) && d.gap(i + k) == " "))) {
                continue;
            }
            let nx = i + m;
            if !d.is_w(nx) || d.ss[nx] || d.nl(nx) {
                break;
            }
            let nw = d.l(nx);
            let lim = d.sent_last(i);
            let ok = match mode {
                0 => !matches!(nw, "of" | "to" | "as" | "not"),
                1 => intro_subj(nw),
                2 => however_next(nw) && !(intro_pron(nw) && d.has_comma(nx, (nx + 6).min(lim))),
                _ => intro_pron(nw) && !d.has_comma(nx, (nx + 6).min(lim)),
            };
            if ok {
                let orig = &d.t[d.tk[i].s..d.tk[i + m - 1].e];
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i + m - 1].e,
                    format!("{orig},"),
                    "Punctuation",
                    format!("Add a comma after “{orig}”"),
                    "Put a comma after an introductory word or phrase.",
                );
            }
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------
// Introductory clause: "When the meeting ended everyone left."
// ---------------------------------------------------------------------------------------

wordset!(adv_end, "again early late today yesterday tomorrow now soon together away anyway already tonight morning evening afternoon noon midnight one two three four five six seven eight nine ten eleven twelve");
wordset!(particle, "in out up down off back away over");

fn intro_candidate(d: &Doc, s0: usize, j: usize) -> bool {
    let (w, prev) = (d.l(j), d.l(j - 1));
    if !d.is_wn(j - 1) {
        return false;
    }
    if is_block_prev(prev) || is_compl_verb(prev) {
        if !(matches!(prev, "before" | "after") && d.cap(j) && !d.ss[j] && is_ly(d.l(j + 1))) {
            return false;
        }
    }
    if (s0..j).any(|k| is_compl_verb(d.l(k))) {
        return false;
    }
    let has_verb = (s0..j).any(|k| {
        let t = d.l(k);
        verb_any(t) || (t.len() > 4 && t.ends_with("ing")) || subj_contraction(t)
    });
    if !has_verb {
        return false;
    }
    let nx = d.l(j + 1);
    let verbish_next = |nx: &str| verb_strict(nx) || is_present_verb(nx) || s_form(nx) || (is_adv_word(nx) || is_ly(nx));
    match w {
        "we" | "i" | "he" | "she" | "they" => true,
        "you" | "it" => verbish_next(nx) && d.is_w(j + 1),
        "everyone" | "everybody" | "nobody" => d.is_w(j + 1) && (verb_strict(nx) || s_form(nx)),
        "please" => d.is_w(j + 1),
        "make" | "be" => nx == "sure",
        "let" => matches!(nx, "me" | "us"),
        "feel" => nx == "free",
        _ => {
            if subj_contraction(w) {
                return true;
            }
            if d.cap(j) && !d.ss[j] && d.is_w(j) && w != "i" {
                return (ed_verb(nx) || is_irregular_past(nx) || is_ly(nx)) && !d.cap(j - 1);
            }
            if matches!(w, "the" | "my" | "our" | "his" | "her" | "their" | "your") && j >= s0 + 4 {
                let last = d.sent_last(j);
                let lead = (prev.len() > 5 && prev.ends_with("ing")) || is_ly(prev) || particle(prev);
                let verb2 = d.l(j + 2);
                return lead && d.is_w(j + 1) && (verb_strict(verb2) || is_ly(verb2)) && !is_prep(d.l(j + 1)) && !d.has_comma(j, last);
            }
            false
        }
    }
}

fn r_intro_clause(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 0..n {
        if !d.ss[i] || !d.is_w(i) {
            continue;
        }
        let w0 = d.l(i);
        if !is_subord(w0) && w0 != "as" && w0 != "even" {
            continue;
        }
        let s0 = if w0 == "as" && d.l(i + 1) == "soon" && d.l(i + 2) == "as" {
            i + 3
        } else if w0 == "as" && d.l(i + 1) == "long" && d.l(i + 2) == "as" {
            i + 3
        } else if w0 == "even" && matches!(d.l(i + 1), "though" | "if") {
            i + 2
        } else if is_subord(w0) {
            i + 1
        } else {
            continue;
        };
        if d.ends_question(i) {
            continue;
        }
        let end = d.sent_last(i);
        let mut j = s0;
        while j <= end && j < s0 + 18 {
            if d.is_p(j) {
                if d.p(j, '-') && d.tight(j) && d.tight(j + 1) {
                    j += 1;
                    continue;
                }
                break;
            }
            if j >= s0 + 3 && intro_candidate(d, s0, j) {
                let orig = d.w(j - 1);
                hit1(
                    h,
                    d.tk[j - 1].s,
                    d.tk[j - 1].e,
                    format!("{orig},"),
                    "Punctuation",
                    "Add a comma after the opening clause",
                    "A comma usually separates an opening clause (starting with “when”, “if”, “because”, “after”…) from the main sentence.",
                );
                break;
            }
            j += 1;
        }
    }
}

// ---------------------------------------------------------------------------------------
// Compound sentences: comma before but / so / yet / and
// ---------------------------------------------------------------------------------------

wordset!(det_subj, "the a an my our your his her their this these those some many most every each another");
wordset!(indef_subj, "nobody everyone everybody someone somebody nothing everything anything anybody");
wordset!(paren_verb, "said says asked replied added noted wrote explained thought think thinks know knows guess mean suppose believe hope bet feel see seems seemed appears appeared remember understand say suspect imagine assume figure promise swear guarantee assure");
wordset!(
    start_block,
    "and or but so yet nor then now not no yes well oh ok okay hi hello thanks thank dear as also still however therefore thus instead otherwise meanwhile every each whenever wherever whoever whatever whichever any if because when while although though since once unless until after before even only just maybe perhaps honestly actually first second third finally next last today tomorrow yesterday tonight lately recently sometimes often usually sadly luckily fortunately unfortunately obviously clearly basically frankly personally generally overall to"
);
wordset!(
    sub_words,
    "if when because although though unless until whenever whereas wherever while that which who whom whose where whether since once"
);
wordset!(comp_lead, "more less better worse harder faster sooner longer bigger higher lower closer greater");

fn verb_ok_after_noun(d: &Doc, k: usize) -> bool {
    let t = d.l(k);
    let prev = d.l(k - 1);
    if verb_strict(t) {
        return true;
    }
    if is_ly(prev) || is_adv_word(prev) {
        return is_present_verb(t) || s_form(t);
    }
    if is_present_verb(t) && prev.ends_with('s') && !is_sing_s(prev) {
        return true;
    }
    s_form(t) && !prev.ends_with('s')
}

/// Does an independent clause (subject + verb) start at token `n`?
fn clause_starts(d: &Doc, n: usize) -> bool {
    if !d.is_w(n) {
        return false;
    }
    let w = d.l(n);
    if is_pron(w) || indef_subj(w) {
        if paren_verb(d.l(n + 1)) {
            return false;
        }
        let mut j = n + 1;
        if is_adv_word(d.l(j)) || is_ly(d.l(j)) {
            j += 1;
        }
        let v = d.l(j);
        if paren_verb(v) {
            return false;
        }
        return d.is_w(j) && (verb_strict(v) || is_present_verb(v) || (matches!(w, "he" | "she" | "it") && s_form(v)));
    }
    if subj_contraction(w) {
        return !paren_verb(w);
    }
    if det_subj(w) {
        for k in (n + 2)..=(n + 4) {
            let t = d.l(k);
            if !d.is_w(k) || is_prep(t) || matches!(t, "and" | "or" | "but" | "that" | "which" | "who") {
                return false;
            }
            if verb_ok_after_noun(d, k) {
                return (n + 1..k).all(|m| d.is_w(m) && !is_prep(d.l(m)));
            }
        }
        return false;
    }
    let nx = d.l(n + 1);
    if d.is_w(n + 1) && (ed_verb(nx) || is_irregular_past(nx)) {
        if d.cap(n) && !d.ss[n] && !is_det(w) {
            return true;
        }
        if w.len() >= 4 && w.ends_with('s') && !w.ends_with("ss") && !is_sing_s(w) && !d.cap(n) {
            return true;
        }
    }
    false
}

fn r_compound(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 1..n {
        let c = d.l(i);
        if !matches!(c, "but" | "so" | "yet" | "and") || !d.is_w(i) || d.ss[i] || d.nl(i) || !d.is_wn(i - 1) {
            continue;
        }
        let s = d.sent_first(i);
        if d.ends_question(i) {
            continue;
        }
        // the clause before the conjunction starts after the last clause punctuation
        let mut cs = s;
        for k in s..i {
            if d.is_p(k) && !(d.p(k, '-') && d.tight(k) && d.tight(k + 1)) && !d.p(k, '"') && !d.p(k, '“') && !d.p(k, '”') {
                cs = k + 1;
            }
        }
        if cs >= i || !d.is_w(cs) || i - cs < 3 {
            continue;
        }
        if cs == s && (is_subord(d.l(s)) || sub_words(d.l(s)) || is_aux(d.l(s)) || matches!(d.l(s), "what" | "how" | "why" | "please" | "to")) {
            continue;
        }
        if (cs..i).any(|k| (k > cs && sub_words(d.l(k))) || is_compl_verb(d.l(k)) || matches!(d.l(k), "both" | "either" | "neither" | "between" | "not" | "also") || d.p(k, '"')) {
            continue;
        }
        if !(cs + 1..i).any(|k| verb_strict(d.l(k)) || is_present_verb(d.l(k)) || subj_contraction(d.l(k))) {
            continue;
        }
        let prev = d.l(i - 1);
        if (d.cap(i - 1) && !d.ss[i - 1] && !day_names(prev) && !matches!(prev, "january" | "february" | "march" | "april" | "may" | "june" | "july" | "august" | "september" | "october" | "november" | "december")) || matches!(prev, "me" | "him" | "her" | "us" | "them" | "you" | "it" | "nothing" | "anything" | "all" | "everything") {
            continue;
        }
        // second clause
        let j = i + 1;
        let nw = d.l(j);
        let starts = if is_pron(nw) {
            let mut k = j + 1;
            if is_adv_word(d.l(k)) || is_ly(d.l(k)) {
                k += 1;
            }
            let v = d.l(k);
            d.is_w(k) && !paren_verb(v) && (verb_strict(v) || is_present_verb(v) || (matches!(nw, "he" | "she" | "it") && s_form(v)))
        } else if matches!(nw, "that" | "this") {
            matches!(d.l(j + 1), "is" | "was" | "will" | "would" | "can" | "could" | "has" | "had")
        } else if det_subj(nw) {
            clause_starts(d, j)
        } else {
            subj_contraction(nw)
        };
        if !starts || (c == "so" && matches!(nw, "that")) {
            continue;
        }
        let orig = d.w(i - 1);
        hit1(
            h,
            d.tk[i - 1].s,
            d.tk[i - 1].e,
            format!("{orig},"),
            "Punctuation",
            format!("Add a comma before “{}”", d.w(i)),
            "Put a comma before a conjunction that joins two complete sentences.",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Comma splices and run-ons
// ---------------------------------------------------------------------------------------

fn seg_ok(d: &Doc, s: usize, c: usize) -> bool {
    if c < s + 3 || !d.plain_words(s, c) || !d.is_w(s) {
        return false;
    }
    let w0 = d.l(s);
    if start_block(w0) || is_subord(w0) || is_prep(w0) || (w0.len() > 4 && w0.ends_with("ing")) {
        return false;
    }
    if w0 == "the" && comp_lead(d.l(s + 1)) {
        return false;
    }
    if matches!(w0, "the" | "every" | "each" | "any" | "next" | "last" | "first" | "this" | "that" | "one" | "by") && (s..s + 3).any(|k| matches!(d.l(k), "time" | "moment" | "minute" | "instant")) {
        return false;
    }
    if (s + 1..c).any(|k| sub_words(d.l(k))) {
        return false;
    }
    w0 == "please" || (s + 1..c).any(|k| verb_any(d.l(k)) || subj_contraction(d.l(k)))
}

fn cap_next(s: &str) -> String {
    capitalize(s)
}

fn r_splice(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for c in 0..n {
        if !d.p(c, ',') || c + 2 >= n || !d.is_w(c + 1) || d.nl(c + 1) {
            continue;
        }
        let s = d.sent_first(c);
        if s >= c || !seg_ok(d, s, c) {
            continue;
        }
        let nw = d.l(c + 1);
        // ", however," between two clauses
        if matches!(nw, "however" | "therefore" | "moreover" | "consequently" | "nevertheless" | "nonetheless" | "furthermore") {
            let (m, e) = if d.p(c + 2, ',') { (c + 3, c + 2) } else { (c + 2, c + 1) };
            if clause_starts(d, m) {
                let word = d.w(c + 1).to_lowercase();
                hit(
                    h,
                    d.tk[c].s,
                    d.tk[e].e,
                    vec![format!("; {word},")],
                    "Punctuation",
                    format!("Use a semicolon before “{word}”"),
                    "A comma alone cannot join two complete sentences, even with a word like “however”. Use a semicolon, or start a new sentence.",
                );
            }
            continue;
        }
        if !clause_starts(d, c + 1) {
            continue;
        }
        let e = d.tk[c + 1].e;
        let next = &d.t[d.tk[c + 1].s..e];
        let up = if next == "I" || next.starts_with("I'") { next.to_string() } else { cap_next(next) };
        hit(
            h,
            d.tk[c].s,
            e,
            vec![format!("; {next}"), format!(". {up}")],
            "Punctuation",
            "Fix the comma splice",
            "Two complete sentences are joined by only a comma. Use a semicolon, or start a new sentence.",
        );
    }
}

wordset!(
    starter_cap,
    "the this that we she he they it there our my their these those his her you please"
);
wordset!(
    run_det,
    "the this my our his her their these those"
);
wordset!(run_aux, "is are was were will can could would has have had");

fn r_runon(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for j in 1..n {
        if !d.is_w(j) || d.ss[j] || d.nl(j) || !d.is_wn(j - 1) {
            continue;
        }
        let w = d.l(j);
        let s = d.sent_first(j);
        // A: a capitalized sentence opener glued to the previous word: "on Monday The client"
        if d.cap(j) && starter_cap(w) && w != "i" {
            let prev = d.l(j - 1);
            if is_block_prev(prev) || is_prep(prev) || !d.is_w(j + 1) || d.cap(j + 1) || d.p(j - 1, '.') {
                continue;
            }
            let caps = (s..j).filter(|&k| d.is_w(k) && d.cap(k)).count();
            if caps >= 3 || j < s + 3 {
                continue;
            }
            if !(s..j).any(|k| k > s && (verb_strict(d.l(k)) || is_present_verb(d.l(k)) || s_form(d.l(k)))) {
                continue;
            }
            let (ps, pe) = (d.tk[j - 1].s, d.tk[j].e);
            let orig = &d.t[ps..pe];
            let _ = orig;
            let pw = d.w(j - 1);
            hit(
                h,
                ps,
                pe,
                vec![format!("{pw}. {}", d.w(j)), format!("{pw}; {}", d.w(j).to_lowercase())],
                "Punctuation",
                "Fix the run-on sentence",
                "Two sentences are run together with no punctuation. Separate them with a period or a semicolon.",
            );
            continue;
        }
        if d.cap(j) {
            continue;
        }
        // B: lowercase clause starts after a complete clause
        let cand = matches!(w, "nobody" | "everyone" | "everybody" | "someone" | "somebody" | "she" | "he" | "they" | "we") || run_det(w);
        if !cand || j < s + 3 || d.ends_question(j) {
            continue;
        }
        let w0 = d.l(s);
        if start_block(w0) || is_subord(w0) || is_prep(w0) || sub_words(w0) || is_aux(w0) || matches!(w0, "what" | "how" | "why" | "who") {
            continue;
        }
        let prev = d.l(j - 1);
        if is_block_prev(prev) || is_compl_verb(prev) || !d.plain_words(s, j) {
            continue;
        }
        if (s..j).any(|k| is_compl_verb(d.l(k)) || sub_words(d.l(k)) || matches!(d.l(k), "and" | "or" | "but" | "than" | "as")) {
            continue;
        }
        if !(s + 1..j).any(|k| verb_strict(d.l(k)) || is_present_verb(d.l(k)) || s_form(d.l(k))) {
            continue;
        }
        let nx = d.l(j + 1);
        let ok = if matches!(w, "nobody" | "everyone" | "everybody" | "someone" | "somebody") {
            d.is_w(j + 1) && (verb_strict(nx) || s_form(nx))
        } else if matches!(w, "she" | "he" | "they" | "we") {
            let mut k = j + 1;
            if is_adv_word(d.l(k)) || is_ly(d.l(k)) {
                k += 1;
            }
            (adv_end(prev) || is_ly(prev)) && d.is_w(k) && (verb_strict(d.l(k)) || is_present_verb(d.l(k)))
        } else {
            // determiner + noun + auxiliary + (adverb | adjective), not a relative clause
            let aux = d.l(j + 2);
            let after = d.l(j + 3);
            d.is_w(j + 1) && !is_ly(nx) && !is_prep(nx) && run_aux(aux) && d.is_w(j + 3) && !is_prep(after) && !after.ends_with("ing") && !ed_verb(after) && !after.ends_with("en") && !is_irregular_past(after)
        };
        if !ok {
            continue;
        }
        let pw = d.w(j - 1);
        let ps = d.tk[j - 1].s;
        let pe = d.tk[j].e;
        hit(
            h,
            ps,
            pe,
            vec![format!("{pw}. {}", capitalize(d.w(j))), format!("{pw}; {}", d.w(j))],
            "Punctuation",
            "Fix the run-on sentence",
            "Two sentences are run together with no punctuation. Separate them with a period or a semicolon.",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Comma before restrictive "that"
// ---------------------------------------------------------------------------------------

wordset!(
    that_demo,
    "is isn't makes sounds means explains seems looks feels proves would will 's doesn't didn't wasn't being way one night day morning evening afternoon week weekend year time much many kind sort said"
);

fn r_that(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for c in 1..n.saturating_sub(2) {
        if !d.p(c, ',') || d.l(c + 1) != "that" || !d.is_w(c + 1) || d.nl(c + 1) || !d.is_w(c + 2) || !d.is_wn(c - 1) {
            continue;
        }
        let after = d.l(c + 2);
        if that_demo(after) {
            continue;
        }
        if matches!(after, "was" | "were") && !ed_verb(d.l(c + 3)) {
            continue;
        }
        hit1(
            h,
            d.tk[c].s,
            d.tk[c + 1].e,
            " that".into(),
            "Punctuation",
            "Remove the comma before “that”",
            "A clause that is needed to identify or complete the idea (“the file that…”, “she said that…”) takes no comma before “that”.",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Direct address
// ---------------------------------------------------------------------------------------

wordset!(
    vocative,
    "everyone everybody guys folks team all friends sir madam ma'am buddy mate mom dad grandma grandpa honey sweetie class ladies gentlemen y'all boss doctor coach"
);
wordset!(
    role_noun,
    "manager boss director friend brother sister mom dad doctor teacher professor neighbor neighbour colleague coworker hire engineer designer customer client uncle aunt cousin president captain coach officer senator mr mrs ms dr"
);
wordset!(time_word, "tomorrow today tonight later soon again now yesterday then here there home anyway");
wordset!(reply_word, "yes no okay ok sure sorry");

fn title_case_sentence(d: &Doc, a: usize, b: usize) -> bool {
    let words: Vec<usize> = (a..=b).filter(|&k| d.is_w(k)).collect();
    words.len() >= 3 && words.iter().all(|&k| d.cap(k) || d.w(k).len() <= 3)
}

fn is_vocative(d: &Doc, v: usize) -> bool {
    if !d.is_w(v) {
        return false;
    }
    let w = d.l(v);
    if vocative(w) {
        return true;
    }
    d.cap(v) && !d.ss[v] && w != "i" && w.len() > 1 && !is_pron(w) && !is_det(w) && !is_prep(w) && !matches!(w, "and" | "but" | "or" | "the" | "to" | "for")
}

fn r_address(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    let msg = "Set off the name of the person you are speaking to with a comma.";
    // A: greeting at the start of a sentence followed by a name
    for i in 0..n {
        if !d.ss[i] || !d.is_w(i) {
            continue;
        }
        let w = d.l(i);
        let m = match w {
            "thanks" | "hello" | "hi" | "hey" | "congratulations" | "goodbye" | "bye" | "cheers" | "welcome" | "yes" | "no" | "okay" | "ok" | "sorry" => {
                if w == "welcome" && d.l(i + 1) == "back" {
                    2
                } else {
                    1
                }
            }
            "thank" if d.l(i + 1) == "you" => 2,
            "good" if matches!(d.l(i + 1), "morning" | "afternoon" | "evening" | "night" | "luck") => 2,
            "happy" if d.l(i + 1) == "birthday" => 2,
            _ => continue,
        };
        // "Good luck on the interview Chris" has words between: handled by B
        let v = i + m;
        if d.nl(v) || !is_vocative(d, v) || d.tight(v) {
            continue;
        }
        if title_case_sentence(d, i, d.sent_last(i)) {
            continue;
        }
        if reply_word(w) && m == 1 {
            // replies need the name to end the clause: "Yes Maria, I will."
            let after = v + 1;
            if !(after >= d.n() || d.is_p(after) || d.nl(after)) {
                continue;
            }
        }
        let after = d.l(v + 1);
        if d.cap(v) && d.is_w(v + 1) && d.cap(v + 1) && !d.ss[v + 1] && !matches!(after, "and") {
            // "Hello Kitty Bag": two capitalized words in a row may be a title
            if d.w(v).len() > 1 && !title_case_sentence(d, i, d.sent_last(i)) {
                // a full name like "Thanks Maria Lopez for ..." is still a name; keep going
            }
        }
        let last = i + m - 1;
        let orig = d.w(last);
        hit1(h, d.tk[last].s, d.tk[last].e, format!("{orig},"), "Punctuation", "Add a comma before the name", msg);
    }
    // B: a name or "everyone" closing a question, a farewell or a request
    for e in 2..n {
        let nm = e;
        if !d.is_w(nm) || d.ss[nm] || d.nl(nm) || !(d.cap(nm) || vocative(d.l(nm))) {
            continue;
        }
        let at_end = e + 1 >= n || d.nl(e + 1) || (d.is_p(e + 1) && matches!(d.w(e + 1), "." | "!" | "?"));
        if !at_end {
            continue;
        }
        if d.cap(nm) && (is_pron(d.l(nm)) || d.l(nm) == "i") {
            continue;
        }
        let s = d.sent_first(nm);
        if !d.is_p(nm - 1) && d.is_w(nm - 1) {
            let prev = d.l(nm - 1);
            if is_prep(prev) || is_det(prev) || role_noun(prev) || prev == "and" || prev == "or" {
                continue;
            }
            let ctx = (s..nm).any(|k| matches!(d.l(k), "you" | "your" | "please" | "luck" | "thanks" | "thank" | "welcome" | "hello" | "hi" | "hey" | "goodbye" | "bye" | "cheers" | "congratulations" | "birthday"));
            if !ctx {
                continue;
            }
            let pp = d.l(nm - 2);
            let ok = time_word(prev) || ((is_det(pp) || is_prep(pp)) && d.is_w(nm - 2) && !role_noun(prev) && !ed_verb(prev) && !is_irregular_past(prev) && !is_present_verb(prev));
            if !ok || (d.ends_question(nm) && !matches!(d.l(s), "can" | "could" | "would" | "will")) {
                continue;
            }
            if title_case_sentence(d, s, nm) {
                continue;
            }
            let orig = d.w(nm - 1);
            hit1(h, d.tk[nm - 1].s, d.tk[nm - 1].e, format!("{orig},"), "Punctuation", "Add a comma before the name", msg);
        }
    }
    // "Let's eat Grandma."
    for i in 0..n {
        if d.ss[i] && d.l(i) == "let's" && d.l(i + 1) == "eat" && matches!(d.l(i + 2), "grandma" | "grandpa" | "mom" | "dad" | "everyone" | "kids") && !d.is_w(i + 3) {
            hit1(h, d.tk[i + 1].s, d.tk[i + 1].e, "eat,".into(), "Punctuation", "Add a comma before the name", "A comma changes the meaning here: “Let’s eat, Grandma” invites her to dinner.");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Apostrophes
// ---------------------------------------------------------------------------------------

wordset!(
    quant_word,
    "many several few some all both various numerous multiple plenty dozens hundreds thousands those these two three four five six seven eight nine ten eleven twelve fifteen twenty thirty forty fifty hundred"
);
wordset!(
    pl_next,
    "for to and or in on at of with from by this that last next every were are have had will can could would should did do who which but so as than when while"
);
wordset!(no_plural_apos, "it he she that there here what who let one everyone someone anyone today tonight tomorrow yesterday where how when why");
wordset!(
    decade_next,
    "were was are is had have saw brought and or to in of as with when that which who but from for by on at the a an produced introduced marked began ended gave became seemed did"
);
wordset!(
    its_uncond,
    "a an the been going getting being becoming not just also always never already almost probably really quite too so okay ok what because if up supposed now here there"
);
wordset!(
    its_cond,
    "hard easy simple fast good bad great nice hot cold late early important possible necessary clear true fine likely unlikely impossible obvious cheap big small long short"
);
wordset!(
    its_after_cond,
    "to for that and but or if when enough than as so because in on at"
);
wordset!(
    poss_noun,
    "own tail age size shape weight price owner purpose value origin history name color colour findings members headquarters mission creator wheels wings surface edges version design population users customers employees features performance quality capacity speed length width height flavor taste smell location role impact effect reflection beauty charm edge corner leader voice mother"
);
wordset!(poss_adj, "new first second third last latest biggest largest current entire whole main final original previous annual overall");
wordset!(
    possessor,
    "brother sister manager team friend parent mother father husband wife neighbor neighbour colleague coworker customer client employee doctor teacher student boss son daughter cousin uncle aunt grandmother grandfather partner owner player user member leader director supervisor professor nurse driver author writer company city country school world state county town week month year day hour child person woman man dog cat family group"
);
wordset!(
    possessed,
    "car laptop feedback performance schedule phone house home office desk idea ideas opinion work job name address email book books bag keys key computer report plan plans decision advice help birthday wedding room family friends mother father parents voice face hair eyes hands truck bike dog cat boss husband wife presentation meeting results revenue budget team message speech suggestion suggestions response reply attitude behavior behaviour effort efforts contribution contributions success mistake fault role vision approach strategy goal goals lead guidance leadership support kindness generosity"
);
wordset!(possessive_dets, "my your his her our their the this that a an whose another each every");

fn plural_of(base: &str) -> String {
    if matches!(base, "tomato" | "potato" | "hero" | "echo" | "veto" | "torpedo" | "tornado" | "mosquito" | "volcano") {
        return format!("{base}es");
    }
    if base.ends_with('s') || base.ends_with('x') || base.ends_with('z') || base.ends_with("ch") || base.ends_with("sh") {
        return format!("{base}es");
    }
    if base.len() > 2 && base.ends_with('y') && !matches!(base.as_bytes()[base.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u') {
        return format!("{}ies", &base[..base.len() - 1]);
    }
    format!("{base}s")
}

fn split_apos_s(raw: &str) -> Option<&str> {
    raw.strip_suffix("'s").or_else(|| raw.strip_suffix("’s"))
}

fn r_apostrophes(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    let curly = d.t.contains('’');
    let ap = if curly { "’" } else { "'" };
    for i in 0..n {
        if !matches!(d.k(i), Some(K::W) | Some(K::N)) {
            continue;
        }
        let raw = d.w(i);
        let lw = d.l(i);
        let nx = d.l(i + 1);
        // ---- its / it's
        if lw == "its" {
            let nn = d.l(i + 2);
            let hyph = d.p(i + 2, '-') && d.tight(i + 2);
            let fire = d.is_w(i + 1)
                && !hyph
                && (its_uncond(nx)
                    || (its_cond(nx) && (!d.is_w(i + 2) || its_after_cond(nn))))
                && !(nx == "going" && !matches!(nn, "to" | "well" | "on" | "for"));
            if fire {
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i].e,
                    like_case(raw, &format!("it{ap}s")),
                    "Punctuation",
                    "Use “it’s”",
                    "“It’s” is short for “it is” or “it has”. “Its” (no apostrophe) shows ownership.",
                );
            }
            continue;
        }
        if lw == "it's" {
            let nn = d.l(i + 2);
            let fire = d.is_w(i + 1)
                && (poss_noun(nx)
                    || (poss_adj(nx)
                        && d.is_w(i + 2)
                        && !is_prep(nn)
                        && !is_aux(nn)
                        && !verb_strict(nn)
                        && !matches!(nn, "and" | "or" | "but" | "than" | "yet" | "that" | "time" | "thing" | "one" | "step" | "place" | "day" | "so" | "too" | "to" | "not")));
            if fire {
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i].e,
                    like_case(raw, "its"),
                    "Punctuation",
                    "Remove the apostrophe from “its”",
                    "“Its” without an apostrophe shows ownership. “It’s” always means “it is” or “it has”.",
                );
            }
            continue;
        }
        let Some(base) = split_apos_s(raw) else {
            // ---- possessives written without an apostrophe
            if d.is_w(i) && !d.cap(i) || (d.ss[i] && d.is_w(i)) {
                missing_possessive(d, i, h, ap);
            }
            continue;
        };
        if base.is_empty() {
            continue;
        }
        // ---- decades: 1990's
        if d.k(i) == Some(K::N) {
            if base.len() >= 2 && base.len() <= 4 && base.chars().all(|c| c.is_ascii_digit()) && base.ends_with('0') && (decade_next(nx) || !d.is_w(i + 1)) {
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i].e,
                    format!("{base}s"),
                    "Punctuation",
                    "Remove the apostrophe",
                    "Decades are plural, so they take no apostrophe: 1990s, not 1990’s.",
                );
            }
            continue;
        }
        // ---- acronyms: CD's
        if base.len() >= 2 && base.len() <= 6 && base.chars().all(|c| c.is_ascii_uppercase()) {
            if decade_next(nx) || pl_next(nx) || !d.is_w(i + 1) {
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i].e,
                    format!("{base}s"),
                    "Punctuation",
                    "Remove the apostrophe",
                    "Plurals of acronyms take no apostrophe: CDs, PDFs.",
                );
            }
            continue;
        }
        // ---- plain plurals: six pizza's
        if base.len() < 3 || !base.chars().all(|c| c.is_ascii_lowercase()) || no_plural_apos(base) || d.cap(i) {
            continue;
        }
        let quant = (i.saturating_sub(3)..i).any(|k| quant_word(d.l(k)) || d.k(k) == Some(K::N));
        let after_plain = nx.is_empty() || !d.is_w(i + 1) || pl_next(nx) || verb_strict(nx) || is_present_verb(nx);
        let agrees = matches!(nx, "were" | "are" | "have" | "don't" | "weren't" | "haven't");
        let prog = (i.saturating_sub(2)..i).any(|k| d.l(k).len() > 5 && d.l(k).ends_with("ing")) && matches!(nx, "this" | "last" | "next" | "every" | "today" | "tomorrow" | "yesterday" | "tonight");
        if (quant && after_plain) || agrees || prog {
            hit1(
                h,
                d.tk[i].s,
                d.tk[i].e,
                plural_of(base),
                "Punctuation",
                "Remove the apostrophe",
                "A plain plural takes no apostrophe. Use one only to show ownership (the dog’s tail) or in a contraction.",
            );
        }
    }
}

fn missing_possessive(d: &Doc, i: usize, h: &mut Vec<Hit>, ap: &str) {
    let lw = d.l(i);
    if lw.len() < 5 || !lw.ends_with('s') {
        return;
    }
    let fixed: Option<&str> = match lw {
        "everyones" => Some("everyone"),
        "someones" => Some("someone"),
        "anyones" => Some("anyone"),
        "nobodys" => Some("nobody"),
        "todays" => Some("today"),
        "tomorrows" => Some("tomorrow"),
        "yesterdays" => Some("yesterday"),
        "childrens" => Some("children"),
        "womens" => Some("women"),
        "mens" => Some("men"),
        _ => None,
    };
    let raw = d.w(i);
    if let Some(b) = fixed {
        if d.cap(i) && !d.ss[i] {
            return;
        }
        hit1(h, d.tk[i].s, d.tk[i].e, like_case(raw, &format!("{b}{ap}s")), "Punctuation", "Add an apostrophe", "This word shows ownership, so it needs an apostrophe before the “s”.");
        return;
    }
    // consonant + "ys": "companys", "citys"
    let b = lw.as_bytes();
    let before_y = b[b.len() - 3];
    if lw.ends_with("ys") && !matches!(before_y, b'a' | b'e' | b'i' | b'o' | b'u') && lw.chars().all(|c| c.is_ascii_lowercase()) && !d.cap(i) {
        let stem = &lw[..lw.len() - 1];
        if matches!(lw, "gladys" | "rhys") {
            return;
        }
        hit(
            h,
            d.tk[i].s,
            d.tk[i].e,
            vec![format!("{stem}{ap}s"), format!("{}ies", &lw[..lw.len() - 2])],
            "Punctuation",
            "Add an apostrophe",
            "This word shows ownership, so it needs an apostrophe before the “s” (company’s). For more than one, the plural is companies.",
        );
        return;
    }
    // "my brothers car"
    let stem = &lw[..lw.len() - 1];
    if possessor(stem) && possessive_dets(d.l(i.wrapping_sub(1))) && d.is_w(i + 1) && possessed(d.l(i + 1)) && !d.nl(i + 1) {
        hit(
            h,
            d.tk[i].s,
            d.tk[i].e,
            vec![format!("{stem}{ap}s"), format!("{stem}s{ap}")],
            "Punctuation",
            "Add an apostrophe",
            "This word shows ownership, so it needs an apostrophe: one owner takes ’s, several owners take s’.",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Compound adjectives before a noun
// ---------------------------------------------------------------------------------------

const COMPOUNDS: &str = "well known|well written|well organized|well designed|well respected|well established|full time|part time|long term|short term|medium term|high quality|low quality|high level|low level|low cost|high end|high tech|low income|high ranking|last minute|real time|up to date|state of the art|decision making|first class|second hand|open source|open plan|long lasting|world class|face to face|step by step|one on one|day to day|end to end|built in|old fashioned|easy to use|hands on|in depth|on site|large scale|small scale|wide ranging|far reaching|fast growing|long standing|record breaking|eye catching|user friendly|thought provoking|eco friendly|family friendly|customer facing|cost effective|time consuming|hard working|best known|full length|high speed|high profile|low key|mid level|top level|entry level|first hand|all inclusive|all in one|hand made|home made|self made|mind blowing|never ending";

fn compounds() -> &'static std::collections::HashMap<&'static str, Vec<Vec<&'static str>>> {
    static M: OnceLock<std::collections::HashMap<&'static str, Vec<Vec<&'static str>>>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m: std::collections::HashMap<&'static str, Vec<Vec<&'static str>>> = std::collections::HashMap::new();
        for p in COMPOUNDS.split('|') {
            let ws: Vec<&'static str> = p.split(' ').collect();
            m.entry(ws[0]).or_default().push(ws);
        }
        m
    })
}

wordset!(
    adj_det,
    "a an the this that our my your his her their its each every another some any no"
);
wordset!(
    adj_stop,
    "is are was were be been being am and or but nor in on at to for of by with who that which as than so when if the a an ago old from into after before too enough not has have had will would can could may might must should does did also still just very really quite here there now then up out over off down about this these those it its their his her our my your we i you he she they there's yet while where whose whom what how why because since until unless though although per via"
);
wordset!(
    num_word,
    "one two three four five six seven eight nine ten eleven twelve fifteen twenty thirty forty fifty hundred"
);
wordset!(
    unit_sing,
    "year month week day hour minute second page word mile foot inch door seat story star step line course"
);

fn noun_follows(d: &Doc, k: usize, gap_prev: usize) -> bool {
    if !d.is_w(k) || d.nl(k) || d.gap(k) != " " || d.cap(k) && !d.ss[k] && false {
        return false;
    }
    let _ = gap_prev;
    let w = d.l(k);
    !adj_stop(w) && !is_ly(w) && !verb_strict(w) && w.len() > 1
}

fn r_compound_adj(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 1..n {
        if !d.is_w(i) && d.k(i) != Some(K::N) {
            continue;
        }
        if !adj_det(d.l(i - 1)) || !d.is_w(i - 1) {
            continue;
        }
        let w0 = d.l(i);
        // table entries
        if let Some(cands) = compounds().get(w0) {
            for c in cands {
                let m = c.len();
                if !c.iter().enumerate().all(|(k, w)| d.is_w(i + k) && d.l(i + k) == *w && (k == 0 || d.gap(i + k) == " ")) {
                    continue;
                }
                if !noun_follows(d, i + m, 0) {
                    continue;
                }
                let orig = &d.t[d.tk[i].s..d.tk[i + m - 1].e];
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i + m - 1].e,
                    c.join("-"),
                    "Punctuation",
                    "Add a hyphen",
                    format!("Join the words with hyphens when they work together as one adjective before a noun (“{orig}” becomes “{}”).", c.join("-")),
                );
                break;
            }
            continue;
        }
        // number + unit: "five year contract"
        let is_num = num_word(w0) || (d.k(i) == Some(K::N) && d.w(i).chars().all(|c| c.is_ascii_digit()));
        if is_num && d.is_w(i + 1) && d.gap(i + 1) == " " && unit_sing(d.l(i + 1)) && noun_follows(d, i + 2, 0) {
            let (a, b) = (d.w(i), d.w(i + 1));
            // "a one year" is fine as a count only with an adjective use, so require a following noun
            hit1(
                h,
                d.tk[i].s,
                d.tk[i + 1].e,
                format!("{a}-{b}"),
                "Punctuation",
                "Add a hyphen",
                format!("Use a hyphen in a number-and-unit adjective before a noun (“{a}-{b} …”)."),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Missing end punctuation
// ---------------------------------------------------------------------------------------

fn para_last(d: &Doc, i: usize) -> bool {
    let n = d.n();
    if i + 1 >= n {
        return true;
    }
    let g = d.gap(i + 1);
    let breaks = g.chars().filter(|&c| c == '\n').count();
    if breaks >= 2 {
        return true;
    }
    if breaks == 1 && d.cap(i + 1) {
        // a long line followed by a capitalized line is its own paragraph
        let mut k = i;
        let mut words = 0;
        loop {
            if d.is_w(k) {
                words += 1;
            }
            if k == 0 || d.nl(k) {
                break;
            }
            k -= 1;
        }
        return words >= 8 && !is_block_prev(d.l(i));
    }
    false
}

wordset!(
    q_start,
    "can could would will should do does did is are was were have has had what where when why who how which whose may might shall"
);

/// Title of the missing-end-punctuation card (the host hides it at the caret).
pub const END_PUNCT_TITLE: &str = "Add end punctuation";

fn r_end_punct(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 0..n {
        if !d.is_wn(i) || !para_last(d, i) {
            continue;
        }
        if !(i + 1 >= n || d.nl(i + 1)) {
            continue;
        }
        let s = d.sent_first(i);
        if !d.cap(s) || !d.is_w(s) {
            continue;
        }
        // the paragraph must not start with a list marker or a number
        if s > 0 && !d.nl(s) && matches!(d.w(s - 1), "-" | "*" | "•" | "#" | ">") {
            continue;
        }
        if (s..=i).any(|k| d.k(k) == Some(K::O)) {
            continue;
        }
        let words = (s..=i).filter(|&k| d.is_wn(k)).count();
        if words < 5 || title_case_sentence(d, s, i) {
            continue;
        }
        let has_verb = d.l(s) == "please" || (s + 1..=i).any(|k| verb_any(d.l(k)) || subj_contraction(d.l(k)));
        if !has_verb && words < 8 {
            continue;
        }
        if (s..=i).any(|k| d.is_p(k) && matches!(d.w(k), "{" | "}" | "=" | "<" | "|" | "\\" | "`" | "$" | "%")) {
            continue;
        }
        // all-caps shouting or a heading in capitals
        if (s..=i).filter(|&k| d.is_w(k)).all(|k| d.w(k).chars().all(|c| !c.is_lowercase())) {
            continue;
        }
        let w = d.w(i);
        let q = q_start(d.l(s));
        let sug = if q { vec![format!("{w}?"), format!("{w}.")] } else { vec![format!("{w}.")] };
        hit(h, d.tk[i].s, d.tk[i].e, sug, "Punctuation", END_PUNCT_TITLE, "This sentence has no punctuation at the end.");
    }
}

// ---------------------------------------------------------------------------------------
// A space before ";" or ":"
// ---------------------------------------------------------------------------------------

fn r_space_punct(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 1..n {
        if !(d.p(i, ';') || d.p(i, ':')) || d.gap(i) != " " || !d.is_wn(i - 1) {
            continue;
        }
        // followed by a space (or the end), so "10 :30" and emoticons are left alone
        if i + 1 < n && d.tight(i + 1) {
            continue;
        }
        let c = d.w(i).to_string();
        let what = if c == ";" { "semicolon" } else { "colon" };
        hit1(
            h,
            d.tk[i - 1].e,
            d.tk[i].e,
            c,
            "Punctuation",
            format!("Remove the space before the {what}"),
            "Punctuation marks attach to the word before them, with no space.",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Quotes: American style puts periods and commas inside the closing quote
// ---------------------------------------------------------------------------------------

fn r_quotes(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    let mut i = 0;
    while i < n {
        // paragraph = run of tokens up to a blank line
        let start = i;
        let mut end = i;
        while end + 1 < n && d.gap(end + 1).chars().filter(|&c| c == '\n').count() < 2 {
            end += 1;
        }
        i = end + 1;
        let straight: Vec<usize> = (start..=end).filter(|&k| d.p(k, '"')).collect();
        let straight_ok = straight.len() % 2 == 0;
        let mut open = false;
        for k in start..=end {
            let closing = if d.p(k, '”') {
                true
            } else if d.p(k, '"') && straight_ok {
                open = !open;
                !open
            } else {
                false
            };
            if !closing || !d.tight(k) || !d.is_wn(k.wrapping_sub(1)) {
                continue;
            }
            if d.k(k - 1) == Some(K::N) && d.l(k - 1).chars().all(|c| c.is_ascii_digit()) {
                continue; // inch marks
            }
            let q = d.w(k).to_string();
            if (d.p(k + 1, '.') || d.p(k + 1, ',')) && d.tight(k + 1) && !(d.p(k + 2, '.') && d.tight(k + 2)) && !(d.is_wn(k + 2) && d.tight(k + 2)) {
                let punct = d.w(k + 1).to_string();
                let what = if punct == "." { "period" } else { "comma" };
                hit1(
                    h,
                    d.tk[k].s,
                    d.tk[k + 1].e,
                    format!("{punct}{q}"),
                    "Punctuation",
                    format!("Move the {what} inside the quotes"),
                    format!("In American English, a {what} goes inside the closing quotation mark."),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// who / whom
// ---------------------------------------------------------------------------------------

wordset!(
    whom_prep,
    "to with for from by about of at on upon among against between toward towards through without within under over behind beside near"
);
wordset!(be_form, "am are is was were be been being");
wordset!(
    whom_aux,
    "should did do does can could would will shall"
);
wordset!(
    who_det,
    "this that the these those it he she they we you i"
);

fn r_whom(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 1..n {
        if !d.is_w(i) {
            continue;
        }
        let w = d.l(i);
        let raw = d.w(i);
        if w == "who" {
            let prev = d.l(i - 1);
            let nx = d.l(i + 1);
            let prep_before = d.is_w(i - 1) && whom_prep(prev);
            let mut fire = false;
            if prep_before && d.ss[i - 1] {
                // question: "To who should I...?"
                fire = whom_aux(nx) || (matches!(nx, "is" | "are" | "was" | "were" | "has" | "have") && d.ends_question(i) && who_det(d.l(i + 2)));
            }
            if prep_before && is_pron(nx) && d.is_w(i + 1) {
                fire = true;
            }
            if !prep_before && !d.ss[i] && is_pron(nx) && d.is_w(i + 1) && prev != "than" {
                let nn = d.l(i + 2);
                let skip = be_form(nn) || is_compl_verb(nn) || (i + 3..=i + 4).any(|k| be_form(d.l(k)) && !d.is_p(k)) || !d.is_w(i + 2);
                fire = !skip;
            }
            if fire {
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[i].e,
                    like_case(raw, "whom"),
                    "Grammar",
                    "Use “whom”",
                    "Use “whom” when the word is the object of a verb or preposition (to whom, whom we met). Use “who” when it is the subject.",
                );
            }
        } else if w == "whom" && d.ss[i] {
            let nx = d.l(i + 1);
            let nn = d.l(i + 2);
            if matches!(nx, "is" | "was" | "are" | "were") && (nn.ends_with("ing") || matches!(nn, "responsible" | "ready" | "late" | "here" | "there" | "able" | "available" | "next" | "right" | "wrong" | "correct")) {
                hit1(h, d.tk[i].s, d.tk[i].e, like_case(raw, "who"), "Grammar", "Use “who”", "Use “who” when the word is the subject of the verb.");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Pronoun case
// ---------------------------------------------------------------------------------------

wordset!(case_prep, "between to for with from by about at among near beside behind");
wordset!(
    case_after_ok,
    "after before tonight tomorrow today later too as so because but and or when while if on in at for about from with"
);

fn obj_form(p: &str) -> Option<&'static str> {
    match p {
        "i" => Some("me"),
        "she" => Some("her"),
        "he" => Some("him"),
        "they" => Some("them"),
        "we" => Some("us"),
        _ => None,
    }
}

fn r_pronoun_case(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 2..n {
        // preposition + X and I
        if d.l(i) == "and" && d.is_w(i) && d.is_w(i + 1) && d.is_w(i - 1) {
            if let Some(obj) = obj_form(d.l(i + 1)) {
                let x = d.l(i - 1);
                let by_pron = matches!(x, "him" | "her" | "them" | "you" | "us" | "me");
                let shape = if case_prep(d.l(i - 2)) && (by_pron || d.cap(i - 1)) {
                    true
                } else {
                    i >= 3 && is_det(d.l(i - 2)) && case_prep(d.l(i - 3)) && !is_present_verb(x)
                };
                let nx = d.l(i + 2);
                let after_ok = !d.is_w(i + 2) || case_after_ok(nx) || is_prep(nx);
                if shape && after_ok && !(d.l(i - 2) == "to" && is_present_verb(x)) {
                    hit1(
                        h,
                        d.tk[i + 1].s,
                        d.tk[i + 1].e,
                        obj.to_string(),
                        "Grammar",
                        format!("Use “{obj}”"),
                        "After a preposition (between, to, for, with…), use the object form: “between you and me”, “to Sarah and me”.",
                    );
                }
            }
        }
    }
    for i in 0..n {
        if !d.ss[i] || !d.is_w(i) {
            continue;
        }
        let w = d.l(i);
        // "Me and him went" / "Me and Priya finished"
        if w == "me" && d.l(i + 1) == "and" && d.is_w(i + 2) {
            let x = d.l(i + 2);
            let (span_end, text) = match x {
                "him" => (i + 2, "He and I".to_string()),
                "her" => (i + 2, "She and I".to_string()),
                "them" => (i + 2, "They and I".to_string()),
                "you" => (i + 2, "You and I".to_string()),
                _ if d.cap(i + 2) && !is_pron(x) && d.k(i + 2) == Some(K::W) => (i + 2, format!("{} and I", d.w(i + 2))),
                _ if is_det(x) && matches!(x, "my" | "our" | "his" | "her" | "their" | "the") && d.is_w(i + 3) => (i + 3, format!("{} {} and I", capitalize(d.w(i + 2)), d.w(i + 3))),
                _ => continue,
            };
            let nx = d.l(span_end + 1);
            if d.is_w(span_end + 1) && nx != "and" && (verb_any(nx) || is_ly(nx) || is_adv_word(nx)) {
                hit1(
                    h,
                    d.tk[i].s,
                    d.tk[span_end].e,
                    text,
                    "Grammar",
                    "Use the subject form",
                    "As the subject of the sentence, use “I”, and put the other person first: “Priya and I finished”.",
                );
            }
            continue;
        }
        // "Him and I went" / "Her and I went" / "Them and I"
        if matches!(w, "him" | "her" | "them") && d.l(i + 1) == "and" && d.is_w(i + 2) {
            let x = d.l(i + 2);
            let subj = match w {
                "him" => "He",
                "her" => "She",
                _ => "They",
            };
            if w == "her" && x == "her" {
                if d.is_w(i + 3) && d.is_w(i + 4) && (verb_strict(d.l(i + 4)) || is_present_verb(d.l(i + 4))) {
                    hit1(h, d.tk[i].s, d.tk[i].e, "She".into(), "Grammar", "Use the subject form", "As the subject of the sentence, use “she”, not “her”.");
                }
                continue;
            }
            let x_ok = matches!(x, "i" | "me") || (w != "her" && (matches!(x, "she" | "he" | "they" | "we" | "you") || (d.cap(i + 2) && !d.ss[i + 2]) || matches!(x, "his" | "my" | "their" | "our")));
            if !x_ok || (d.cap(i + 2) && (0..3).all(|k| d.cap(i + k))) {
                continue;
            }
            // a verb must follow the pair
            let after = if matches!(x, "his" | "my" | "their" | "our") { i + 4 } else { i + 3 };
            let v = d.l(after);
            if !(verb_strict(v) || is_present_verb(v) || is_ly(v)) {
                continue;
            }
            if x == "me" {
                hit1(h, d.tk[i].s, d.tk[i + 2].e, format!("{subj} and I"), "Grammar", "Use the subject form", "As the subject of the sentence, use “he” or “she” with “I”: “He and I are going”.");
            } else {
                hit1(h, d.tk[i].s, d.tk[i].e, subj.into(), "Grammar", "Use the subject form", "As the subject of the sentence, use “he”, “she” or “they”, not “him”, “her” or “them”.");
            }
            continue;
        }
        // "Us engineers have"
        if w == "us" && d.is_w(i + 1) && d.l(i + 1).ends_with('s') && d.l(i + 1).len() > 3 && verb_strict(d.l(i + 2)) && !d.cap(i + 1) {
            hit1(h, d.tk[i].s, d.tk[i].e, "We".into(), "Grammar", "Use “we”", "As the subject of the sentence, use “we”, not “us”.");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Confusables: your/you're, to/too, then/than, loose/lose, their/there/they're
// ---------------------------------------------------------------------------------------

fn fix(h: &mut Vec<Hit>, d: &Doc, i: usize, new: &str, kind: &'static str, title: &str, msg: &str) {
    hit1(h, d.tk[i].s, d.tk[i].e, like_case(d.w(i), new), kind, title, msg);
}

wordset!(your_uncond, "not always never really quite probably also a an the too");
wordset!(
    your_cond,
    "ready late early sure able lucky funny crazy amazing awesome great here there all so"
);
wordset!(
    your_end,
    "to for and but or if that because as in at on too anytime now again enough about with than"
);
wordset!(
    free_end,
    "on at to tomorrow tonight later this next today after before for and but if then now anytime"
);
wordset!(
    your_noun,
    "name address email phone number account password order request message time help support feedback patience project team company business family life job career school seat belt ticket bag keys car house room desk boss manager mom dad mother father wife husband son daughter brother sister dog cat idea plan goal goals report document file files data results review application resume profile page site website payment card subscription budget schedule calendar meeting appointment flight reservation booking question questions answer answers opinion thoughts input attention cooperation understanding assistance effort efforts contribution contributions work visit day week year"
);
wordset!(
    your_adj,
    "first last next new own current updated previous old latest entire whole main favorite favourite personal final original daily weekly"
);
wordset!(
    doing_prev,
    "how what when where why if hope know sure"
);

wordset!(
    too_adj,
    "hot cold late early soon bad big small long far hard expensive confusing difficult easy heavy loud quiet young old tired busy good high short tall tight little complicated complex risky dangerous strong weak sweet salty spicy serious simple obvious personal"
);
wordset!(
    copula,
    "is are was were be been being am isn't aren't wasn't weren't it's that's he's she's there's what's you're they're we're i'm seems seem seemed looks look looked feels feel felt sounds sound sounded get gets got become becomes became way not very much just"
);
wordset!(
    to_end_prev,
    "me him her them us you does do did is am are was were can will would could should might must"
);
wordset!(
    too_det,
    "the a an my your his her their our this that these those"
);
wordset!(
    too_verb,
    "confirm let tell ask inform invite thank announce introduce schedule send say share update request remind ensure check discuss review apply see get make take give find use help go work call meet join attend"
);
wordset!(
    too_verb_prev,
    "here going hoping trying wanted want like need ready glad happy excited sorry able unable"
);
wordset!(
    comp_word,
    "more less better worse rather other fewer greater sooner closer faster slower bigger smaller taller shorter higher lower easier harder cheaper larger longer older younger earlier stronger weaker happier wiser richer newer nicer safer simpler cleaner quicker louder deeper wider heavier lighter brighter darker tougher smarter healthier busier"
);
wordset!(
    comp_adj,
    "important prepared expensive difficult likely useful efficient careful comfortable interesting popular successful common reliable challenging effective complicated convenient valuable productive powerful beautiful dangerous serious"
);
wordset!(
    lose_prev,
    "to could would will can might may must should shall don't doesn't didn't won't wouldn't couldn't shouldn't can't cannot not never gonna about i you we they"
);
wordset!(
    loose_next_ok,
    "ends end change fitting fit clothing clothes cannon tooth thread screw screws leaf skin hair lips women bolt bolts cloth soil"
);
wordset!(
    lose_after_prev,
    "too so very slightly bit came come comes coming got get gets getting is are was were be been being feels felt seems seemed looks looked hang hangs hung set break breaks broke work works worked cut let turn turned cast quite rather"
);
wordset!(
    lose_obj,
    "the my your our his her their weight money track sight interest focus control hope faith patience it them"
);
wordset!(
    loose_not_prev,
    "let set cut turn break come came hang hung work worked get got is are was were be been a an the so too very bit little quite rather feels felt seems looks"
);
wordset!(
    place_prev,
    "over out up down in right from are is was were be been get got go went come came stay stayed sit sat stand stood live lived put left leave look looked looking standing sitting waiting"
);
wordset!(
    place_next,
    "by on in at near under behind beside next between across around inside outside along above below"
);
wordset!(
    their_ing,
    "going coming planning leaving trying getting looking making working doing having being waiting hoping wanting thinking saying"
);
wordset!(
    ing_follow,
    "to a an the on in at for with out up home back away too soon now today tomorrow tonight next this our my your"
);
wordset!(
    their_noun,
    "best own kids children friends family parents jackets coats notebooks books bags phones keys cars house houses home homes money help support work jobs team company business opinion opinions ideas plan plans goals needs names name address email emails hands eyes faces hearts minds lives life favorite respective mother father mom dad sister brother boss manager teacher teachers neighbors neighbours employees customers clients students players members fans supporters products services results performance success victory mistakes fault responsibility decision choice story stories history data files documents reports report tickets passports luggage belongings things stuff"
);
wordset!(
    there_prev_noun,
    "forgot forget left lost took take brought bring did do does done with in on at for to of from by about into among all that told if when because as but both between"
);
wordset!(
    there_prev_ing,
    "i think if hope know when because that say believe guess assume bet wonder whether once while though although unless until as but so since what where why how"
);
wordset!(
    theyre_noun,
    "own jackets coats notebooks backpacks phones house money opinion performance responsibility belongings luggage passports"
);
wordset!(
    theyre_prev,
    "left forgot lost took take brought bring with in on at for to of from by about into did do does all both said told"
);

fn r_confusables(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 0..n {
        if !d.is_w(i) {
            continue;
        }
        let w = d.l(i);
        let nx = d.l(i + 1);
        let nn = d.l(i + 2);
        let prev = d.l(i.wrapping_sub(1));
        let hy = d.p(i + 2, '-') && d.tight(i + 2);
        match w {
            "your" => {
                if !d.is_w(i + 1) {
                    continue;
                }
                let end_ok = |set: fn(&str) -> bool| !d.is_w(i + 2) || set(nn);
                let fire = if your_uncond(nx) && !hy {
                    true
                } else if nx == "going" {
                    nn == "to" && !matches!(d.l(i + 3), "" | "the" | "a") || (nn == "to" && d.is_w(i + 3))
                } else if nx == "gonna" {
                    true
                } else if nx == "doing" {
                    doing_prev(prev)
                } else if nx == "welcome" {
                    !d.is_w(i + 2) || matches!(nn, "to" | "back" | "anytime" | "here" | "any")
                } else if matches!(nx, "right" | "wrong" | "correct") {
                    !d.is_w(i + 2) || matches!(nn, "about" | "and" | "but" | "that" | "because" | "as" | "so" | "if" | "again")
                } else if nx == "free" {
                    end_ok(free_end)
                } else if your_cond(nx) && !hy {
                    end_ok(your_end)
                } else {
                    false
                };
                if fire {
                    fix(h, d, i, "you're", "Usage", "Use “you’re”", "“You’re” is short for “you are”. “Your” shows ownership.");
                }
            }
            "you're" => {
                if !d.is_w(i + 1) {
                    continue;
                }
                let fire = your_noun(nx) || (your_adj(nx) && d.is_w(i + 2) && (your_noun(nn) || matches!(nn, "time" | "day" | "week" | "year" | "visit" | "order" | "name")));
                if fire {
                    fix(h, d, i, "your", "Usage", "Use “your”", "“Your” shows ownership. “You’re” is short for “you are”.");
                }
            }
            "to" => {
                let tm = "“Too” means “also” or “more than needed”. “To” points toward something or comes before a verb.";
                if nx == "much" && nn != "of" && !matches!(prev, "according" | "compared" | "close" | "similar" | "next" | "owing" | "thanks" | "due") && d.is_w(i + 1) {
                    fix(h, d, i, "too", "Usage", "Use “too”", tm);
                } else if nx == "many" && nn != "of" && matches!(prev, "have" | "has" | "had" | "are" | "were" | "is" | "was") {
                    fix(h, d, i, "too", "Usage", "Use “too”", tm);
                } else if too_adj(nx) && copula(prev) && d.is_w(i + 1) && !hy {
                    fix(h, d, i, "too", "Usage", "Use “too”", tm);
                } else if (d.p(i + 1, '.') || d.p(i + 1, '!')) && (to_end_prev(prev) || d.p(i.wrapping_sub(1), ',')) && !d.ends_question(i) && i > 0 {
                    let s = d.sent_first(i);
                    if !matches!(d.l(s), "who" | "what" | "which" | "where" | "whom") {
                        fix(h, d, i, "too", "Usage", "Use “too”", tm);
                    }
                }
            }
            "too" => {
                let tm = "“To” points toward something or comes before a verb. “Too” means “also” or “more than needed”.";
                if too_det(nx) && d.is_w(i + 1) && !d.is_p(i.wrapping_sub(1)) && i > 0 && !matches!(prev, "me" | "us" | "them" | "him" | "her" | "you" | "it" | "this" | "that") {
                    fix(h, d, i, "to", "Usage", "Use “to”", tm);
                } else if too_verb(nx) && d.is_w(i + 1) && (prev.ends_with("ing") || too_verb_prev(prev)) {
                    fix(h, d, i, "to", "Usage", "Use “to”", tm);
                }
            }
            "then" => {
                if !d.is_w(i + 1) || d.nl(i) || i == 0 {
                    continue;
                }
                let s = d.sent_first(i);
                if is_subord(d.l(s)) || d.p(i - 1, ',') || d.is_p(i - 1) {
                    continue;
                }
                let lo = i.saturating_sub(8).max(s);
                let no_p = |a: usize| (a..i).all(|m| !d.is_p(m) || (d.p(m, '-') && d.tight(m)));
                let direct = comp_word(prev);
                let more = (lo..i).any(|k| matches!(d.l(k), "more" | "less" | "fewer") && comp_adj(d.l(k + 1)) && k + 1 < i && no_p(k));
                let rather = (lo..i).any(|k| d.l(k) == "rather" && no_p(k)) || (lo..i).any(|k| d.l(k) == "i'd" && d.l(k + 1) == "rather");
                if direct || more || rather {
                    fix(h, d, i, "than", "Usage", "Use “than”", "Use “than” to compare things. Use “then” for time or order (“first this, then that”).");
                }
            }
            "than" => {
                let tm = "Use “then” for time or order (“first this, then that”). Use “than” to compare things.";
                if d.ss[i] {
                    fix(h, d, i, "then", "Usage", "Use “then”", tm);
                    continue;
                }
                if i == 0 || !d.is_w(i + 1) {
                    continue;
                }
                let s = d.sent_first(i);
                let lo = i.saturating_sub(7).max(s);
                let has_comp = (lo..i).any(|k| comp_word(d.l(k)) || matches!(d.l(k), "rather" | "other" | "more" | "less"));
                if d.p(i - 1, ',') && !has_comp {
                    fix(h, d, i, "then", "Usage", "Use “then”", tm);
                } else if prev == "and" && !has_comp {
                    fix(h, d, i, "then", "Usage", "Use “then”", tm);
                }
            }
            "loose" => {
                let tm = "“Lose” is the verb (lose a game). “Loose” is the adjective (a loose screw).";
                if d.p(i + 1, '-') && d.tight(i + 1) || loose_next_ok(nx) {
                    continue;
                }
                if lose_prev(prev) || (lose_obj(nx) && d.is_w(i + 1) && !loose_not_prev(prev)) {
                    fix(h, d, i, "lose", "Usage", "Use “lose”", tm);
                }
            }
            "lose" => {
                let tm = "“Loose” is the adjective (a loose screw). “Lose” is the verb (lose a game).";
                if lose_after_prev(prev) || matches!(nx, "fitting" | "fit" | "clothes" | "clothing") {
                    fix(h, d, i, "loose", "Usage", "Use “loose”", tm);
                }
            }
            "their" => {
                let tm = "“Their” shows ownership, “there” points to a place, and “they’re” is short for “they are”.";
                if matches!(nx, "is" | "are" | "was" | "were" | "has" | "have" | "been" | "isn't" | "aren't" | "wasn't" | "weren't") && d.is_w(i + 1) {
                    fix(h, d, i, "there", "Usage", "Use “there”", tm);
                } else if their_ing(nx) && ing_follow(nn) && d.is_w(i + 2) {
                    fix(h, d, i, "they're", "Usage", "Use “they’re”", tm);
                } else if i > 0 && ((place_prev(prev) && place_next(nx) && !(d.p(i + 2, '-') && d.tight(i + 2))) || (place_prev(prev) && matches!(d.w(i + 1), "." | "!" | "?" | ","))) {
                    fix(h, d, i, "there", "Usage", "Use “there”", tm);
                } else if matches!(d.w(i + 1), "." | "!" | "?") && d.is_p(i + 1) && i > 0 && !matches!(prev, "of" | "and" | "or") && d.is_w(i - 1) && place_prev(prev) {
                    fix(h, d, i, "there", "Usage", "Use “there”", tm);
                }
            }
            "there" => {
                let tm = "“Their” shows ownership, “there” points to a place, and “they’re” is short for “they are”.";
                if their_noun(nx) && d.is_w(i + 1) && there_prev_noun(prev) && i > 0 {
                    fix(h, d, i, "their", "Usage", "Use “their”", tm);
                } else if their_ing(nx) && ing_follow(nn) && d.is_w(i + 2) && (d.ss[i] || there_prev_ing(prev)) && !(nx == "going" && nn == "to" && d.l(i + 3) == "be") {
                    fix(h, d, i, "they're", "Usage", "Use “they’re”", tm);
                }
            }
            "they're" => {
                let tm = "“They’re” is short for “they are”. “Their” shows ownership.";
                if theyre_noun(nx) && d.is_w(i + 1) && theyre_prev(prev) && i > 0 {
                    fix(h, d, i, "their", "Usage", "Use “their”", tm);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------
// Agreement
// ---------------------------------------------------------------------------------------

wordset!(
    num_pl,
    "two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen twenty several many multiple various numerous dozens hundreds"
);
wordset!(
    measure_unit,
    "percent hours minutes days weeks months years miles dollars pounds euros cents feet inches kilometers"
);
wordset!(
    there_prev_ok,
    "that if when because think know say said hope but and so since as while whether what which who how why unless though although once"
);
wordset!(
    coll_head,
    "number variety couple majority minority lot lots bunch handful range host plethora array series pair percent percentage half rest remainder none all most some part group team committee family panel board class audience crowd public staff council government company band jury cast crew faculty management data media"
);
wordset!(
    irreg_plural,
    "people children men women police feet teeth geese mice criteria alumni phenomena folks cattle"
);
wordset!(
    roman_prev,
    "type chapter part phase class grade category level vitamin world war section volume tier group option plan step version rule act book"
);
wordset!(
    suggest_verb,
    "suggest suggests suggested recommend recommends recommended insist insists insisted demand demands demanded require requires required ask asks asked important essential necessary vital imperative wish wished lest rather"
);
wordset!(
    names,
    "sam dana john mary james jennifer michael sarah david emma daniel olivia matthew sophia chris alex jordan taylor maria priya tom jake anna peter linda mark laura paul susan kevin karen steven nancy brian lisa jason amy eric rachel adam emily ryan megan justin hannah nick kate ben lucy josh jessica andrew carol dave ellen frank grace henry irene jack kelly leo mia nora oscar pam quinn ruth sean tina victor wendy zoe ana carlos ahmed fatima wei yuki raj sara tim bob ted ann joe jim bill"
);
wordset!(
    skip_to_verb_prev,
    "to let make makes made help helps helped see saw watch watched hear heard feel felt notice noticed have had get got than do does did will would can could may might must shall should"
);

fn plural_noun(w: &str) -> bool {
    irreg_plural(w) || (w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") && !is_sing_s(w))
}

fn r_agreement(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    let agr = "The verb should agree with its subject.";
    for i in 0..n {
        if !d.is_w(i) {
            continue;
        }
        let w = d.l(i);
        let prev = d.l(i.wrapping_sub(1));
        // ---- there is + plural
        let there_form = (matches!(w, "is" | "was") && prev == "there") || w == "there's";
        if there_form {
            let tpos = if w == "there's" { i } else { i.wrapping_sub(1) };
            let before = d.l(tpos.wrapping_sub(1));
            if !(d.ss[tpos] || there_prev_ok(before)) {
                continue;
            }
            let k = i + 1;
            let q = d.l(k);
            let mut plural = false;
            if num_pl(q) && d.is_w(k) {
                plural = true;
            } else if d.k(k) == Some(K::N) {
                if let Ok(v) = d.w(k).parse::<u32>() {
                    plural = (2..=99).contains(&v);
                }
            } else if q == "a" && d.l(k + 1) == "few" {
                plural = true;
            }
            let after = if q == "a" { d.l(k + 2) } else { d.l(k + 1) };
            if plural && !measure_unit(after) && !(d.p(k + 1, '-') && d.tight(k + 1)) && !d.p(k + 1, '%') && !d.p(k + 1, ':') {
                let new = if w == "was" { "were" } else if w == "there's" { "there are" } else { "are" };
                fix(h, d, i, new, "Agreement", "Use a plural verb", "Use “are” or “were” when the thing that follows is plural: “There are three seats”.");
            }
            continue;
        }
        // ---- he/she/it don't, have
        if matches!(prev, "he" | "she" | "it") && d.is_w(i.wrapping_sub(1)) {
            let before = d.l(i.wrapping_sub(2));
            if w == "don't" {
                fix(h, d, i, "doesn't", "Agreement", "Use “doesn’t”", "With “he”, “she” and “it”, use “doesn’t”.");
                continue;
            }
            if w == "do" && d.l(i + 1) == "not" && d.is_w(i + 1) {
                hit1(h, d.tk[i].s, d.tk[i + 1].e, like_case(d.w(i), "does not"), "Agreement", "Use “does not”", "With “he”, “she” and “it”, use “does not”.");
                continue;
            }
            if w == "have" && !skip_to_verb_prev(before) && !suggest_verb(before) && !(before == "that" && (0..4).any(|k| suggest_verb(d.l(i.wrapping_sub(3 + k))))) {
                fix(h, d, i, "has", "Agreement", "Use “has”", "With “he”, “she” and “it”, use “has”.");
                continue;
            }
        }
        // ---- we/they/you + was
        if matches!(prev, "we" | "they" | "you") && matches!(w, "was" | "wasn't") {
            let s = d.sent_first(i);
            if prev == "you" && (s..i).any(|k| d.l(k).len() > 4 && d.l(k).ends_with("ing")) {
                continue;
            }
            let new = if w == "was" { "were" } else { "weren't" };
            fix(h, d, i, new, "Agreement", "Use “were”", "With “we”, “they” and “you”, use “were”.");
            continue;
        }
        // ---- I is / I are
        if d.w(i.wrapping_sub(1)) == "I" && matches!(w, "is" | "are") && !roman_prev(d.l(i.wrapping_sub(2))) {
            // "Jake and I are" is a plural subject.
            if matches!(d.l(i.wrapping_sub(2)), "and" | "or" | "nor") {
                if w == "is" {
                    fix(h, d, i, "are", "Agreement", "Use “are”", "Two people together take “are”.");
                }
                continue;
            }
            fix(h, d, i, "am", "Agreement", "Use “am”", "With “I”, use “am”.");
            continue;
        }
        // ---- should of
        if matches!(prev, "should" | "could" | "would" | "must" | "might" | "shouldn't" | "couldn't" | "wouldn't" | "mustn't") && w == "of" {
            let nx = d.l(i + 1);
            if d.is_w(i + 1) && !matches!(nx, "course" | "the" | "a" | "an" | "these" | "those" | "which" | "this" | "that") {
                fix(h, d, i, "have", "Grammar", "Use “have”", "It is “should have”, “could have”, “would have”. It sounds like “of” but the word is “have”.");
                continue;
            }
        }
        // ---- Name and Name was
        if matches!(w, "was" | "has") && prev.len() > 0 && d.is_w(i.wrapping_sub(1)) && i >= 3 && d.l(i - 2) == "and" && d.ss[i - 3] {
            let (a, b) = (d.l(i - 3), d.l(i.wrapping_sub(1)));
            let person = |x: &str| names(x) || matches!(x, "he" | "she" | "i" | "you" | "we" | "they");
            if person(a) && person(b) && d.cap(i - 3) {
                let new = if w == "was" { "were" } else { "have" };
                fix(h, d, i, new, "Agreement", "Use a plural verb", "Two people joined by “and” take a plural verb: “Sam and Dana were”.");
                continue;
            }
        }
        // ---- each/one of the plural + have/are/were
        if matches!(w, "each" | "one") && d.l(i + 1) == "of" && !is_prep(prev) {
            let mut k = i + 2;
            if matches!(d.l(k), "the" | "these" | "those" | "my" | "our" | "your" | "their" | "his" | "her") {
                k += 1;
            }
            for v in k + 1..=k + 3 {
                let t = d.l(v);
                if !d.is_w(v) {
                    break;
                }
                if matches!(t, "are" | "were" | "have") && plural_noun(d.l(v - 1)) && (k..v).all(|m| !verb_strict(d.l(m))) {
                    let new = match t {
                        "are" => "is",
                        "were" => "was",
                        _ => "has",
                    };
                    fix(h, d, v, new, "Agreement", "Use a singular verb", "“Each of…” and “one of…” take a singular verb.");
                    break;
                }
                if verb_strict(t) || is_prep(t) || matches!(t, "and" | "or" | "that" | "who" | "which") {
                    break;
                }
            }
            continue;
        }
        // ---- everyone in the building were
        if matches!(w, "everyone" | "everybody" | "someone" | "somebody" | "anyone" | "anybody" | "nobody" | "everything" | "nothing") {
            let nx = d.l(i + 1);
            let direct = matches!(nx, "are" | "were");
            let mut v = None;
            if direct {
                v = Some(i + 1);
            } else if is_prep(nx) && nx != "to" && d.is_w(i + 1) {
                for k in i + 3..=i + 5 {
                    let t = d.l(k);
                    if !d.is_w(k) || verb_strict(t) && !matches!(t, "are" | "were" | "have") || matches!(t, "and" | "or" | "that" | "who" | "which") {
                        break;
                    }
                    if matches!(t, "are" | "were" | "have") {
                        v = Some(k);
                        break;
                    }
                }
            }
            if let Some(v) = v {
                let new = match d.l(v) {
                    "are" => "is",
                    "were" => "was",
                    _ => "has",
                };
                fix(h, d, v, new, "Agreement", "Use a singular verb", "Words like “everyone” and “nobody” take a singular verb.");
            }
        }
    }
    // ---- head noun + prepositional phrase + verb: "The list of items are"
    for s in 0..n {
        if !d.ss[s] || !d.is_w(s) || !matches!(d.l(s), "the" | "a" | "an" | "this" | "that") {
            continue;
        }
        let mut p = None;
        for k in s + 1..=s + 3 {
            if !d.is_w(k) || is_aux(d.l(k)) {
                break;
            }
            if k > s + 1 && is_prep(d.l(k)) && d.l(k) != "to" {
                p = Some(k);
                break;
            }
        }
        let Some(p) = p else { continue };
        let head = p - 1;
        let hw = d.l(head);
        if (d.cap(head) && head != s) || coll_head(hw) {
            continue;
        }
        let plural = plural_noun(hw);
        for v in p + 2..=p + 4 {
            let t = d.l(v);
            if !d.is_w(v) || matches!(t, "and" | "or" | "but" | "that" | "which" | "who") || is_prep(t) {
                break;
            }
            if matches!(t, "is" | "are" | "was" | "were" | "has" | "have") {
                let sing_v = matches!(t, "is" | "was" | "has");
                if plural && sing_v {
                    let new = match t {
                        "is" => "are",
                        "was" => "were",
                        _ => "have",
                    };
                    fix(h, d, v, new, "Agreement", "Use a plural verb", agr);
                } else if !plural && !sing_v {
                    let new = match t {
                        "are" => "is",
                        "were" => "was",
                        _ => "has",
                    };
                    fix(h, d, v, new, "Agreement", "Use a singular verb", agr);
                }
                break;
            }
            if verb_strict(t) {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Verb tense and forms
// ---------------------------------------------------------------------------------------

fn participle(w: &str) -> Option<&'static str> {
    Some(match w {
        "went" => "gone",
        "ran" => "run",
        "came" => "come",
        "did" => "done",
        "saw" => "seen",
        "ate" => "eaten",
        "wrote" => "written",
        "took" => "taken",
        "gave" => "given",
        "began" => "begun",
        "drank" => "drunk",
        "swam" => "swum",
        "rode" => "ridden",
        "drove" => "driven",
        "flew" => "flown",
        "grew" => "grown",
        "knew" => "known",
        "threw" => "thrown",
        "wore" => "worn",
        "chose" => "chosen",
        "broke" => "broken",
        "spoke" => "spoken",
        "stole" => "stolen",
        "froze" => "frozen",
        "forgot" => "forgotten",
        "fell" => "fallen",
        "sang" => "sung",
        "shook" => "shaken",
        "woke" => "woken",
        "drew" => "drawn",
        "blew" => "blown",
        "tore" => "torn",
        "swore" => "sworn",
        _ => return None,
    })
}

const IRREG: &[(&str, &str)] = &[
    ("go", "went"),
    ("send", "sent"),
    ("come", "came"),
    ("see", "saw"),
    ("take", "took"),
    ("give", "gave"),
    ("make", "made"),
    ("get", "got"),
    ("find", "found"),
    ("think", "thought"),
    ("tell", "told"),
    ("leave", "left"),
    ("know", "knew"),
    ("run", "ran"),
    ("begin", "began"),
    ("feel", "felt"),
    ("keep", "kept"),
    ("hold", "held"),
    ("bring", "brought"),
    ("build", "built"),
    ("write", "wrote"),
    ("stand", "stood"),
    ("hear", "heard"),
    ("meet", "met"),
    ("sit", "sat"),
    ("speak", "spoke"),
    ("lose", "lost"),
    ("pay", "paid"),
    ("spend", "spent"),
    ("win", "won"),
    ("teach", "taught"),
    ("buy", "bought"),
    ("catch", "caught"),
    ("fall", "fell"),
    ("grow", "grew"),
    ("draw", "drew"),
    ("drive", "drove"),
    ("fly", "flew"),
    ("rise", "rose"),
    ("choose", "chose"),
    ("break", "broke"),
    ("eat", "ate"),
    ("drink", "drank"),
    ("sing", "sang"),
    ("swim", "swam"),
    ("wear", "wore"),
    ("throw", "threw"),
    ("understand", "understood"),
    ("become", "became"),
    ("lead", "led"),
    ("sell", "sold"),
    ("sleep", "slept"),
    ("forget", "forgot"),
    ("ride", "rode"),
    ("say", "said"),
];

const REGULAR: &str = "visit launch finish start walk talk work call open close play move change create ask help look use try turn show watch order deliver receive arrive return wait plan push pull add remove update review test fix check join request cancel confirm discuss complete prepare organize explain describe develop design improve increase reduce share support learn study want need like love hate enjoy appreciate miss decide agree believe happen remain continue consider include suggest recommend require allow offer provide expect arrange attend collect clean cook stay jump pick pack park pass post press save serve ship shop skip sort stop drop step trip chat hug grab admit prefer refer occur submit";

fn past_map() -> &'static std::collections::HashMap<&'static str, String> {
    static M: OnceLock<std::collections::HashMap<&'static str, String>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        for (b, p) in IRREG {
            m.insert(*b, p.to_string());
        }
        for b in REGULAR.split_whitespace() {
            let double = matches!(b, "stop" | "plan" | "drop" | "ship" | "shop" | "skip" | "step" | "trip" | "chat" | "hug" | "grab" | "admit" | "prefer" | "refer" | "occur" | "submit");
            let p = if b.ends_with('e') {
                format!("{b}d")
            } else if b.ends_with('y') && !matches!(b.as_bytes()[b.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u') {
                format!("{}ied", &b[..b.len() - 1])
            } else if double {
                let last = b.chars().last().unwrap();
                format!("{b}{last}ed")
            } else {
                format!("{b}ed")
            };
            m.insert(b, p);
        }
        m
    })
}

fn past_form(base: &str) -> Option<&'static String> {
    past_map().get(base)
}

fn third_person(b: &str) -> String {
    match b {
        "go" => "goes".into(),
        "do" => "does".into(),
        "have" => "has".into(),
        _ if b.ends_with('s') || b.ends_with('x') || b.ends_with('z') || b.ends_with("ch") || b.ends_with("sh") => format!("{b}es"),
        _ if b.ends_with('y') && b.len() > 2 && !matches!(b.as_bytes()[b.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u') => format!("{}ies", &b[..b.len() - 1]),
        _ => format!("{b}s"),
    }
}

fn ing_form(b: &str) -> String {
    match b {
        "sit" | "run" | "swim" | "shop" | "stop" | "plan" | "drop" => format!("{b}{}ing", b.chars().last().unwrap()),
        "dance" | "drive" | "ride" | "write" | "come" | "take" | "make" | "give" | "have" | "live" | "move" | "use" => format!("{}ing", &b[..b.len() - 1]),
        _ => format!("{b}ing"),
    }
}

wordset!(
    prog_base,
    "walk go come wait talk drive sit stand watch look try think write eat sleep cook clean travel fly swim shop study speak sing dance ride pack run"
);
wordset!(
    day_names,
    "night week weekend month year summer winter spring autumn fall monday tuesday wednesday thursday friday saturday sunday decade century"
);
wordset!(
    comp_start_ok,
    "the a an its his her their our my your this that these those to in on at out up off down over back away with for from into about by me him them us it you and but"
);

fn past_marker(d: &Doc, a: usize, b: usize) -> bool {
    let b = b.min(d.n().saturating_sub(1));
    (a..=b).any(|k| {
        let w = d.l(k);
        w == "yesterday"
            || w == "ago"
            || (w == "last" && day_names(d.l(k + 1)))
            || (w == "in" && d.k(k + 1) == Some(K::N) && d.w(k + 1).len() == 4 && d.w(k + 1).parse::<u32>().map_or(false, |y| (1000..=2025).contains(&y)))
            || (w == "back" && d.l(k + 1) == "then")
    })
}

fn clause_is_past(d: &Doc, a: usize, v: usize) -> bool {
    let s = d.sent_first(a);
    let breaker = |k: usize| d.p(k, ',') || d.p(k, ';') || matches!(d.l(k), "and" | "but" | "so" | "because" | "while" | "when");
    let mut c0 = s;
    for k in (s..a).rev() {
        if breaker(k) {
            c0 = k + 1;
            break;
        }
    }
    let mut c1 = d.sent_last(a);
    for k in v + 1..=c1 {
        if d.is_p(k) && !(d.p(k, '-') && d.tight(k)) || breaker(k) {
            c1 = k - 1;
            break;
        }
    }
    if past_marker(d, c0, c1) {
        return true;
    }
    if c0 > s && d.p(c0 - 1, ',') && c0 >= 2 {
        let e = c0 - 2;
        if e >= s && e - s <= 8 {
            if past_marker(d, s, e) {
                return true;
            }
            if matches!(d.l(s), "when" | "after" | "before" | "while" | "as" | "once") && (s..=e).any(|k| matches!(d.l(k), "was" | "were") || ed_verb(d.l(k)) || is_irregular_past(d.l(k))) {
                return true;
            }
        }
    }
    false
}

fn r_tense(d: &Doc, h: &mut Vec<Hit>) {
    let n = d.n();
    for i in 1..n {
        if !d.is_w(i) {
            continue;
        }
        let w = d.l(i);
        let prev = d.l(i - 1);
        // ---- had went / has ran
        if let Some(pp) = participle(w) {
            let aux = |t: &str| matches!(t, "had" | "has" | "have" | "hadn't" | "hasn't" | "haven't" | "having") || (t.ends_with("'ve") && t.len() > 3);
            let direct = aux(prev);
            let spaced = i >= 2 && (is_adv_word(prev) || is_ly(prev) || prev == "all") && aux(d.l(i - 2));
            if (direct || spaced) && !(w == "did" && d.l(i + 1) == "not") {
                fix(h, d, i, pp, "Grammar", "Use the past participle", "After “have”, “has” or “had”, use the past participle (“had gone”, “has eaten”).");
                continue;
            }
        }
        // ---- I seen
        if w == "seen" && d.w(i - 1) == "I" {
            fix(h, d, i, "saw", "Grammar", "Use “saw”", "Use “saw” for the past, or “have seen” with a helper verb.");
            continue;
        }
        // ---- was walk -> was walking
        if matches!(w, "was" | "were" | "am" | "is" | "are") && d.is_w(i + 1) {
            let b = d.l(i + 1);
            let subj_ok = matches!(prev, "i" | "he" | "she" | "we" | "they" | "you") || (d.cap(i - 1) && !d.ss[i - 1] && !is_pron(prev) && names(prev));
            if prog_base(b) && subj_ok && !(d.p(i + 2, '-') && d.tight(i + 2)) && (!d.is_w(i + 2) || comp_start_ok(d.l(i + 2)) || is_ly(d.l(i + 2)) || matches!(d.l(i + 2), "home" | "around" | "there" | "here" | "outside")) {
                fix(h, d, i + 1, &ing_form(b), "Grammar", "Use the -ing form", "After “was”, “is” or “are”, the verb takes -ing (“was walking”).");
                continue;
            }
        }
        // ---- subject + base verb
        let Some(past) = past_form(w) else { continue };
        if i < 1 || !d.is_w(i - 1) {
            // an adverb could sit between subject and verb; handled below
            if !(i >= 2 && (is_adv_word(prev) || is_ly(prev))) {
                continue;
            }
        }
        let mut sj = i - 1;
        if (is_adv_word(d.l(sj)) || is_ly(d.l(sj))) && sj >= 1 {
            sj -= 1;
        }
        let sw = d.l(sj);
        let nx = d.l(i + 1);
        let comp_ok = !d.is_w(i + 1) || comp_start_ok(nx) || is_ly(nx) || is_adv_word(nx) || nx == "out";
        if d.nl(i) {
            continue;
        }
        let before = d.l(sj.wrapping_sub(1));
        let s = d.sent_first(sj);
        if matches!(sw, "he" | "she" | "it") {
            if skip_to_verb_prev(before) || (before.len() > 0 && verb_strict(before) && sw == "it") || (s..sj).any(|k| suggest_verb(d.l(k))) || d.ends_question(sj) {
                continue;
            }
            if sw == "it" && (is_present_verb(before) || is_prep(before) || !comp_ok) {
                continue;
            }
            let sent_past = clause_is_past(d, sj, i) || (s..=d.sent_last(sj)).any(|k| k != i && (ed_verb(d.l(k)) || matches!(d.l(k), "was" | "were" | "had") || is_irregular_past(d.l(k))));
            let p3 = third_person(w);
            let sug = if sent_past { vec![past.clone(), p3] } else { vec![p3, past.clone()] };
            hit(h, d.tk[i].s, d.tk[i].e, sug.into_iter().map(|x| like_case(d.w(i), &x)).collect(), "Agreement", "Fix the verb form", "After “he”, “she” or “it”, a present-tense verb ends in -s; for the past, use the past form.");
            continue;
        }
        if skip_to_verb_prev(before) || d.ends_question(sj) {
            continue;
        }
        if matches!(sw, "i" | "we" | "you" | "they") {
            if clause_is_past(d, sj, i) {
                hit1(h, d.tk[i].s, d.tk[i].e, like_case(d.w(i), past), "Grammar", "Use the past tense", "The time words in this sentence point to the past, so the verb should be in the past tense.");
            }
            continue;
        }
        // determiner + noun + base verb
        if sj >= 1 && d.is_w(sj) && adj_det(d.l(sj - 1)) && d.is_w(sj - 1) && !is_prep(sw) && !is_aux(sw) && sw.len() > 2 && comp_ok && clause_is_past(d, sj, i) {
            let singular = !sw.ends_with('s') || is_sing_s(sw);
            let mut sug = vec![past.clone()];
            if singular {
                sug.push(third_person(w));
            }
            hit(h, d.tk[i].s, d.tk[i].e, sug.into_iter().map(|x| like_case(d.w(i), &x)).collect(), "Grammar", "Use the past tense", "The time words in this sentence point to the past, so the verb should be in the past tense.");
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    fn alerts(t: &str) -> Vec<Alert> {
        rule_alerts(t, &TextMap::new(t))
    }

    /// Every text you get by applying one alert's suggestion.
    fn outputs(t: &str) -> Vec<String> {
        let cs: Vec<char> = t.chars().collect();
        let mut v = Vec::new();
        for a in alerts(t) {
            for s in &a.suggestions {
                let out: String = cs[..a.start].iter().collect::<String>() + s + &cs[a.end..].iter().collect::<String>();
                v.push(out);
            }
        }
        v
    }

    fn pos(t: &str, expected: &str) {
        assert!(outputs(t).iter().any(|o| o == expected), "{t:?} should become {expected:?}; got {:?}", outputs(t));
    }

    fn neg(t: &str) {
        let a = alerts(t);
        assert!(a.is_empty(), "{t:?} is correct but fired: {:?}", a.iter().map(|x| (&x.title, &x.original)).collect::<Vec<_>>());
    }

    #[test]
    fn dashes() {
        pos("We finished early - the client was thrilled.", "We finished early—the client was thrilled.");
        pos("The deadline is Friday--no exceptions.", "The deadline is Friday—no exceptions.");
        pos("Please read pages 10-20 tonight.", "Please read pages 10–20 tonight.");
        pos("It ran from 2018-2022 without trouble.", "It ran from 2018–2022 without trouble.");
        neg("Call 555-0142 today.");
        neg("The date is 2026-11-01 for the launch.");
        neg("A well-known user-friendly tool.");
        neg("Run the tool with --verbose to see more.");
        neg("The result of 12 - 5 is 7.");
    }

    #[test]
    fn intro_words() {
        pos("However the team kept going.", "However, the team kept going.");
        pos("Unfortunately we missed the train.", "Unfortunately, we missed the train.");
        pos("For example the first draft was too long.", "For example, the first draft was too long.");
        pos("Yes we can finish by Friday.", "Yes, we can finish by Friday.");
        neg("However, the team kept going.");
        neg("However hard we try, it stays difficult.");
        neg("Instead of waiting we left.");
        neg("No one noticed the change.");
        neg("Well done to everyone involved.");
    }

    #[test]
    fn intro_clause() {
        pos("When the meeting finally ended everyone rushed out.", "When the meeting finally ended, everyone rushed out.");
        pos("Because the shipment was delayed by the storm we rescheduled.", "Because the shipment was delayed by the storm, we rescheduled.");
        neg("When the meeting finally ended, everyone rushed out.");
        neg("Everyone rushed out when the meeting finally ended.");
        neg("When did we agree to meet?");
        neg("If you know the answer I will listen.");
    }

    #[test]
    fn compound() {
        pos("The proposal was strong but the committee rejected it.", "The proposal was strong, but the committee rejected it.");
        pos("We tested the update on Monday and it passed every check.", "We tested the update on Monday, and it passed every check.");
        neg("The proposal was well written but rejected anyway.");
        neg("Sarah and I finished the slides last night.");
        neg("We tested the update, and it passed every check.");
        neg("I think Tom and he should go together.");
    }

    #[test]
    fn splice() {
        pos("The server crashed at noon, nobody noticed until evening.", "The server crashed at noon; nobody noticed until evening.");
        pos("Our lease ends in March, we have to find a new office.", "Our lease ends in March. We have to find a new office.");
        pos("The test failed, however we kept going.", "The test failed; however, we kept going.");
        neg("After the storm passed, we went outside.");
        neg("I came, I saw, I conquered.");
        neg("The more you read, the more you know.");
        neg("\"I am tired,\" she said.");
    }

    #[test]
    fn runon() {
        pos("The meeting ran long on Monday The client asked for a demo.", "The meeting ran long on Monday. The client asked for a demo.");
        pos("I love this restaurant the pasta is always fresh.", "I love this restaurant. The pasta is always fresh.");
        neg("The man we saw was tall.");
        neg("She said the plan was fine.");
        neg("Welcome To The Jungle is a song.");
    }

    #[test]
    fn comma_that() {
        pos("We need a tool, that can handle thousands of requests.", "We need a tool that can handle thousands of requests.");
        pos("She said, that she was tired.", "She said that she was tired.");
        neg("We need a tool that can handle thousands of requests.");
        neg("It rained all day, that is why we stayed in.");
        neg("Is it this, that, or the other?");
    }

    #[test]
    fn direct_address() {
        pos("Thanks Maria for covering my shift.", "Thanks, Maria for covering my shift.");
        pos("Can you open the window Sam?", "Can you open the window, Sam?");
        pos("Good morning everyone and thank you.", "Good morning, everyone and thank you.");
        neg("Thanks for covering my shift.");
        neg("Can you call Sam?");
        neg("Thanks, Maria, for covering my shift.");
    }

    #[test]
    fn apostrophes() {
        pos("The 1990's were a decade of change.", "The 1990s were a decade of change.");
        pos("I burned three CD's and gave them away.", "I burned three CDs and gave them away.");
        pos("We ordered six pizza's for lunch.", "We ordered six pizzas for lunch.");
        pos("I think its going to rain.", "I think it's going to rain.");
        pos("The dog wagged it's tail.", "The dog wagged its tail.");
        pos("My brothers car broke down.", "My brother's car broke down.");
        neg("The 1990s were a decade of change.");
        neg("The CD's cover was scratched.");
        neg("It's been a long day, and its edges are rough.");
        neg("The dog's tail and Tom's hat are here.");
    }

    #[test]
    fn compound_adjectives() {
        pos("She is a well known author.", "She is a well-known author.");
        pos("They signed a five year contract.", "They signed a five-year contract.");
        pos("It uses a state of the art lab.", "It uses a state-of-the-art lab.");
        neg("The author is well known in the region.");
        neg("She is a well-known author.");
        neg("We worked full time for a year.");
    }

    #[test]
    fn end_punctuation() {
        pos("The conference starts on Monday. Please arrive early to collect your badge", "The conference starts on Monday. Please arrive early to collect your badge.");
        neg("The conference starts on Monday. Please arrive early.");
        neg("Q3 Planning Meeting Agenda Overview Today");
        neg("- remember to bring the printed badge with you");
        neg("Ingredients");
    }

    #[test]
    fn quote_punctuation() {
        pos("He told me to \"stay calm\".", "He told me to \"stay calm.\"");
        pos("She called it \"ambitious\", though risky.", "She called it \"ambitious,\" though risky.");
        pos("She called it “ambitious”, though risky.", "She called it “ambitious,” though risky.");
        neg("He told me to \"stay calm.\"");
        neg("The screen is 12\" wide.");
        neg("He said \"hi\" to everyone.");
    }

    #[test]
    fn who_whom() {
        pos("To who should I address the package?", "To whom should I address the package?");
        pos("The candidate who we interviewed accepted the offer.", "The candidate whom we interviewed accepted the offer.");
        neg("To whom should I address the package?");
        neg("Be who you want to be.");
        neg("I know who he is.");
        neg("The man who came late left early.");
    }

    #[test]
    fn pronoun_case() {
        pos("Between you and I, it will fail.", "Between you and me, it will fail.");
        pos("He gave the book to Sarah and I.", "He gave the book to Sarah and me.");
        pos("Me and him went home.", "He and I went home.");
        pos("Me and Priya finished the slides.", "Priya and I finished the slides.");
        neg("Between you and me, it will fail.");
        neg("I went to the store, and I bought milk.");
        neg("Sarah and I finished the slides.");
    }

    #[test]
    fn confusables() {
        pos("Your going to love it.", "You're going to love it.");
        pos("Send me you're updated address.", "Send me your updated address.");
        pos("It is to late now.", "It is too late now.");
        pos("The new one is faster then the old one.", "The new one is faster than the old one.");
        pos("Mix the flour, than add the eggs.", "Mix the flour, then add the eggs.");
        pos("Don't loose the receipt.", "Don't lose the receipt.");
        pos("The screw came lose.", "The screw came loose.");
        pos("The kids forgot there notebooks.", "The kids forgot their notebooks.");
        pos("Their is no reason to panic.", "There is no reason to panic.");
        neg("Your invitation is on the table.");
        neg("You're welcome to stay.");
        neg("It went to the store, and then we left.");
        neg("The screw is loose, and the lid is tight.");
        neg("There is a reason, and their plan is sound.");
        neg("It's too late to change it.");
    }

    #[test]
    fn agreement_and_tense() {
        pos("The list of items are on the desk.", "The list of items is on the desk.");
        pos("There is three open seats.", "There are three open seats.");
        pos("Sam and Dana was in charge.", "Sam and Dana were in charge.");
        pos("She don't know the answer.", "She doesn't know the answer.");
        pos("He have worked here for years.", "He has worked here for years.");
        pos("We was late to the meeting.", "We were late to the meeting.");
        pos("I should of called you.", "I should have called you.");
        pos("She has went home already.", "She has gone home already.");
        pos("Yesterday I go to the dentist.", "Yesterday I went to the dentist.");
        pos("I was walk home when it rained.", "I was walking home when it rained.");
        neg("The list of items is on the desk.");
        neg("A number of options are available.");
        neg("Does she have a pen?");
        neg("Meeting you was a pleasure.");
        neg("I go to the dentist every year.");
        neg("There are many reasons to try.");
    }

    #[test]
    fn rules_stay_quiet_on_the_extra_clean_corpus() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/corpus/clean_extra.txt");
        let raw = std::fs::read_to_string(path).expect("clean_extra.txt");
        let mut n = 0;
        let mut bad = Vec::new();
        for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
            n += 1;
            let a = alerts(line);
            if !a.is_empty() {
                bad.push(format!("{line} -> {:?}", a.iter().map(|x| (&x.title, &x.original)).collect::<Vec<_>>()));
            }
        }
        assert!(n >= 60, "{n}");
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn rules_stay_quiet_on_the_main_clean_corpus() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/corpus/clean.txt");
        let raw = std::fs::read_to_string(path).expect("clean.txt");
        let mut bad = Vec::new();
        for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let a = alerts(line);
            if !a.is_empty() {
                bad.push(format!("{line} -> {:?}", a.iter().map(|x| (&x.title, &x.original)).collect::<Vec<_>>()));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn rule_layer_is_fast() {
        let para = "The quick brown fox jumps over the lazy dog, and the team finished the long report before noon. However we kept going, because the client asked for more. She said that it's fine, but I think its time to rest. ";
        let mut text = String::new();
        while text.split_whitespace().count() < 10_000 {
            text.push_str(para);
        }
        let map = TextMap::new(&text);
        let _ = rule_alerts(&text, &map);
        let t = std::time::Instant::now();
        for _ in 0..5 {
            let _ = rule_alerts(&text, &map);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / 5.0;
        println!("RULES 10k words: {ms:.2} ms ({:.3} ms per 1000 words)", ms / 10.0);
        // The budget is for the optimized app; an unoptimized test build (CI) is many times slower.
        let budget = if cfg!(debug_assertions) { 40.0 } else { 2.0 };
        assert!(ms / 10.0 < budget, "{ms} ms for 10k words");
    }

    #[test]
    fn offsets_round_trip_with_emoji() {
        let t = "😀 Thanks Maria for the lift, I think its fine.";
        for a in alerts(t) {
            assert_eq!(crate::textmap::slice_utf16(t, a.u16_start, a.u16_end), a.original);
        }
    }
}
