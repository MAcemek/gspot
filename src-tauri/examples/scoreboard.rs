//! Objective benchmark: `cargo run --release --example scoreboard [-- --verbose] [-- --cat <name>]`.
//! Scores the engine on benches/corpus/errors.jsonl (catch/fix) and clean.txt (false alarms).

use grammarlama_lib::engine::{Alert, Category, Engine};
use std::collections::BTreeMap;
use std::fmt::Write as _;

struct Item {
    cat: String,
    text: String,
    err: String,
    fix: String,
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ignored(a: &Alert) -> bool {
    matches!(a.category, Category::Engagement)
        || a.kind.eq_ignore_ascii_case("Engagement")
        || a.kind.to_lowercase().contains("readab")
}

#[derive(Default)]
struct Stat {
    n: usize,
    caught: usize,
    fixed: usize,
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        0.0
    } else {
        100.0 * a as f64 / b as f64
    }
}

/// `--ai`: also run the AI grammar pass with the user's configured model (falls back to local Ollama).
struct Ai {
    rt: tokio::runtime::Runtime,
    cfg: grammarlama_lib::settings::LlmConfig,
    cache: grammarlama_lib::features::grammarpass::Cache,
    asked: usize,
    ms: u128,
    rejected: usize,
    log: Vec<String>,
}

impl Ai {
    fn new() -> Ai {
        use grammarlama_lib::{commands, llm, settings};
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        let st = settings::load();
        let mut cfg = commands::effective_llm(&st).unwrap_or_else(|_| st.llm.clone());
        let probe = rt.block_on(llm::complete(&cfg, "Reply with OK.", &[llm::msg("user", "ping")], false, 0.0));
        if let Err(e) = probe {
            let first = e.to_string().chars().take(60).collect::<String>();
            let models = rt.block_on(llm::ollama_models(settings::OLLAMA_BASE_URL)).unwrap_or_default();
            let model = llm::pick_ollama_model(&models).expect("configured model failed and no local Ollama model found");
            eprintln!("configured model {} ({:?}) failed ({first}); using local {model}", cfg.model, cfg.provider);
            cfg = settings::LlmConfig { provider: settings::Provider::Ollama, base_url: settings::OLLAMA_BASE_URL.into(), api_key: String::new(), model };
        }
        println!("AI grammar pass model: {} ({:?})", cfg.model, cfg.provider);
        Ai { rt, cfg, cache: Default::default(), asked: 0, ms: 0, rejected: 0, log: Vec::new() }
    }

