//! Re-ranks Harper's spelling suggestions (filled in by the spelling task).
use harper_core::spell::{Dictionary, FstDictionary};
use harper_core::Dialect;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static DICT: OnceLock<Arc<FstDictionary>> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

/// Typo-aware Damerau-Levenshtein from what was typed (a) to a dictionary word (b).
/// Doubling slips and swapped neighbours are cheap; a dropped letter is cheaper than a wrong one.
fn typo_cost(a: &str, b: &str) -> f32 {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let vowel = |c: char| "aeiou".contains(c);
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0f32; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i as f32;
    }
    for j in 0..=m {
        d[0][j] = j as f32;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = if a[i - 1] == b[j - 1] {
                0.0
            } else if vowel(a[i - 1]) && vowel(b[j - 1]) {
                0.7
            } else {
                1.0
            };
            // typed an extra letter: cheap when it doubles its neighbour ("comming")
            let del = if (i >= 2 && a[i - 2] == a[i - 1]) || (i < n && a[i] == a[i - 1]) { 0.4 } else { 1.0 };
            // dropped a letter: cheap when it is a doubled letter ("realy"), otherwise a bit cheaper than a wrong letter
            let ins = if (j >= 2 && b[j - 2] == b[j - 1]) || (j < m && b[j] == b[j - 1]) { 0.4 } else { 0.8 };
            let mut v = (d[i - 1][j] + del).min(d[i][j - 1] + ins).min(d[i - 1][j - 1] + sub);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 0.6);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
}

/// Scores dictionary candidates and Harper's own list, then returns the best four (lower-case as stored).
fn rank(typo: &str, harper: &[String]) -> Vec<String> {
    let dict = DICT.get_or_init(FstDictionary::curated);
    let mut c: Vec<(f32, String)> = Vec::new();
    for m in dict.fuzzy_match_str(&typo.to_lowercase(), 3, 80) {
        let w: String = m.word.iter().collect();
        if w.chars().next().map(|ch| ch.is_uppercase()).unwrap_or(false) {
            continue;
        }
        if !m.metadata.dialects.is_dialect_enabled(Dialect::American) {
            continue;
        }
        let mut score = typo_cost(typo, &w);
        if m.metadata.common {
            score -= 0.35;
        }
        if w.chars().next() != typo.to_lowercase().chars().next() {
            score += 0.3;
        }
        c.push((score, w));
    }
    for (i, h) in harper.iter().enumerate() {
        if c.iter().any(|(_, w)| w == h) {
            continue;
        }
        let s = if h.contains(' ') { 2.5 } else { typo_cost(typo, h) + 0.1 * i as f32 };
        c.push((s, h.clone()));
    }
    c.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<String> = Vec::new();
    for (_, w) in c {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&w) || o.to_lowercase() == w.to_lowercase()) {
            out.push(w);
        }
        if out.len() == 4 {
            break;
        }
    }
    let chars: Vec<char> = typo.chars().collect();
    if chars.len() > 1 && chars.iter().all(|ch| !ch.is_lowercase()) {
        out = out.into_iter().map(|w| w.to_uppercase()).collect();
    } else if chars.first().map(|ch| ch.is_uppercase()).unwrap_or(false) {
        out = out
            .into_iter()
            .map(|w| {
                let mut it = w.chars();
                match it.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + it.as_str(),
                    None => w,
                }
            })
            .collect();
    }
    out
}

