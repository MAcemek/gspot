//! Timing for long documents: `cargo run --release --example engine_bench`.

use grammarlama_lib::engine::Engine;
use std::time::Instant;

const PARAS: &[&str] = &[
    "The quarterly review covered hiring, budget, and the roadmap for the next two releases. Teh team agreed that the plan was very good, although a few people wanted more detail before they signed off on the schedule and the staffing levels.",
    "We should recieve the final numbers on Monday, and then we can decide how to split the work between the platform group and the product group. I think that the biggest risk is the timeline, because every dependency is owned by a different team.",
    "Customers told us the onboarding flow felt long, and the support queue backs that up. A lot of tickets mention the same three screens, so fixing those first is a small change with a big payoff, and it is a very simple thing to measure afterward.",
    "Writing clearly takes practice. Start with one idea per sentence, cut the words that carry no weight, and read the draft aloud before you send it. It is really about respecting the reader, who has limited time and plenty of other mail to get through.",
    "The migration finished without incident, and the dashboards show latency back to normal. Next week we will remove the old service, update the runbooks, and write a short note for the on-call rotation so nobody is surprised by the new alert names.",
];

fn doc(words: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut i = 0;
    while n < words {
        let p = PARAS[i % PARAS.len()];
        // Vary each paragraph a little so the cache sees distinct text.
        let para = format!("{p} Section {i}.");
        n += para.split_whitespace().count();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&para);
        i += 1;
    }
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let mut e = Engine::new();
    println!("{:>7} {:>8} {:>10} {:>9} {:>16}", "words", "chars", "check ms", "alerts", "incremental ms");
    for words in [1_000, 5_000, 10_000] {
        let text = doc(words);
        let mut times = Vec::new();
        let mut alerts = 0;
        for _ in 0..5 {
            let t = Instant::now();
            alerts = e.check(&text).len();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        // Incremental: warm the cache, change one paragraph, time the next call.
        let mut inc = Vec::new();
        for k in 0..5 {
            let mut e2 = Engine::new();
            e2.check_incremental(&text);
            let edited = text.replacen("Section 3.", &format!("Section 3 edited {k} teh."), 1);
            let t = Instant::now();
            e2.check_incremental(&edited);
            inc.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{:>7} {:>8} {:>10.1} {:>9} {:>16.1}", words, text.chars().count(), median(times), alerts, median(inc));
    }
}
