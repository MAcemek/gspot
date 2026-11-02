# GSpot — build plan with gates

Goal (set 2026-10-14): a **signed macOS app** that launches, shows the Grammarly-style floating panel
**docked on the left**, and runs every sidebar feature end to end on real text in real apps.

## Stack (mirrors what Grammarly actually ships, with open-source parts)

| Concern | Choice | Why |
|---|---|---|
| Shell | Tauri 2 + Rust + React/TS (Vite) | Grammarly's own panel is a Tauri app (`app_inkwell`). Reuses WhimprFlow signing + paste code. |
| Correctness engine | `harper-core` (Rust, in-process, offline, <10 ms) | Real grammar/spelling lints with spans + suggestions. No server. |
| Text access | macOS Accessibility (AX) FFI from Rust | Same approach as `GRAccessibilityKit`: focused element, value, selection, `AXBoundsForRange`. |
| Replacement | AX selected-text set → pasteboard patch fallback → verify | Same ladder Grammarly uses (`PasteboardPatchingService`). |
| LLM features | **publik API** (zero setup: the app mints its own key, CONTRACT §3.2/§12), or your own key (one field; provider detected from the prefix, model picked from the account's list); no local-model option since Oct 18 (the e2e rig alone runs on Ollama, through the debug probe) | Paraphraser, Humanizer, Reader Reactions, Translate, Explain It, Tone, Chat, AI Rewriter rewrites, AI Detector judge, Spot check. |
| AI phrases | curated frequency list (3 buckets) + regex | AI Rewriter flags; LLM rewrites. |
| Plagiarism | DuckDuckGo HTML search (keyless) or Brave Search API key → fetch → n-gram overlap → APA/MLA/Chicago citation | Real web matches without a paid API. |
| Score | local formula from alerts/words + readability | Same labels as Grammarly. |
| Signing | `Developer ID Application: Mann Bellani (R5R3ZS54LV)`, hardened runtime, empty entitlements | Notarization through the publik `notarize-artifact.yml` workflow or an app-specific password. |

## Gates

| Gate | Deliverable | Proof |
|---|---|---|
| G0 Scaffold | App builds, launches, panel docked left with the full icon rail, signed. | `cargo test`, `pnpm test`, `scripts/verify-macos.sh` (codesign + Gatekeeper + launch). |
| G1 Host text | AX permission flow; reads focused field (text, selection, bounds) in TextEdit/Notes/Chrome; G-button pill follows the field. | `tests/e2e/host_textedit.sh`: AppleScript types a fixture into TextEdit, debug probe returns the same text. |
| G2 Correctness | Harper alerts → 4 categories, underlines drawn at `AXBoundsForRange`, Review list, Accept applies into the host app, Dismiss persists. | unit tests for span→UTF-16 mapping and category mapping; e2e: accept "teh"→"the" and read TextEdit back. |
| G3 Score + Tone + History | Performance panel numbers/labels, Tone insights (LLM), history list. | unit tests for score math; e2e smoke against Ollama. |
| G4 LLM features | Chat (streaming), Paraphraser, Humanizer (+custom voice), Reader Reactions (+custom audience), Translate, Explain It, AI Rewriter, AI Detector. | `tests/e2e/features.sh` calls each through the debug probe and asserts shape + non-empty output. |
| G5 Plagiarism | Search + fetch + match + citation. | e2e with a Wikipedia sentence expects ≥1 source. |
| G6 Release | Signed .app + .dmg, verify script green, notarization attempted. | `scripts/build-macos.sh`, `scripts/verify-macos.sh`. |

## Known trade-offs (stated up front)
* AI Detector v1 = stylometry + LLM judge (a real score, not Binoculars). A llama.cpp-based Binoculars is a follow-up.
* "Matter" is Grammarly's licensed font; Inter is used everywhere.
* Delivery suggestions are rule-based in v1 (hedging/confidence phrases), LLM-backed later.

## Status (2026-10-14, final)

All gates ran against the **signed release bundle** (`src-tauri/target/release/bundle/macos/GSpot.app`, also installed to `/Applications`).

| Gate | Result |
|---|---|
| G0 Scaffold | ✅ `cargo test` 62/62, `pnpm test` 6/6; `scripts/verify-macos.sh`: signature intact, Developer ID Application (R5R3ZS54LV), hardened runtime, dmg mounts, app launches and stays running. |
| G1 Host text | ✅ `tests/e2e/host_textedit.sh` 27/27 (Oct 15, signed bundle): all four webviews (panel, G button, underlines, edge tab) mount from the embedded bundle; app starts tucked (tab shown, panel hidden); `/panel {"tucked":false}` slides the panel out to x=12 and hides the tab; `/panel {"tucked":true}` slides it back; TextEdit document read through AX; frame + per-line underline geometry; hover: a synthetic pointer (`/mouse`) over the 'teh' underline opens the card, onto the card keeps it open, away closes it (regression for the stuck card); G button follows the field. The gate skips itself while a video call runs (FaceTime/Zoom/Teams) because it activates windows and moves the pointer. |
| G2 Correctness | ✅ Harper alerts in 4 categories; Accept applied via AX and TextEdit's text changed; re-lint after the edit; Dismiss removes and persists. Spot check on the live document: the wordy sentence gets a Clarity suggestion anchored to the text, underlined, and Accept rewrites TextEdit. |
| G3 Score + Tone + History | ✅ score math unit-tested; tone returns named tones + emoji on real text; history populated. |
| G4 LLM features | ✅ (Oct 15, signed bundle: features 15/15, spotcheck 13/13 at 98 %, setup 17/17) `tests/e2e/features.sh` on Ollama `qwen2.5:7b`: paraphrase (sentence count kept; a selected sentence stays one sentence with its numbers), humanize (sentence count kept), reader reactions, translate, explain, tone, AI rewriter scan + rewrite, AI detector (human and AI text), summarize, spot check, chat ping. `tests/e2e/spotcheck.sh`: 12-document corpus ≥ 90 %. `tests/e2e/setup.sh`: publik provision → 402-per-contract → forget; own key probe → ping; Ollama detection. |
| G5 Plagiarism | ✅ Wikipedia sentence → 5 sources incl. en.wikipedia.org, 44 % matched, APA/MLA/Chicago citations. Keyless search chain: Brave HTML → Bing → DuckDuckGo (DDG serves a bot challenge; Bing drops quotes). |
| G6 Release | ✅ signed `.app` + `.dmg`; ⏳ **not notarized** (needs an app-specific password via `NOTARY_PROFILE`/`APPLE_PASSWORD`, or a Blueturboguy07 release-asset URL for publik's `notarize-artifact.yml`). |

Tuck (Oct 14 PM): `overlay.rs` owns a `tab` window (48×96 pt, flush with the dock edge, 42 % down the screen, non-focusable, click-through transparent margin) and `set_tucked(app, side, tucked, animate, focus)`, which slides the panel 16 steps × 16 ms with an ease-out cubic on its own thread (a newer call cancels an older slide via a generation counter) and remembers the panel's last position. Close button, `hide_panel`, and the rail's "Tuck away" chevron tuck; the tab, the G button, the tray menu (with focus) and `open_panel` untuck. First run with no Accessibility grant starts untucked so the permission gate is visible.

Hover card fix (Oct 14 PM): the underline window ignores mouse events whenever the pointer is over neither an underline nor the card, so DOM `mouseleave` never reached the card and a stale "over card" flag kept it open. Now the host hit-tests the card rect every tick and reports `over_card` in `host://geometry`; the UI opens/closes from that signal alone (400 ms grace), and reports a bridge rect between the underline and the card so the window stays live while the pointer crosses the gap.

Hover card window (Nov 1, 0.5.0): the card now lives in its own always-on-top, non-activating "card" window placed by the host (`overlay::place_card`: below the word, above near the bottom edge, clamped to the screen), so the underline window is permanently click-through. The hover thread owns open/close (450 ms grace plus a bridge to the card); `/state` reports `card_rects` as `[card_rect]` in screen coordinates.

Spot check + sentence-preserving rewrites (Oct 15): see `docs/SPOTCHECK.md`. Sentence-level Clarity suggestions ("Make it more concise", "Use the active voice", "Rephrase for clarity", "Remove the repeated/filler sentence") from the model, with deterministic hints and guards, cached per sentence, re-anchored after edits; Paraphraser/Humanizer run per paragraph chunk and retry when sentences go missing. Tuned on a 12-document corpus (95 % on qwen2.5:7b). Gates: `tests/e2e/spotcheck.sh`, the TextEdit gate's spot block, `features.sh` sentence-count checks.

Spot check round 2 (Oct 15 AM, after Mann's screenshot of a 44-word sentence struck through with no rewrite): long sentences get a split (model split guarded, else a deterministic lossless cut at the middle joint), a content-preservation guard rejects summaries, unhinted rewrites need confidence ≥ 4 and may not shrink below half or grow, Harper's long-sentence note is titled "Split the long sentence" and yields to a spot rewrite, the auto check runs from the host loop for any panel view, rewrites show as word diffs, the panel has no emoji and a card-style category header. Corpus is 14 documents; `tests/tuning/live_corpus.sh` scores it through the pipeline alone.

Onboarding + AI set-up (Oct 15): first run (after Accessibility) shows one screen with three cards: publik API (recommended, no setup: disclosure → `POST /installs` → first-run card with the balance line, the justification sentence and "Sign in with publik"), your own key (one password field; `probe_key` detects OpenAI / Anthropic / OpenRouter / Groq / Google from the prefix, lists models, picks one, pings), and a local model (only when Ollama is detected). URLs, ports and model names live under a collapsed "Advanced" section, in onboarding and in Settings. The publik key lives in `~/Library/Application Support/publik/apps/gspot.json` (0600), never in settings.json; a 402 renders the gateway's message plus exactly one publikhq.com link. Gate: `tests/e2e/setup.sh`.

Local model removed (Oct 18): the "Local model" card and the "Ollama (local)" entry under Advanced are gone, so the picker has two cards. The default provider is publik API, so a skipped onboarding answers "publik API is not set up on this computer yet" instead of "Ollama is not reachable". A settings file that still names Ollama migrates to the publik default on load (`migrate_local_model`). The Ollama client stays for the rig: `tests/e2e/common.sh` `rig_model` points the app at `qwen2.5:7b` through the probe after every `start_app`, and `setup.sh` asserts it.

Known gaps: the panel activates the app when clicked (Grammarly's is a non-activating NSPanel); AI Detector is stylometry + LLM judge, not a trained classifier; "Matter" font replaced by Inter; underline hover cards are hit-tested by a 220 ms poll.
