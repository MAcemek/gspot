# Scoreboard (HOLDOUT)

Date: 2026-11-01

Run: `cargo run --release --example scoreboard -- --holdout` from `src-tauri/`. Corpus: `src-tauri/benches/corpus/` (errors_holdout.jsonl, clean_holdout.txt).

| category | n | caught | fixed | catch% | fix% |
|---|---:|---:|---:|---:|---:|
| comma_splice | 10 | 6 | 6 | 60% | 60% |
| missing_comma_intro | 10 | 6 | 6 | 60% | 60% |
| missing_comma_compound | 10 | 6 | 6 | 60% | 60% |
| comma_before_that | 10 | 10 | 10 | 100% | 100% |
| missing_comma_direct_address | 10 | 5 | 5 | 50% | 50% |
| apostrophe_its | 10 | 7 | 7 | 70% | 70% |
| apostrophe_plural | 10 | 7 | 7 | 70% | 70% |
| apostrophe_possessive | 10 | 4 | 4 | 40% | 40% |
| hyphen_vs_dash | 10 | 9 | 9 | 90% | 90% |
| compound_adjective_hyphen | 10 | 7 | 7 | 70% | 70% |
| double_space | 10 | 10 | 10 | 100% | 100% |
| space_before_punct | 10 | 10 | 10 | 100% | 100% |
| missing_end_punct | 10 | 8 | 8 | 80% | 80% |
| quote_punct | 10 | 10 | 10 | 100% | 100% |
| their_there | 10 | 8 | 8 | 80% | 80% |
| your_youre | 10 | 5 | 5 | 50% | 50% |
| then_than | 10 | 9 | 9 | 90% | 90% |
| affect_effect | 10 | 10 | 10 | 100% | 100% |
| to_too | 10 | 5 | 5 | 50% | 50% |
| a_an | 10 | 10 | 10 | 100% | 100% |
| loose_lose | 10 | 8 | 8 | 80% | 80% |
| who_whom | 10 | 5 | 5 | 50% | 50% |
| subject_verb | 10 | 6 | 6 | 60% | 60% |
| verb_tense | 10 | 8 | 5 | 80% | 50% |
| pronoun_case | 10 | 8 | 8 | 80% | 80% |
| capitalization | 10 | 9 | 8 | 90% | 80% |
| run_on | 10 | 3 | 3 | 30% | 30% |
| **overall** | 270 | 199 | 195 | 73.7% | 72.2% |

False alarms on clean text: 7/100

Alert kinds firing on clean text:

- Agreement / Use “am”: 1
- Delivery / Choose a stronger word: 2
- Delivery / Sound more confident: 1
- Formatting / Fix formatting: 1
- Spelling / Correct your spelling: 2
- Style / Rephrase for clarity: 1


Worst 5 categories by fix%: run_on (30% fixed, 30% caught); apostrophe_possessive (40% fixed, 40% caught); missing_comma_direct_address (50% fixed, 50% caught); to_too (50% fixed, 50% caught); verb_tense (50% fixed, 80% caught)
