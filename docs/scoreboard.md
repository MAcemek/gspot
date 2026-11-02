# Scoreboard

Date: 2026-11-01

Run: `cargo run --release --example scoreboard` from `src-tauri/`. Corpus: `src-tauri/benches/corpus/` (errors.jsonl, clean.txt).

| category | n | caught | fixed | catch% | fix% |
|---|---:|---:|---:|---:|---:|
| comma_splice | 10 | 10 | 10 | 100% | 100% |
| missing_comma_intro | 10 | 9 | 9 | 90% | 90% |
| missing_comma_compound | 10 | 10 | 10 | 100% | 100% |
| comma_before_that | 10 | 10 | 10 | 100% | 100% |
| missing_comma_direct_address | 10 | 10 | 10 | 100% | 100% |
| apostrophe_its | 10 | 10 | 10 | 100% | 100% |
| apostrophe_plural | 10 | 9 | 9 | 90% | 90% |
| apostrophe_possessive | 10 | 10 | 10 | 100% | 100% |
| compound_adjective_hyphen | 10 | 10 | 10 | 100% | 100% |
| double_space | 10 | 10 | 10 | 100% | 100% |
| space_before_punct | 10 | 10 | 10 | 100% | 100% |
| missing_end_punct | 10 | 10 | 10 | 100% | 100% |
| quote_punct | 10 | 10 | 10 | 100% | 100% |
| their_there | 10 | 10 | 10 | 100% | 100% |
| your_youre | 10 | 10 | 10 | 100% | 100% |
| then_than | 10 | 10 | 10 | 100% | 100% |
| affect_effect | 10 | 10 | 10 | 100% | 100% |
| to_too | 10 | 9 | 9 | 90% | 90% |
| a_an | 10 | 10 | 10 | 100% | 100% |
| loose_lose | 10 | 10 | 10 | 100% | 100% |
| who_whom | 10 | 10 | 10 | 100% | 100% |
| subject_verb | 10 | 10 | 10 | 100% | 100% |
| verb_tense | 10 | 10 | 10 | 100% | 100% |
| pronoun_case | 10 | 10 | 10 | 100% | 100% |
| capitalization | 10 | 10 | 10 | 100% | 100% |
| run_on | 10 | 6 | 6 | 60% | 60% |
| hyphen_vs_dash | 10 | 10 | 10 | 100% | 100% |
| **overall** | 270 | 263 | 263 | 97.4% | 97.4% |

False alarms on clean text: 1/80

Alert kinds firing on clean text:

- Delivery / Sound more confident: 1


Worst 5 categories by fix%: run_on (60% fixed, 60% caught); apostrophe_plural (90% fixed, 90% caught); missing_comma_intro (90% fixed, 90% caught); to_too (90% fixed, 90% caught); a_an (100% fixed, 100% caught)