/// Best-first spelling suggestions for `typo`: Harper's own list merged with common dictionary words
/// near it, ranked by a typo-aware edit cost. At most 4, American spellings, original capitalization.
pub fn rerank(typo: &str, harper: &[String]) -> Vec<String> {
    let plain = typo.is_empty()
        || typo.chars().count() > 40
        || !typo.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-');
    if plain {
        return harper.iter().take(4).cloned().collect();
    }
    let key = format!("{typo}\u{1}{}", harper.join("\u{2}"));
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = cache.lock() {
        if let Some(hit) = g.get(&key) {
            return hit.clone();
        }
    }
    let out = rank(typo, harper);
    if let Ok(mut g) = cache.lock() {
        if g.len() > 4000 {
            g.clear();
        }
        g.insert(key, out.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harper_for(e: &mut crate::engine::Engine, t: &str) -> Vec<String> {
        e.check(&format!("I will {t} it soon."))
            .iter()
            .find(|x| x.original.eq_ignore_ascii_case(t))
            .map(|x| x.suggestions.clone())
            .unwrap_or_default()
    }

    fn run(pairs: &[(&str, &str)], min: usize) {
        let mut e = crate::engine::Engine::new();
        let mut misses = Vec::new();
        for (t, want) in pairs {
            let h = harper_for(&mut e, t);
            let r = rerank(t, &h);
            if r.first().map(|f| f != want).unwrap_or(true) {
                misses.push(format!("{t}: want {want}, harper={h:?}, ranked={r:?}"));
            }
        }
        let ok = pairs.len() - misses.len();
        println!("{ok}/{} first-correct", pairs.len());
        for m in &misses {
            println!("MISS {m}");
        }
        assert!(ok >= min, "only {ok}/{} correct (need {min})", pairs.len());
    }

    #[test]
    fn common_typos_rank_the_right_word_first() {
        run(
            &[("recieve","receive"),("definately","definitely"),("seperate","separate"),("becuase","because"),("wierd","weird"),("acheive","achieve"),("tommorow","tomorrow"),("occured","occurred"),("untill","until"),("beleive","believe"),("goverment","government"),("enviroment","environment"),("wich","which"),("thier","their"),("freind","friend"),("realy","really"),("finaly","finally"),("buisness","business"),("calender","calendar"),("embarass","embarrass"),("accomodate","accommodate"),("adress","address"),("aparent","apparent"),("begining","beginning"),("comming","coming"),("knowlege","knowledge"),("neccessary","necessary"),("probaly","probably"),("suprise","surprise"),("writting","writing")],
            29,
        );
    }

    #[test]
    fn unseen_typos_rank_the_right_word_first() {
        run(
            &[("absense","absence"),("acceptible","acceptable"),("accidently","accidentally"),("agressive","aggressive"),("arguement","argument"),("basicly","basically"),("beautifull","beautiful"),("carribean","Caribbean"),("cemetary","cemetery"),("collegue","colleague"),("comittee","committee"),("concious","conscious"),("curiousity","curiosity"),("dissapoint","disappoint"),("existance","existence"),("familar","familiar"),("foriegn","foreign"),("fourty","forty"),("gaurd","guard"),("grammer","grammar"),("happend","happened"),("harrass","harass"),("immediatly","immediately"),("independant","independent"),("interupt","interrupt"),("libary","library"),("lisence","license"),("maintainance","maintenance"),("mischievious","mischievous"),("noticable","noticeable"),("occassion","occasion"),("persistant","persistent"),("posession","possession"),("prefered","preferred"),("publically","publicly"),("recomend","recommend"),("relevent","relevant"),("sieze","seize"),("tounge","tongue"),("wether","whether")],
            37,
        );
    }

    #[test]
    fn keeps_capitalization() {
        let mut e = crate::engine::Engine::new();
        let h = harper_for(&mut e, "Recieve");
        assert_eq!(rerank("Recieve", &h).first().map(String::as_str), Some("Receive"));
        let h = harper_for(&mut e, "RECIEVE");
        assert_eq!(rerank("RECIEVE", &h).first().map(String::as_str), Some("RECEIVE"));
    }

    #[test]
    fn typo_cost_prefers_doubling_slips() {
        assert!(typo_cost("realy", "really") < typo_cost("realy", "ready"));
        assert!(typo_cost("comming", "coming") < typo_cost("comming", "cumming"));
    }

    #[test]
    fn odd_input_passes_through() {
        assert_eq!(rerank("abc123", &["x".to_string()]), vec!["x".to_string()]);
    }
}
