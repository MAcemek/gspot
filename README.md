# GSpot

> **This is a fork of [blueturboguy07/gspot](https://github.com/blueturboguy07/gspot)** (AGPL-3.0) that adds:
>
> * **Windows support.** Text access goes through Windows UI Automation (`src-tauri/src/ax_win.rs`) in place of the
>   macOS Accessibility API, answering the same calls so the host loop, engine and UI are shared.
>   Replacements select the range and paste (Ctrl+V), then fall back to UIA `ValuePattern` for plain fields.
> * **Local AI (Ollama)**, back as the default: every AI feature can run on a model on your own computer,
>   free and private. Settings → AI → Local AI lists the models Ollama has; publik API and your own key still work.
> * **Talk like a CEO**: cuts a message down to the ask, then types it the way a busy exec would
>   (Polished, Casual, or "Sent from my phone"). The model only shortens; the rough edges are added by a
>   deterministic pass (`src-tauri/src/features/ceo.rs`), so names and numbers are never mangled.
> * Small hardening: `open_url` only opens `http(s)` links.
>
> ### Install on Windows
>
> Download **`GSpot_0.1.0_x64-setup.exe`** from the [latest release](https://github.com/MAcemek/gspot/releases/latest) and run it
> (installs for your user, no admin). The build is not code-signed yet, so SmartScreen may ask: **More info → Run anyway**.
>
> ### Build from source on Windows
>
> ```powershell
> # prerequisites: Rust (rustup), Node 20+, pnpm, Visual Studio C++ Build Tools, WebView2 (built into Windows 11)
> pnpm install
> pnpm dev          # run with hot reload
> pnpm build        # installer in src-tauri/target/release/bundle/nsis
> ```
>
> For AI features, install [Ollama](https://ollama.com/download), run `ollama pull qwen2.5:7b`, and pick the model
> under Settings → AI → Local AI. Spelling and grammar (Harper) work offline without any of that.
>
> Windows support is new: plain editors (Notepad, WordPad, standard text boxes) work best; browsers, Electron apps and
> Word vary by app. Use "Skip this app" for any app where underlines land in the wrong place, and please open an issue.

![GSpot beside a TextEdit draft: the Review panel lists the suggestions, the draft is underlined in place, and a card offers the fix](docs/hero.png)

A local-first, open-source Grammarly-style writing assistant for macOS. A floating panel docked to the
left edge of the screen follows the text field you are typing in (any app), underlines issues in place,
and runs the full sidebar of tools: suggestions, AI chat, plagiarism, tone, performance score,
paraphraser, humanizer, reader reactions, translate, explain it, AI rewriter, AI detector.

The panel starts **tucked**: a small G tab sits flush with the left screen edge. Click the tab and the
panel slides out; the red close button or the "Tuck away" chevron at the bottom of the rail slides it
back. The tab shows the current suggestion count. (Grammarly tucks into the right edge; GSpot
tucks into the left. `dock_side` in Settings flips it.)

* Correctness engine: [Harper](https://writewithharper.com) (`harper-core`, in-process, offline).
  A sentence over 40 words gets a "Split the long sentence" card with a lossless split ready to accept;
  the spot check (the model) replaces it with a better split when one passes the guards.
* Text access: the macOS Accessibility API (focused field, selection, `AXBoundsForRange` geometry).
  Terminals (Terminal, iTerm2, Warp, Ghostty, kitty, Alacritty, WezTerm, Hyper) are skipped: a shell
  transcript is not writing. Any other app can be skipped from the Review view ("Skip this app") and
  turned back on under Settings → Skipped apps.
* Replacements: AX selected-text set, then a verified pasteboard patch fallback.
* AI features: **publik API** by default (no account, no key: the app mints its own key on this
  computer after a two-sentence disclosure; you sign in with publik to add free use or a plan),
  or your own key (paste it; OpenAI, Anthropic, OpenRouter, Groq and Google are detected from the
  prefix and a model is picked for you). URLs, ports and model names sit under "Advanced". There
  is no local-model option; only the e2e rig points the app at Ollama, through the debug probe.
* Spot check: sentence-level suggestions (make it concise, active voice, rephrase for clarity,
  remove a repeated or filler sentence) from the model, guarded so numbers, names and quotes
  survive and clean prose is left alone. See `docs/SPOTCHECK.md`.
* Plagiarism: web search through publik API when it is on (`POST /search`, one cent a search), your
  own Brave Search API key if you paste one in Settings, else a keyless chain (Brave HTML, Bing,
  DuckDuckGo); 7-gram overlap, APA/MLA/Chicago citations.

See `docs/REVERSE-ENGINEERING.md` for what was learned from Grammarly Desktop and `docs/PLAN.md` for the gates.

## Install

GSpot is listed on [publik](https://publikhq.com/gspot).

1. Download `GSpot_<version>_aarch64.dmg` from the [latest GitHub release](https://github.com/Blueturboguy07/gspot/releases/latest) (Apple silicon Macs).
2. Open the dmg and drag GSpot to Applications.
3. Open GSpot. The app is signed with a Developer ID and notarized by Apple, so macOS opens it without a warning.
4. Grant Accessibility when asked, under System Settings, Privacy & Security, Accessibility.

GSpot lives in the menu bar and as a tab on the left screen edge. AI features run through publik API by
default; the first use shows a two-sentence disclosure and mints a key for this computer.

## Build

```bash
pnpm install        # installs ui/ as well (postinstall)
pnpm dev            # hot reload: Vite + the app
# signed release (.app + .dmg)
scripts/build-debug.sh                   # debug build with the publik app token compiled in (for the e2e gates)
scripts/build-macos.sh --skip-notarize   # or with NOTARY_PROFILE / APPLE_ID+APPLE_PASSWORD+APPLE_TEAM_ID
scripts/verify-macos.sh                  # codesign, Gatekeeper, launch
```

Grant Accessibility to GSpot once (System Settings → Privacy & Security → Accessibility).
The publik app token is read from `PUBLIK_APP_TOKEN` or `~/.publik/gspot-app-token` at build
time (minted with publik's `scripts/mint-app-token.mts gspot`); a build without it still
offers your own key. `PUBLIK_SELF_TEST=1 /Applications/GSpot.app/Contents/MacOS/grammarlama` prints one
line that proves the publik API wiring of a built app.

## Test

```bash
cd ui && pnpm test                         # TypeScript unit tests
cd src-tauri && cargo test                 # Rust unit tests (engine, score, phrases, plagiarism, …)
tests/e2e/host_textedit.sh                 # reads/lints/fixes a real TextEdit document through AX (incl. a spot-check accept)
tests/e2e/features.sh                      # every LLM/web feature against the rig's Ollama + the live web
tests/e2e/spotcheck.sh                     # the 14-document spot-check corpus through the app (>= 90 % to pass)
tests/tuning/live_corpus.sh                # the same corpus through the pipeline alone (no app windows)
tests/e2e/setup.sh                         # publik API provisioning, own-key probe, the rig's Ollama
```

The e2e scripts launch the debug binary with `GRAMMARLAMA_DEBUG_PORT=47331` and talk to a loopback
probe (`/state`, `/accept`, `/feature`, `/lint`). `start_app` then points the app at the rig's Ollama
(`qwen2.5:7b`, or `E2E_MODEL`) through the probe, because the product itself offers no local model;
`E2E_KEEP_LLM=1` runs the gates on whatever the app is set to. A debug (`cargo build`) binary loads the Vite dev
server, which the harness starts for you. To test the signed bundle instead:

```bash
APP_BIN=src-tauri/target/release/bundle/macos/GSpot.app/Contents/MacOS/grammarlama tests/e2e/host_textedit.sh
```

Accessibility trust: a binary launched from a terminal inherits the terminal's grant (macOS charges
the "responsible process"). Launched from Finder, GSpot asks for its own grant once.