    /// `alerts` plus the AI grammar alerts that no deterministic alert overlaps.
    fn add(&mut self, text: &str, mut alerts: Vec<Alert>) -> Vec<Alert> {
        use grammarlama_lib::features::grammarpass;
        let t = std::time::Instant::now();
        match self.rt.block_on(grammarpass::run(&self.cfg, text, &mut self.cache)) {
            Ok(o) => {
                self.asked += o.checked;
                self.rejected += o.rejected;
                self.log.extend(o.rejections.iter().cloned());
                let ai = grammarpass::dedupe_against(o.alerts, &alerts);
                alerts.extend(ai);
            }
            Err(e) => eprintln!("grammar pass failed: {e}"),
        }
        self.ms += t.elapsed().as_millis();
        alerts
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let holdout = args.iter().any(|a| a == "--holdout");
    let mut ai: Option<Ai> = if args.iter().any(|a| a == "--ai") { Some(Ai::new()) } else { None };
    let only_cat = args.iter().position(|a| a == "--cat").and_then(|i| args.get(i + 1)).cloned();

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let (errs_name, clean_name) = if holdout {
        ("errors_holdout.jsonl", "clean_holdout.txt")
    } else {
        ("errors.jsonl", "clean.txt")
    };
    let errs_raw = std::fs::read_to_string(root.join("benches/corpus").join(errs_name)).expect(errs_name);
    let clean_raw = std::fs::read_to_string(root.join("benches/corpus").join(clean_name)).expect(clean_name);

    let mut items: Vec<Item> = Vec::new();
    for (i, line) in errs_raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("line {}: {e}", i + 1));
        let g = |k: &str| v[k].as_str().unwrap_or_else(|| panic!("line {}: missing {k}", i + 1)).to_string();
        let it = Item { cat: g("cat"), text: g("text"), err: g("err"), fix: g("fix") };
        let occ = it.text.matches(it.err.as_str()).count();
        if occ != 1 {
            eprintln!("WARNING line {}: err {:?} occurs {} times in {:?}", i + 1, it.err, occ, it.text);
        }
        items.push(it);
    }
    let clean: Vec<&str> = clean_raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect();

    let mut engine = Engine::new();
    let mut stats: BTreeMap<String, Stat> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut verbose_out = String::new();

    for it in &items {
        if let Some(c) = &only_cat {
            if &it.cat != c {
                continue;
            }
        }
        if !stats.contains_key(&it.cat) {
            order.push(it.cat.clone());
        }
        let st = stats.entry(it.cat.clone()).or_default();
        st.n += 1;

        let byte = match it.text.find(&it.err) {
            Some(b) => b,
            None => continue,
        };
        let e_start = it.text[..byte].chars().count();
        let e_end = e_start + it.err.chars().count();
        let chars: Vec<char> = it.text.chars().collect();
        let expected = norm(&format!(
            "{}{}{}",
            chars[..e_start].iter().collect::<String>(),
            it.fix,
            chars[e_end..].iter().collect::<String>()
        ));

        let mut alerts: Vec<Alert> = engine.check(&it.text).into_iter().filter(|a| !ignored(a)).collect();
        if let Some(a) = ai.as_mut() {
            alerts = a.add(&it.text, alerts);
        }
        let overlapping: Vec<&Alert> = alerts.iter().filter(|a| a.start < e_end && e_start < a.end).collect();
        let caught = !overlapping.is_empty();
        let fixed = overlapping.iter().any(|a| {
            a.suggestions.iter().any(|s| {
                let s_end = a.end.min(chars.len());
                let s_start = a.start.min(s_end);
                let out = format!(
                    "{}{}{}",
                    chars[..s_start].iter().collect::<String>(),
                    s,
                    chars[s_end..].iter().collect::<String>()
                );
                norm(&out) == expected
            })
        });
        if caught {
            st.caught += 1;
        }
        if fixed {
            st.fixed += 1;
        }
        if verbose && (!caught || !fixed) {
            let _ = writeln!(
                verbose_out,
                "{} [{}] {}\n  text: {}\n  err: {:?}  fix: {:?}",
                if caught { "CAUGHT-NOT-FIXED" } else { "MISS" },
                it.cat,
                "",
                it.text,
                it.err,
                it.fix
            );
            if alerts.is_empty() {
                let _ = writeln!(verbose_out, "  alerts: (none)");
            }
            for a in &alerts {
                let _ = writeln!(
                    verbose_out,
                    "  alert [{}] {:?} {}..{} {:?} -> {:?}",
                    a.kind, a.title, a.start, a.end, a.original, a.suggestions
                );
            }
        }
    }

    // Clean sentences.
    let mut false_alarms = 0usize;
    let mut fa_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut clean_out = String::new();
    if only_cat.is_none() {
        for s in &clean {
            let mut alerts: Vec<Alert> = engine.check(s).into_iter().filter(|a| !ignored(a)).collect();
            if let Some(a) = ai.as_mut() {
                alerts = a.add(s, alerts);
            }
            if !alerts.is_empty() {
                false_alarms += 1;
                for a in &alerts {
                    *fa_kinds.entry(format!("{} / {}", a.kind, a.title)).or_default() += 1;
                    let _ = writeln!(
                        clean_out,
                        "FALSE ALARM: {}\n  alert [{}] {:?}: {:?}",
                        s, a.kind, a.title, a.original
                    );
                }
            }
        }
    }

    // Summary table.
    let mut table = String::new();
    let _ = writeln!(table, "| category | n | caught | fixed | catch% | fix% |");
    let _ = writeln!(table, "|---|---:|---:|---:|---:|---:|");
    let (mut tn, mut tc, mut tf) = (0, 0, 0);
    let mut rows: Vec<(String, usize, usize, usize)> = Vec::new();
    for c in &order {
        let s = &stats[c];
        let _ = writeln!(
            table,
            "| {} | {} | {} | {} | {:.0}% | {:.0}% |",
            c,
            s.n,
            s.caught,
            s.fixed,
            pct(s.caught, s.n),
            pct(s.fixed, s.n)
        );
        tn += s.n;
        tc += s.caught;
        tf += s.fixed;
        rows.push((c.clone(), s.n, s.caught, s.fixed));
    }
    let _ = writeln!(
        table,
        "| **overall** | {} | {} | {} | {:.1}% | {:.1}% |",
        tn,
        tc,
        tf,
        pct(tc, tn),
        pct(tf, tn)
    );
    let mut tail = String::new();
    if only_cat.is_none() {
        let _ = writeln!(tail, "\nFalse alarms on clean text: {}/{}", false_alarms, clean.len());
        if !fa_kinds.is_empty() {
            let _ = writeln!(tail, "\nAlert kinds firing on clean text:\n");
            for (k, n) in &fa_kinds {
                let _ = writeln!(tail, "- {k}: {n}");
            }
        }
    }

    if verbose {
        println!("{verbose_out}");
        println!("{clean_out}");
    }
    if holdout {
        println!("HOLDOUT");
    }
    println!("{table}{tail}");
    if let Some(a) = &ai {
        println!("AI pass: model {}; {} sentences asked, {} ms total ({:.0} ms/sentence), {} answers rejected by guards", a.cfg.model, a.asked, a.ms, a.ms as f64 / a.asked.max(1) as f64, a.rejected);
        if verbose {
            for l in &a.log {
                println!("  rejected {l}");
            }
        }
    }

    if only_cat.is_none() {
        rows.sort_by(|a, b| pct(a.3, a.1).partial_cmp(&pct(b.3, b.1)).unwrap().then(a.0.cmp(&b.0)));
        let worst: Vec<String> = rows
            .iter()
            .take(5)
            .map(|r| format!("{} ({:.0}% fixed, {:.0}% caught)", r.0, pct(r.3, r.1), pct(r.2, r.1)))
            .collect();
        println!("Worst 5 categories by fix%: {}", worst.join("; "));

        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut md = String::new();
        let _ = writeln!(md, "{}\n", if holdout { "# Scoreboard (HOLDOUT)" } else { "# Scoreboard" });
        let _ = writeln!(md, "Date: {date}\n");
        let _ = writeln!(
            md,
            "Run: `cargo run --release --example scoreboard{}` from `src-tauri/`. Corpus: `src-tauri/benches/corpus/` ({}, {}).\n",
            if holdout { " -- --holdout" } else { "" },
            errs_name,
            clean_name
        );
        let _ = writeln!(md, "{table}{tail}");
        let _ = writeln!(md, "\nWorst 5 categories by fix%: {}", worst.join("; "));
        let docs = root.join("../docs");
        let _ = std::fs::create_dir_all(&docs);
        if ai.is_none() {
            let _ = std::fs::write(docs.join(if holdout { "scoreboard-holdout.md" } else { "scoreboard.md" }), md);
        }
    }
}
